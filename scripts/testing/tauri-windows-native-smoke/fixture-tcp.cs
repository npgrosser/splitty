// Test-only TCP inventory. No process access, ownership or termination APIs.
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Net;
using System.Net.Sockets;
using System.Runtime.InteropServices;
public static class SplittyFixtureTcp {
  [DllImport("iphlpapi.dll",SetLastError=true)] static extern uint GetExtendedTcpTable(IntPtr table,ref uint size,bool order,uint family,uint tableClass,uint reserved);
  public sealed class Row { public uint Family,State,OwningProcess;public string LocalAddress,RemoteAddress;public int LocalPort,RemotePort; }
  static string Address(IntPtr row,int offset,int bytes,uint scope) {byte[] value=new byte[bytes];Marshal.Copy(IntPtr.Add(row,offset),value,0,bytes);return bytes==16?new IPAddress(value,scope).ToString():new IPAddress(value).ToString();}
  static uint U32(IntPtr row,int offset){return unchecked((uint)Marshal.ReadInt32(row,offset));}
  static int Port(IntPtr row,int offset){return Marshal.ReadByte(row,offset)*256+Marshal.ReadByte(row,offset+1);}
  public static Row[] Snapshot(uint[] ownedIds) {
    if(ownedIds==null||ownedIds.Length>256)throw new ArgumentException("bounded owned PID list required");
    HashSet<uint> ids=new HashSet<uint>(ownedIds);List<Row> rows=new List<Row>();
    foreach(uint family in new uint[]{2,23}) {
      uint size=0;uint error=GetExtendedTcpTable(IntPtr.Zero,ref size,false,family,5,0);
      if(error!=122&&error!=0)throw new Win32Exception((int)error,"TCP table size");
      bool complete=false;
      for(int attempt=0;attempt<3;attempt++) {
        if(size<4||size>1024*1024)throw new InvalidOperationException("TCP table buffer bound");
        uint allocated=size;IntPtr buffer=Marshal.AllocHGlobal((int)allocated);
        try {
          error=GetExtendedTcpTable(buffer,ref size,false,family,5,0);
          if(error==122)continue;
          if(error!=0)throw new Win32Exception((int)error,"TCP table capture");
          int stride=family==2?24:56;uint count=U32(buffer,0);
          if(count>16384||4L+(long)count*stride>allocated)throw new InvalidOperationException("invalid TCP table rows");
          for(int i=0;i<count;i++) {
            IntPtr row=IntPtr.Add(buffer,4+i*stride);uint pid=U32(row,family==2?20:52);
            if(!ids.Contains(pid))continue;
            rows.Add(new Row{Family=family,State=U32(row,family==2?0:48),OwningProcess=pid,
              LocalAddress=Address(row,family==2?4:0,family==2?4:16,family==2?0:U32(row,16)),LocalPort=Port(row,family==2?8:20),
              RemoteAddress=Address(row,family==2?12:24,family==2?4:16,family==2?0:U32(row,40)),RemotePort=Port(row,family==2?16:44)});
          }
          complete=true;break;
        } finally {Marshal.FreeHGlobal(buffer);}
      }
      if(!complete)throw new InvalidOperationException("TCP table changed beyond bounded retries");
    }
    return rows.ToArray();
  }
  // Only connects to the listener created here; does not inspect/connect to
  // arbitrary sampled ports. Verifies actual API byte order and state decoder.
  static string SelfTestFamily(IPAddress address,uint family) {
    uint pid=(uint)System.Diagnostics.Process.GetCurrentProcess().Id;
    TcpListener listener=new TcpListener(address,0);listener.Start();
    TcpClient client=null,peer=null;
    try {
      int port=((IPEndPoint)listener.LocalEndpoint).Port;bool listening=false;
      foreach(Row row in Snapshot(new uint[]{pid}))if(row.Family==family&&row.State==2&&row.LocalAddress==address.ToString()&&row.LocalPort==port)listening=true;
      if(!listening){string detail="";foreach(Row row in Snapshot(new uint[]{pid}))detail+=" family="+row.Family+" state="+row.State+" local="+row.LocalAddress+":"+row.LocalPort+" remote="+row.RemoteAddress+":"+row.RemotePort;throw new InvalidOperationException("own listener decoder control failed family="+family+" expected="+address+":"+port+detail);}
      client=new TcpClient(address.AddressFamily);var connected=client.ConnectAsync(address,port);
      if(!connected.Wait(2000))throw new TimeoutException("own TCP control connect");connected.GetAwaiter().GetResult();
      var accepted=listener.AcceptTcpClientAsync();if(!accepted.Wait(2000))throw new TimeoutException("own TCP control accept");peer=accepted.GetAwaiter().GetResult();
      int clientPort=((IPEndPoint)client.Client.LocalEndPoint).Port;bool established=false;
      foreach(Row row in Snapshot(new uint[]{pid}))if(row.Family==family&&row.State==5&&row.LocalAddress==address.ToString()&&row.LocalPort==clientPort&&row.RemotePort==port)established=true;
      if(!established)throw new InvalidOperationException("own established decoder control failed");
      return "PASS family="+family+" own listener=2 established=5 and exact port byte order";
    } finally {if(peer!=null)peer.Dispose();if(client!=null)client.Dispose();listener.Stop();}
  }
  public static string SelfTest(){return SelfTestFamily(IPAddress.Loopback,2)+"; "+SelfTestFamily(IPAddress.IPv6Loopback,23);}
}
