param([string]$Executable, [string]$OtherExecutable, [string]$RootDir, [string]$FixtureB)
throw 'Windows GUI driver disabled pending independent process-ownership cleanup review'
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
trap {
  if ($roots) { foreach ($handle in $roots.Values) { if (-not $handle.HasExited) { & taskkill.exe /PID $handle.Id /T /F | Out-Null } } }
  throw $_
}
if (-not (Test-Path (Join-Path $RootDir 'TEST-ONLY.txt'))) { throw 'unmarked fixture root' }
$ownership = Get-Content (Join-Path $RootDir 'owned-distros.json') -Raw | ConvertFrom-Json
if ($FixtureB -notmatch '^splitty-native-fixture-[0-9a-f]{12}-[ab]$' -or $ownership.names -notcontains $FixtureB) { throw 'unowned fixture target' }
$evidence = Join-Path $RootDir 'runtime-evidence'
New-Item -ItemType Directory -Force $evidence | Out-Null
Remove-Item (Join-Path $evidence 'runtime-result.json') -ErrorAction SilentlyContinue
$env:SPLITTY_SMOKE_EVIDENCE = $evidence
$env:SPLITTY_SMOKE_WEBVIEW_DATA = Join-Path $evidence 'webview-data'
$exePaths = @($Executable, $OtherExecutable) | Where-Object { $_ }
$existing = @(Get-CimInstance Win32_Process | Where-Object { $exePaths -contains $_.ExecutablePath })
if ($existing.Count) { throw 'fixture executable is already running; refusing duplicate GUI launch' }
$application = Start-Process $Executable -WorkingDirectory $RootDir -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $evidence 'stdout.log') -RedirectStandardError (Join-Path $evidence 'stderr.log')
$null = $application.Handle
$roots = @{}; $roots[[string]$application.Id] = $application
$observed = @{}; $listeners = @(); $deadline = (Get-Date).AddSeconds(210)
$guestProbe = @'
for entry in /proc/[0-9]*; do
  read -r name < "$entry/comm" 2>/dev/null || continue
  [ "$name" = splitty ] || continue
  printf 'PID:%s\n' "${entry##*/}"
  for fd in "$entry"/fd/*; do
    link=$(readlink "$fd" 2>/dev/null) || continue
    case "$link" in
      socket:\[*\])
        needle=${link#socket:}; needle=${needle#\[}; needle=${needle%\]}
        for table in /proc/net/tcp /proc/net/tcp6; do
          while read -r sl local remote state queues timers retrans uid timeout inode rest; do
            if [ "$state" = 0A ] && [ "$inode" = "$needle" ]; then printf 'LISTEN:%s\n' "$local"; fi
          done < "$table"
        done
        ;;
    esac
  done
done
'@
do {
  $all = @(Get-CimInstance Win32_Process)
  foreach ($item in $all) {
    if ($exePaths -contains $item.ExecutablePath -and -not $roots.ContainsKey([string]$item.ProcessId)) {
      try { $handle = [System.Diagnostics.Process]::GetProcessById($item.ProcessId); $null = $handle.Handle; $roots[[string]$item.ProcessId] = $handle } catch {}
    }
  }
  $owned = @($roots.Keys | ForEach-Object { [int]$_ }); $changed = $true
  while ($changed) {
    $changed = $false
    foreach ($item in $all) {
      if ($owned -contains [int]$item.ParentProcessId -and $owned -notcontains [int]$item.ProcessId) { $owned += [int]$item.ProcessId; $changed = $true }
    }
  }
  foreach ($item in $all) {
    if ($owned -contains [int]$item.ProcessId) { $observed[[string]$item.ProcessId] = @{Id=[int]$item.ProcessId; Parent=[int]$item.ParentProcessId; Name=$item.Name; Creation=$item.CreationDate.ToUniversalTime().ToString('o')} }
  }
  $listeners += @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $owned -contains [int]$_.OwningProcess } | Select-Object LocalAddress,LocalPort,OwningProcess)
  if ((Test-Path (Join-Path $evidence 'checkpoint-native-open.json')) -and -not (Test-Path (Join-Path $evidence 'guest-live.txt'))) {
    & wsl.exe -d $FixtureB --exec sh -c $guestProbe | Out-File -Encoding utf8 (Join-Path $evidence 'guest-live.txt')
    if ($LASTEXITCODE -ne 0) { throw 'guest socket inventory failed' }
  }
  $alive = @($roots.Values | Where-Object { -not $_.HasExited })
  if ((Test-Path (Join-Path $evidence 'runtime-result.json')) -and $alive.Count -eq 0) { break }
  Start-Sleep -Milliseconds 250
} while ((Get-Date) -lt $deadline)
$timedOut = -not (Test-Path (Join-Path $evidence 'runtime-result.json')) -or $alive.Count -ne 0
if ($timedOut) { foreach ($handle in $roots.Values) { if (-not $handle.HasExited) { & taskkill.exe /PID $handle.Id /T /F | Out-Null } } }
$exits = @(); foreach ($handle in $roots.Values) { $handle.WaitForExit(); $exits += @{Id=$handle.Id; Created=$handle.StartTime.ToUniversalTime().ToString('o'); ExitCode=$handle.ExitCode} }
Start-Sleep -Seconds 3
$remaining = @()
foreach ($entry in $observed.Values) {
  $current = Get-CimInstance Win32_Process -Filter "ProcessId=$($entry.Id)" -ErrorAction SilentlyContinue
  if ($current -and $current.CreationDate.ToUniversalTime().ToString('o') -eq $entry.Creation) { $remaining += $entry }
}
$result = [ordered]@{TimedOut=$timedOut; AppInstances=$exits; Processes=@($observed.Values); Listeners=$listeners; RemainingOwned=$remaining}
$result | ConvertTo-Json -Depth 5 | Out-File -Encoding utf8 (Join-Path $evidence 'processes.json')
# Cleanup only exact process generations descended from the fresh fixture apps.
foreach ($entry in $remaining) {
  $current = Get-CimInstance Win32_Process -Filter "ProcessId=$($entry.Id)" -ErrorAction SilentlyContinue
  if ($current -and $current.CreationDate.ToUniversalTime().ToString('o') -eq $entry.Creation) { Stop-Process -Id $entry.Id -Force -ErrorAction SilentlyContinue }
}
$last = $exits | Sort-Object Created -Descending | Select-Object -First 1
if ($timedOut -or $last.ExitCode -ne 0 -or $exits.Count -ne 2 -or $listeners.Count -or $remaining.Count) { exit 1 }
