// Test harness only. Ownership and cleanup are kernel Job membership, never PID ancestry.
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using Microsoft.Win32.SafeHandles;

public sealed class SplittyFixtureJob : IDisposable {
  const uint CREATE_SUSPENDED=0x4, CREATE_NO_WINDOW=0x08000000;
  const uint KILL_ON_JOB_CLOSE=0x2000, ACTIVE_PROCESS_LIMIT=0x8;
  const int JobObjectBasicAccountingInformation=1, JobObjectBasicProcessIdList=3, JobObjectExtendedLimitInformation=9;
  const uint WAIT_TIMEOUT=258, QUERY_LIMITED_INFORMATION=0x1000;
  static int liveHandles;
  sealed class OwnedHandle : SafeHandleZeroOrMinusOneIsInvalid {
    public OwnedHandle(IntPtr value) : base(true) { SetHandle(value); if (!IsInvalid) Interlocked.Increment(ref liveHandles); }
    protected override bool ReleaseHandle() { bool result=CloseHandle(handle); if(result)Interlocked.Decrement(ref liveHandles); return result; }
  }
  [StructLayout(LayoutKind.Sequential)] struct BasicLimits { public long ProcessTime,JobTime; public uint Flags; public UIntPtr MinimumWorkingSet,MaximumWorkingSet; public uint ActiveProcesses; public UIntPtr Affinity; public uint Priority,Scheduling; }
  [StructLayout(LayoutKind.Sequential)] struct IoCounters { public ulong ReadOps,WriteOps,OtherOps,ReadBytes,WriteBytes,OtherBytes; }
  [StructLayout(LayoutKind.Sequential)] struct ExtendedLimits { public BasicLimits Basic; public IoCounters Io; public UIntPtr ProcessMemory,JobMemory,PeakProcessMemory,PeakJobMemory; }
  [StructLayout(LayoutKind.Sequential)] struct Accounting { public long UserTime,KernelTime,PeriodUserTime,PeriodKernelTime; public uint PageFaults,TotalProcesses,ActiveProcesses,TerminatedProcesses; }
  [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)] struct Startup { public uint Size; public string Reserved,Desktop,Title; public uint X,Y,XSize,YSize,XChars,YChars,Fill,Flags; public ushort Show,ReservedBytes; public IntPtr ReservedData,StdIn,StdOut,StdErr; }
  [StructLayout(LayoutKind.Sequential)] struct StartupEx { public Startup Startup; public IntPtr Attributes; }
  [StructLayout(LayoutKind.Sequential)] struct SecurityAttributes { public uint Size; public IntPtr Descriptor; [MarshalAs(UnmanagedType.Bool)] public bool Inherit; }
  [StructLayout(LayoutKind.Sequential)] struct ProcessInfo { public IntPtr Process,Thread; public uint ProcessId,ThreadId; }
  public sealed class Member { public uint Id; public string Creation,ExecutablePath; public bool Alive; public uint ExitCode; }
  sealed class RetainedMember { public OwnedHandle Handle; public Member Value; }
  readonly Dictionary<string,RetainedMember> history=new Dictionary<string,RetainedMember>();
  [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr CreateJobObject(IntPtr attributes,string name);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool SetInformationJobObject(OwnedHandle job,int kind,ref ExtendedLimits value,uint size);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool QueryInformationJobObject(OwnedHandle job,int kind,IntPtr value,uint size,out uint returned);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool AssignProcessToJobObject(OwnedHandle job,OwnedHandle process);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool IsProcessInJob(OwnedHandle process,OwnedHandle job,out bool result);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool TerminateJobObject(OwnedHandle job,uint exitCode);
  [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool CreateProcess(string application,StringBuilder command,IntPtr processAttributes,IntPtr threadAttributes,bool inherit,uint flags,IntPtr environment,string directory,ref StartupEx startup,out ProcessInfo info);
  [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr CreateFile(string path,uint access,uint share,ref SecurityAttributes attributes,uint disposition,uint flags,IntPtr template);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool InitializeProcThreadAttributeList(IntPtr list,int count,uint flags,ref UIntPtr size);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool UpdateProcThreadAttribute(IntPtr list,uint flags,UIntPtr attribute,IntPtr value,UIntPtr size,IntPtr previous,IntPtr returned);
  [DllImport("kernel32.dll")] static extern void DeleteProcThreadAttributeList(IntPtr list);
  [DllImport("kernel32.dll",SetLastError=true)] static extern uint ResumeThread(OwnedHandle thread);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool TerminateProcess(OwnedHandle process,uint exitCode);
  [DllImport("kernel32.dll",SetLastError=true)] static extern uint WaitForSingleObject(OwnedHandle handle,uint milliseconds);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool GetExitCodeProcess(OwnedHandle process,out uint code);
  [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr OpenProcess(uint access,bool inherit,uint id);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool GetProcessTimes(OwnedHandle process,out long created,out long exited,out long kernel,out long user);
  [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool QueryFullProcessImageName(OwnedHandle process,uint flags,StringBuilder path,ref uint length);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool CloseHandle(IntPtr handle);
  OwnedHandle job,process,thread;
  bool disposed;
  public uint RootId { get; private set; }
  public string RootCreation { get; private set; }
  public string LaunchedCommandLine { get; private set; }
  public string StandardOutputPath { get; private set; }
  public string StandardErrorPath { get; private set; }
  public static bool LastFailureConfirmedExited { get; private set; }
  public static int LastFailureAssignmentError { get; private set; }
  public static uint LastFailureRootId { get; private set; }
  public static int LiveHandleCount { get { return liveHandles; } }
  static Win32Exception Error(string operation) { return new Win32Exception(Marshal.GetLastWin32Error(),operation); }
  static string Quote(string value) {
    if(value.IndexOf('\0')>=0) throw new ArgumentException("NUL in argument");
    // wsl.exe interprets quoted option tokens as a command boundary. Preserve
    // plain option/name tokens; quote only arguments that require Windows CRT
    // escaping (whitespace, quotes, or empty values).
    if(value.Length>0&&value.IndexOfAny(new char[]{' ','\t','\r','\n','\v','"'})<0)return value;
    StringBuilder result=new StringBuilder("\""); int slashes=0;
    foreach(char c in value) { if(c=='\\') {slashes++;continue;} if(c=='"') result.Append('\\',slashes*2+1).Append(c); else result.Append('\\',slashes).Append(c); slashes=0; }
    return result.Append('\\',slashes*2).Append('"').ToString();
  }
  public static string FormatCommandLine(string executable,string[] arguments) {
    StringBuilder command=new StringBuilder(Quote(executable));foreach(string argument in arguments)command.Append(' ').Append(Quote(argument));return command.ToString();
  }
  public SplittyFixtureJob(string executable,string[] arguments,string directory,bool rejectAssignmentForTest) {
    if(!System.IO.Path.IsPathRooted(executable)) throw new ArgumentException("absolute executable required");
    LastFailureConfirmedExited=false;LastFailureRootId=0;LastFailureAssignmentError=0;
    try {
      job=new OwnedHandle(CreateJobObject(IntPtr.Zero,null)); if(job.IsInvalid) throw Error("CreateJobObject");
      ExtendedLimits limits=new ExtendedLimits(); limits.Basic.Flags=KILL_ON_JOB_CLOSE|ACTIVE_PROCESS_LIMIT; limits.Basic.ActiveProcesses=256;
      // BREAKAWAY_OK and SILENT_BREAKAWAY_OK are deliberately absent.
      if(!SetInformationJobObject(job,JobObjectExtendedLimitInformation,ref limits,(uint)Marshal.SizeOf(typeof(ExtendedLimits)))) throw Error("SetInformationJobObject");
      LaunchedCommandLine=FormatCommandLine(executable,arguments);
      ProcessInfo info=CreatePaused(executable,new StringBuilder(LaunchedCommandLine),directory);
      process=new OwnedHandle(info.Process);thread=new OwnedHandle(info.Thread);RootId=info.ProcessId;
      bool assigned=false;
      try {
        if(rejectAssignmentForTest) {
          // Exercise the real OS assignment-failure path with an invalid Job
          // handle, while this exact newly created process is still suspended.
          using(OwnedHandle invalidJob=new OwnedHandle(IntPtr.Zero)) {
            if(!AssignProcessToJobObject(invalidJob,process)){LastFailureAssignmentError=Marshal.GetLastWin32Error();throw new Win32Exception(LastFailureAssignmentError,"fixture assignment failure before resume");}
          }
          throw new InvalidOperationException("OS unexpectedly accepted an invalid Job");
        }
        if(!AssignProcessToJobObject(job,process))throw Error("AssignProcessToJobObject"); assigned=true;
        bool member;if(!IsProcessInJob(process,job,out member)||!member)throw Error("initial Job membership");
        long born,ended,kernel,user;if(!GetProcessTimes(process,out born,out ended,out kernel,out user))throw Error("GetProcessTimes root");
        RootCreation=DateTime.FromFileTimeUtc(born).ToString("o");
        if(ResumeThread(thread)==UInt32.MaxValue)throw Error("ResumeThread");
      } catch (Exception original) {
        // This exact handle came from CreateProcess while its primary thread
        // was suspended. Never traverse or resolve a PID on this failure path.
        LastFailureRootId=RootId;
        bool terminated=assigned?TerminateJobObject(job,1):TerminateProcess(process,1);
        int cleanupError=terminated?0:Marshal.GetLastWin32Error();
        LastFailureConfirmedExited=WaitForSingleObject(process,5000)==0;
        if(!terminated||!LastFailureConfirmedExited)throw new AggregateException(original,new Win32Exception(cleanupError,"exact suspended process cleanup failed"));
        throw;
      }
    } catch { Dispose();throw; }
  }
  ProcessInfo CreatePaused(string executable,StringBuilder command,string directory) {
    string token=Guid.NewGuid().ToString("N");StandardOutputPath=System.IO.Path.Combine(directory,"job-"+token+".stdout.log");StandardErrorPath=System.IO.Path.Combine(directory,"job-"+token+".stderr.log");
    SecurityAttributes attributes=new SecurityAttributes();attributes.Size=(uint)Marshal.SizeOf(typeof(SecurityAttributes));attributes.Inherit=true;
    OwnedHandle input=null,output=null,error=null;IntPtr list=IntPtr.Zero,handles=IntPtr.Zero;bool initialized=false;
    try {
      input=new OwnedHandle(CreateFile("NUL",0x80000000,3,ref attributes,3,0,IntPtr.Zero));if(input.IsInvalid)throw Error("CreateFile fixture stdin");
      output=new OwnedHandle(CreateFile(StandardOutputPath,0x40000000,3,ref attributes,1,0,IntPtr.Zero));if(output.IsInvalid)throw Error("CreateFile fixture stdout");
      error=new OwnedHandle(CreateFile(StandardErrorPath,0x40000000,3,ref attributes,1,0,IntPtr.Zero));if(error.IsInvalid)throw Error("CreateFile fixture stderr");
      UIntPtr bytes=UIntPtr.Zero;InitializeProcThreadAttributeList(IntPtr.Zero,1,0,ref bytes);
      if(bytes==UIntPtr.Zero)throw Error("size fixture handle allowlist");
      list=Marshal.AllocHGlobal(checked((int)bytes.ToUInt64()));
      if(!InitializeProcThreadAttributeList(list,1,0,ref bytes))throw Error("initialize fixture handle allowlist");initialized=true;
      handles=Marshal.AllocHGlobal(3*IntPtr.Size);Marshal.WriteIntPtr(handles,0,input.DangerousGetHandle());Marshal.WriteIntPtr(handles,IntPtr.Size,output.DangerousGetHandle());Marshal.WriteIntPtr(handles,2*IntPtr.Size,error.DangerousGetHandle());
      if(!UpdateProcThreadAttribute(list,0,new UIntPtr(0x00020002),handles,new UIntPtr((uint)(3*IntPtr.Size)),IntPtr.Zero,IntPtr.Zero))throw Error("set fixture handle allowlist");
      StartupEx startup=new StartupEx();startup.Startup.Size=(uint)Marshal.SizeOf(typeof(StartupEx));startup.Startup.Flags=0x100;startup.Startup.StdIn=input.DangerousGetHandle();startup.Startup.StdOut=output.DangerousGetHandle();startup.Startup.StdErr=error.DangerousGetHandle();startup.Attributes=list;
      ProcessInfo info;
      // The HANDLE_LIST permits ONLY the three fixture-owned stdio handles.
      // The unnamed Job, process/thread handles, and ambient launcher handles
      // are excluded. The process cannot execute until Job assignment below.
      if(!CreateProcess(executable,command,IntPtr.Zero,IntPtr.Zero,true,CREATE_SUSPENDED|CREATE_NO_WINDOW|0x00080000,IntPtr.Zero,directory,ref startup,out info))throw Error("CreateProcess suspended with fixture stdio");
      return info;
    } finally {
      if(initialized)DeleteProcThreadAttributeList(list);if(list!=IntPtr.Zero)Marshal.FreeHGlobal(list);if(handles!=IntPtr.Zero)Marshal.FreeHGlobal(handles);
      if(error!=null)error.Dispose();if(output!=null)output.Dispose();if(input!=null)input.Dispose();
    }
  }
  public bool RootAlive { get { if(disposed) return false;return WaitForSingleObject(process,0)==WAIT_TIMEOUT; } }
  public uint RootExitCode { get { uint code;if(!GetExitCodeProcess(process,out code))throw Error("GetExitCodeProcess");return code; } }
  public uint ActiveProcesses {
    get { IntPtr value=Marshal.AllocHGlobal(Marshal.SizeOf(typeof(Accounting)));try {uint returned;if(!QueryInformationJobObject(job,JobObjectBasicAccountingInformation,value,(uint)Marshal.SizeOf(typeof(Accounting)),out returned))throw Error("Job accounting");return ((Accounting)Marshal.PtrToStructure(value,typeof(Accounting))).ActiveProcesses;}finally{Marshal.FreeHGlobal(value);} }
  }
  public Member[] Snapshot() {
    List<Member> result=new List<Member>();int bytes=8+256*IntPtr.Size;IntPtr buffer=Marshal.AllocHGlobal(bytes);
    try {
      uint returned;if(!QueryInformationJobObject(job,JobObjectBasicProcessIdList,buffer,(uint)bytes,out returned))throw Error("Job process list");
      int count=Marshal.ReadInt32(buffer,4);if(count<0||count>256)throw new InvalidOperationException("invalid Job process count");
      for(int index=0;index<count;index++) {
        uint id=unchecked((uint)(IntPtr.Size==8?Marshal.ReadInt64(buffer,8+index*8):Marshal.ReadInt32(buffer,8+index*4)));
        OwnedHandle member=new OwnedHandle(OpenProcess(QUERY_LIMITED_INFORMATION|0x00100000,false,id));bool retained=false;try {
          if(member.IsInvalid)continue;bool belongs;
          // Snapshot PIDs are only lookup hints: revalidate the opened handle
          // against this kernel Job. Inventory never authorizes termination.
          if(!IsProcessInJob(member,job,out belongs)||!belongs)continue;
          long born,ended,kernel,user;if(!GetProcessTimes(member,out born,out ended,out kernel,out user))continue;
          StringBuilder path=new StringBuilder(32768);uint length=32768;if(!QueryFullProcessImageName(member,0,path,ref length))continue;
          string key=id+"|"+born;RetainedMember entry;
          if(!history.TryGetValue(key,out entry)){if(history.Count>=4096)throw new InvalidOperationException("fixture Job history limit");entry=new RetainedMember{Handle=member,Value=new Member{Id=id,Creation=DateTime.FromFileTimeUtc(born).ToString("o"),ExecutablePath=path.ToString()}};history.Add(key,entry);retained=true;}
          uint code;if(GetExitCodeProcess(entry.Handle,out code)){entry.Value.ExitCode=code;entry.Value.Alive=WaitForSingleObject(entry.Handle,0)==WAIT_TIMEOUT;}
          result.Add(entry.Value);
        } finally {if(!retained)member.Dispose();}
      }
    } finally {Marshal.FreeHGlobal(buffer);}return result.ToArray();
  }
  public Member[] History() {
    foreach(RetainedMember entry in history.Values){uint code;if(GetExitCodeProcess(entry.Handle,out code)){entry.Value.ExitCode=code;entry.Value.Alive=WaitForSingleObject(entry.Handle,0)==WAIT_TIMEOUT;}}
    List<Member> values=new List<Member>();foreach(RetainedMember entry in history.Values)values.Add(entry.Value);return values.ToArray();
  }
  public bool Stop(uint milliseconds) {
    if(disposed)throw new ObjectDisposedException("SplittyFixtureJob");
    if(!TerminateJobObject(job,1))throw Error("TerminateJobObject");
    DateTime until=DateTime.UtcNow.AddMilliseconds(milliseconds);
    while(ActiveProcesses!=0&&DateTime.UtcNow<until)Thread.Sleep(10);
    return ActiveProcesses==0;
  }
  public void Dispose() {
    if(disposed)return;disposed=true;
    // Closing our only noninherited Job handle kills only its kernel members.
    if(job!=null)job.Dispose();foreach(RetainedMember entry in history.Values)entry.Handle.Dispose();history.Clear();if(thread!=null)thread.Dispose();if(process!=null)process.Dispose();
  }
}
