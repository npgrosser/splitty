param([string]$Executable, [string]$OtherExecutable, [string]$RootDir, [string]$FixtureB, [switch]$PermitIsolatedFullRun, [switch]$PermitIsolatedSshHost, [ValidateSet(1,2)][int]$ExpectedAppInstances=2)
if(-not $PermitIsolatedFullRun){throw 'Explicit isolated full WSL permit required'}
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
if(-not(Test-Path (Join-Path $RootDir 'TEST-ONLY.txt'))){throw 'unmarked fixture root'}
$ownership=Get-Content (Join-Path $RootDir 'owned-distros.json') -Raw|ConvertFrom-Json
if($FixtureB-notmatch '^splitty-native-fixture-[0-9a-f]{12}-[ab]$'-or $ownership.names-notcontains $FixtureB){throw 'unowned fixture target'}
. (Join-Path $RootDir 'fixture-registration.ps1')
. (Join-Path $RootDir 'fixture-listener.ps1')
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
$sshServer=$null;$sshEvidence=[ordered]@{Requested=[bool]$PermitIsolatedSshHost;Authenticated=$false;Attempts=@();JobEmpty=$null;CommandLine=$null;StdOutPath=$null;StdErrPath=$null};$app=$null;$sentinels=@();$listeners=@();$remaining=@();$history=@();$preserved=$false;$timedOut=$false;$empty=$false;$cleanupErrors=@();$exits=@()
try {
 if($PermitIsolatedSshHost){
  if($ExpectedAppInstances-ne 1){throw 'isolated SSH host requires one-app isolation mode'}
  $sshConfig=Join-Path $RootDir 'isolation-ssh/config'
  if(-not(Test-Path -LiteralPath $sshConfig -PathType Leaf)){throw 'private SSH config missing'}
  $hostName=[string]$ownership.names[0]
  Invoke-FixtureGuest $hostName @('/usr/bin/mkdir','-p','/run/sshd') 'root' | Out-Null
  # sshd is a persistent foreground service, never a one-shot guest command.
  # Keep the exact Windows Job until the outer finally; distro teardown owns
  # any Linux lifetime remaining after that Windows Job is stopped.
  $serverArgs=Get-FixtureGuestArguments $hostName @('/usr/sbin/sshd','-D','-f','/etc/sshd_config','-E','/tmp/fixture-sshd.log') 'root'
  $sshServer=[SplittyFixtureJob]::new((Join-Path $env:SystemRoot 'System32\wsl.exe'),$serverArgs,$RootDir,$false)
  $sshEvidence.CommandLine=$sshServer.LaunchedCommandLine;$sshEvidence.StdOutPath=$sshServer.StandardOutputPath;$sshEvidence.StdErrPath=$sshServer.StandardErrorPath
  $nonce=[guid]::NewGuid().ToString('N')
  $transaction='test "$HOME" = /home/splitty-fixture && test "$(id -u)" = 1000 && printf "%s" '+$nonce
  $readyUntil=(Get-Date).AddSeconds(20)
  for($attempt=0;$attempt-lt 4-and(Get-Date)-lt $readyUntil;$attempt++){
   if(-not $sshServer.RootAlive){throw 'private foreground SSH host exited before authentication'}
   $probe=$null
   try{
    $probe=[SplittyFixtureJob]::new((Join-Path $env:SystemRoot 'System32\OpenSSH\ssh.exe'),[string[]]@('-F',$sshConfig,'splitty-isolation-one',$transaction),$RootDir,$false)
    $until=(Get-Date).AddSeconds(5)
    while($probe.ActiveProcesses-ne 0-and(Get-Date)-lt $until){Start-Sleep -Milliseconds 25}
    $exit=$null;if($probe.ActiveProcesses-eq 0){$exit=$probe.RootExitCode}
    $stdout=[IO.File]::ReadAllText($probe.StandardOutputPath).Trim()
    $stderr=[IO.File]::ReadAllText($probe.StandardErrorPath)
    $sshEvidence.Attempts+=@([ordered]@{CommandLine=$probe.LaunchedCommandLine;ExitCode=$exit;Stdout=$stdout;Stderr=$stderr})
    if($exit-eq 0-and $stdout-ceq $nonce){$sshEvidence.Authenticated=$true;break}
   }finally{if($probe){try{if(-not $probe.Stop(5000)){throw 'SSH readiness Job remains active'}}finally{$probe.Dispose()}}}
   Start-Sleep -Milliseconds 100
  }
  if(-not $sshEvidence.Authenticated){throw 'private SSH host did not pass bounded authenticated HOME/UID/nonce transaction'}
 }
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
  # Snapshot seeds kernel-validated retained handles. Freeze scalar values:
  # History updates those same handles rather than reopening sampled PIDs.
  $null=$app.Snapshot()
  $beforeAt=[DateTime]::UtcNow.ToString('o')
  $before=@($app.History()|Where-Object{$_.Alive}|ForEach-Object{[pscustomobject]@{Id=$_.Id;Creation=$_.Creation;ExecutablePath=$_.ExecutablePath;Alive=[bool]$_.Alive}})
  $ids=@($before|ForEach-Object{$_.Id})
  if($ids.Count){
   $socketRows=@(Get-NetTCPConnection -State Listen -OwningProcess $ids -ErrorAction SilentlyContinue)
   $after=@($app.History()|Where-Object{$_.Alive}|ForEach-Object{[pscustomobject]@{Id=$_.Id;Creation=$_.Creation;ExecutablePath=$_.ExecutablePath;Alive=[bool]$_.Alive}})
   $afterAt=[DateTime]::UtcNow.ToString('o')
   foreach($socket in $socketRows){
    $pre=@($before|Where-Object{$_.Id-eq $socket.OwningProcess})
    $post=@($after|Where-Object{$_.Id-eq $socket.OwningProcess})
    $socketCreated=$(if($socket.CreationTime){$socket.CreationTime.ToUniversalTime().ToString('o')}else{$null})
    $matched=Test-FixtureSocketGeneration $pre $post $socketCreated $afterAt
    $listeners += [pscustomobject]@{LocalAddress=$socket.LocalAddress;LocalPort=$socket.LocalPort;RemoteAddress=$socket.RemoteAddress;RemotePort=$socket.RemotePort;State=[string]$socket.State;SocketCreationUTC=$socketCreated;BeforeUTC=$beforeAt;AfterUTC=$afterAt;OwningProcess=$socket.OwningProcess;Classification=$(if($matched){'retained-owned-generation'}else{'UNCLASSIFIED'});BeforeMember=$pre;AfterMember=$post}
   }
  }
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
 if($sshServer){try{$sshEvidence.JobEmpty=$sshServer.Stop(5000);if(-not $sshEvidence.JobEmpty){$cleanupErrors+='SSH server Windows Job remains active'}}catch{$cleanupErrors+=$_.Exception.Message}finally{$sshServer.Dispose()}}
 $sshEvidence|ConvertTo-Json -Depth 6|Set-Content (Join-Path $evidence 'ssh-host.json')
 if($app){try{$empty=$app.Stop(5000);$history=@($app.History())}catch{$cleanupErrors+=$_.Exception.Message}finally{$app.Dispose()}}
 foreach($sentinel in $sentinels){try{if(-not $sentinel.Stop(5000)){$cleanupErrors+='sentinel Job remains active'}}catch{$cleanupErrors+=$_.Exception.Message}finally{$sentinel.Dispose()}}
 $result=[ordered]@{TimedOut=$timedOut;AppInstances=$exits;Processes=$history;Listeners=$listeners;RemainingOwned=$remaining;IndependentFixturePreserved=$preserved;JobEmptyAfterCleanup=$empty;OwnedHandlesAfterCleanup=[SplittyFixtureJob]::LiveHandleCount;CleanupErrors=$cleanupErrors;SentinelLaunches=$sentinelLaunches;AppCommandLine=$(if($app){$app.LaunchedCommandLine}else{$null})}
 $result|ConvertTo-Json -Depth 5|Out-File -Encoding utf8 (Join-Path $evidence 'processes.json')
}
if($timedOut-or $exits.Count-ne $ExpectedAppInstances-or @($exits|Where-Object{$_.ExitCode-ne 0-or $_.Alive}).Count-or $listeners.Count-or $remaining.Count-or-not $preserved-or-not $empty-or $cleanupErrors.Count-or [SplittyFixtureJob]::LiveHandleCount-ne 0){exit 1}
