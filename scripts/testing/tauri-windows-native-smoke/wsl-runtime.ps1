param([string]$Executable, [string]$OtherExecutable, [string]$RootDir, [string]$FixtureB, [switch]$PermitIsolatedFullRun)
if(-not $PermitIsolatedFullRun){throw 'Explicit isolated full WSL permit required'}
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
if(-not(Test-Path (Join-Path $RootDir 'TEST-ONLY.txt'))){throw 'unmarked fixture root'}
$ownership=Get-Content (Join-Path $RootDir 'owned-distros.json') -Raw|ConvertFrom-Json
if($FixtureB-notmatch '^splitty-native-fixture-[0-9a-f]{12}-[ab]$'-or $ownership.names-notcontains $FixtureB){throw 'unowned fixture target'}
. (Join-Path $RootDir 'fixture-registration.ps1')
Assert-FixtureExecutable $RootDir $Executable
$registered=@(Get-ChildItem HKCU:\Software\Microsoft\Windows\CurrentVersion\Lxss|ForEach-Object{Get-ItemProperty $_.PSPath|Select-Object DistributionName,BasePath})
Assert-FixtureRegistrations $RootDir $ownership $registered $env:TEMP
if($FixtureB-cne $ownership.names[1]){throw 'fixture B must be the second recorded distribution'}
Add-Type -Path (Join-Path $RootDir 'fixture-job.cs')
$evidence=Join-Path $RootDir 'runtime-evidence';New-Item -ItemType Directory -Force $evidence|Out-Null
Remove-Item (Join-Path $evidence 'runtime-result.json') -ErrorAction SilentlyContinue
$env:SPLITTY_SMOKE_EVIDENCE=$evidence;$env:SPLITTY_SMOKE_WEBVIEW_DATA=Join-Path $evidence 'webview-data'
$exePaths=@($Executable,$OtherExecutable)|Where-Object{$_}
$guestProbe=@'
for entry in /proc/[0-9]*; do
 read -r name < "$entry/comm" 2>/dev/null || continue
 [ "$name" = splitty ] || continue
 printf 'PID:%s\n' "${entry##*/}"
 for fd in "$entry"/fd/*; do
  link=$(readlink "$fd" 2>/dev/null) || continue
  case "$link" in
   socket:\[*\])
    needle=${link#socket:};needle=${needle#\[};needle=${needle%\]}
    for table in /proc/net/tcp /proc/net/tcp6; do
     while read -r sl local remote state queues timers retrans uid timeout inode rest; do
      if [ "$state" = 0A ] && [ "$inode" = "$needle" ];then printf 'LISTEN:%s\n' "$local";fi
     done < "$table"
    done;;
  esac
 done
done
'@
. (Join-Path $RootDir 'fixture-sentinel.ps1')
$guestSentinels=@();$sentinelLaunches=@()
$app=$null;$sentinels=@();$listeners=@();$remaining=@();$history=@();$preserved=$false;$timedOut=$false;$empty=$false;$cleanupErrors=@();$exits=@()
try {
 foreach($name in $ownership.names){
  $sentinelJob = [SplittyFixtureJob]::new((Join-Path $env:SystemRoot 'System32\wsl.exe'),(Get-FixtureGuestArguments $name @('/bin/sh','-c',$sentinelScript)),$RootDir,$false)
  $sentinels += $sentinelJob
  $sentinelLaunches += [pscustomobject]@{Distro=$name;CommandLine=$sentinelJob.LaunchedCommandLine;StdOutPath=$sentinelJob.StandardOutputPath;StdErrPath=$sentinelJob.StandardErrorPath}
  $birth=([string]::Join("`n",[string[]]@(Invoke-FixtureGuest $name @('/bin/sh','-c',$sentinelBirth)))).Trim() -split ' '
  if($birth.Count-ne 2-or $birth[0]-notmatch '^[0-9]+$'-or $birth[1]-notmatch '^[0-9]+$'){throw 'sentinel guest birth was not recorded'}
  $guestSentinels += [pscustomobject]@{Distro=$name;GuestPid=$birth[0];StartTick=$birth[1];Reply=$null}
 }
 $guestSentinels|ConvertTo-Json -Depth 4|Set-Content (Join-Path $evidence 'sentinel-guests-before.json')
 # A kernel Job exists/configures no-breakaway before CreateProcess suspended.
 # Assignment succeeds before ResumeThread; every descendant inherits this Job.
 $app=[SplittyFixtureJob]::new($Executable,[string[]]@(),$RootDir,$false)
 $deadline=(Get-Date).AddSeconds(210)
 do {
  $members=@($app.Snapshot());$ids=@($members|ForEach-Object{$_.Id})
  if($ids.Count){$listeners += @(Get-NetTCPConnection -State Listen -OwningProcess $ids -ErrorAction SilentlyContinue|Select-Object LocalAddress,LocalPort,OwningProcess)}
  if((Test-Path (Join-Path $evidence 'checkpoint-native-open.json'))-and-not(Test-Path (Join-Path $evidence 'guest-live.txt'))){
   Invoke-FixtureGuest $FixtureB @('/bin/sh','-c',$guestProbe)|Out-File -Encoding utf8 (Join-Path $evidence 'guest-live.txt')

  }
  if($app.ActiveProcesses-eq 0){break}
  Start-Sleep -Milliseconds 100
 }while((Get-Date)-lt $deadline)
 $timedOut=$app.ActiveProcesses-ne 0;$remaining=@($app.Snapshot());$history=@($app.History())
 $exits=@($history|Where-Object{$exePaths-contains $_.ExecutablePath}|Select-Object Id,Creation,ExitCode,Alive)
 $preserved=@($sentinels|Where-Object{-not $_.RootAlive-or $_.ActiveProcesses-eq 0}).Count-eq 0
 foreach($guest in $guestSentinels){
  $nonce=[guid]::NewGuid().ToString('N')
  $reply=([string]::Join("`n",[string[]]@(Invoke-FixtureGuest $guest.Distro @('/bin/sh','-c',$sentinelCheck,'sentinel-check',$guest.GuestPid,$guest.StartTick,$nonce)))).Trim()
  if($reply-cne ($guest.GuestPid+' '+$guest.StartTick+' '+$nonce)){throw 'independent guest transaction or birth check failed'}
  $guest.Reply=$reply
 }
 $guestSentinels|ConvertTo-Json -Depth 4|Set-Content (Join-Path $evidence 'sentinel-guests-after.json')
}finally{
 if($app){try{$empty=$app.Stop(5000);$history=@($app.History())}catch{$cleanupErrors+=$_.Exception.Message}finally{$app.Dispose()}}
 foreach($sentinel in $sentinels){try{if(-not $sentinel.Stop(5000)){$cleanupErrors+='sentinel Job remains active'}}catch{$cleanupErrors+=$_.Exception.Message}finally{$sentinel.Dispose()}}
 $result=[ordered]@{TimedOut=$timedOut;AppInstances=$exits;Processes=$history;Listeners=$listeners;RemainingOwned=$remaining;IndependentFixturePreserved=$preserved;JobEmptyAfterCleanup=$empty;OwnedHandlesAfterCleanup=[SplittyFixtureJob]::LiveHandleCount;CleanupErrors=$cleanupErrors;SentinelLaunches=$sentinelLaunches;AppCommandLine=$(if($app){$app.LaunchedCommandLine}else{$null})}
 $result|ConvertTo-Json -Depth 5|Out-File -Encoding utf8 (Join-Path $evidence 'processes.json')
}
if($timedOut-or $exits.Count-ne 2-or @($exits|Where-Object{$_.ExitCode-ne 0-or $_.Alive}).Count-or $listeners.Count-or $remaining.Count-or-not $preserved-or-not $empty-or $cleanupErrors.Count-or [SplittyFixtureJob]::LiveHandleCount-ne 0){exit 1}
