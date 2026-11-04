$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'owned-processes.ps1')
function Entry($id,$parent,$time,$path) { [pscustomobject]@{Id=$id;Parent=$parent;Creation=$time;ExecutablePath=$path;Name=[System.IO.Path]::GetFileName($path)} }
$root=Entry 100 9 '2026-11-03T16:37:14.8990210Z' 'C:\Temp\fixture\splitty-gui.exe'
$webview=Entry 3664 100 '2026-11-03T16:37:15.0000000Z' 'C:\Runtime\msedgewebview2.exe'
$firefox=Entry 33624 3664 '2026-11-03T15:03:13.3663330Z' 'C:\Firefox\firefox.exe'
$oldChild=Entry 29192 33624 '2026-11-03T15:03:13.7903350Z' 'C:\Firefox\firefox.exe'
$ssh=Entry 200 100 '2026-11-03T16:37:16.0000000Z' 'C:\Windows\System32\OpenSSH\ssh.exe'
$items=@($root,$webview,$firefox,$oldChild,$ssh)
$owned=@(Get-FixtureOwnedSnapshot $items @($root))
if ($owned.Count -ne 3 -or @($owned|Where-Object {$_.Id -in @(33624,29192)}).Count) { throw 'PID reuse admitted an old unrelated tree' }
$record=@($owned|Where-Object {$_.Id -eq 200})[0]
if (-not (Test-FixtureCleanupGeneration $record $ssh $owned @($root))) { throw 'valid generation denied' }
$reused=Entry 200 100 '2026-11-03T16:40:00.0000000Z' $ssh.ExecutablePath
if (Test-FixtureCleanupGeneration $record $reused $owned @($root)) { throw 'reused PID admitted for cleanup' }
$otherExe=Entry 200 100 $ssh.Creation 'C:\Firefox\firefox.exe'
if (Test-FixtureCleanupGeneration $record $otherExe $owned @($root)) { throw 'executable mismatch admitted' }
if (Test-FixtureCleanupGeneration $record $ssh @($record) @($root)) { throw 'missing ancestry admitted' }
$wrongRoot=Entry 100 9 '2026-11-03T14:00:00.0000000Z' $root.ExecutablePath
if (Test-FixtureCleanupGeneration $record $ssh $owned @($wrongRoot)) { throw 'root generation mismatch admitted' }
$oldRoot=Entry 100 9 '2026-11-03T14:00:00.0000000Z' $root.ExecutablePath
if (@(Get-FixtureOwnedSnapshot $items @($oldRoot)).Count) { throw 'root PID reused into fixture admitted' }
'PASS: 7 pure snapshot ownership checks; no processes launched or terminated'
