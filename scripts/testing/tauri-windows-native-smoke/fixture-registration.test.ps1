# Pure metadata tests. No registry, guest, process or filesystem operation.
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'fixture-registration.ps1')
$fixtureRoot='C:\Temp\splitty-native-windows-123456789abc'
$a='splitty-native-fixture-123456789abc-a';$b='splitty-native-fixture-abcdef123456-b'
function Metadata {
 $created=[ordered]@{};$created[$a]=$fixtureRoot+'\distributions\'+$a;$created[$b]=$fixtureRoot+'\distributions\'+$b
 return [pscustomobject]@{names=@($a,$b);before=[pscustomobject]@{Ubuntu='C:\existing'};created=[pscustomobject]$created}
}
function Registered {return @([pscustomobject]@{DistributionName=$a;BasePath='\\?\'+$fixtureRoot+'\distributions\'+$a},[pscustomobject]@{DistributionName=$b;BasePath=$fixtureRoot+'\distributions\'+$b})}
function Refuses($ownership,$entries,$temp='C:\Temp'){$refused=$false;try{Assert-FixtureRegistrations $fixtureRoot $ownership $entries $temp}catch{$refused=$true};if(-not$refused){throw 'invalid synthetic fixture metadata accepted'}}
Assert-FixtureRegistrations $fixtureRoot (Metadata) (Registered) 'C:\Temp'
$changed=Registered;$changed[1].BasePath='C:\existing';Refuses (Metadata) $changed
$old=Metadata;$old.before|Add-Member -NotePropertyName $a -NotePropertyValue 'C:\existing';Refuses $old (Registered)
$outside=Metadata;$outside.created.$b='C:\elsewhere';Refuses $outside (Registered)
$one=Metadata;$one.names=@($a);Refuses $one (Registered)
$duplicates=Metadata;$duplicates.names=@($a,$a);Refuses $duplicates (Registered)
Refuses (Metadata) @((Registered)[0])
Refuses (Metadata) ((Registered)+@((Registered)[0]))
Refuses (Metadata) (Registered) 'C:\OtherTemp'
'PASS: exact A/B paths plus 8 invalid registration/ownership cases; pure metadata only'

Assert-FixtureExecutable $fixtureRoot ($fixtureRoot+'\target\debug\splitty-gui.exe')
foreach($foreign in @('C:\Windows\System32\notepad.exe','C:\Temp\old-cache\target\debug\splitty-gui.exe',($fixtureRoot+'\target\debug\other.exe'))){
 $refused=$false;try{Assert-FixtureExecutable $fixtureRoot $foreign}catch{$refused=$true}
 if(-not$refused){throw 'unrelated executable accepted for fixture launch'}
}
'PASS: only exact fresh fixture executable accepted; 3 foreign/cache/filename paths refused'
