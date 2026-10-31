<#
.SYNOPSIS
Builds the complete Splitty Windows installer locally.

.DESCRIPTION
Builds the frontend on Windows, the static Linux backend in WSL, and the
Tauri/NSIS application with the native MSVC toolchain. Incremental Cargo
artifacts are reused by both the release desktop tests and the Tauri build.

.PARAMETER Distro
WSL distribution used for the Linux payload. The WSL default is used when
omitted.

.PARAMETER SkipTests
Skips the frontend type check and Windows desktop unit tests.
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
$pnpm = Get-RequiredApplication -Names @("pnpm.cmd", "pnpm.exe") -InstallHint "Install Node.js 24 and pnpm 10."
$cargo = Get-RequiredApplication -Names @("cargo.exe") -InstallHint "Install Rust with the MSVC toolchain from https://rustup.rs/."
$rustup = Get-RequiredApplication -Names @("rustup.exe") -InstallHint "Install Rust with the MSVC toolchain from https://rustup.rs/."
$git = Get-RequiredApplication -Names @("git.exe") -InstallHint "Install Git for Windows."

$wslPrefix = @()
if (-not [string]::IsNullOrWhiteSpace($Distro)) {
    $wslPrefix = @("-d", $Distro)
}

Write-Host "[1/4] Preparing frontend"
Invoke-InDirectory -Directory (Join-Path $repoRoot "frontend") -Script {
    Invoke-External -FilePath $pnpm -ArgumentList @("install", "--frozen-lockfile")
    if (-not $SkipTests) {
        Invoke-External -FilePath $pnpm -ArgumentList @("check")
    }
    Invoke-External -FilePath $pnpm -ArgumentList @("build")
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

Write-Host "[2/4] Building WSL and remote backend payloads"
$payloadScriptWsl = "$repoRootWsl/packaging/windows/build-wsl-payload.sh"
$resourcesWsl = "$repoRootWsl/crates/splitty-tauri/resources"
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

Write-Host "[3/4] Compiling native Windows application"
Invoke-External -FilePath $rustup -ArgumentList @("target", "add", "x86_64-pc-windows-msvc")

$env:SPLITTY_COMMIT = $commit
$env:SPLITTY_BUILD_TIME = "1970-01-01T00:00:00Z"
$env:SOURCE_DATE_EPOCH = "0"
$env:SPLITTY_REMOTE_PAYLOAD_X86_64 = Join-Path $repoRoot "crates\splitty-tauri\resources\splitty-wsl-x86_64.tar.gz"
$env:SPLITTY_REMOTE_PAYLOAD_AARCH64 = Join-Path $repoRoot "build\remote-payloads\splitty-wsl-aarch64.tar.gz"
$env:SPLITTY_REMOTE_PAYLOAD_DARWIN_AARCH64 = Join-Path $repoRoot "build\remote-payloads\splitty-darwin-aarch64.tar.gz"
$env:SPLITTY_REMOTE_PAYLOAD_DARWIN_X86_64 = Join-Path $repoRoot "build\remote-payloads\splitty-darwin-x86_64.tar.gz"

if (-not $SkipTests) {
    Invoke-InDirectory -Directory $repoRoot -Script {
        Invoke-External -FilePath $cargo -ArgumentList @(
            "test",
            "--locked",
            "--release",
            "--package",
            "splitty-gui",
            "--target",
            "x86_64-pc-windows-msvc",
            "--lib"
        )
    }
}

$tauri = Join-Path $repoRoot "frontend\node_modules\.bin\tauri.cmd"
if (-not (Test-Path -LiteralPath $tauri -PathType Leaf)) {
    throw "Tauri CLI was not installed at $tauri."
}

Write-Host "[4/4] Creating per-user NSIS installer"
Invoke-InDirectory -Directory (Join-Path $repoRoot "crates\splitty-tauri") -Script {
    Invoke-External -FilePath $tauri -ArgumentList @(
        "build",
        "--target",
        "x86_64-pc-windows-msvc",
        "--bundles",
        "nsis"
    )
}

$bundleDirectory = Join-Path $repoRoot "target\x86_64-pc-windows-msvc\release\bundle\nsis"
$installer = Get-ChildItem -LiteralPath $bundleDirectory -Filter "*.exe" -File |
    Sort-Object LastWriteTimeUtc -Descending |
    Select-Object -First 1
if ($null -eq $installer) {
    throw "The build succeeded but no NSIS installer was found in $bundleDirectory."
}

$installerHash = (Get-FileHash -LiteralPath $installer.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
Write-Host ""
Write-Host "Installer ready: $($installer.FullName)"
Write-Host "SHA-256: $installerHash"
