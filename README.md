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
| `artist/detail` / `artist/songs` / `artist/album` | 歌手资料与作品 | `data` / `tracks[]` / `data[]` |
| `album` / `album/detail` | 专辑资料与曲目 | `data`（含 `tracks[]`） |
| `user/playlist` / `playlist/detail` / `playlist/track/all` | 用户歌单与曲目 | `data[]` / `data.tracks[]` |
| `playlist/create` | 创建歌单（需登录） | 根层 `id` / `playlist.id` |
| `like` / `likelist` | 喜欢、取消喜欢 / 全量喜欢列表（需登录） | `code` / 根层 `ids[]` |
| `playlist/tracks` | 添加 / 移除歌单歌曲（需登录） | `code` / `body.count` |
| `playlist/delete` / `playlist/subscribe` | 删除自建歌单 / 收藏、取消收藏歌单（需登录） | `code` |
| `user/playlist/create` / `user/playlist/collect` | 我创建 / 收藏的歌单（只读） | `playlist[]` / `data[]` |
| `recommend/resource` / `recommend/songs` | 推荐歌单与每日推荐 | `data[]` / `data.dailySongs[]` |
| `personalized` / `top_playlist` / `top_playlist_highquality` | 推荐 / 热门 / 精选歌单 | `result[]` / `playlists[]` |
| `top_album` / `album/new` / `album/newest` | 分区新碟 | `albums[]` |
| `top_artists` | 热门歌手 | `artists[]` |
| `top_song` / `personalized_newsong` | 新歌速递 | `data[]` / `result[]` |
| `banner` | 首页焦点图及可打开的音乐专区 | `banners[]` |
| `personal_fm` / `simi_song` / `playmode/intelligence/list` | 私人 FM / 相似推荐 / 心动模式 | `data[]` / `songs[]` |
| `comment/new` / `comment/floor` / `comment` | 评论列表、楼层与发表 | `data.comments[]` / `data.comment` |
| `search/hot/detail` | 热搜词 | `searchWord` |
| `search/suggest` | 搜索联想 | 建议词列表 |
| `login/qr/key` → `login/qr/create` → `login/qr/check` | 扫码登录 | `unikey` / `qrurl` / `code` + `cookie` |
| `login/status` | 登录态检查 | `data.account` / `data.profile`（未登录时为 `null`） |

歌曲对象是扁平的：`{ id, title, artist, album, duration, coverUrl }`。`duration` 单位为**秒**，`artist` 是可直接展示的字符串 —— 不是旧版网易云的 `songs[].ar[].name` 那套嵌套结构。清单声明 `apiVersion: 3` 与 `apiVersionTag: "standard-v3"`，扩展接口也沿用统一的 `code` / `data` 响应外壳。

返回值永远是 JSON 字符串。上游失败也是 `{"code": 500, "msg": "..."}`，不会抛出异常或让宿主崩溃。

---

## JNI 接口

类名固定为 `cp.player.core.provider.JniProvider`（JNI 按 `Java_<包名下划线化>_<类名>_<方法名>` 查符号，改名即 `UnsatisfiedLinkError`）：

```java
native void   startNativeServer(String host, int port);          // 启动内置 HTTP 调试服务
native String nativeCallApi(String method, String paramsJson);   // 进程内直调（推荐）
native String analyzeAudioFile(String path);                     // 暂未实现，返回 error JSON
```

内置 HTTP 服务的 `/api/<method>` 与 `nativeCallApi(method, paramsJson)` 共用 Standard v3 适配层，例如 `POST /api/login/status`；桌面端隔离 Worker 也通过这一入口调用模块。参数放在 JSON 对象的顶层，也可通过 URL 查询参数传入（同名时请求体优先）；该入口不缓存响应，避免登录态或不同请求参数串用缓存。

不带 `/api/` 的路径保留原生酷狗接口格式，不能与 Standard v3 混用。找不到路由或请求 JSON 无效时，服务分别返回 HTTP 404 / 400 和 JSON 错误对象，不再返回 `Cannot POST ...` 等纯文本响应。

歌单标识也需要适配：个人歌单的 `listid`、公开歌单的数字 `specialid`、上游读取公开歌单所需的 `collection_…` 全局标识不是同一种 ID。Standard 接口会先识别当前账号内的个人歌单；公开数字 ID 则通过歌单资料解析成全局标识，再读取详情与歌曲。个人歌单走云歌单 `/v4/get_list_all_file_v3`，不再误用公开歌单接口。详情支持上游 `data[]` 结构，返回完整歌曲总数；分页保留精确 `offset`，并同时提供根层与 `data.hasMore`。创建歌单将 `privacy` 转成上游 `is_pri`，将新 `listid` 转成宿主识别的根层 `id`。上游失败保留 `errmsg` 和错误码，不再仅返回笼统的 500。

