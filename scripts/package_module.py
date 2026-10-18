#!/usr/bin/env python3
"""把编译好的 JNI 动态库打包成 CPPlayer 可直接导入的 Standard v3 模块 zip。

打包布局（符合 CPPlayer 宿主 PlatformSupport.resolveEntryPoint 规范）：

    module.zip
    ├── manifest.json
    └── lib/<abi>/<entryPoint>

宿主会依次尝试 lib/<平台ABI>/<entryPoint>（Android 的 ABI 取自
Build.SUPPORTED_ABIS，桌面端为 x86_64 / aarch64 / amd64 等），找不到才回退到
<entryPoint>。

用法示例：
    # Android：一个 zip 装载多 ABI
    python scripts/package_module.py --platform android \
        --lib arm64-v8a=target/jniLibs/arm64-v8a/libkugou_music_rs.so \
        --lib x86_64=target/jniLibs/x86_64/libkugou_music_rs.so

    # Windows：
    python scripts/package_module.py --platform windows \
        --lib x86_64=target/release/kugou_music_rs.dll

    # Linux：
    python scripts/package_module.py --platform linux \
        --lib x86_64=target/x86_64-unknown-linux-gnu/release/libkugou_music_rs.so
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import zipfile

DEFAULT_ID = "kugou-music-rs"
DEFAULT_NAME = "酷狗音乐"

EXPECTED_SUFFIX = {
    "android": ".so",
    "linux": ".so",
    "windows": ".dll",
    "macos": ".dylib",
}


def die(msg: str) -> None:
    print(f"Error: {msg}", file=sys.stderr)
    raise SystemExit(1)


def read_crate_version(root: str) -> str:
    """从 Cargo.toml 读取 version，避免 manifest 与代码版本漂移。"""
    cargo_toml = os.path.join(root, "Cargo.toml")
    try:
        with open(cargo_toml, encoding="utf-8") as f:
            for line in f:
                m = re.match(r'\s*version\s*=\s*"([^"]+)"', line)
                if m:
                    return m.group(1)
    except OSError:
        pass
    return "0.1.0"


def parse_lib_args(pairs: list[str]) -> list[tuple[str, str]]:
    out = []
    for item in pairs:
        if "=" not in item:
            die(f"--lib 需要写成 <abi>=<path>，收到: {item!r}")
        abi, path = item.split("=", 1)
        abi = abi.strip()
        if not abi:
            die(f"--lib 的 abi 为空: {item!r}")
        out.append((abi, path))
    if not out:
        die("至少需要一个 --lib <abi>=<path>")
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description="打包 CPPlayer JNI 模块 zip (Standard v3)")
    parser.add_argument(
        "--platform",
        required=True,
        choices=sorted(EXPECTED_SUFFIX),
        help="目标平台，决定产物后缀与 zip 命名",
    )
    parser.add_argument(
        "--lib",
        action="append",
        default=[],
        metavar="ABI=PATH",
        help="待打包的动态库，可重复传入多架构",
    )
    parser.add_argument("--module-id", default=DEFAULT_ID, help="manifest 的 id")
    parser.add_argument("--name", default=DEFAULT_NAME, help="manifest 的 name")
    parser.add_argument("--version", default=None, help="manifest 版本，默认读 Cargo.toml")
    parser.add_argument(
        "--expected-version",
        default=None,
        help="校验 Cargo.toml 的 version 与之一致",
    )
    parser.add_argument(
        "--out",
        default=None,
        help="输出 zip 路径",
    )
    parser.add_argument(
        "--staging-dir",
        default=None,
        help="中间目录，默认 target/cp_module/<platform>",
    )
    args = parser.parse_args()

    root = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
    entries = parse_lib_args(args.lib)

    if args.expected_version:
        crate = read_crate_version(root)
        if crate != args.expected_version:
            die(f"版本漂移：期望 {args.expected_version}，但 Cargo.toml 的 version 是 {crate}。")

    lib_names = {os.path.basename(path) for _, path in entries}
    if len(lib_names) != 1:
        die(f"各 ABI 的库文件名必须一致，收到: {sorted(lib_names)}")
    entry_point = lib_names.pop()

    suffix = EXPECTED_SUFFIX[args.platform]
    if not entry_point.endswith(suffix):
        die(f"{args.platform} 的产物应为 *{suffix}，收到 {entry_point!r}")

    staging = args.staging_dir or os.path.join(root, "target", "cp_module", args.platform)
    out_zip = args.out or os.path.join(root, "target", f"kugou-music-rs-{args.platform}.zip")
    os.makedirs(os.path.dirname(out_zip), exist_ok=True)

    if os.path.isdir(staging):
        for dirpath, _dirnames, filenames in os.walk(staging, topdown=False):
            for name in filenames:
                os.remove(os.path.join(dirpath, name))
            os.rmdir(dirpath)
    os.makedirs(staging, exist_ok=True)

    abis = []
    for abi, path in entries:
        if not os.path.isfile(path):
            die(f"找不到动态库: {path}")
        size = os.path.getsize(path)
        if size < 1024:
            die(f"动态库过小 ({size} bytes): {path}")
        abis.append(abi)
        dest_dir = os.path.join(staging, "lib", abi)
        os.makedirs(dest_dir, exist_ok=True)
        dest = os.path.join(dest_dir, entry_point)
        with open(path, "rb") as src, open(dest, "wb") as dst:
            dst.write(src.read())
        print(f"  + lib/{abi}/{entry_point}  ({size} bytes)")

    # 桌面端如果只有一个库，也在根目录下放一份，方便向后兼容直接回退
    if args.platform != "android" and len(entries) == 1:
        root_dest = os.path.join(staging, entry_point)
        with open(entries[0][1], "rb") as src, open(root_dest, "wb") as dst:
            dst.write(src.read())
        print(f"  + {entry_point}  (根目录兼容)")

    version = args.version or read_crate_version(root)
    manifest = {
        "id": args.module_id,
        "name": args.name,
        "version": version,
        "type": "jni",
        "entryPoint": entry_point,
        "contract": "standard",
        "capabilities": [
            "search",
            "playback",
            "lyricsSearch",
            "qrLogin",
            "charts",
        ],
        "loginMethods": ["qr", "cookie"],
        "targetAppPackage": "com.kugou.android",
        "supportedAbis": abis,
        "apiMap": {
            "cloudsearch": "search",
            "song/url/v1": "song/url",
            "song/url/v1/302": "song/url",
            "song/download/url/v1": "song/url",
            "lyric/new": "lyric",
            "song/detail": "song/detail",
            "search/hot/detail": "search/hot",
            "search/suggest": "search/suggest",
            "toplist": "toplist",
            "toplist/detail": "toplist",
            "login/qr/key": "login/qr/key",
            "login/qr/create": "login/qr/create",
            "login/qr/check": "login/qr/check",
            "login/status": "login/status",
        },
    }

    manifest_path = os.path.join(staging, "manifest.json")
    with open(manifest_path, "w", encoding="utf-8", newline="") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=2)
        f.write("\n")
    print(f"  + manifest.json  id={manifest['id']} version={version} abis={abis} contract=standard")

    if os.path.exists(out_zip):
        os.remove(out_zip)
    with zipfile.ZipFile(out_zip, "w", zipfile.ZIP_DEFLATED) as zf:
        for dirpath, _dirnames, filenames in os.walk(staging):
            for name in sorted(filenames):
                full = os.path.join(dirpath, name)
                zf.write(full, os.path.relpath(full, staging))

    print(f"\nOK -> {os.path.relpath(out_zip, root)}")
    with zipfile.ZipFile(out_zip) as zf:
        for info in zf.infolist():
            print(f"    {info.filename}  ({info.file_size} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
