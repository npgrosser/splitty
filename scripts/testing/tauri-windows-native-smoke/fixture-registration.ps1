# Pure registration validation. Caller supplies the metadata snapshot; no guest,
# process or registry operation occurs inside this helper.
function Convert-FixtureRegistrationPath([string]$Value) {
 if(-not $Value){throw 'empty fixture registration path'}
 if($Value.StartsWith('\\?\',[StringComparison]::Ordinal)){$Value=$Value.Substring(4)}
 return [System.IO.Path]::GetFullPath($Value).TrimEnd([char[]]'\/')
}
function Assert-FixtureRegistrations([string]$RootDir,$Ownership,$Registered,[string]$ExpectedTempRoot) {
 $root=Convert-FixtureRegistrationPath $RootDir
 $temp=Convert-FixtureRegistrationPath $ExpectedTempRoot
 if(-not [System.IO.Path]::GetDirectoryName($root).Equals($temp,[StringComparison]::OrdinalIgnoreCase)){throw 'fixture root is outside the expected TEMP directory'}
 if([System.IO.Path]::GetFileName($root)-notmatch '^splitty-native-windows-[0-9a-f]{12}$'){throw 'unexpected marked fixture root name'}
 if(@($Ownership.names).Count-ne 2-or @($Ownership.created.PSObject.Properties).Count-ne 2){throw 'exactly two recorded fixture distributions required'}
 if($Ownership.names[0]-ceq $Ownership.names[1]-or-not $Ownership.names[0].EndsWith('-a')-or-not $Ownership.names[1].EndsWith('-b')){throw 'expected distinct A and B fixture names'}
 foreach($name in $Ownership.names){
  if($name-notmatch '^splitty-native-fixture-[0-9a-f]{12}-[ab]$'-or $Ownership.before.PSObject.Properties[$name]){throw 'distribution name is not newly owned'}
  $property=$Ownership.created.PSObject.Properties[$name]
  if(-not $property){throw 'recorded fixture path absent'}
  $expected=Convert-FixtureRegistrationPath (Join-Path (Join-Path $root 'distributions') $name)
  $recorded=Convert-FixtureRegistrationPath ([string]$property.Value)
  if(-not $recorded.Equals($expected,[StringComparison]::OrdinalIgnoreCase)){throw 'recorded fixture path outside its marked root'}
  $matches=@($Registered|Where-Object{$_.DistributionName-ceq $name})
  if($matches.Count-ne 1){throw 'expected exactly one registration for the private fixture'}
  $actual=Convert-FixtureRegistrationPath ([string]$matches[0].BasePath)
  if(-not $actual.Equals($expected,[StringComparison]::OrdinalIgnoreCase)){throw 'fixture registration path changed'}
 }
}

function Assert-FixtureExecutable([string]$RootDir,[string]$Executable) {
 $root=Convert-FixtureRegistrationPath $RootDir
 $expected=Convert-FixtureRegistrationPath (Join-Path $root 'target\debug\splitty-gui.exe')
 $actual=Convert-FixtureRegistrationPath $Executable
 if(-not $actual.Equals($expected,[StringComparison]::OrdinalIgnoreCase)){throw 'only the freshly built fixture executable may launch'}
}
