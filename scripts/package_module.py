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


def load_base_manifest(root: str) -> dict:
    """读取 packaging/manifest.json 作为唯一数据源。

    历史上这里把 manifest 字段硬编码在脚本里，导致 `packaging/manifest.json` 与
    实际包内清单长期漂移（新增 `minHostApiVersion` 时只改了一边）。现在以仓库里的
    manifest 为基座，脚本只覆盖与产物强相关的字段。
    """
    path = os.path.join(root, "packaging", "manifest.json")
    try:
        # utf-8-sig：允许被记事本改过、带 BOM 的文件
        with open(path, encoding="utf-8-sig") as f:
            data = json.load(f)
    except FileNotFoundError:
        die(f"缺少模块清单基座: {path}")
    except json.JSONDecodeError as e:
        die(f"模块清单不是合法 JSON: {path}: {e}")
    if not isinstance(data, dict):
        die(f"模块清单根节点必须是对象: {path}")
    return data


def write_manifest(path: str, manifest: dict) -> None:
    """写 manifest.json 并自检。

    宿主（CPPlayer-KMP）用 kotlinx.serialization 直接解析该文件，**不接受 BOM、
    注释、尾逗号**。所以这里既不手写字符串，也不依赖 shell/PS 的 JSON 序列化
    （PS 5.1 的 ConvertTo-Json 会带 BOM，历史上还产出过尾逗号，直接导致模块
    "导入失败/不显示"）。统一 json.dump 之后回读校验。
    """
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=2)
        f.write("\n")

    raw = open(path, "rb").read()
    if raw.startswith(b"\xef\xbb\xbf"):
        die("manifest.json 带 UTF-8 BOM，CPPlayer 宿主会解析失败")
    try:
        parsed = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as e:
        die(f"manifest.json 自检失败: {e}")
    for key in ("id", "name", "version", "type", "entryPoint"):
        if not parsed.get(key):
            die(f"manifest.json 缺少必需字段: {key}")


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
        required=False,
        choices=sorted(EXPECTED_SUFFIX),
        help="目标平台，决定产物后缀与 zip 命名",
    )
    parser.add_argument(
        "--validate-only",
        action="store_true",
        help="只校验 packaging/manifest.json（不打包），CI 用",
    )
    parser.add_argument(
        "--lib",
        action="append",
        default=[],
        metavar="ABI=PATH",
        help="待打包的动态库，可重复传入多架构",
    )
    parser.add_argument("--module-id", default=None, help="覆盖 manifest 的 id（默认取 packaging/manifest.json）")
    parser.add_argument("--name", default=None, help="覆盖 manifest 的 name（默认取 packaging/manifest.json）")
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

    if args.validate_only:
        base = load_base_manifest(root)
        # 与宿主 ModuleManifest 的必填项保持一致，另外把 CI 最容易漏的
        # 「version 与 Cargo.toml 漂移」也一起查掉。
        for key in ("id", "name", "version", "type", "entryPoint"):
            if not base.get(key):
                die(f"packaging/manifest.json 缺少必需字段: {key}")
        if base.get("type") != "jni":
            die(f"packaging/manifest.json 的 type 必须是 jni，收到 {base.get('type')!r}")
        crate = read_crate_version(root)
        if base.get("version") != crate:
            die(f"版本漂移：manifest={base.get('version')} Cargo.toml={crate}")
        print(
            f"OK packaging/manifest.json  id={base['id']} version={base['version']} "
            f"contract={base.get('contract')} minHostApiVersion={base.get('minHostApiVersion')}"
        )
        return 0

    if not args.platform:
        die("缺少 --platform（或用 --validate-only 只校验清单）")

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

    manifest = load_base_manifest(root)
    # 与产物强相关的字段由脚本决定；capabilities / apiMap / minHostApiVersion
    # 等一律沿用 packaging/manifest.json，避免两处维护、悄悄漂移。
    manifest["id"] = args.module_id or manifest.get("id") or DEFAULT_ID
    manifest["name"] = args.name or manifest.get("name") or DEFAULT_NAME
    manifest["version"] = version
    manifest["type"] = "jni"
    manifest["entryPoint"] = entry_point
    manifest["supportedAbis"] = abis
    manifest.setdefault("contract", "standard")

    manifest_path = os.path.join(staging, "manifest.json")
    write_manifest(manifest_path, manifest)
    print(
        f"  + manifest.json  id={manifest['id']} version={version} "
        f"abis={abis} contract={manifest.get('contract')}"
    )

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
