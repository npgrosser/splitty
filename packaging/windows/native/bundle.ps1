<#
.SYNOPSIS
Builds the per-user NSIS installer of the native Splitty app.

.DESCRIPTION
Packs an already built splitty-gpui.exe with the static WSL backend archive
and its checksum file (which lies beside the archive) using installer.nsi.
Writes <OutDir>\Splitty_<version>_x64-setup.exe and prints its path and
SHA-256.

.PARAMETER Executable
The release build of splitty-gpui.exe.

.PARAMETER Payload
splitty-wsl-x86_64.tar.gz; <Payload>.sha256 must exist beside it.

.PARAMETER OutDir
Directory for the installer; created when missing.

.PARAMETER Version
The app version. Defaults to the workspace version in Cargo.toml.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Executable,

    [Parameter(Mandatory = $true)]
    [string] $Payload,

    [Parameter(Mandatory = $true)]
    [string] $OutDir,

    [Parameter()]
    [string] $Version = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path

function Resolve-File {
    param([string] $Path, [string] $What)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$What is missing: $Path"
    }
    return (Resolve-Path -LiteralPath $Path).Path
}

$Executable = Resolve-File $Executable "The native executable"
$Payload = Resolve-File $Payload "The WSL backend archive"
$checksumFile = Resolve-File "$Payload.sha256" "The WSL backend checksum"
$icon = Resolve-File (Join-Path $repoRoot "crates\splitty-tauri\icons\icon.ico") "The application icon"
$license = Resolve-File (Join-Path $repoRoot "LICENSE") "The license"
$script = Resolve-File (Join-Path $PSScriptRoot "installer.nsi") "The installer script"

# The app verifies the archive against this file before installing it; a
# mismatch here would ship an installer that cannot start.
$expected = ((Get-Content -LiteralPath $checksumFile -Raw).Trim() -split "\s+")[0].ToLowerInvariant()
$actual = (Get-FileHash -LiteralPath $Payload -Algorithm SHA256).Hash.ToLowerInvariant()
if ($expected -ne $actual) {
    throw "The WSL backend archive does not match $checksumFile."
}

if ([string]::IsNullOrWhiteSpace($Version)) {
    $inPackage = $false
    foreach ($line in Get-Content -LiteralPath (Join-Path $repoRoot "Cargo.toml")) {
        if ($line -match '^\[workspace\.package\]') { $inPackage = $true; continue }
        if ($line -match '^\[') { $inPackage = $false }
        if ($inPackage -and $line -match '^version = "(.*)"$') { $Version = $Matches[1]; break }
    }
}
if ($Version -notmatch '^(\d+)\.(\d+)\.(\d+)(-[0-9A-Za-z.]+)?$') {
    throw "'$Version' is not a version (MAJOR.MINOR.PATCH[-pre-release])."
}
$numericVersion = "$($Matches[1]).$($Matches[2]).$($Matches[3]).0"

$makensis = Get-Command "makensis.exe" -CommandType Application -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty Source
if (-not $makensis) {
    $candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} "NSIS\makensis.exe"),
        (Join-Path $env:ProgramFiles "NSIS\makensis.exe"),
        # The Tauri CLI downloads NSIS here when it builds the legacy installer.
        (Join-Path $env:LOCALAPPDATA "tauri\NSIS\makensis.exe"),
        (Join-Path $env:LOCALAPPDATA "tauri\NSIS\Bin\makensis.exe")
    )
    $makensis = $candidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
}
if (-not $makensis) {
    throw "makensis.exe was not found. Install NSIS 3 (https://nsis.sourceforge.io) or add it to PATH."
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path
$installer = Join-Path $OutDir "Splitty_${Version}_x64-setup.exe"

& $makensis "/V2" "/DVERSION=$Version" "/DNUMERIC_VERSION=$numericVersion" `
    "/DEXECUTABLE=$Executable" "/DPAYLOAD=$Payload" "/DCHECKSUM=$checksumFile" `
    "/DICON=$icon" "/DLICENSE=$license" "/DOUTFILE=$installer" $script
if ($LASTEXITCODE -ne 0) {
    throw "makensis failed with exit code $LASTEXITCODE."
}
if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
    throw "makensis succeeded but wrote no installer at $installer."
}

$hash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
Write-Host "Installer ready: $installer"
Write-Host "SHA-256: $hash"
