# Pure source unit: no registry, process, WSL, or global configuration actions.
param([string]$Source,[string]$GuardBase64)
$ErrorActionPreference='Stop'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseInput($Source,[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'fixture helper parse failed'}
$assignment=$ast.Find({param($node)$node-is[Management.Automation.Language.AssignmentStatementAst]-and $node.Left.Extent.Text-eq '$fixtureGuestGuard'},$true)
$function=$ast.Find({param($node)$node-is[Management.Automation.Language.FunctionDefinitionAst]-and $node.Name-eq 'Get-FixtureGuestArguments'},$true)
if(-not $assignment-or-not $function){throw 'pure delegated helper definitions absent'}
$RootDir=Join-Path ([IO.Path]::GetTempPath()) ('splitty-guard-source-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $RootDir|Out-Null
try{
 [IO.File]::WriteAllBytes((Join-Path $RootDir 'fixture-guest-guard.sh'),[Convert]::FromBase64String($GuardBase64))
 $name='splitty-native-fixture-000000000000-a'
 $created=[pscustomobject]@{};$created|Add-Member -NotePropertyName $name -NotePropertyValue 'synthetic-path'
 $ownership=[pscustomobject]@{names=@($name);created=$created}
 # Only the side-effect-free argument helper and its fixture-local guard read
 # execute. Invoke-FixtureGuest and every launch function remain unevaluated.
 . ([scriptblock]::Create($assignment.Extent.Text))
 . ([scriptblock]::Create($function.Extent.Text))
 $arguments=Get-FixtureGuestArguments $name @('/bin/sh','-c',"printf touched > `"`$1`"`r`n",'synthetic','literal-data')
 if($arguments[5].Contains("`r")-or $arguments[10].Contains("`r")){throw 'delegated guest shell source retains CRLF'}
 $refused=$false
 try{Get-FixtureGuestArguments 'unowned-distro' @('/usr/bin/id')|Out-Null}catch{$refused=$true}
 if(-not $refused){throw 'unowned delegated target accepted'}
 $dataArguments=Get-FixtureGuestArguments $name @('/usr/bin/printf','%s',"data`r`nkept")
 if($dataArguments[-1]-cne "data`r`nkept"){throw 'non-shell data was normalized'}
 [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes(($arguments|ConvertTo-Json -Compress)))
}finally{Remove-Item -LiteralPath $RootDir -Recurse -Force}
