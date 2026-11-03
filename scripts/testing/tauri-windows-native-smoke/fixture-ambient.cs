// Console-test-only ambient inheritable marker. Never used by runtime drivers.
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public sealed class SplittyAmbientFixtureHandle : IDisposable {
  [StructLayout(LayoutKind.Sequential)] struct Attributes {public uint Size;public IntPtr Descriptor;[MarshalAs(UnmanagedType.Bool)]public bool Inherit;}
  [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)]static extern SafeFileHandle CreateFile(string path,uint access,uint share,ref Attributes attributes,uint disposition,uint flags,IntPtr template);
  [DllImport("kernel32.dll",SetLastError=true)]static extern bool GetHandleInformation(SafeFileHandle handle,out uint flags);
  readonly SafeFileHandle handle;
  public SplittyAmbientFixtureHandle(string path){Attributes attributes=new Attributes();attributes.Size=(uint)Marshal.SizeOf(typeof(Attributes));attributes.Inherit=true;handle=CreateFile(path,0xc0000000,3,ref attributes,1,0,IntPtr.Zero);if(handle.IsInvalid)throw new Win32Exception();}
  public long Value {get{return handle.DangerousGetHandle().ToInt64();}}
  public bool Inheritable {get{uint flags;if(!GetHandleInformation(handle,out flags))throw new Win32Exception();return (flags&1)!=0;}}
  public bool Closed {get{return handle.IsClosed;}}
  public void Dispose(){handle.Dispose();}
}
