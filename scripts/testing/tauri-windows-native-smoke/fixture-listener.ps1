# Read-only inventory and pure classification; never grants process-control authority.
function Get-FixtureTcpRows([uint32[]]$OwnedIds) {
 if(-not $OwnedIds.Count){return}
 if($OwnedIds.Count-gt 256-or $OwnedIds-contains 0){throw 'invalid owned TCP inventory PID set'}
 $filter=($OwnedIds|Sort-Object -Unique|ForEach-Object{"OwningProcess=$_"}) -join ' OR '
 # No State filter: a provider returning null/zero must not look like absence.
 Get-CimInstance -Namespace root/StandardCimv2 -ClassName MSFT_NetTCPConnection -Filter $filter -OperationTimeoutSec 5 -ErrorAction Stop
}

function Get-FixtureTcpState($RawState) {
 # MSFT_NetTCPConnection documents 1..12. The Windows NetTCPIP cmdlet
 # additionally documents Bound; its shipped CDXML maps Bound to 100.
 # https://learn.microsoft.com/en-us/windows/win32/fwp/wmi/nettcpipprov/msft-nettcpconnection
 # https://learn.microsoft.com/en-us/powershell/module/nettcpip/get-nettcpconnection
 $numeric=$RawState-is [byte]-or $RawState-is [int16]-or $RawState-is [uint16]-or $RawState-is [int32]-or $RawState-is [uint32]-or $RawState-is [int64]-or $RawState-is [uint64]
 if($numeric-and $RawState-eq 2){return 'listener'}
 if($numeric-and (($RawState-ge 1-and $RawState-le 12)-or $RawState-eq 100)){return 'non-listening'}
 return 'unknown'
}

function Test-FixtureSocketGeneration([object[]]$Before,[object[]]$After,$SocketCreated,$SampleEnd) {
 if($Before.Count-ne 1-or $After.Count-ne 1){return $false}
 if(-not $Before[0].Alive-or-not $After[0].Alive-or $Before[0].Id-ne $After[0].Id-or $Before[0].Creation-cne $After[0].Creation-or $Before[0].ExecutablePath-cne $After[0].ExecutablePath){return $false}
 if(-not $SocketCreated-or-not $SampleEnd){return $false}
 try{
  $birth=[DateTimeOffset]::Parse([string]$Before[0].Creation,[Globalization.CultureInfo]::InvariantCulture)
  $created=[DateTimeOffset]::Parse([string]$SocketCreated,[Globalization.CultureInfo]::InvariantCulture)
  $end=[DateTimeOffset]::Parse([string]$SampleEnd,[Globalization.CultureInfo]::InvariantCulture)
 }catch{return $false}
 return $created-ge $birth-and $created-le $end
}
