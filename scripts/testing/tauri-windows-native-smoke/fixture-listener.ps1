# Pure inventory classification only; never grants process-control authority.
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
