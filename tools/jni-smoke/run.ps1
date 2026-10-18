# JNI 冒烟测试（Windows）。
#
# 用与宿主完全一致的包名 / 类名 / 方法签名加载模块，跑一遍真实接口。
# 需要 JDK 17+（javac.exe / java.exe 在 PATH 里，或设置 $env:JAVA_HOME）。
#
#   powershell -ExecutionPolicy Bypass -File tools\jni-smoke\run.ps1
#   powershell -ExecutionPolicy Bypass -File tools\jni-smoke\run.ps1 -Lib target\release\kugou_music_rs.dll
[CmdletBinding()]
param(
    [string]$Lib = "",
    [string]$Keyword = "纯音乐"
)

$ErrorActionPreference = "Stop"
$Root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
if (-not $Lib) { $Lib = Join-Path $Root "target\release\kugou_music_rs.dll" }
if (-not [IO.Path]::IsPathRooted($Lib)) { $Lib = Join-Path $Root $Lib }

if (-not (Test-Path $Lib)) {
    throw "找不到动态库: $Lib`n先构建：cargo build --release --lib 或 .\build_module_desktop.ps1"
}

$java = if ($env:JAVA_HOME) { Join-Path $env:JAVA_HOME "bin\java.exe" } else { "java" }
$javac = if ($env:JAVA_HOME) { Join-Path $env:JAVA_HOME "bin\javac.exe" } else { "javac" }

$out = Join-Path $Root "target\jni-smoke-classes"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Path $out -Force | Out-Null

# 参数一律用数组传：Windows PowerShell 会把内联的 `-Dfile.encoding=UTF-8`
# 当成自己的参数解析（报 "找不到或无法加载主类 .encoding=UTF-8"）。
$javacArgs = @("-encoding", "UTF-8", "-d", $out, (Join-Path $Root "tools\jni-smoke\JniProvider.java"))
& $javac @javacArgs
if ($LASTEXITCODE -ne 0) { throw "javac 编译失败" }

$javaArgs = @("-Dfile.encoding=UTF-8", "-cp", $out, "cp.player.core.provider.JniProvider", $Lib, $Keyword)
& $java @javaArgs
exit $LASTEXITCODE
