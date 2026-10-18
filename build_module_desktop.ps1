# Requires PowerShell 5.1+
[CmdletBinding()]
param(
    [string]$MsvcRoot = "E:\project\build\VC\Tools\MSVC\14.51.36231",
    [string]$SdkRoot = "C:\Program Files (x86)\Windows Kits\10",
    # 目标架构：x86_64（默认）或 aarch64。决定 cargo --target 与 zip 内的 lib/<abi>/ 目录名。
    [string]$Arch = "x86_64"
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
$target = if ($Arch -eq "aarch64") { "aarch64-pc-windows-msvc" } else { "x86_64-pc-windows-msvc" }
cargo build --release --lib --features jni --target $target
if ($LASTEXITCODE -ne 0) { throw "cargo build 失败（退出码 $LASTEXITCODE）" }

$dll = "target\$target\release\kugou_music_rs.dll"
if (-not (Test-Path $dll)) {
    throw "Build output not found: $dll"
}

# 5. Packaging CPPlayer Module
#    统一走 scripts/package_module.py：
#      · manifest 以 packaging/manifest.json 为唯一数据源（不再在这里拼字段）
#      · 输出用 Python json.dump，**没有 BOM、没有尾逗号**
#    以前这里用 ConvertTo-Json 写 manifest，PS 5.1 会带 UTF-8 BOM，历史上还产出过
#    尾逗号 —— 宿主 kotlinx.serialization 直接解析失败，模块表现为"导入失败/不显示"。
Write-Host "==> Packaging CPPlayer Module..." -ForegroundColor Green

# 优先用 scripts/package_module.py（与 CI 完全同一条路径）。
# 找一个**真的能用**的 Python：PATH 里排在前面的
# %LOCALAPPDATA%\Microsoft\WindowsApps\python.exe 是应用商店占位符，未安装时
# 一执行就返回 9009 —— 会被误读成"打包失败"。
function Resolve-Python {
    $candidates = @()
    if ($env:PYTHON) { $candidates += $env:PYTHON }
    foreach ($name in @("python", "python3", "py")) {
        $cmd = Get-Command $name -ErrorAction SilentlyContinue
        if ($cmd -and $cmd.Source) { $candidates += $cmd.Source }
    }
    $candidates += Get-ChildItem "$env:LOCALAPPDATA\Programs\Python\*\python.exe" -ErrorAction SilentlyContinue |
        Select-Object -ExpandProperty FullName

    foreach ($exe in $candidates) {
        if (-not (Test-Path $exe)) { continue }
        & $exe -c "import sys" 2>$null 1>$null
        if ($LASTEXITCODE -eq 0) { return $exe }
    }
    return $null
}

$outZip = Join-Path $Root "target\kugou-music-rs-windows.zip"
$python = Resolve-Python

if ($python) {
    Write-Host "    using python: $python" -ForegroundColor DarkGray
    & $python (Join-Path $Root "scripts\package_module.py") `
        --platform windows `
        --lib "$Arch=$dll" `
        --out $outZip
    if ($LASTEXITCODE -ne 0) { throw "打包失败: package_module.py 退出码 $LASTEXITCODE" }
} else {
    # 无 Python 时的原生兜底。要点：**显式 UTF8 无 BOM**。
    # 原先用 `Set-Content -Encoding UTF8`，PS 5.1 会写入 BOM，宿主
    # kotlinx.serialization 解析直接失败（模块导入即失败/不显示）。
    Write-Host "    python 不可用，改用内置打包器" -ForegroundColor DarkYellow

    $stagingDir = Join-Path $Root "target\cp_module\windows"
    if (Test-Path $stagingDir) { Remove-Item -Recurse -Force $stagingDir }
    New-Item -ItemType Directory -Path (Join-Path $stagingDir "lib\$Arch") -Force | Out-Null
    Copy-Item $dll -Destination (Join-Path $stagingDir "lib\$Arch\kugou_music_rs.dll")
    Copy-Item $dll -Destination (Join-Path $stagingDir "kugou_music_rs.dll")

    $manifest = Get-Content (Join-Path $Root "packaging\manifest.json") -Raw -Encoding UTF8 | ConvertFrom-Json
    $manifest.entryPoint = "kugou_music_rs.dll"
    $manifest.supportedAbis = @($Arch)
    $json = $manifest | ConvertTo-Json -Depth 10

    # 尾逗号是非法 JSON，但 PS 的 ConvertFrom-Json 有时会放过；先自查再用严格解析。
    if ($json -match ',\s*[}\]]') {
        throw "生成的 manifest.json 含尾逗号（非法 JSON），请安装 Python 3 后重试。"
    }
    $outPath = Join-Path $stagingDir "manifest.json"
    [IO.File]::WriteAllText($outPath, $json, (New-Object System.Text.UTF8Encoding($false)))

    $bytes = [IO.File]::ReadAllBytes($outPath)
    if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
        throw "manifest.json 带 BOM，宿主会解析失败。"
    }
    Get-Content $outPath -Raw | ConvertFrom-Json | Out-Null

    if (Test-Path $outZip) { Remove-Item -Force $outZip }
    Compress-Archive -Path "$stagingDir\*" -DestinationPath $outZip -Force
}

Write-Host "`n[SUCCESS] Windows module packaged successfully at target/kugou-music-rs-windows.zip" -ForegroundColor Green
