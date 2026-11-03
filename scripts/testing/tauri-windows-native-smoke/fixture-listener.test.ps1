$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'fixture-listener.ps1')
$before=@([pscustomobject]@{Id=42;Creation='2026-11-03T12:00:00.0000000Z';ExecutablePath='C:\private\fixture.exe';Alive=$true})
$after=@([pscustomobject]@{Id=42;Creation='2026-11-03T12:00:00.0000000Z';ExecutablePath='C:\private\fixture.exe';Alive=$true})
$end='2026-11-03T12:00:10.0000000Z'
foreach($created in @('2026-11-03T12:00:00.0000000Z','2026-11-03T12:00:05.0000000Z',$end)){
 if(-not(Test-FixtureSocketGeneration $before $after $created $end)){throw 'valid bracketed socket rejected'}
}
foreach($created in @($null,'bad-date','2026-11-03T11:59:59.0000000Z','2026-11-03T12:00:11.0000000Z')){
 if(Test-FixtureSocketGeneration $before $after $created $end){throw 'unknown/older/future socket wrongly attributed'}
}
$after[0].Creation='2026-11-03T12:00:01.0000000Z'
if(Test-FixtureSocketGeneration $before $after '2026-11-03T12:00:05.0000000Z' $end){throw 'reused process generation attributed'}
$after[0].Creation=$before[0].Creation;$after[0].ExecutablePath='C:\other.exe'
if(Test-FixtureSocketGeneration $before $after '2026-11-03T12:00:05.0000000Z' $end){throw 'other executable attributed'}
Write-Output 'PASS 9 synthetic listener classification cases; no process or WSL action'
