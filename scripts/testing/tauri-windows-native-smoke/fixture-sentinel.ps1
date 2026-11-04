# Every actual guest command starts with the exact pre-recorded target gate.
$fixtureGuestGuard=(Get-Content (Join-Path $RootDir 'fixture-guest-guard.sh') -Raw).Trim()
function Get-FixtureGuestArguments([string]$Name,[string[]]$Arguments,[string]$User='') {
 if($Name-notmatch '^splitty-native-fixture-[0-9a-f]{12}-[ab]$'-or $ownership.names-notcontains $Name-or -not $ownership.created.PSObject.Properties[$Name]){throw 'unowned fixture guest target'}
 $prefix=@('--distribution',$Name)
 if($User){if($User-cne 'root'){throw 'unsupported fixture user'};$prefix+=@('--user','root')}
 return [string[]]($prefix+@('--exec','/bin/sh','-c',$fixtureGuestGuard,'splitty-fixture-guard',$Name)+$Arguments)
}
function Invoke-FixtureGuest([string]$Name,[string[]]$Arguments,[string]$User='') {
 $guarded=Get-FixtureGuestArguments $Name $Arguments $User
 $guestJob=$null;$guestOutput='';$guestResult=[ordered]@{Distro=$Name;Arguments=$guarded;CommandLine=$null;StdOutPath=$null;StdErrPath=$null;ExitCode=$null;JobEmpty=$false;HandlesBefore=[SplittyFixtureJob]::LiveHandleCount;HandlesAfter=$null}
 try{
  $guestJob=[SplittyFixtureJob]::new((Join-Path $env:SystemRoot 'System32\wsl.exe'),$guarded,$RootDir,$false)
  $guestResult.CommandLine=$guestJob.LaunchedCommandLine;$guestResult.StdOutPath=$guestJob.StandardOutputPath;$guestResult.StdErrPath=$guestJob.StandardErrorPath
  $until=(Get-Date).AddSeconds(20)
  while($guestJob.ActiveProcesses-ne 0-and(Get-Date)-lt $until){Start-Sleep -Milliseconds 20}
  if($guestJob.ActiveProcesses-ne 0){throw 'gated guest command deadline'}
  $guestResult.ExitCode=$guestJob.RootExitCode
  if($guestResult.ExitCode-ne 0){throw ('gated guest command refused or failed: '+$guestResult.ExitCode)}
  $guestOutput=[string](Get-Content $guestJob.StandardOutputPath -Raw)
 }finally{
  if($guestJob){try{$guestResult.JobEmpty=$guestJob.Stop(5000)}finally{$guestJob.Dispose()}}
  $guestResult.HandlesAfter=[SplittyFixtureJob]::LiveHandleCount
  $guestResult|ConvertTo-Json -Depth 5|Set-Content (Join-Path $RootDir ('guest-invocation-'+[guid]::NewGuid().ToString('N')+'.json'))
 }
 if(-not $guestResult.JobEmpty-or $guestResult.HandlesAfter-ne $guestResult.HandlesBefore){throw 'gated guest Job cleanup failed'}
 return $guestOutput
}
# Private disposable guest sentinel scripts; no launch or cleanup actions.
$sentinelScript=@'
read -r stat < /proc/$$/stat
set -- $stat
printf '%s %s\n' "$$" "${22}" > /tmp/splitty-sentinel.birth
while :; do
 if [ -f /tmp/splitty-sentinel.request ]; then
  read -r nonce < /tmp/splitty-sentinel.request
  read -r guest_pid guest_birth < /tmp/splitty-sentinel.birth
  printf '%s %s %s\n' "$guest_pid" "$guest_birth" "$nonce" > /tmp/splitty-sentinel.reply
  rm /tmp/splitty-sentinel.request
 fi
 sleep 0.1
done
'@
$sentinelCheck=@'
guest_pid=$1; guest_birth=$2; nonce=$3
kill -0 "$guest_pid" || exit 21
read -r stat < /proc/$guest_pid/stat || exit 22
set -- $stat
[ "${22}" = "$guest_birth" ] || exit 23
printf '%s\n' "$nonce" > /tmp/splitty-sentinel.request
i=0
while [ ! -s /tmp/splitty-sentinel.reply ]; do
 i=$((i+1)); [ "$i" -lt 100 ] || exit 24; sleep 0.1
done
read -r reply_pid reply_birth reply_nonce < /tmp/splitty-sentinel.reply
[ "$reply_pid" = "$guest_pid" ] && [ "$reply_birth" = "$guest_birth" ] && [ "$reply_nonce" = "$nonce" ] || exit 25
printf '%s %s %s\n' "$reply_pid" "$reply_birth" "$reply_nonce"
'@
$sentinelBirth=@'
i=0
while [ ! -s /tmp/splitty-sentinel.birth ]; do
 i=$((i+1)); [ "$i" -lt 100 ] || exit 20; sleep 0.1
done
cat /tmp/splitty-sentinel.birth
'@
