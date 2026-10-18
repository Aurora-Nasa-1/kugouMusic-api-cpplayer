# kugou-music-rs

CPPlayer-KMP 的酷狗音乐音源模块。Rust 写成的 `cdylib`，以 **Standard v3（`contract: "standard"`）** 契约直接跑在宿主进程里 —— 无独立服务、无端口转发，搜索/播放链接/歌词/榜单/扫码登录都在 Native 层完成。

源码提取自 [md3Music](https://github.com/zzyoxml/md3Music) 的 `kugou_api_server/rust`，之后按 CPPlayer 的模块规范做了大幅改造：JNI 边界、扁平化数据契约、设备身份注册、跨平台打包都已重写。

---

## 直接安装（推荐）

去 [Releases](https://github.com/Aurora-Nasa-1/kugouMusic-api-cpplayer/releases) 下载与宿主平台对应的 zip，在 CPPlayer「音源管理 → 导入模块」里选中即可，不需要自己编译。

| 宿主平台 | 下载 |
| :--- | :--- |
| Android（arm64-v8a / x86_64 / armeabi-v7a，三合一包） | `kugou-music-rs-android.zip` |
| Windows x64 | `kugou-music-rs-windows-x86_64.zip` |
| Windows on ARM | `kugou-music-rs-windows-aarch64.zip` |
| Linux x64 | `kugou-music-rs-linux-x86_64.zip` |
| Linux arm64 | `kugou-music-rs-linux-aarch64.zip` |

每个 zip 旁边有 `SHA256SUMS.txt` 可校验下载完整性。

> 各平台库的扩展名不同（`.so` / `.dll`），而 manifest 里的 `entryPoint` 只能写一个文件名，**所以不存在"一个包通吃所有平台"**。Android 之所以能三个 ABI 合成一个包，是因为它们同名同后缀。装错平台的包，宿主会提示找不到入口库。

---

## 已实现的路由

宿主通过 `nativeCallApi(method, paramsJson)` 调用，`method` 与 `manifest.json` 的 `apiMap` 对应：

| 路由 | 说明 | 关键返回字段（`data`） |
| :--- | :--- | :--- |
| `cloudsearch` | 歌曲搜索 | `tracks[]` / `total` |
| `song/url/v1` | 播放直链 | `url` / `format` / `quality` / `headers` |
| `lyric/new` | 歌词（LRC） | `lyric` |
| `song/detail` | 歌曲详情 | `tracks[]` |
| `toplist` / `toplist/detail` | 榜单与榜单歌曲 | `list[]` / `tracks[]` |
| `search/hot/detail` | 热搜词 | `searchWord` |
| `search/suggest` | 搜索联想 | 建议词列表 |
| `login/qr/key` → `login/qr/create` → `login/qr/check` | 扫码登录 | `unikey` / `qrurl` / `code` + `cookie` |
| `login/status` | 登录态检查 | 酷狗 VIP 详情原样透传 |

歌曲对象是扁平的：`{ id, title, artist, album, duration, coverUrl }`。`duration` 单位为**秒**，`artist` 是可直接展示的字符串 —— 不是旧版网易云的 `songs[].ar[].name` 那套嵌套结构。

返回值永远是 JSON 字符串。上游失败也是 `{"code": 500, "msg": "..."}`，不会抛出异常或让宿主崩溃。

---

## JNI 接口

类名固定为 `cp.player.core.provider.JniProvider`（JNI 按 `Java_<包名下划线化>_<类名>_<方法名>` 查符号，改名即 `UnsatisfiedLinkError`）：

```java
native void   startNativeServer(String host, int port);          // 启动内置 HTTP 调试服务
native String nativeCallApi(String method, String paramsJson);   // 进程内直调（推荐）
native String analyzeAudioFile(String path);                     // 暂未实现，返回 error JSON
```

`startNativeServer` 是可选能力：只有想用浏览器/curl 调 REST 接口时才需要，端口传 `0` 让系统分配。

`legacy-jni-symbols` feature 额外导出三个历史包名的别名符号（`cp.cpplayer.core.provider`、`cp.player.kmp.provider`、`cp.player.provider`），默认不开启。

---

## 从源码构建

```bash
cargo build --release --lib          # jni 已是默认 feature
```

产物：Windows `target/release/kugou_music_rs.dll`，Linux/Android `libkugou_music_rs.so`。

打成可安装的模块包：

```bash
python scripts/package_module.py --platform windows \
  --lib x86_64=target/release/kugou_music_rs.dll \
  --out target/kugou-music-rs-windows-x86_64.zip
```

脚本以 `packaging/manifest.json` 为唯一数据源，生成的 manifest 会回读自检（合法性、无 BOM、必填字段），`--validate-only` 可单独校验清单。

Windows 本地一键脚本（自动找 MSVC 环境、编译、打包）：

```powershell
powershell -ExecutionPolicy Bypass -File .\build_module_desktop.ps1
```

Android 需要 NDK 与 `cargo-ndk`：

```bash
./build_module.sh android        # arm64-v8a / x86_64 / armeabi-v7a
```

---

## 模块包结构

```text
kugou-music-rs-windows-x86_64.zip
├── manifest.json                 # 模块元数据 + apiMap
├── kugou_music_rs.dll            # 根目录一份（桌面端直接加载，兼容旧宿主）
└── lib/
    └── x86_64/
        └── kugou_music_rs.dll    # 按 ABI 分目录
```

Android 包同理，换成 `lib/<abi>/libkugou_music_rs.so`。

---

## 排障

**装上了但调不通（`UnsatisfiedLinkError`）**
库的导出符号必须与 `cp.player.core.provider.JniProvider` 严格对应。用 `tools/jni-smoke` 可以在不启动宿主的情况下复现：

```bash
bash tools/jni-smoke/run.sh target/release/libkugou_music_rs.so
# Windows
powershell -ExecutionPolicy Bypass -File tools\jni-smoke\run.ps1
```

它会用与宿主完全一致的包名/类名/方法签名加载库，逐个跑真实接口并汇总 pass/fail。

**搜索有结果但播放没声**
播放直链接口需要真实设备身份（`dfid`）。模块会在首次调用 `song/url` 时自动 `register/dev` 并缓存到数据目录的 `device_info.json`，正常情况下无需干预。如果返回 `no playable url`，看返回里的 `reason`：常见是版权/付费曲，或调用方传了无效 `cookie` 触发了风控。

**模块导入失败**
多半是 `manifest.json` 不合法（BOM、尾逗号、缺字段）。跑 `python scripts/package_module.py --validate-only` 定位。

---

## CI

`.github/workflows/build.yml`：

- push 任意分支 / PR / 手动触发 → 构建全部平台，产物上传为 workflow artifact
- push tag `v*` → 先跑清单校验与单测，再构建 Android 3 ABI、Windows x64+arm64、Linux x64+arm64，合并发 Release

```bash
git tag v0.1.0 && git push origin v0.1.0
```

---

## 免责声明

本项目仅用于学习与技术研究。所有音乐内容版权归酷狗音乐及相应版权方所有，请勿用于任何商业用途。接口来自第三方客户端，可能随时变更或失效。
