# Parse production source, execute only its pure evidence function. No driver,
# hosted-environment spoofing, registry, WSL or process-launch operation.
$ErrorActionPreference='Stop'
$source=Join-Path $PSScriptRoot 'run.ps1'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile($source,[ref]$tokens,[ref]$errors)
if($errors.Count){throw ($errors|ForEach-Object{$_.Message}|Out-String)}
$function=$ast.Find({param($node)$node-is [Management.Automation.Language.FunctionDefinitionAst]-and $node.Name-eq 'Assert-IsolationReplayEvidence'},$true)
if(-not $function){throw 'pure replay guard missing'}
. ([scriptblock]::Create($function.Extent.Text))
$capture=[pscustomobject]@{phase='capture-requested';sessionState=[pscustomobject]@{writeAcceptedAt=10;replayOpenStartedAt=11;attachSentAt=12}}
$replay=[pscustomobject]@{phase='pressure-replay';replay=[pscustomobject]@{markerPresent=$true}}
if((Assert-IsolationReplayEvidence $capture $replay).attachSentAt-ne 12){throw 'valid evidence refused'}
foreach($field in @('writeAcceptedAt','replayOpenStartedAt','attachSentAt')){
 $saved=$capture.sessionState.$field;$capture.sessionState.$field=$null;$denied=$false
 try{Assert-IsolationReplayEvidence $capture $replay|Out-Null}catch{$denied=$true}
 if(-not $denied){throw 'missing timing accepted'};$capture.sessionState.$field=$saved
}
$capture.sessionState.attachSentAt=9;$denied=$false
try{Assert-IsolationReplayEvidence $capture $replay|Out-Null}catch{$denied=$true}
if(-not $denied){throw 'reversed timing accepted'};$capture.sessionState.attachSentAt=12
$replay.replay.markerPresent=$false;$denied=$false
try{Assert-IsolationReplayEvidence $capture $replay|Out-Null}catch{$denied=$true}
if(-not $denied){throw 'missing marker accepted'}
$branch=$ast.Find({param($node)$node-is [Management.Automation.Language.IfStatementAst]-and $node.Clauses[0].Item1.Extent.Text-eq '$RunIsolationConsumer'-and $node.Clauses[0].Item2.Extent.Text.Contains('Invoke-NativeApplication -IsolationConsumer')},$true)
if(-not $branch){throw 'separate isolation scenario missing'}
if($branch.Clauses[0].Item2.Extent.Text-match 'Write-Config \$false|Probe-Forwarding \$false'){throw 'true scenario enters false controls'}
if($branch.ElseClause.Extent.Text-notmatch 'Write-Config \$false'-or $branch.ElseClause.Extent.Text-notmatch "Probe-Forwarding \`$false 'after-native'"){throw 'existing false mode changed'}
Write-Output 'PASS pure replay chronology guard and separate true/false scenario AST checks; no driver execution'
