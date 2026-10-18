#!/usr/bin/env bash
#
# 构建 CPPlayer 酷狗音源模块 (Standard v3 JNI)。
#
# 用法：
#   ./build_module.sh                 # 按当前 OS 构建桌面端（linux / macos / windows）
#   ./build_module.sh android         # Android 多架构：arm64-v8a / x86_64 / armeabi-v7a
#   ./build_module.sh linux|windows|macos
#
# 产物：target/kugou-music-rs-<platform>.zip，可直接在 CPPlayer 音源管理中导入。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

PLATFORM=""
for arg in "$@"; do
    case "$arg" in
        -h|--help)
            sed -n '2,12p' "$0"
            exit 0
            ;;
        *) PLATFORM="$arg" ;;
    esac
done

if [ -z "$PLATFORM" ]; then
    case "$(uname -s)" in
        Linux*)  PLATFORM="linux" ;;
        Darwin*) PLATFORM="macos" ;;
        MINGW*|MSYS*|CYGWIN*) PLATFORM="windows" ;;
        *) echo "无法识别当前系统: $(uname -s)，请显式指定平台 (android/linux/windows/macos)" >&2; exit 1 ;;
    esac
fi

case "$PLATFORM" in
    android|linux|windows|macos) ;;
    *) echo "未知平台: $PLATFORM（可选 android / linux / windows / macos）" >&2; exit 1 ;;
esac

PYTHON="${PYTHON:-}"
if [ -z "$PYTHON" ]; then
    if command -v python3 >/dev/null 2>&1; then PYTHON=python3; else PYTHON=python; fi
fi

FEATURES="jni"

echo "==> 平台: $PLATFORM    features: $FEATURES"

case "$PLATFORM" in
    android)
        ABIS=("arm64-v8a" "x86_64" "armeabi-v7a")
        if ! command -v cargo-ndk >/dev/null 2>&1; then
            echo "Error: 未安装 cargo-ndk，请先执行: cargo install cargo-ndk" >&2
            exit 1
        fi
        if [ -z "${ANDROID_NDK_HOME:-}" ] && [ -d "$HOME/Android/Sdk/ndk" ]; then
            LATEST_NDK="$(ls -d "$HOME"/Android/Sdk/ndk/* 2>/dev/null | sort -V | tail -n 1)"
            if [ -n "$LATEST_NDK" ]; then
                export ANDROID_NDK_HOME="$LATEST_NDK"
                echo "Set ANDROID_NDK_HOME=$ANDROID_NDK_HOME"
            fi
        fi
        # Android 15+ 要求的 16KB page size 兼容
        export RUSTFLAGS="-Clink-arg=-Wl,-z,max-page-size=16384"

        echo "==> cargo ndk 构建: ${ABIS[*]}"
        cargo ndk -t arm64-v8a -t x86_64 -t armeabi-v7a \
            -o ./target/jniLibs build --release --features "$FEATURES,jni-android"

        LIB_ARGS=()
        for abi in "${ABIS[@]}"; do
            SO_FILE="$(find "./target/jniLibs/$abi" -maxdepth 1 -name 'lib*.so' | head -n 1)"
            if [ -z "$SO_FILE" ] || [ ! -f "$SO_FILE" ]; then
                echo "Error: ./target/jniLibs/$abi 下没找到 .so" >&2
                exit 1
            fi
            LIB_ARGS+=("--lib" "$abi=$SO_FILE")
        done
        ;;

    linux)
        TRIPLE="${TARGET_TRIPLE:-x86_64-unknown-linux-gnu}"
        LIB_NAME="libkugou_music_rs.so"
        ABI="x86_64"
        if [ "$TRIPLE" = "aarch64-unknown-linux-gnu" ]; then
            ABI="aarch64"
        fi
        echo "==> cargo build --target $TRIPLE"
        cargo build --release --features "$FEATURES" --target "$TRIPLE"
        LIB_ARGS=("--lib" "$ABI=target/$TRIPLE/release/$LIB_NAME")
        ;;

    windows)
        TRIPLE="${TARGET_TRIPLE:-x86_64-pc-windows-msvc}"
        LIB_NAME="kugou_music_rs.dll"
        ABI="x86_64"
        echo "==> cargo build --target $TRIPLE"
        cargo build --release --features "$FEATURES" --target "$TRIPLE"
        LIB_ARGS=("--lib" "$ABI=target/$TRIPLE/release/$LIB_NAME")
        ;;

    macos)
        if [ "$(uname -m)" = "arm64" ]; then
            TRIPLE="${TARGET_TRIPLE:-aarch64-apple-darwin}"
            ABI="aarch64"
        else
            TRIPLE="${TARGET_TRIPLE:-x86_64-apple-darwin}"
            ABI="x86_64"
        fi
        LIB_NAME="libkugou_music_rs.dylib"
        echo "==> cargo build --target $TRIPLE"
        cargo build --release --features "$FEATURES" --target "$TRIPLE"
        LIB_ARGS=("--lib" "$ABI=target/$TRIPLE/release/$LIB_NAME")
        ;;
esac

echo "==> 打包模块 zip"
"$PYTHON" scripts/package_module.py --platform "$PLATFORM" "${LIB_ARGS[@]}"
