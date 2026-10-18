# kugou-music-rs (CPPlayer-KMP 酷狗音源模块)

本项目从开源项目 [md3Music](https://github.com/zzyoxml/md3Music) 的 `kugou_api_server/rust` 中完整提取并保留 Rust 服务端源码，在此基础上进行了深度改造与适配：

1. **CPPlayer-KMP JNI 接口对接**：提供与 CPPlayer-KMP 完全兼容的高性能 JNI 动态链接库出口。
2. **纯粹 Standard v3 API 规范**：严格执行扁平化 `Standard v3` 数据契约（`contract: "standard"`），彻底摒弃旧版网易云多层嵌套兼容模式。
3. **三端两架构跨平台支持**：覆盖 **Android / Windows / Linux (以及 macOS)** 三大平台，支持 `x86_64` 与 `aarch64` (ARM64) 两大主流 CPU 架构（以及 Android `armeabi-v7a`）。
4. **双工作模式**：
   - **进程内 Direct JNI 调用**：零端口占用、无网络转发损耗，直接在 Native 层完成解密并返回 Standard v3 JSON。
   - **内置 HTTP Server 模式**：通过 `startNativeServer(port)` 启动内置 Actix-web 服务，支持标准 RESTful 调用。

---

## 目录结构

```text
kugouMusic-api-cpplayer/
├── Cargo.toml                     # Rust 编译配置 (cdylib, rlib, jni features)
├── build_module_desktop.ps1       # Windows MSVC 本地一键编译与打包脚本
├── build_module.sh                # Linux / Android / macOS 交叉编译与打包脚本
├── .github/
│   └── workflows/
│       └── build.yml              # GitHub Actions 三端二架构自动化打包与 Release
├── packaging/
│   └── manifest.json              # CPPlayer 模块清单 (contract: "standard", apiMap, capabilities)
├── scripts/
│   └── package_module.py          # 跨平台规范 Zip 打包工具
├── src/
│   ├── lib.rs                     # 库入口，导出 C FFI / JNI
│   ├── jni.rs                     # CPPlayer JNI 接口实现 (nativeCallApi, startNativeServer)
│   ├── standard_api.rs            # Standard v3 API 扁平化数据转换器
│   ├── server.rs                  # Actix-web 内置 HTTP 服务
│   ├── service/                   # 酷狗业务核心 (搜索, 播放链接, 歌词, 榜单, 二维码等)
│   ├── crypto/                    # 酷狗专有加密算法 (Kugou MD5, RSA, DES)
│   ├── model/                     # 酷狗原始数据模型
│   └── utils/                     # 辅助工具函数
└── tests/
    └── standard_api_test.rs       # Standard v3 契约自动化单测
```

---

## Standard v3 API 契约规范

模块在 `manifest.json` 中声明 `"contract": "standard"`，通过 `nativeCallApi(path, queryJson)` 返回的所有数据均遵循标准格式：

### 响应基础结构
所有响应均为扁平 JSON：
```json
{
  "code": 0,
  "msg": "success",
  "data": { ... }
}
```

### 核心接口映射

| Standard v3 路由 | 说明 | 关键返回字段 (`data`) |
| :--- | :--- | :--- |
| `/api/v3/search` | 歌曲搜索 | `{ "tracks": [ { "id", "title", "artist", "album", "duration", "coverUrl" } ], "total": 100 }` |
| `/api/v3/song/url` | 获取播放链接 | `{ "url": "https://...", "format": "mp3", "quality": "320k", "headers": { ... } }` |
| `/api/v3/lyric` | 歌词获取 | `{ "lyric": "[00:00.00]...", "transLyric": null, "romaLyric": null }` |
| `/api/v3/toplist` | 热门榜单推荐 | `[ { "id": "rank_id", "name": "酷狗飙升榜", "coverUrl": "..." } ]` |
| `/api/v3/toplist/detail`| 榜单歌曲详情 | `{ "tracks": [ { "id", "title", "artist", "album", "duration", "coverUrl" } ] }` |
| `/api/v3/login/qr/key` | 生成二维码 Key | `{ "unikey": "...", "qrurl": "https://..." }` |
| `/api/v3/login/qr/check` | 轮询二维码状态 | `{ "code": 801/802/803, "msg": "...", "cookie": "..." }` |

> **注意与旧版网易云结构的区别**：
> - 歌曲列表直接命名为 `tracks`，不再嵌套 `songs[].al.picUrl` / `songs[].ar[].name`。
> - 时长字段 `duration` 统一以**秒 (s)**为单位，不是毫秒 (ms)。
> - 艺术家字段 `artist` 为直接可展示的扁平字符串。

---

## JNI 导出接口规范

Java 层类名统一对应 `cp.player.core.provider.JniProvider`：

```rust
// 启动内置 HTTP 服务器（可选）
#[no_mangle]
pub extern "system" fn Java_cp_player_core_provider_JniProvider_startNativeServer(
    env: JNIEnv,
    _class: JClass,
    port: jint,
) -> jint

// 直接内存调用 API（推荐，高性能无端口占用）
#[no_mangle]
pub extern "system" fn Java_cp_player_core_provider_JniProvider_nativeCallApi(
    mut env: JNIEnv,
    _class: JClass,
    j_path: JString,
    j_query_json: JString,
) -> jstring

// 音频文件本地元数据解析（支持 kgm / flac / mp3 等分析）
#[no_mangle]
pub extern "system" fn Java_cp_player_core_provider_JniProvider_analyzeAudioFile(
    mut env: JNIEnv,
    _class: JClass,
    j_file_path: JString,
) -> jstring
```

同时也保留了 `com_plugin_kugou_NativeServer_*` 与 `com_kugou_music_NativeBridge_*` 兼容性符号别名。

---

## 本地编译与打包

### 1. Windows 桌面版 (x86_64 / MSVC)
运行仓库自带的 PowerShell 打包脚本：
```powershell
powershell -ExecutionPolicy Bypass -File .\build_module_desktop.ps1
```
产物将输出至：`target/kugou-music-rs-windows.zip`。

### 2. Linux / Android 交叉编译
确保系统已安装相应 target 以及 Android NDK，执行：
```bash
chmod +x build_module.sh
# 编译全部支持架构
./build_module.sh --all
# 或只编译 Android 架构
./build_module.sh --android
```

---

## GitHub Actions 持续集成 (CI/CD)

项目已配置完整的 GitHub Actions 工作流 (`.github/workflows/build.yml`)：
- **Android**: `aarch64-linux-android` (arm64-v8a), `x86_64-linux-android` (x86_64), `armv7-linux-androideabi` (armeabi-v7a)
- **Windows**: `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`
- **Linux**: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`
- **自动化 Release**: 推送 tag（如 `v1.0.0`）时，CI 会自动编译所有平台的库并打包成符合 CPPlayer 规范的 Zip 发布包。

---

## CPPlayer-KMP 模块包规格

打包输出的 Zip 文件内部结构如下：

```text
kugou-music-rs-windows.zip
├── manifest.json              # 模块元数据与 API 路由表
├── kugou_music_rs.dll         # 根目录库（兼容桌面端直接加载）
└── lib/
    └── x86_64/
        └── kugou_music_rs.dll # 多架构目录
```

在 Android 或全架构全平台包中：
```text
kugou-music-rs-all.zip
├── manifest.json
└── lib/
    ├── arm64-v8a/
    │   └── libkugou_music_rs.so
    ├── armeabi-v7a/
    │   └── libkugou_music_rs.so
    ├── x86_64/
    │   ├── libkugou_music_rs.so
    │   └── kugou_music_rs.dll
    └── aarch64/
        ├── libkugou_music_rs.so
        └── kugou_music_rs.dll
```
将该 Zip 包直接放入 CPPlayer-KMP 的 `modules/` 目录或通过客户端“本地安装扩展模块”即可直接生效。
