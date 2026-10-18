# Requires PowerShell 5.1+
[CmdletBinding()]
param(
    [string]$MsvcRoot = "E:\project\build\VC\Tools\MSVC\14.51.36231",
    [string]$SdkRoot = "C:\Program Files (x86)\Windows Kits\10"
)

$ErrorActionPreference = "Stop"
$Root = $PSScriptRoot
Set-Location $Root

function Get-LatestVersionDir {
    param([string]$Parent, [string]$Probe)
    if (-not (Test-Path $Parent)) { return $null }
    Get-ChildItem -Directory $Parent |
        Sort-Object { try { [version]$_.Name } catch { [version]'0.0.0' } } -Descending |
        Where-Object { Test-Path (Join-Path $_.FullName $Probe) } |
        Select-Object -First 1 -ExpandProperty FullName
}

# 1. Locate MSVC
if (-not (Test-Path $MsvcRoot)) {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $install = & $vswhere -latest -products * -property installationPath | Select-Object -First 1
        if ($install) { $MsvcRoot = Join-Path $install "VC\Tools\MSVC" }
    }
}

$msvcVersionDir = if (Test-Path (Join-Path $MsvcRoot "bin\Hostx64\x64\cl.exe")) {
    $MsvcRoot
} else {
    Get-LatestVersionDir -Parent $MsvcRoot -Probe "bin\Hostx64\x64\cl.exe"
}

if (-not $msvcVersionDir) { throw "MSVC cl.exe not found under $MsvcRoot" }

# 2. Locate Windows SDK
$sdkLibDir = Join-Path $SdkRoot "Lib"
$sdkVersionDir = Get-LatestVersionDir -Parent $sdkLibDir -Probe "um\x64\kernel32.lib"
if (-not $sdkVersionDir) { throw "Windows SDK kernel32.lib not found under $SdkRoot" }
$sdkVersion = Split-Path -Leaf $sdkVersionDir

Write-Host "==> MSVC: $msvcVersionDir" -ForegroundColor Cyan
Write-Host "==> Windows SDK: $SdkRoot ($sdkVersion)" -ForegroundColor Cyan

# 3. Environment variables
$env:Path = "$(Join-Path $msvcVersionDir 'bin\Hostx64\x64');" + $env:Path
$incList = @(
    (Join-Path $msvcVersionDir 'include'),
    (Join-Path $SdkRoot "Include\$sdkVersion\ucrt"),
    (Join-Path $SdkRoot "Include\$sdkVersion\shared"),
    (Join-Path $SdkRoot "Include\$sdkVersion\um")
)
$env:INCLUDE = ($incList -join ";") + ";$env:INCLUDE"

$libList = @(
    (Join-Path $msvcVersionDir 'lib\x64'),
    (Join-Path $SdkRoot "Lib\$sdkVersion\ucrt\x64"),
    (Join-Path $SdkRoot "Lib\$sdkVersion\um\x64")
)
$env:LIB = ($libList -join ";") + ";$env:LIB"

# 4. Cargo build
Write-Host "==> Building Release Dynamic Library..." -ForegroundColor Green
cargo build --release --lib --features jni

$dll = "target\release\kugou_music_rs.dll"
if (-not (Test-Path $dll)) {
    throw "Build output not found: $dll"
}

# 5. Packaging CPPlayer Module
Write-Host "==> Packaging CPPlayer Module..." -ForegroundColor Green
$stagingDir = Join-Path $Root "target\cp_module\windows"
if (Test-Path $stagingDir) { Remove-Item -Recurse -Force $stagingDir }
New-Item -ItemType Directory -Path (Join-Path $stagingDir "lib\x86_64") -Force | Out-Null

Copy-Item $dll -Destination (Join-Path $stagingDir "lib\x86_64\kugou_music_rs.dll")
Copy-Item $dll -Destination (Join-Path $stagingDir "kugou_music_rs.dll")

$manifestPath = Join-Path $Root "packaging\manifest.json"
$manifestJson = Get-Content $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
$manifestJson.entryPoint = "kugou_music_rs.dll"
$manifestJson | ConvertTo-Json -Depth 10 | Set-Content (Join-Path $stagingDir "manifest.json") -Encoding UTF8

$outZip = Join-Path $Root "target\kugou-music-rs-windows.zip"
if (Test-Path $outZip) { Remove-Item -Force $outZip }
Compress-Archive -Path "$stagingDir\*" -DestinationPath $outZip -Force

Write-Host "`n[SUCCESS] Windows module packaged successfully at target/kugou-music-rs-windows.zip" -ForegroundColor Green
