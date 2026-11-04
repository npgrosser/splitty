using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
public class FixtureConsole {
  [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)] struct Startup { public uint Size; public string Reserved,Desktop,Title; public uint X,Y,XSize,YSize,XChars,YChars,Fill,Flags; public ushort Show,ReservedBytes; public IntPtr ReservedData,StdIn,StdOut,StdErr; }
  [StructLayout(LayoutKind.Sequential)] struct Info { public IntPtr Process,Thread; public uint Pid,Tid; }
  [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool CreateProcess(string exe,StringBuilder command,IntPtr pa,IntPtr ta,bool inherit,uint flags,IntPtr env,string cwd,ref Startup start,out Info info);
  [DllImport("kernel32.dll")] static extern bool TerminateProcess(IntPtr handle,uint code);
  [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr handle,uint ms);
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
  [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)]static extern uint GetFinalPathNameByHandle(IntPtr handle,StringBuilder path,uint size,uint flags);
  static string Q(string value) { return "\""+value.Replace("\\","\\").Replace("\"","\\\"")+"\""; }
  static string Exe { get {return System.Reflection.Assembly.GetExecutingAssembly().Location;} }
  static void Spawn(string mode,string dir) { using(Process p=Process.Start(new ProcessStartInfo(Exe,Q(mode)+" "+Q(dir)){UseShellExecute=false,CreateNoWindow=true})){} }
  public static int Main(string[] args) {
    string mode=args[0],dir=args[1];Directory.CreateDirectory(dir);
    if(mode=="commandline") {File.WriteAllText(Path.Combine(dir,"commandline-result"),Environment.CommandLine);return 0;}
    if(mode=="ambient") {
      StringBuilder path=new StringBuilder(32768);uint length=GetFinalPathNameByHandle(new IntPtr(Int64.Parse(args[2])),path,32768,0);
      string actual=path.ToString();if(actual.StartsWith(@"\\?\"))actual=actual.Substring(4);
      bool inherited=length!=0&&String.Equals(actual,args[3],StringComparison.OrdinalIgnoreCase);
      File.WriteAllText(Path.Combine(dir,"ambient-result"),inherited?"inherited":"excluded");return inherited?1:0;
    }
    if(mode=="mark") {File.WriteAllText(Path.Combine(dir,"should-not-run"),"executed");return 0;}
    if(mode=="breakaway") {
      Startup start=new Startup();start.Size=(uint)Marshal.SizeOf(typeof(Startup));Info info;
      bool started=CreateProcess(Exe,new StringBuilder(Q(Exe)+" "+Q("mark")+" "+Q(dir)),IntPtr.Zero,IntPtr.Zero,false,0x01000000|0x08000000,IntPtr.Zero,dir,ref start,out info);
      int error=Marshal.GetLastWin32Error();
      // Unexpected success is still cleaned only through our exact freshly
      // returned handle; it can never authorize a PID lookup/traversal.
      if(started){TerminateProcess(info.Process,1);WaitForSingleObject(info.Process,5000);CloseHandle(info.Thread);CloseHandle(info.Process);}
      File.WriteAllText(Path.Combine(dir,"breakaway-result"),(started?"started":"refused")+":"+error);return started?1:0;
    }
    File.WriteAllText(Path.Combine(dir,mode+"-ready"),Process.GetCurrentProcess().Id.ToString());
    if(mode=="tree")Spawn("child",dir);if(mode=="child")Spawn("grandchild",dir);
    Thread.Sleep(60000);return 0;
  }
}