从 0.1.1 起，云歌单的 `status: 1`、`count: 0`、`info: null` 被识别为正常空歌单，不再误报 502。空歌单没有真实封面时保留空值；非空歌单详情可使用第一首曲目的真实封面。歌曲封面支持 `trans_param.union_cover` 等嵌套字段，跳过空的高优先级字段，并将酷狗图片地址规范为 HTTPS、替换尺寸占位符。

歌曲详情缺少图片时，通过 `/images` 批量查询真实专辑封面和歌手资料；补图失败不会丢弃已经取得的歌曲。歌手搜索缺少头像时，以最多 4 个并发请求补查详情，单次请求超时 2 秒，4 秒后不再调度新的补图请求。专辑搜索兼容原生 `albumname`、`singername`、`imgurl` 字段，避免把正常结果过滤为空。

排查是否仍加载旧库时，调用 Standard 方法 `provider/info`（HTTP 调试入口为 `/api/provider/info`），检查 `data.version` 是否与导入包的版本一致。此接口不访问上游、不返回登录凭据。重新导入包后应重启播放器，使已经载入进程的旧 DLL 退出。

从 0.1.2 起，排行榜列表返回独立的虚拟歌单 ID（`2^52 + rankid`，仍在 JavaScript 安全整数范围内），避免榜单 ID 与酷狗公共歌单、账号本地歌单 ID 混用。宿主点击榜单时继续调用 `playlist/detail` 和 `playlist/track/all`，模块按虚拟 ID 转到 `/rank/info`、`/rank/audio`，保留榜单名称、封面、总数和分页。榜单歌曲优先使用真实音频 Hash，而不是数值型资源 ID。更新后请刷新首页排行榜，旧缓存中的原始榜单 ID 不能安全地与普通歌单 ID 区分。

`user/playlist/create` 按宿主契约表示“我创建的歌单”，`user/playlist/collect` 表示“我收藏的歌单”，均为只读；真正的新建操作只有 `playlist/create`。

从 0.1.3 起，首页 Standard 方法名被显式适配，不再把 `top_song`、`top_album`、`top_artists`、`personalized`、`personal_fm` 等交给无法匹配的原生路由。日推读取真实的 `data.song_list`；推荐歌单改用歌单推荐服务，避免把日推歌曲误当歌单。歌单封面优先取真实歌单图片，不使用推荐条目中表示作者头像的 `pic`。

新碟「全部」交错合并华语、欧美、日本、韩国四个分区并去重。新歌「全部」走原生新歌发布流；华语 / 欧美 / 日本 / 韩国按 `7 / 96 / 8 / 16` 从对应分区的新碟提取歌曲（响应标记 `source: regional_new_albums`），不是将 Standard 分区数字误当酷狗榜单 ID。

焦点图的音乐专区通过独立虚拟歌单 ID（`2^51 + ip_id`）打开真实专区歌曲，支持详情、封面和精确 offset 分页；使用固定顺序，并以 `extra.page_total` 限定真正可读取的总数，避免大专区显示数千首却在上游最多可读的 1000 首后继续翻页报错。没有播放器可打开目标的外部广告不伪造为歌曲或歌单。相似歌曲以准确匹配 Hash 的 `album_audio_id`（搜索结果中的 `MixSongID`）调用酷狗 AI 推荐，不能将普通 `audio_id` 混用为推荐种子。心动模式先验证所选歌单可读取，再按种子歌曲调用同一推荐服务；并不承诺与其他平台算法相同。

`startNativeServer` 是可选能力：只有想用浏览器/curl 调 REST 接口时才需要，端口传 `0` 让系统分配。

`legacy-jni-symbols` feature 额外导出三个历史包名的别名符号（`cp.cpplayer.core.provider`、`cp.player.kmp.provider`、`cp.player.provider`），默认不开启。

---

## 喜欢与歌单写入

以下为 Standard 方法，HTTP 调试时在路径前加 `/api/`；JNI 直接使用表中的方法名。所有方法都需要当前账号的 `userid`、`token`，支持字符串或对象形式的 `cookie`。

