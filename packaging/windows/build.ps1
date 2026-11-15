<#
.SYNOPSIS
Builds the complete Splitty Windows installer locally.

.DESCRIPTION
Builds the static Linux backend in WSL and the
Windows application with the native MSVC toolchain, then packs the NSIS
installer.

.PARAMETER Distro
WSL distribution used for the Linux payload. The WSL default is used when
omitted.

.PARAMETER SkipTests
Skips the Windows unit tests of the native app.

The native app links the pinned libghostty-vt. GHOSTTY_VT_DIR names its
install prefix; when it is unset, scripts\bootstrap-libghostty-vt.py builds it
(Python 3 on PATH).
#>

[CmdletBinding()]
param(
    [Parameter()]
    [string] $Distro = "",

    [Parameter()]
    [switch] $SkipTests
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-RequiredApplication {
    param(
        [Parameter(Mandatory = $true)]
        [string[]] $Names,

        [Parameter(Mandatory = $true)]
        [string] $InstallHint
    )

    foreach ($name in $Names) {
        $command = Get-Command $name -CommandType Application -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($null -ne $command) {
            return $command.Source
        }
    }

    throw "Missing command '$($Names -join "' or '")'. $InstallHint"
}

function Invoke-External {
    param(
        [Parameter(Mandatory = $true)]
        [string] $FilePath,

        [Parameter()]
        [string[]] $ArgumentList = @()
    )

    & $FilePath @ArgumentList
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed with exit code ${LASTEXITCODE}: $FilePath $($ArgumentList -join ' ')"
    }
}

function Invoke-InDirectory {
    param(
        [Parameter(Mandatory = $true)]
        [string] $Directory,

        [Parameter(Mandatory = $true)]
        [scriptblock] $Script
    )

    Push-Location $Directory
    try {
        & $Script
    }
    finally {
        Pop-Location
    }
}

