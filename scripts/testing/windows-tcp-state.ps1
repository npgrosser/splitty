# Diagnostic only: never launches WSL, an application, or a process observer.
# All TCP controls belong to this exact PowerShell process and close in finally.
[CmdletBinding()]
param([Parameter(Mandatory=$true)][switch]$AllowHostedSelfSocketControls,
      [Parameter(Mandatory=$true)][string]$Evidence)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
Set-StrictMode -Version Latest
if(-not $AllowHostedSelfSocketControls-or $env:GITHUB_ACTIONS-cne 'true'-or $env:RUNNER_ENVIRONMENT-cne 'github-hosted'-or $env:RUNNER_OS-cne 'Windows'){
 throw 'Self-socket controls require explicit opt-in on a disposable GitHub-hosted Windows VM'
}
$Evidence=[IO.Path]::GetFullPath($Evidence)
if(-not [IO.Path]::GetDirectoryName($Evidence).Equals([IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd([char[]]'\/'),[StringComparison]::OrdinalIgnoreCase)-or(Test-Path $Evidence)){throw 'fresh direct RUNNER_TEMP evidence child required'}
New-Item -ItemType Directory $Evidence|Out-Null
$helper=Join-Path $PSScriptRoot 'tauri-windows-native-smoke/fixture-tcp.cs'
$result=[ordered]@{Passed=$false;Failure=$null;SelfControl=$null;Families=@();SocketsClosed=$false;Scope='Only exact self-owned IPv4/IPv6 loopback listener and established socket controls; no application inventory'}
$controls=[Collections.Generic.List[object]]::new()
$stateFailures=[Collections.Generic.List[string]]::new()
try{
 Add-Type -Path $helper
 $dll=Join-Path $env:SystemRoot 'System32/iphlpapi.dll'
 $identity=[ordered]@{Commit=$env:GITHUB_SHA;Run=$env:GITHUB_RUN_ID;ImageOS=$env:ImageOS;ImageVersion=$env:ImageVersion;PowerShell=$PSVersionTable;OS=[Environment]::OSVersion.ToString();PointerBytes=[IntPtr]::Size;SelfPid=$PID;Api='GetExtendedTcpTable/TCP_TABLE_OWNER_PID_ALL';HelperSha256=(Get-FileHash $helper -Algorithm SHA256).Hash;DriverSha256=(Get-FileHash $PSCommandPath -Algorithm SHA256).Hash;IphlpapiVersion=[Diagnostics.FileVersionInfo]::GetVersionInfo($dll).FileVersion;IphlpapiSha256=(Get-FileHash $dll -Algorithm SHA256).Hash;ExpectedStates=@{Listener=2;Established=5}}
 $identity|ConvertTo-Json -Depth 8|Set-Content (Join-Path $Evidence 'identity.json')
 Copy-Item -LiteralPath $helper -Destination (Join-Path $Evidence 'fixture-tcp.cs')
 Copy-Item -LiteralPath $PSCommandPath -Destination (Join-Path $Evidence 'driver.ps1')
 # Keep independent API self-control failure as evidence, but continue far
 # enough to retain raw provider results for the known controls below.
 try{$result.SelfControl=[SplittyFixtureTcp]::SelfTest()}catch{$result.SelfControl=$_.ToString()}
 foreach($family in @([ordered]@{Address=[Net.IPAddress]::Loopback;Number=2},[ordered]@{Address=[Net.IPAddress]::IPv6Loopback;Number=23})){
  $listener=[Net.Sockets.TcpListener]::new($family.Address,0)
  $control=[pscustomobject]@{Listener=$listener;Client=$null;Peer=$null};$controls.Add($control)
  $listener.Start()
  $port=([Net.IPEndPoint]$listener.LocalEndpoint).Port
  $accepted=$listener.AcceptTcpClientAsync()
  $control.Client=[Net.Sockets.TcpClient]::new($family.Address.AddressFamily)
  $connected=$control.Client.ConnectAsync($family.Address,$port)
  if(-not $connected.Wait(2000)){throw 'own control connect deadline'}
  [void]$connected.GetAwaiter().GetResult()
  if(-not $accepted.Wait(2000)){throw 'own control accept deadline'}
  $control.Peer=$accepted.GetAwaiter().GetResult()
  $clientPort=([Net.IPEndPoint]$control.Client.Client.LocalEndPoint).Port
  $address=$family.Address.ToString()
  # Restrict every retained row to this PID and exact self-created endpoint
  # ports/address. Never infer ownership from arbitrary sampled process IDs.
  $api=@([SplittyFixtureTcp]::Snapshot([uint32[]]@($PID))|Where-Object{$_.Family-eq $family.Number-and $_.LocalAddress-ceq $address-and $_.LocalPort-in @($port,$clientPort)})
  $filter="OwningProcess=$PID AND (LocalPort=$port OR LocalPort=$clientPort)"
  $raw=@(Get-CimInstance -Namespace root/StandardCimv2 -ClassName MSFT_NetTCPConnection -Filter $filter -OperationTimeoutSec 10|Where-Object{$_.LocalAddress-ceq $address})
  $cim=@($raw|ForEach-Object{[ordered]@{LocalAddress=$_.LocalAddress;LocalPort=$_.LocalPort;RemoteAddress=$_.RemoteAddress;RemotePort=$_.RemotePort;OwningProcess=$_.OwningProcess;State=$_.State;StateType=$(if($null-ne $_.State){$_.State.GetType().FullName}else{$null});RawState=$_.CimInstanceProperties['State'].Value;RawStateType=$(if($null-ne $_.CimInstanceProperties['State'].Value){$_.CimInstanceProperties['State'].Value.GetType().FullName}else{$null});Properties=@($_.CimInstanceProperties|ForEach-Object{[ordered]@{Name=$_.Name;Value=$_.Value;CimType=[string]$_.CimType}})}})
  $cmdlet=@(Get-NetTCPConnection -OwningProcess $PID -ErrorAction Stop|Where-Object{$_.LocalAddress-ceq $address-and $_.LocalPort-in @($port,$clientPort)}|Select-Object LocalAddress,LocalPort,RemoteAddress,RemotePort,OwningProcess,State)
  $record=[ordered]@{Family=$family.Number;Address=$address;ListenerPort=$port;ClientPort=$clientPort;ApiRows=$api;RawCimRows=$cim;CmdletRows=$cmdlet}
  $result.Families+=@($record)
  # Save actual rows before assertions: zero/unknown is diagnostic failure,
  # never a benign-listener waiver or a substituted state.
  $record|ConvertTo-Json -Depth 12|Set-Content (Join-Path $Evidence ("family-"+$family.Number+'.json'))
  $apiListen=@($api|Where-Object{$_.LocalPort-eq $port-and $_.State-eq 2})
  $apiEstablished=@($api|Where-Object{$_.LocalPort-eq $clientPort-and $_.RemotePort-eq $port-and $_.State-eq 5})
  $cimListen=@($raw|Where-Object{$_.LocalPort-eq $port-and [int]$_.CimInstanceProperties['State'].Value-eq 2})
  $cimEstablished=@($raw|Where-Object{$_.LocalPort-eq $clientPort-and $_.RemotePort-eq $port-and [int]$_.CimInstanceProperties['State'].Value-eq 5})
  if($apiListen.Count-ne 1-or $apiEstablished.Count-ne 1-or $cimListen.Count-ne 1-or $cimEstablished.Count-ne 1){$stateFailures.Add("Unknown or incorrect own TCP states in family $($family.Number); inspect raw evidence")}
 }
 if($stateFailures.Count){throw [string]::Join('; ',$stateFailures)}
 if($result.SelfControl-notlike 'PASS family=2*PASS family=23*'){throw 'independent API self-control failed'}
 $result.Passed=$true
}catch{$result.Failure=$_.ToString();Write-Warning $result.Failure}finally{
 foreach($control in $controls){if($control.Peer){$control.Peer.Dispose()};if($control.Client){$control.Client.Dispose()};$control.Listener.Stop()}
 $result.SocketsClosed=$true
 $result|ConvertTo-Json -Depth 14|Set-Content (Join-Path $Evidence 'result.json')
}
if(-not $result.Passed){throw 'TCP state diagnostic FAILED/UNKNOWN; no state waiver'}
Write-Host 'PROVED exact self-owned IPv4/IPv6 API and raw CIM listener=2 established=5; controls closed'
