# Pure snapshot functions: these functions never launch or terminate processes.
function Get-FixtureProcessKey($Entry) {
  return ([string]$Entry.Id + '|' + [string]$Entry.Creation + '|' + [string]$Entry.ExecutablePath).ToLowerInvariant()
}
function Get-FixtureOwnedSnapshot($Processes, $Roots) {
  $owned = @{}
  foreach ($root in $Roots) {
    if (-not $root.ExecutablePath -or -not $root.Creation) { continue }
    foreach ($item in $Processes) {
      if ((Get-FixtureProcessKey $item) -ceq (Get-FixtureProcessKey $root)) {
        $owned[[string]$item.Id] = [pscustomobject]@{Id=$item.Id; Parent=$item.Parent; Creation=$item.Creation; ExecutablePath=$item.ExecutablePath; Name=$item.Name; RootKey=(Get-FixtureProcessKey $root); ParentKey=$null}
      }
    }
  }
  $changed = $true
  while ($changed) {
    $changed = $false
    foreach ($item in $Processes) {
      if ($owned.ContainsKey([string]$item.Id) -or -not $item.ExecutablePath -or -not $item.Creation) { continue }
      $parent = $owned[[string]$item.Parent]
      # A PID can be recycled while old children still refer to it. A genuine
      # child cannot predate this specific parent process generation.
      if ($parent -and [datetime]$item.Creation -ge [datetime]$parent.Creation) {
        $owned[[string]$item.Id] = [pscustomobject]@{Id=$item.Id; Parent=$item.Parent; Creation=$item.Creation; ExecutablePath=$item.ExecutablePath; Name=$item.Name; RootKey=$parent.RootKey; ParentKey=(Get-FixtureProcessKey $parent)}
        $changed = $true
      }
    }
  }
  return @($owned.Values)
}
function Test-FixtureCleanupGeneration($Recorded, $Current, $OwnedSnapshot, $Roots) {
  if (-not $Recorded -or -not $Current -or -not $Recorded.ExecutablePath -or -not $Recorded.Creation) { return $false }
  $key = Get-FixtureProcessKey $Recorded
  if ($key -cne (Get-FixtureProcessKey $Current)) { return $false }
  $matches = @($OwnedSnapshot | Where-Object { (Get-FixtureProcessKey $_) -ceq $key -and $_.RootKey -ceq $Recorded.RootKey })
  if ($matches.Count -ne 1) { return $false }
  $rootKeys = @($Roots | ForEach-Object { Get-FixtureProcessKey $_ })
  if ($rootKeys -cnotcontains $Recorded.RootKey) { return $false }
  $cursor = $matches[0]; $seen = @{}
  while ((Get-FixtureProcessKey $cursor) -cne $cursor.RootKey) {
    $cursorKey = Get-FixtureProcessKey $cursor
    if ($seen.ContainsKey($cursorKey)) { return $false }; $seen[$cursorKey] = $true
    $parents = @($OwnedSnapshot | Where-Object { (Get-FixtureProcessKey $_) -ceq $cursor.ParentKey -and $_.RootKey -ceq $cursor.RootKey })
    if ($parents.Count -ne 1 -or [datetime]$cursor.Creation -lt [datetime]$parents[0].Creation) { return $false }
    $cursor = $parents[0]
  }
  return $true
}