$windowsArchitecture = if ([string]::IsNullOrWhiteSpace($env:PROCESSOR_ARCHITEW6432)) {
    $env:PROCESSOR_ARCHITECTURE
}
else {
    $env:PROCESSOR_ARCHITEW6432
}
if ($windowsArchitecture -ne "AMD64") {
    throw "The local installer build currently supports Windows x64 only."
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
if ($repoRoot.StartsWith("\\")) {
    throw 'Build from a checkout on a Windows drive (for example C:\Code\splitty), not directly from \\wsl$.'
}

$wsl = Get-RequiredApplication -Names @("wsl.exe") -InstallHint "Install WSL 2 and Ubuntu or Debian first."
$cargo = Get-RequiredApplication -Names @("cargo.exe") -InstallHint "Install Rust with the MSVC toolchain from https://rustup.rs/."
$rustup = Get-RequiredApplication -Names @("rustup.exe") -InstallHint "Install Rust with the MSVC toolchain from https://rustup.rs/."
$git = Get-RequiredApplication -Names @("git.exe") -InstallHint "Install Git for Windows."

$wslPrefix = @()
if (-not [string]::IsNullOrWhiteSpace($Distro)) {
    $wslPrefix = @("-d", $Distro)
}

$commitOutput = & $git -C $repoRoot rev-parse HEAD
if ($LASTEXITCODE -ne 0) {
    throw "Could not determine the Git commit."
}
$commit = ($commitOutput -join "`n").Trim()

$wslPathArguments = @($wslPrefix) + @("--exec", "wslpath", "-a", "-u", $repoRoot)
$wslPathOutput = & $wsl @wslPathArguments
if ($LASTEXITCODE -ne 0) {
    $selection = if ($Distro) { " '$Distro'" } else { "" }
    throw "Could not start WSL distribution${selection}."
}
$repoRootWsl = ($wslPathOutput -join "`n").Trim()
if ([string]::IsNullOrWhiteSpace($repoRootWsl)) {
    throw "WSL returned no Linux path for $repoRoot."
}

Write-Host "[1/3] Building WSL and remote backend payloads"
$payloadScriptWsl = "$repoRootWsl/packaging/windows/build-wsl-payload.sh"
$resourcesWsl = "$repoRootWsl/build/windows-payload"
$remotePayloadsWsl = "$repoRootWsl/build/remote-payloads"
# Build ARM and macOS first so the x86_64 WSL backend can also install those
# hosts. The macOS servers are cross-built with zig (packaging/zig-toolchain.sh).
foreach ($remoteArch in @("aarch64", "darwin-aarch64", "darwin-x86_64")) {
    $remotePayloadArguments = @($wslPrefix) + @(
        "--exec",
        "sh",
        $payloadScriptWsl,
        $repoRootWsl,
        $remotePayloadsWsl,
        $commit,
        $remoteArch
    )
    Invoke-External -FilePath $wsl -ArgumentList $remotePayloadArguments
}

$payloadArguments = @($wslPrefix) + @(
    "--exec",
    "env",
    "SPLITTY_REMOTE_PAYLOAD_AARCH64=$remotePayloadsWsl/splitty-wsl-aarch64.tar.gz",
    "SPLITTY_REMOTE_PAYLOAD_DARWIN_AARCH64=$remotePayloadsWsl/splitty-darwin-aarch64.tar.gz",
    "SPLITTY_REMOTE_PAYLOAD_DARWIN_X86_64=$remotePayloadsWsl/splitty-darwin-x86_64.tar.gz",
    "sh",
    $payloadScriptWsl,
    $repoRootWsl,
    $resourcesWsl,
    $commit
)
Invoke-External -FilePath $wsl -ArgumentList $payloadArguments

Write-Host "[2/3] Compiling Windows application"
Invoke-External -FilePath $rustup -ArgumentList @("target", "add", "x86_64-pc-windows-msvc")

$env:SPLITTY_COMMIT = $commit
$env:SPLITTY_BUILD_TIME = "1970-01-01T00:00:00Z"
$env:SOURCE_DATE_EPOCH = "0"
$wslPayload = Join-Path $repoRoot "build\windows-payload\splitty-wsl-x86_64.tar.gz"

# The native app reaches hosts through its WSL backend, which carries the
# servers for other hosts; the executable itself embeds none.
$ghosttyLibrary = "lib\ghostty-vt-static.lib"
if ([string]::IsNullOrWhiteSpace($env:GHOSTTY_VT_DIR)) {
    $python = Get-RequiredApplication -Names @("python.exe", "py.exe") -InstallHint "Install Python 3, or set GHOSTTY_VT_DIR to a built libghostty-vt prefix."
    $prefix = & $python (Join-Path $repoRoot "scripts\bootstrap-libghostty-vt.py")
    if ($LASTEXITCODE -ne 0) {
        throw "Could not build the pinned libghostty-vt."
    }
    $env:GHOSTTY_VT_DIR = ($prefix | Select-Object -Last 1).Trim()
}
if (-not (Test-Path -LiteralPath (Join-Path $env:GHOSTTY_VT_DIR $ghosttyLibrary) -PathType Leaf)) {
    throw "GHOSTTY_VT_DIR ($env:GHOSTTY_VT_DIR) holds no $ghosttyLibrary."
}

Invoke-InDirectory -Directory $repoRoot -Script {
    if (-not $SkipTests) {
        Invoke-External -FilePath $cargo -ArgumentList @(
            "test", "--locked", "--release", "--package", "splitty-gpui",
            "--target", "x86_64-pc-windows-msvc"
        )
    }
    Invoke-External -FilePath $cargo -ArgumentList @(
        "build", "--locked", "--release", "--package", "splitty-gpui",
        "--target", "x86_64-pc-windows-msvc"
    )
}

Write-Host "[3/3] Creating per-user NSIS installer"
$bundleDirectory = Join-Path $repoRoot "target\x86_64-pc-windows-msvc\release\bundle\native"
# Only this build's installer is left there, whatever versions came before.
if (Test-Path -LiteralPath $bundleDirectory) {
    Get-ChildItem -LiteralPath $bundleDirectory -Filter "*-setup.exe" -File | Remove-Item -Force
}
& (Join-Path $PSScriptRoot "native\bundle.ps1") `
    -Executable (Join-Path $repoRoot "target\x86_64-pc-windows-msvc\release\splitty-gpui.exe") `
    -Payload $wslPayload `
    -OutDir $bundleDirectory