| 方法 | 参数 |
| :--- | :--- |
| `like` | `id`：32 位歌曲 Hash；`like`：`true` / `false`，兼容布尔值和字符串，默认 `true` |
| `likelist` | 可选 `uid`，必须与登录账号一致；返回根层 `ids[]`，Hash 统一为小写 |
| `playlist/tracks` | `pid`：账号内歌单 `listid`；`op`：`add` / `del`；`tracks`：逗号分隔 Hash 或 Hash 数组 |
| `playlist/delete` | `id`：当前账号创建的歌单 `listid` |
| `playlist/subscribe` | `id`：公开歌单 ID；`t`：`1` 收藏 / `2` 取消收藏；取消收藏也支持账号列表返回的本地 `listid` |

喜欢操作会完整分页查找账号中名为“我喜欢”“我喜欢的音乐”“我喜欢的歌曲”或“喜欢的音乐”的自建歌单，不硬编码 `listid=1`；找不到或存在歧义时返回 404 / 409，不自动创建或误写其他歌单。喜欢列表会读取全部分页，而不是只同步前 30 首。

加歌按 Hash 查询原生歌曲资料，保留歌名、专辑和混音 ID，并用结构化数据传给云歌单，歌名含逗号或竖线也不会拆坏。移除歌曲、取消喜欢会先读取目标歌单，将 Hash 精确匹配为真实 `fileid` 后删除；已经不存在的歌曲按幂等成功处理。歌曲写入只允许当前账号的自建歌单，禁止删除“我喜欢”歌单，也不接受榜单或音乐专区的虚拟 ID。

成功响应为 `code: 0`；`playlist/tracks` 的 `body.count` 表示实际提交的歌曲条数。参数错误、未登录、无权限分别返回 400 / 401 / 403。上游未确认成功、歌曲资料缺失、分页异常或返回业务错误时不会伪造成功；多批写入中途失败会返回 `partial` 和 `completed`，表示是否可能已部分执行及已确认完成的条数。

逐曲结果中的 `code=1` 不直接套用 HTTP 成功码，也不盲目视为成功。增删歌曲遇到这种含义不明的结果时，会完整回读目标歌单，最多确认 3 次（只重试读取，不重发写入）：添加按 Hash 确认存在，删除按本次提交的 `fileid` 确认已移除；确认成功返回 `body.verified: true`。仍无法确认时保留错误，提供 `upstreamOperation`、`upstreamField`、`verification`（`mismatch` / `unavailable`）及必要的 `verificationError`，避免只返回没有来源的 `upstreamCode: 1`。`partial: true` 表示可能部分执行，不等同于确定成功了一部分。

`like` 请求示例：

```json
{"id":"aabbccddeeff00112233445566778899","like":"true","cookie":"userid=123; token=YOUR_TOKEN"}
```

`playlist/tracks` 请求示例：

```json
{"op":"add","pid":"198","tracks":"aabbccddeeff00112233445566778899","cookie":"userid=123; token=YOUR_TOKEN"}
```

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

Windows 可使用 `python tools/standard-api-audit.py <DLL绝对路径>` 只读检查公共歌单、曲目、歌曲详情、歌手/专辑及真实图片下载。设置一个保存 Cookie 的环境变量，并传 `--cookie-env <变量名>`，还可检查登录账号下的歌单；脚本只输出状态和数量，不输出 Cookie、用户资料或歌单标题，也不创建或修改歌单。

**搜索有结果但播放没声**
播放直链接口需要真实设备身份（`dfid`）。模块会在首次调用 `song/url` 时自动 `register/dev` 并缓存到数据目录的 `device_info.json`，正常情况下无需干预。Standard 入口会把宿主的 Cookie 字符串转换为底层请求需要的对象，保留 Token、用户 ID 与会员字段。

如果返回 `no playable url`，看返回里的 `data.reason`：`pkg,buy` 等上游限制会保留，并返回非成功 `code`，不再以 `code: 0` / `reason: null` 掩盖失败。需要会员或购买的曲目仍受账号实际权限限制。成功响应的 `data.quality` 表示实际返回音质，`data.requestedQuality` 表示请求音质。

**扫码后一直等待确认**
酷狗二维码进度在 `data.status`，不是顶层请求 `status`：`0` 为过期、`1` 为等待扫码、`2` 为等待确认、`4` 为授权成功。Standard 入口依次映射为 `800` / `801` / `802` / `803`；只有成功取得有效 Token 和用户 ID 才返回 `803` 与 Cookie。

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
