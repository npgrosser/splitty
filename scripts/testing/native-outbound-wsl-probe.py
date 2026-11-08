#!/usr/bin/env python3
"""Bounded owned Windows/WSL localhost endpoint and collision observation.

This does not launch Splitty or claim native Windows forwarding acceptance.
Run inside the selected WSL distribution; the only Windows children are the
PowerShell probes started here, and their competing listeners stop in finally.
"""

import argparse
import base64
import http.server
import json
import os
from pathlib import Path
import secrets
import select
import socket
import subprocess
import threading

POWERSHELL = Path("/mnt/c/WINDOWS/System32/WindowsPowerShell/v1.0/powershell.exe")
CURL = Path("/mnt/c/WINDOWS/System32/curl.exe")


def responder(identity):
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            body = json.dumps({"identity": identity, "path": self.path,
                               "pid": os.getpid()}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass
    return Handler


def powershell(script, timeout=12):
    encoded = base64.b64encode(script.encode("utf-16le")).decode("ascii")
    run = subprocess.run([str(POWERSHELL), "-NoProfile", "-NonInteractive",
                          "-EncodedCommand", encoded], capture_output=True,
                         text=True, timeout=timeout)
    if run.returncode:
        raise RuntimeError(f"PowerShell exited {run.returncode}: {run.stderr[-1000:]}")
    return run.stdout.strip()


def windows_port_free(port):
    script = ("$ErrorActionPreference='Stop'; "
              f"$items=@(Get-NetTCPConnection -State Listen -LocalPort {port} "
              "-ErrorAction SilentlyContinue); "
              "if($items.Count -eq 0){'free'}else{'occupied'}")
    return powershell(script, timeout=8).strip() == "free"


def windows_curl(port, token):
    values = {}
    for host in ("localhost", "127.0.0.1", "[::1]"):
        for family in ("default", "ipv4", "ipv6"):
            url = f"http://{host}:{port}/probe?token={token}"
            arguments = [str(CURL), "--silent", "--show-error", "--noproxy", "*",
                         "--max-time", "3"]
            if family == "ipv4":
                arguments.append("-4")
            elif family == "ipv6":
                arguments.append("-6")
            run = subprocess.run([*arguments, url], capture_output=True, text=True, timeout=5)
            values[f"{host}/{family}"] = {"url": url, "exitCode": run.returncode,
                                           "body": run.stdout.strip(),
                                           "error": run.stderr.strip()[:300]}
    return values


def windows_probe(port, token):
    script = r'''
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$port=__PORT__
$token='__TOKEN__'
function Fetch([string]$url) {
  try {
    $request=[System.Net.HttpWebRequest]::Create($url)
    $request.Proxy=$null; $request.Timeout=3500
    $response=$request.GetResponse()
    $reader=[System.IO.StreamReader]::new($response.GetResponseStream())
    $body=$reader.ReadToEnd(); $reader.Close(); $response.Close()
    return @{ok=$true;body=$body}
  } catch { return @{ok=$false;error=$_.Exception.Message} }
}
function CollisionFetch([string]$url,$v4,$v6) {
  try {
    $request=[System.Net.HttpWebRequest]::Create($url)
    $request.Proxy=$null; $request.Timeout=3500
    $pending=$request.BeginGetResponse($null,$null)
    $deadline=[DateTime]::UtcNow.AddSeconds(4)
    $served='none'
    while (-not $pending.IsCompleted -and [DateTime]::UtcNow -lt $deadline) {
      $listener=$null
      if($v6 -and $v6.Pending()) {$listener=$v6;$served='windows-v6'}
      elseif($v4 -and $v4.Pending()) {$listener=$v4;$served='windows-v4'}
      if($listener) {
        $client=$listener.AcceptTcpClient()
        $stream=$client.GetStream(); $buffer=New-Object byte[] 4096
        $count=$stream.Read($buffer,0,$buffer.Length)
        $body="{`"identity`":`"$served-$token`",`"path`":`"/probe`"}"
        $reply="HTTP/1.1 200 OK`r`nContent-Type: application/json`r`nContent-Length: $([Text.Encoding]::UTF8.GetByteCount($body))`r`nConnection: close`r`n`r`n$body"
        $bytes=[Text.Encoding]::UTF8.GetBytes($reply)
        $stream.Write($bytes,0,$bytes.Length);$stream.Flush();$client.Close()
      } else {Start-Sleep -Milliseconds 20}
    }
    $response=$request.EndGetResponse($pending)
    $reader=[System.IO.StreamReader]::new($response.GetResponseStream())
    $body=$reader.ReadToEnd();$reader.Close();$response.Close()
    return @{ok=$true;body=$body;served=$served}
  } catch {return @{ok=$false;error=$_.Exception.Message}}
}
$result=@{processId=$PID;baseline=@{};collision=@{};bind=@{};listenersBefore=@()}
$result.listenersBefore=@(Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue | Select-Object LocalAddress,LocalPort,OwningProcess)
foreach($name in @('localhost','127.0.0.1','[::1]')) {
  $result.baseline[$name]=Fetch "http://${name}:$port/probe?token=$token"
}
$v4=$null;$v6=$null
try {
  try {$v4=[System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback,$port);$v4.Start();$result.bind.v4='owned'}
  catch {$result.bind.v4=$_.Exception.Message;$v4=$null}
  try {$v6=[System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::IPv6Loopback,$port);$v6.Server.SetSocketOption([System.Net.Sockets.SocketOptionLevel]::IPv6,[System.Net.Sockets.SocketOptionName]::IPv6Only,$true);$v6.Start();$result.bind.v6='owned'}
  catch {$result.bind.v6=$_.Exception.Message;$v6=$null}
  if($v4 -or $v6) {
    foreach($name in @('localhost','127.0.0.1','[::1]')) {
      $result.collision[$name]=CollisionFetch "http://${name}:$port/probe?token=$token" $v4 $v6
    }
  }
} finally {
  if($v4){$v4.Stop()}
  if($v6){$v6.Stop()}
}
ConvertTo-Json -InputObject $result -Depth 7 -Compress
'''.replace("__PORT__", str(port)).replace("__TOKEN__", token)
    return json.loads(powershell(script, timeout=25).splitlines()[-1])


def rival_first(token):
    # Start a Windows-owned rival before WSL binds anything. Mirrored mode may
    # make the later WSL bind fail; that is a valid conflict observation.
    script = r'''
$ErrorActionPreference='Stop'
$port=0;$token='__TOKEN__'
$v4=$null;$v6=$null
try {
  try {$v4=[System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback,$port);$v4.Start();$port=$v4.LocalEndpoint.Port;$four='owned'}
  catch {$v4=$null;$four=$_.Exception.Message}
  try {$v6=[System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::IPv6Loopback,$port);$v6.Server.SetSocketOption([System.Net.Sockets.SocketOptionLevel]::IPv6,[System.Net.Sockets.SocketOptionName]::IPv6Only,$true);$v6.Start();$six='owned'}
  catch {$v6=$null;$six=$_.Exception.Message}
  [Console]::Out.WriteLine((ConvertTo-Json -InputObject @{ready=$true;pid=$PID;port=$port;v4=$four;v6=$six} -Compress))
  [Console]::Out.Flush()
  $deadline=[DateTime]::UtcNow.AddSeconds(16)
  while([DateTime]::UtcNow -lt $deadline) {
    $listener=$null;$identity=''
    if($v6 -and $v6.Pending()) {$listener=$v6;$identity="windows-v6-$token"}
    elseif($v4 -and $v4.Pending()) {$listener=$v4;$identity="windows-v4-$token"}
    if($listener) {
      $client=$listener.AcceptTcpClient()
      try {
        $stream=$client.GetStream();$stream.ReadTimeout=2000
        $buffer=New-Object byte[] 4096
        $count=$stream.Read($buffer,0,$buffer.Length)
        $body="{`"identity`":`"$identity`",`"path`":`"/probe`"}"
        $reply="HTTP/1.1 200 OK`r`nContent-Type: application/json`r`nContent-Length: $([Text.Encoding]::UTF8.GetByteCount($body))`r`nConnection: close`r`n`r`n$body"
        $bytes=[Text.Encoding]::UTF8.GetBytes($reply)
        $stream.Write($bytes,0,$bytes.Length);$stream.Flush()
      } finally {$client.Close()}
    } else {Start-Sleep -Milliseconds 20}
  }
} finally {
  if($v4){$v4.Stop()}
  if($v6){$v6.Stop()}
}
'''.replace("__TOKEN__", token)
    encoded = base64.b64encode(script.encode("utf-16le")).decode("ascii")
    child = subprocess.Popen([str(POWERSHELL), "-NoProfile", "-NonInteractive",
                              "-EncodedCommand", encoded], stdin=subprocess.DEVNULL,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    server = None
    try:
        readable, _, _ = select.select([child.stdout], [], [], 7)
        if not readable:
            raise TimeoutError("owned Windows rival did not become ready")
        line = child.stdout.readline().strip()
        ready = json.loads(line)
        if not ready.get("ready") or ready.get("v4") != "owned":
            raise RuntimeError(f"Windows rival could not bind: {ready}")
        port = ready["port"]
        result = {"port": port, "windowsRival": ready, "wslBind": None,
                  "windowsCurl": None, "windowsRivalAliveDuringProbe": child.poll() is None}
        try:
            server = http.server.ThreadingHTTPServer(("127.0.0.1", port),
                                                      responder("wsl-rival-" + token))
            threading.Thread(target=server.serve_forever, daemon=True).start()
            result["wslBind"] = "owned"
        except OSError as error:
            result["wslBind"] = repr(error)
        result["windowsCurl"] = windows_curl(port, token)
        result["windowsRivalAliveAfterRequests"] = child.poll() is None
        return result
    finally:
        if server is not None:
            server.shutdown()
            server.server_close()
        try:
            child.wait(timeout=18)
        except subprocess.TimeoutExpired:
            child.terminate()
            child.wait(timeout=3)
        if "result" in locals():
            result["windowsRivalExitCode"] = child.returncode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if "microsoft" not in os.uname().release.lower() or not POWERSHELL.is_file() or not CURL.is_file():
        parser.error("run inside WSL with Windows PowerShell available")
    args.output.mkdir(parents=True, exist_ok=False)
    token = secrets.token_hex(8)
    servers = []
    result = {"completed": False, "passed": False,
              "scope": "network endpoint observation only; no Splitty app/forward",
              "token": token, "wslRelease": os.uname().release, "servers": {},
              "windows": None, "error": None}
    mode = subprocess.run(["wslinfo", "--networking-mode"], capture_output=True,
                          text=True, timeout=4)
    result["networkingMode"] = mode.stdout.strip() if mode.returncode == 0 else None
    try:
        for _ in range(16):
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0),
                                                      responder("wsl-v4-" + token))
            port = server.server_address[1]
            if windows_port_free(port):
                servers.append(server)
                break
            server.server_close()
        else:
            raise RuntimeError("no candidate port free in both namespaces")
        # Production's outbound SSH forward binds only 127.0.0.1. Keep the
        # baseline identical before probing rivals in either namespace.
        for server in servers:
            threading.Thread(target=server.serve_forever, daemon=True).start()
            result["servers"]["v6" if server.address_family == socket.AF_INET6 else "v4"] = {
                "address": server.server_address[0], "port": port, "pid": os.getpid()}
        result["windows"] = windows_probe(port, token)
        result["windowsCurl"] = windows_curl(port, token)
        for server in servers:
            server.shutdown()
            server.server_close()
        servers.clear()
        result["rivalFirst"] = rival_first(token)
        def response(record, identity, path):
            try:
                body = json.loads(record["body"])
            except (KeyError, json.JSONDecodeError):
                return False
            return (record["exitCode"] == 0 and body.get("identity") == identity
                    and body.get("path") == path)

        baseline = result["windowsCurl"]
        rival = result["rivalFirst"]
        rival_curl = rival["windowsCurl"]
        result["observations"] = {
            "wslV4LocalhostDefault": response(
                baseline["localhost/default"], "wsl-v4-" + token,
                "/probe?token=" + token),
            "wslV4LocalhostForcedV4": response(
                baseline["localhost/ipv4"], "wsl-v4-" + token,
                "/probe?token=" + token),
            "rivalFirstWslBindConflict": (
                rival["wslBind"].startswith("OSError(98,")
                and rival["windowsRival"]["v4"] == "owned"),
            "rivalFirstDefaultIsWindows": (
                response(rival_curl["localhost/default"], "windows-v4-" + token, "/probe")
                or response(rival_curl["localhost/default"], "windows-v6-" + token, "/probe")),
            "rivalFirstForcedV4IsWindows": response(
                rival_curl["localhost/ipv4"], "windows-v4-" + token, "/probe"),
            "rivalPreservedThroughRequests": (
                rival["windowsRivalAliveDuringProbe"]
                and rival["windowsRivalAliveAfterRequests"]),
            "rivalClosedNormally": rival["windowsRivalExitCode"] == 0,
        }
        result["completed"] = True
        result["passed"] = all(result["observations"].values())
    except Exception as error:
        result["error"] = repr(error)
    finally:
        for server in servers:
            server.shutdown()
            server.server_close()
        result["ownedListenersClosed"] = True
        (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"completed": result["completed"], "passed": result["passed"],
                          "observations": result.get("observations"), "error": result["error"],
                          "result": str(args.output / "result.json")}, indent=2))
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
