# Diagnostic only. Never run on a developer machine or a self-hosted runner.
[CmdletBinding()]
param(
 [Parameter(Mandatory=$true)][switch]$AllowDisposableHostedWslConfig,
 [Parameter(Mandatory=$true)][string]$Artifact,
 [Parameter(Mandatory=$true)][string]$Evidence
)
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
if(-not $AllowDisposableHostedWslConfig -or $env:GITHUB_ACTIONS -cne 'true' -or $env:RUNNER_ENVIRONMENT -cne 'github-hosted' -or $env:RUNNER_OS -cne 'Windows'){
 throw 'Only an explicitly opted-in fresh GitHub-hosted Windows VM may change WSL configuration'
}
$evidencePath=[IO.Path]::GetFullPath($Evidence)
$tempPath=[IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd([char[]]'\/')
if(-not [IO.Path]::GetDirectoryName($evidencePath).Equals($tempPath,[StringComparison]::OrdinalIgnoreCase)-or(Test-Path $evidencePath)){throw 'evidence must be a fresh direct child of RUNNER_TEMP'}
New-Item -ItemType Directory $evidencePath|Out-Null
$Evidence=$evidencePath
$clock=[Diagnostics.Stopwatch]::StartNew()
$events=[Collections.Generic.List[object]]::new()
$jobs=[Collections.Generic.List[object]]::new()
$ownedName='splitty-native-fixture-'+[guid]::NewGuid().ToString('N').Substring(0,12)+'-a'
$ownedPath=Join-Path $Evidence 'distribution'
$wsl=Join-Path $env:SystemRoot 'System32\wsl.exe'
$config=Join-Path $env:USERPROFILE '.wslconfig'
$cleanupPhase=$false;$configChanged=$false;$importAttempted=$false;$passed=$false;$failure=$null;$cleanupErrors=[Collections.Generic.List[string]]::new()
$hadConfig=Test-Path $config
$originalConfig=$null
if($hadConfig){$originalConfig=[IO.File]::ReadAllBytes($config)}
if($hadConfig){[IO.File]::WriteAllBytes((Join-Path $Evidence 'original.wslconfig'),$originalConfig)}
Add-Type -Path (Join-Path $PSScriptRoot '../tauri-windows-native-smoke/fixture-job.cs')
# Git on Windows may check out CRLF; /bin/sh must receive LF source.
$guestGuard=([IO.File]::ReadAllText((Join-Path $PSScriptRoot '../tauri-windows-native-smoke/fixture-guest-guard.sh'))).Replace("`r`n","`n").Trim()
function Registrations {
 $key='HKCU:\Software\Microsoft\Windows\CurrentVersion\Lxss'
 if(Test-Path $key){Get-ChildItem $key|ForEach-Object{Get-ItemProperty $_.PSPath|Select-Object DistributionName,BasePath,Version}}
}
function Assert-Owned([switch]$AllowAbsent){
 $rows=@(Registrations)
 if($rows.Count-eq 0-and $AllowAbsent){return}
 if($rows.Count-ne 1-or $rows[0].DistributionName-cne $ownedName){throw 'unexpected WSL registration; refusing global VM control'}
 $actual=[IO.Path]::GetFullPath(([string]$rows[0].BasePath).Replace('\\?\','')).TrimEnd([char[]]'\/')
 if(-not $actual.Equals($ownedPath,[StringComparison]::OrdinalIgnoreCase)-or $rows[0].Version-ne 2){throw 'owned WSL2 registration identity changed'}
}
function Deadline-Minutes {if($script:cleanupPhase){return 18}else{return 15}}
function Start-Owned([string[]]$Arguments){
 if($clock.Elapsed.TotalMinutes-ge (Deadline-Minutes)){throw '15-minute diagnostic deadline exceeded'}
 $job=[SplittyFixtureJob]::new($wsl,$Arguments,$Evidence,$false)
 $jobs.Add($job)
 return $job
}
function Run-Owned([string[]]$Arguments,[int]$Seconds=30){
 $job=Start-Owned $Arguments
 try{
  $until=[DateTime]::UtcNow.AddSeconds($Seconds)
  while($job.RootAlive-and [DateTime]::UtcNow-lt $until-and $clock.Elapsed.TotalMinutes-lt (Deadline-Minutes)){Start-Sleep -Milliseconds 50}
  if($job.RootAlive){throw "bounded wsl.exe command timed out: $Arguments"}
  $code=$job.RootExitCode
  $stdout=[IO.File]::ReadAllText($job.StandardOutputPath).Replace([string][char]0,'').Trim()
  $stderr=[IO.File]::ReadAllText($job.StandardErrorPath).Replace([string][char]0,'').Trim()
  if($stdout.Length-gt 65536-or $stderr.Length-gt 65536){throw 'unexpectedly large diagnostic output'}
  $events.Add([ordered]@{Arguments=$Arguments;Exit=$code;Stdout=$stdout;Stderr=$stderr})
  if($code-ne 0){throw "wsl.exe failed ($code): $Arguments -- $stderr"}
  return $stdout
 }finally{
  if($job.ActiveProcesses-ne 0-and-not $job.Stop(5000)){throw 'owned Windows Job did not empty'}
  $job.Dispose()
 }
}
function Guest-Arguments([string[]]$Arguments){
 Assert-Owned
 return @('--distribution',$ownedName,'--exec','/bin/sh','-c',$guestGuard,'splitty-fixture-guard',$ownedName)+$Arguments
}
function Guest([string[]]$Arguments){Run-Owned (Guest-Arguments $Arguments)}
function Shutdown-Owned {
 Assert-Owned -AllowAbsent
 Run-Owned @('--shutdown') 60|Out-Null
}
function Write-Config([bool]$Enabled){
 Assert-Owned -AllowAbsent
 $text="[wsl2]`nnetworkingMode=nat`nlocalhostForwarding=$($Enabled.ToString().ToLowerInvariant())`n"
 $script:configChanged=$true
 [IO.File]::WriteAllText($config,$text,[Text.UTF8Encoding]::new($false))
 [IO.File]::WriteAllText((Join-Path $Evidence ("config-"+$Enabled+".txt")),$text)
 Shutdown-Owned
}
function Read-Nonce([int]$Port){
 $client=[Net.Sockets.TcpClient]::new()
 try{
  $task=$client.ConnectAsync('127.0.0.1',$Port)
  if(-not $task.Wait(2000)){throw 'localhost connection timeout'}
  [void]$task.GetAwaiter().GetResult()
  $stream=$client.GetStream();$stream.ReadTimeout=2000
  $reader=[IO.StreamReader]::new($stream)
  try{return $reader.ReadLine()}finally{$reader.Dispose()}
 }finally{$client.Dispose()}
}
function Test-LocalhostBlocked([int]$Port){
 $client=[Net.Sockets.TcpClient]::new()
 try{
  $task=$client.ConnectAsync('127.0.0.1',$Port)
  if(-not $task.Wait(2000)){return $true}
  [void]$task.GetAwaiter().GetResult()
  # Any established TCP connection fails the negative control, even if its
  # payload is absent or wrong. A read failure is not proof of isolation.
  return $false
 }catch{
  $cause=$_.Exception.GetBaseException()
  if($cause-is [Net.Sockets.SocketException]){return $true}
  throw
 }finally{$client.Dispose()}
}
try{
 $before=@(Registrations)
 $before|ConvertTo-Json -Depth 4|Set-Content (Join-Path $Evidence 'registrations-before.json')
 if($before.Count-ne 0){throw 'fresh VM preflight requires zero pre-existing distributions'}
 $manifest=Get-Content (Join-Path $Artifact 'manifest.json') -Raw|ConvertFrom-Json
 if($manifest.sourceSha-cne $env:GITHUB_SHA){throw 'fixture artifact belongs to another source revision'}
 if((Get-FileHash (Join-Path $Artifact 'rootfs.tar') -Algorithm SHA256).Hash.ToLowerInvariant()-cne $manifest.rootfsSha256){throw 'rootfs artifact digest mismatch'}
 $manifest|ConvertTo-Json -Depth 8|Set-Content (Join-Path $Evidence 'fixture-manifest.json')
 [ordered]@{ImageOS=$env:ImageOS;ImageVersion=$env:ImageVersion;Runner=$env:RUNNER_NAME;Commit=$env:GITHUB_SHA;Run=$env:GITHUB_RUN_ID;WslVersion=(Run-Owned @('--version'))}|ConvertTo-Json|Set-Content (Join-Path $Evidence 'host.json')
 Write-Config $true
 New-Item -ItemType Directory $ownedPath|Out-Null
 [ordered]@{Name=$ownedName;Path=$ownedPath;Before=$before}|ConvertTo-Json -Depth 5|Set-Content (Join-Path $Evidence 'ownership.json')
 $importAttempted=$true
 Run-Owned @('--import',$ownedName,$ownedPath,(Join-Path $Artifact 'rootfs.tar'),'--version','2') 120|Out-Null
 Assert-Owned
 if((Guest @('/usr/bin/id','-u'))-cne '1000'){throw 'fixture default user is not UID1000'}
 if((Guest @('/usr/bin/printenv','HOME'))-cne '/home/splitty-fixture'){throw 'fixture HOME mismatch'}
 $kernel=Guest @('/usr/bin/uname','-r')
 if($kernel-notmatch '(?i)microsoft.*wsl2'){throw "guest is not an actual WSL2 kernel: $kernel"}
 $identity=[guid]::NewGuid().ToString('N')
 if((Guest @('/bin/sh','-c','printf "%s" "$1"','identity',$identity))-cne $identity){throw 'guarded guest transaction failed'}
 # Validate the Windows-side socket probe independently, with a real listener.
 $positive=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,0);$positive.Start()
 try{
  $port=([Net.IPEndPoint]$positive.LocalEndpoint).Port
  $accept=$positive.AcceptTcpClientAsync()
  $client=[Net.Sockets.TcpClient]::new();$connect=$client.ConnectAsync('127.0.0.1',$port)
  if(-not $connect.Wait(2000)){throw 'Windows positive-control connect timeout'}
  [void]$connect.GetAwaiter().GetResult()
  if(-not $accept.Wait(2000)){throw 'Windows loopback positive control failed'}
  $peer=$accept.GetAwaiter().GetResult();$peer.Dispose();$client.Dispose()
 }finally{$positive.Stop()}
 foreach($enabled in @($true,$false)){
  if(-not $enabled){Write-Config $false}
  $nonce=[guid]::NewGuid().ToString('N')
  $server=Start-Owned (Guest-Arguments @('/usr/bin/fixture-net','serve',[string]$port,$nonce))
  try{
   $until=[DateTime]::UtcNow.AddSeconds(15)
   do{
    # The live Job still owns its stdout writer. Share write access while
    # reading its bounded readiness line; ReadAllText uses incompatible sharing.
    $liveLog=[IO.File]::Open($server.StandardOutputPath,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::ReadWrite)
    $liveReader=[IO.StreamReader]::new($liveLog)
    try{$ready=$liveReader.ReadToEnd()}finally{$liveReader.Dispose()}
    if($ready.Contains("READY 127.0.0.1:$port $nonce")){break}
    if(-not $server.RootAlive-or [DateTime]::UtcNow-ge $until){throw 'owned guest listener failed readiness'}
    Start-Sleep -Milliseconds 50
   }while($true)
   if((Guest @('/usr/bin/fixture-net','check',[string]$port,$nonce))-cne $nonce){throw 'guest loopback positive control failed'}
   if($enabled){
    # WSL forwarding startup is asynchronous; only this positive control may
    # retry, within a fixed 10-second deadline. Negative probes never retry.
    $until=[DateTime]::UtcNow.AddSeconds(10);$reply=$null
    do{try{$reply=Read-Nonce $port}catch{if([DateTime]::UtcNow-ge $until){throw};Start-Sleep -Milliseconds 100}}while($reply-ne $nonce-and [DateTime]::UtcNow-lt $until)
    if($reply-cne $nonce){throw 'forwarding=true failed exact-nonce positive control'}
    $events.Add(@{Proof='forwarding=true';GuestNonce=$nonce;WindowsNonce=$reply;Port=$port})
   }else{
    $blocked=Test-LocalhostBlocked $port
    if(-not $blocked){throw 'forwarding=false unexpectedly accepted a localhost TCP connection'}
    if((Guest @('/usr/bin/fixture-net','check',[string]$port,$nonce))-cne $nonce){throw 'guest listener disappeared during negative control'}
    $events.Add(@{Proof='forwarding=false';GuestNonceBeforeAndAfter=$nonce;WindowsBlocked=$blocked;Port=$port})
   }
  }finally{
   Shutdown-Owned
   if($server.ActiveProcesses-ne 0-and-not $server.Stop(5000)){throw 'server Windows Job cleanup failed'}
   $server.Dispose()
  }
 }
 $passed=$true
}catch{
 $failure=$_.ToString();Write-Warning $failure
}finally{
 $script:cleanupPhase=$true
 if($importAttempted){
  try{
   Assert-Owned
   Run-Owned @('--unregister',$ownedName) 60|Out-Null
   if(@(Registrations).Count-ne 0){throw 'owned registration remains after cleanup'}
  }catch{$cleanupErrors.Add($_.ToString())}
 }
 foreach($job in $jobs){try{$job.Dispose()}catch{$cleanupErrors.Add($_.ToString())}}
 if($configChanged){try{
  if($hadConfig){[IO.File]::WriteAllBytes($config,$originalConfig)}else{Remove-Item -LiteralPath $config -ErrorAction Stop}
  if($hadConfig-and [Convert]::ToBase64String([IO.File]::ReadAllBytes($config))-cne [Convert]::ToBase64String($originalConfig)){throw 'configuration bytes not restored'}
  if(-not $hadConfig-and(Test-Path $config)){throw 'configuration absence not restored'}
 }catch{$cleanupErrors.Add($_.ToString())}}
 $events|ConvertTo-Json -Depth 8|Set-Content (Join-Path $Evidence 'events.json')
 [ordered]@{Passed=($passed-and $cleanupErrors.Count-eq 0);Failure=$failure;CleanupErrors=@($cleanupErrors);OwnedHandles=[SplittyFixtureJob]::LiveHandleCount;Seconds=$clock.Elapsed.TotalSeconds;Scope='WSL2 NAT localhost forwarding preflight only; no native application proof'}|ConvertTo-Json -Depth 5|Set-Content (Join-Path $Evidence 'result.json')
}
if(-not $passed-or $cleanupErrors.Count-ne 0-or [SplittyFixtureJob]::LiveHandleCount-ne 0){throw 'Hosted WSL preflight FAILED/UNSUPPORTED; inspect evidence (never a skipped proof)'}
Write-Host 'PROVED hosted WSL2 localhostForwarding true/false with guest and Windows positive controls; owned distro removed and config restored'
