# JNI Smoke Test (Windows)
[CmdletBinding()]
param(
    [string]$Lib = "",
    [string]$Keyword = "music"
)

$ErrorActionPreference = "Stop"
$Root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
if (-not $Lib) {
    $candidate1 = Join-Path $Root "target\x86_64-pc-windows-msvc\release\kugou_music_rs.dll"
    $candidate2 = Join-Path $Root "target\release\kugou_music_rs.dll"
    $Lib = if (Test-Path $candidate1) { $candidate1 } else { $candidate2 }
}
if (-not [IO.Path]::IsPathRooted($Lib)) { $Lib = Join-Path $Root $Lib }

if (-not (Test-Path $Lib)) {
    throw "Dynamic library not found: $Lib"
}

$javaDir = if ($env:JAVA_HOME -and (Test-Path $env:JAVA_HOME)) { $env:JAVA_HOME } elseif (Test-Path "E:\java") { "E:\java" } else { "" }
$java = if ($javaDir) { Join-Path $javaDir "bin\java.exe" } else { "java" }
$javac = if ($javaDir) { Join-Path $javaDir "bin\javac.exe" } else { "javac" }

$out = Join-Path $Root "target\jni-smoke-classes"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Path $out -Force | Out-Null

# 1. Typed SDK Wire JNI Test
$javacArgs = @("-encoding", "UTF-8", "-d", $out, (Join-Path $Root "tools\jni-smoke\KugouSessionSmoke.java"))
& $javac @javacArgs
if ($LASTEXITCODE -ne 0) { throw "javac (KugouSessionSmoke) failed" }

Write-Host ">>> Running Typed SDK Session Smoke Test..." -ForegroundColor Cyan
$javaArgs = @("-Dfile.encoding=UTF-8", "-cp", $out, "cp.player.core.provider.kugou.KugouSessionSmoke", $Lib)
& $java @javaArgs
if ($LASTEXITCODE -ne 0) { throw "KugouSessionSmoke failed" }

# 2. Legacy JniProvider Test
$javacArgs = @("-encoding", "UTF-8", "-d", $out, (Join-Path $Root "tools\jni-smoke\JniProvider.java"))
& $javac @javacArgs
if ($LASTEXITCODE -ne 0) { throw "javac (JniProvider) failed" }

Write-Host ""
Write-Host ">>> Running Legacy JniProvider Smoke Test..." -ForegroundColor Cyan
$javaArgs = @("-Dfile.encoding=UTF-8", "-cp", $out, "cp.player.core.provider.JniProvider", $Lib, $Keyword)
& $java @javaArgs
exit $LASTEXITCODE
