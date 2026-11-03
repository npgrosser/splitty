# Pure memory/formatter tests. No process, GUI, WSL or user profile is launched/read.
$ErrorActionPreference='Stop'
Add-Type -Path (Join-Path $PSScriptRoot 'fixture-job.cs')
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class SplittyFixtureArgv {
 [DllImport("shell32.dll",CharSet=CharSet.Unicode,SetLastError=true)]static extern IntPtr CommandLineToArgvW(string line,out int count);
 [DllImport("kernel32.dll")]static extern IntPtr LocalFree(IntPtr value);
 public static string[] Parse(string line){int count;IntPtr buffer=CommandLineToArgvW(line,out count);if(buffer==IntPtr.Zero)throw new Exception("argument parser failed");try{string[] args=new string[count];for(int i=0;i<count;i++)args[i]=Marshal.PtrToStringUni(Marshal.ReadIntPtr(buffer,i*IntPtr.Size));return args;}finally{LocalFree(buffer);}}
}
'@
$exe='C:\Fixture Tools\probe.exe'
$cases=@(
 [pscustomobject]@{Values=[string[]]@('-d','splitty-native-fixture-000000000000-a','--exec','sh','-c','printf "two words"')},
 [pscustomobject]@{Values=[string[]]@('', 'trailing space \', 'slashes\"quote', "tab`tvalue", "line`nvalue", 'plain\path')}
)
foreach($case in $cases){
 $line=[SplittyFixtureJob]::FormatCommandLine($exe,$case.Values)
 $parsed=[SplittyFixtureArgv]::Parse($line)
 if($parsed.Count-ne $case.Values.Count+1-or $parsed[0]-cne $exe){throw 'argument count/executable mismatch'}
 for($index=0;$index-lt $case.Values.Count;$index++){if($parsed[$index+1]-cne $case.Values[$index]){throw ('argument roundtrip mismatch at '+$index)}}
}
$plain=[SplittyFixtureJob]::FormatCommandLine('C:\Windows\System32\wsl.exe',$cases[0].Values)
if(-not $plain.StartsWith('C:\Windows\System32\wsl.exe -d splitty-native-fixture-000000000000-a --exec sh -c ')-or $plain.Contains('"-d"')-or $plain.Contains('"--exec"')){throw 'WSL option/name tokens quoted'}
$nulRejected=$false;try{[SplittyFixtureJob]::FormatCommandLine($exe,@("bad`0arg"))|Out-Null}catch{$nulRejected=$true}
if(-not $nulRejected-or [SplittyFixtureJob]::LiveHandleCount-ne 0){throw 'NUL accepted or pure test opened owned handles'}
'PASS: 2 argument roundtrips, plain WSL options, NUL rejection; zero process launches/owned handles'

foreach($file in @('wsl-runtime.ps1','fixture-sentinel.ps1','fixture-sentinel.test.ps1','fixture-registration.ps1','fixture-registration.test.ps1')){
 $tokens=$null;$errors=$null;$null=[System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot $file),[ref]$tokens,[ref]$errors)
 if($errors.Count){throw ('PowerShell syntax errors in '+$file+': '+($errors|Out-String))}
}
'PASS: gated PowerShell source parses without execution'
$permitRefused=$false
try{& (Join-Path $PSScriptRoot 'wsl-runtime.ps1') -RootDir 'C:\unused'}catch{$permitRefused=$_.Exception.Message-like '*Explicit isolated full WSL permit required*'}
if(-not $permitRefused-or [SplittyFixtureJob]::LiveHandleCount-ne 0){throw 'direct full driver ran without explicit permit'}
'PASS: direct full WSL driver refuses missing permit before fixture operations'
