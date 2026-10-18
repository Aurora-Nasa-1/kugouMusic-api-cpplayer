#!/usr/bin/env bash
#
# JNI 冒烟测试：用与宿主完全一致的包名/类名/方法签名加载模块，跑一遍真实接口。
# 需要 JDK 17+（javac/java 在 PATH 里）。
#
#   tools/jni-smoke/run.sh [动态库路径] [搜索关键词]
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
LIB="${1:-$ROOT/target/release/libkugou_music_rs.so}"
KEYWORD="${2:-纯音乐}"
OUT="$ROOT/target/jni-smoke-classes"

if [ ! -f "$LIB" ]; then
    echo "找不到动态库: $LIB" >&2
    echo "先构建：cargo build --release --lib   或   ./build_module.sh" >&2
    exit 2
fi

rm -rf "$OUT"
mkdir -p "$OUT"
javac -encoding UTF-8 -d "$OUT" "$ROOT/tools/jni-smoke/JniProvider.java"
java -Dfile.encoding=UTF-8 -cp "$OUT" cp.player.core.provider.JniProvider "$LIB" "$KEYWORD"
