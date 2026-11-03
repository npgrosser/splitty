param([string]$RootDir,[string]$Distro,[switch]$PermitSentinelOnly)
if(-not $PermitSentinelOnly){throw 'WSL fixture sentinel requires explicit authorized sentinel-only permit'}
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
if(-not(Test-Path (Join-Path $RootDir 'TEST-ONLY.txt'))){throw 'unmarked private sentinel fixture'}
$ownership=Get-Content (Join-Path $RootDir 'owned-distros.json') -Raw|ConvertFrom-Json
if($Distro-notmatch '^splitty-native-fixture-[0-9a-f]{12}-[ab]$'-or $ownership.names-notcontains $Distro-or -not $ownership.created.PSObject.Properties[$Distro]){throw 'unowned private sentinel distribution'}
Add-Type -Path (Join-Path $RootDir 'fixture-job.cs')
. (Join-Path $RootDir 'fixture-sentinel.ps1')
$job=$null;$result=[ordered]@{Distro=$Distro;GuestBirth=$null;GuestReply=$null;RootAliveBeforeCleanup=$false;RootExitCodeBeforeCleanup=$null;NaturalExitBeforeCleanup=$null;ForcedCleanup=$false;RootExitCodeAfterCleanup=$null;JobEmpty=$false;OwnedHandlesAfterCleanup=$null;StdOutPath=$null;StdErrPath=$null;CommandLine=$null;Arguments=$null;Error=$null}
try{
 $result.Arguments=Get-FixtureGuestArguments $Distro @('/bin/sh','-c',$sentinelScript)
 $job=[SplittyFixtureJob]::new((Join-Path $env:SystemRoot 'System32\wsl.exe'),(Get-FixtureGuestArguments $Distro @('/bin/sh','-c',$sentinelScript)),$RootDir,$false)
 $result.CommandLine=$job.LaunchedCommandLine
 $result.StdOutPath=$job.StandardOutputPath;$result.StdErrPath=$job.StandardErrorPath
 $birthOutput=Invoke-FixtureGuest $Distro @('/bin/sh','-c',$sentinelBirth)
 $birth=([string]::Join("`n",[string[]]@($birthOutput))).Trim() -split ' '
 if($birth.Count-ne 2-or $birth[0]-notmatch '^[0-9]+$'-or $birth[1]-notmatch '^[0-9]+$'){throw 'private sentinel guest birth absent'}
 $result.GuestBirth=$birth -join ' '
 $nonce=[guid]::NewGuid().ToString('N')
 $replyOutput=Invoke-FixtureGuest $Distro @('/bin/sh','-c',$sentinelCheck,'sentinel-check',$birth[0],$birth[1],$nonce)
 $reply=([string]::Join("`n",[string[]]@($replyOutput))).Trim()
 if($reply-cne ($result.GuestBirth+' '+$nonce)){throw 'private sentinel birth/echo transaction failed'}
 $result.GuestReply=$reply
}catch{$result.Error=$_.Exception.Message;throw}finally{
 if($job){try{$result.RootAliveBeforeCleanup=$job.RootAlive;$result.NaturalExitBeforeCleanup=-not $job.RootAlive;$result.RootExitCodeBeforeCleanup=$job.RootExitCode;$result.Members=@($job.Snapshot());$result.ForcedCleanup=$true;$result.JobEmpty=$job.Stop(5000);$result.RootExitCodeAfterCleanup=$job.RootExitCode}finally{$job.Dispose()}}
 $result.OwnedHandlesAfterCleanup=[SplittyFixtureJob]::LiveHandleCount
 $result|ConvertTo-Json -Depth 5|Set-Content (Join-Path $RootDir 'sentinel-console-result.json')
}
if(-not $result.JobEmpty-or $result.OwnedHandlesAfterCleanup-ne 0){throw 'private sentinel Job cleanup failed'}
