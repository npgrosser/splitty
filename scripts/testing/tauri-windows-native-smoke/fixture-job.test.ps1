throw 'Standalone console-spawn fixture remains disabled; supported guarded runtime entrypoints are documented in README.md'
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$root=Join-Path $env:TEMP ('splitty-native-job-console-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $root | Out-Null
Set-Content (Join-Path $root 'TEST-ONLY.txt') 'Fresh isolated console Job ownership test. No GUI or existing applications.'
Copy-Item (Join-Path $PSScriptRoot 'fixture-job.cs') $root
Copy-Item (Join-Path $PSScriptRoot 'fixture-console.cs') $root
Copy-Item (Join-Path $PSScriptRoot 'fixture-ambient.cs') $root
Add-Type -Path (Join-Path $root 'fixture-ambient.cs')
Add-Type -Path (Join-Path $root 'fixture-job.cs')
$exe=Join-Path $root 'fixture-console.exe'
Add-Type -Path (Join-Path $root 'fixture-console.cs') -OutputAssembly $exe -OutputType ConsoleApplication
$jobs=@();$ambient=$null;$result=[ordered]@{Root=$root;ChildGrandchild=$false;UnrelatedFixturePreserved=$false;BreakawayRefused=$false;FailedAssignmentCleaned=$false;HandlesZero=$false}
function Wait-File($path) { $until=(Get-Date).AddSeconds(10);while(-not(Test-Path $path)-and(Get-Date)-lt $until){Start-Sleep -Milliseconds 25};if(-not(Test-Path $path)){throw "fresh console marker absent: $path"} }
try {
  $cliDir=Join-Path $root 'commandline';$cliJob=[SplittyFixtureJob]::new($exe,@('commandline',$cliDir,'--distribution','splitty-native-fixture-000000000000-a','--exec','sh','-c','printf "two words"'),$root,$false);$jobs+=$cliJob
  Wait-File (Join-Path $cliDir 'commandline-result');$raw=Get-Content (Join-Path $cliDir 'commandline-result') -Raw
  if(-not $raw.Contains(' --distribution splitty-native-fixture-000000000000-a --exec sh -c ')-or $raw.Contains('"--distribution"')){throw 'plain WSL option tokens were quoted'}
  $result.PlainOptionTokensUnquoted=$true
  $ambientPath=Join-Path $root 'ambient-inheritable-marker';$ambient=[SplittyAmbientFixtureHandle]::new($ambientPath)
  if(-not $ambient.Inheritable){throw 'ambient test handle was not inheritable'}
  $ambientDir=Join-Path $root 'ambient';$ambientJob=[SplittyFixtureJob]::new($exe,@('ambient',$ambientDir,[string]$ambient.Value,$ambientPath),$root,$false);$jobs+=$ambientJob
  Wait-File (Join-Path $ambientDir 'ambient-result')
  if((Get-Content (Join-Path $ambientDir 'ambient-result') -Raw).Trim()-ne 'excluded'){throw 'ambient inheritable handle escaped allowlist'}
  $result.AmbientInheritableMarkerExcluded=$true
  $unrelatedDir=Join-Path $root 'independent';$independent=[SplittyFixtureJob]::new($exe,@('independent',$unrelatedDir),$root,$false);$jobs+=$independent
  Wait-File (Join-Path $unrelatedDir 'independent-ready');$independentBefore=@($independent.Snapshot());$independentActive=$independent.ActiveProcesses
  $dir=Join-Path $root 'tree';$tree=[SplittyFixtureJob]::new($exe,@('tree',$dir),$root,$false);$jobs+=$tree
  Wait-File (Join-Path $dir 'grandchild-ready')
  $before=@($tree.Snapshot());$before|ConvertTo-Json -Depth 4|Set-Content (Join-Path $root 'tree-members.json')
  foreach($mode in @('tree','child','grandchild')){$id=[uint32](Get-Content (Join-Path $dir ($mode+'-ready')));if(@($before|Where-Object {$_.Id-eq $id-and $_.Alive-and $_.ExitCode-eq 259}).Count-ne 1){throw ('fixture Job membership missing '+$mode+'; count='+$before.Count)}}
  if(-not $tree.Stop(5000)-or $tree.ActiveProcesses-ne 0){throw 'owned Job children did not exit'};$result.ChildGrandchild=$true
  if(-not $independent.RootAlive-or $independent.ActiveProcesses-ne $independentActive){throw 'independent fixture changed'};$result.UnrelatedFixturePreserved=$true
  $breakDir=Join-Path $root 'breakaway';$breakaway=[SplittyFixtureJob]::new($exe,@('breakaway',$breakDir),$root,$false);$jobs+=$breakaway
  Wait-File (Join-Path $breakDir 'breakaway-result');$breakResult=Get-Content (Join-Path $breakDir 'breakaway-result') -Raw
  if($breakResult.Trim()-ne 'refused:5'-or(Test-Path (Join-Path $breakDir 'should-not-run'))){throw ('breakaway was not refused: '+$breakResult)};$result.BreakawayRefused=$true
  $failDir=Join-Path $root 'assignment-failure';$failed=$false
  try{$unexpected=[SplittyFixtureJob]::new($exe,@('mark',$failDir),$root,$true);$jobs+=$unexpected}catch{$failed=$true}
  if(-not $failed-or-not [SplittyFixtureJob]::LastFailureConfirmedExited-or(Test-Path (Join-Path $failDir 'should-not-run'))){throw 'failed assignment resumed or leaked process'};$result.FailedAssignmentCleaned=$true;$result.FailedAssignmentWin32Error=[SplittyFixtureJob]::LastFailureAssignmentError
  if($result.FailedAssignmentWin32Error-eq 0){throw 'real OS assignment failure not exercised'}
} finally {
  foreach($job in $jobs){try{if(-not $job.Stop(5000)){throw 'Job remains active'}}finally{$job.Dispose()}}
  if($ambient){$ambient.Dispose();$result.AmbientMarkerClosed=$ambient.Closed}
  $result.HandlesZero=[SplittyFixtureJob]::LiveHandleCount-eq 0
  Get-FileHash (Join-Path $root 'fixture-job.cs'),(Join-Path $root 'fixture-console.cs'),(Join-Path $root 'fixture-ambient.cs')|Select-Object Path,Hash|ConvertTo-Json|Set-Content (Join-Path $root 'source-hashes.json')
  $result|ConvertTo-Json -Depth 5|Set-Content (Join-Path $root 'result.json')
}
if(-not$result.HandlesZero){throw 'owned kernel handle count not zero'}
Write-Output ($root+'\result.json')
