$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'fixture-listener.ps1')
# Shadow the cmdlet with a pure mock: these tests never query a provider.
$script:queryFails=$false;$script:queryRows=@();$script:queryCalls=0
function Get-CimInstance {
 param($Namespace,$ClassName,$Filter,$OperationTimeoutSec,$ErrorAction)
 $script:queryCalls++
 if($Namespace-cne 'root/StandardCimv2'-or $ClassName-cne 'MSFT_NetTCPConnection'-or $OperationTimeoutSec-ne 5-or $ErrorAction-cne 'Stop'){throw 'unsafe query parameters'}
 if($Filter-cne 'OwningProcess=42 OR OwningProcess=43'){throw 'query must filter only exact owned PIDs'}
 if($script:queryFails){throw 'synthetic provider failure'}
 return $script:queryRows
}
if(@(Get-FixtureTcpRows @(42,43)).Count-ne 0){throw 'successful empty enumeration changed'}
$script:queryFails=$true;$failed=$false
try{Get-FixtureTcpRows @(42,43)|Out-Null}catch{if($_.Exception.Message-notlike '*synthetic provider failure*'){throw};$failed=$true}
if(-not $failed){throw 'provider failure became empty success'}
$script:queryFails=$false
$script:queryRows=@([pscustomobject]@{State=[byte]2},[pscustomobject]@{State=[byte]5},[pscustomobject]@{State=$null})
$rows=@(Get-FixtureTcpRows @(42,43))
if($rows.Count-ne 3-or $null-ne $rows[2].State){throw 'raw query discarded unknown state'}
foreach($value in @($null,0,13,99,101,'Listen','5')){if((Get-FixtureTcpState $value)-cne 'unknown'){throw 'unknown state accepted'}}
if((Get-FixtureTcpState ([byte]2))-cne 'listener'){throw 'documented listener not rejected'}
foreach($value in @(1,3,4,5,6,7,8,9,10,11,12,100)){if((Get-FixtureTcpState $value)-cne 'non-listening'){throw 'documented non-listener state changed'}}
$calls=$script:queryCalls
if(@(Get-FixtureTcpRows @()).Count-ne 0-or $script:queryCalls-ne $calls){throw 'empty PID set queried the host'}
Write-Output 'PASS raw query empty/error/unknown preservation and documented state classification; mocked provider only'
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
