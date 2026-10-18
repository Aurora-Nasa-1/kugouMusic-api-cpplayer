//! CPPlayer Standard v3 API 适配层
//!
//! 实现 CPPlayer-KMP Standard v3 规范：
//!  - 扁平数据模型：tracks: [ { id, title, artist, album, duration, coverUrl } ]
//!  - 播放直链模型：{ url, format, quality, headers }
//!  - 歌词模型：{ lyric }
//!  - 排行榜模型：{ list }
//!  - 搜索热词与建议：{ data: [ { searchWord, content, score } ] }
//!  - 扫码登录模型：800/801/802/803 状态码映射

use crate::modules::{Ctx, ModuleFn};
use serde_json::{json, Value};

/// 统一入口：根据 Standard v3 方法名进行分派转换
pub fn handle_api(method: &str, params: &Value) -> Value {
    let ctx = Ctx {
        ip: String::new(),
        body_bytes: None,
    };

    let normalized_method = method.trim_start_matches('/');

    match normalized_method {
        // 1. 歌曲搜索
        "search" | "cloudsearch" => handle_search(params, &ctx),

        // 2. 音频播放链接
        "song/url" | "song/url/v1" | "song/url/v1/302" | "song/download/url/v1" => {
            handle_song_url(params, &ctx)
        }

        // 3. 歌词
        "lyric" | "lyric/new" => handle_lyric(params, &ctx),

        // 4. 歌曲详情
        "song/detail" | "audio" => handle_song_detail(params, &ctx),

        // 5. 热搜
        "search/hot" | "search/hot/detail" => handle_search_hot(params, &ctx),

        // 6. 搜索建议
        "search/suggest" => handle_search_suggest(params, &ctx),

        // 7. 排行榜
        "toplist" | "toplist/detail" | "rank/list" => handle_toplist(params, &ctx),

        // 8. 扫码登录
        "login/qr/key" => handle_qr_key(params, &ctx),
        "login/qr/create" => handle_qr_create(params, &ctx),
        "login/qr/check" => handle_qr_check(params, &ctx),

        // 9. 登录状态
        "login/status" => handle_login_status(params, &ctx),

        // 10. 回退：透传匹配原生路由
        other => handle_fallback(other, params, &ctx),
    }
}

/// 转换单首酷狗歌曲为 Standard v3 规范结构
pub fn transform_kugou_song(item: &Value) -> Option<Value> {
    // 1. 提取唯一 ID（优先使用音频 Hash）
    let hash = item
        .get("FileHash")
        .or_else(|| item.get("hash"))
        .or_else(|| item.get("HQFileHash"))
        .or_else(|| item.get("SQFileHash"))
        .or_else(|| item.get("sd_hash"))
        .or_else(|| item.pointer("/audio_info/hash"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if hash.is_empty() {
        return None;
    }
    let id = hash.to_lowercase();

    // 2. 提取歌名
    let raw_name = item
        .get("SongName")
        .or_else(|| item.get("songname"))
        .or_else(|| item.get("FileName"))
        .or_else(|| item.get("filename"))
        .or_else(|| item.get("ori_audio_name"))
        .or_else(|| item.get("name"))
        .or_else(|| item.pointer("/base/audio_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // 3. 提取歌手名（支持数组 Singers 与标量 SingerName/author_name）
    let mut artist_name = String::new();
    if let Some(singers) = item
        .get("Singers")
        .or_else(|| item.get("singers"))
        .or_else(|| item.get("singerinfo"))
        .and_then(|v| v.as_array())
    {
        let names: Vec<&str> = singers
            .iter()
            .filter_map(|s| s.get("name").and_then(|n| n.as_str()))
            .filter(|n| !n.is_empty())
            .collect();
        if !names.is_empty() {
            artist_name = names.join(" / ");
        }
    }
    if artist_name.is_empty() {
        if let Some(authors) = item.get("authors").and_then(|v| v.as_array()) {
            let names: Vec<&str> = authors
                .iter()
                .filter_map(|s| s.get("author_name").and_then(|n| n.as_str()))
                .filter(|n| !n.is_empty())
                .collect();
            if !names.is_empty() {
                artist_name = names.join(" / ");
            }
        }
    }
    if artist_name.is_empty() {
        if let Some(an) = item
            .get("SingerName")
            .or_else(|| item.get("author_name"))
            .or_else(|| item.get("singername"))
            .or_else(|| item.get("artist_name"))
            .and_then(|v| v.as_str())
        {
            artist_name = an.to_string();
        }
    }

    // 处理 "歌手 - 歌名" 拼接格式
    let mut title = raw_name.to_string();
    if let Some(sep) = title.find(" - ") {
        if artist_name.is_empty() {
            artist_name = title[..sep].trim().to_string();
        }
        title = title[sep + 3..].trim().to_string();
    }

    // 4. 提取专辑名
    let album_name = item
        .get("AlbumName")
        .or_else(|| item.get("album_name"))
        .or_else(|| item.get("albumname"))
        .or_else(|| item.pointer("/album_info/album_name"))
        .or_else(|| item.pointer("/albuminfo/name"))
        .or_else(|| item.pointer("/base/album_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // 5. 提取时长（归一化为秒）
    let raw_duration = item
        .get("Duration")
        .or_else(|| item.get("duration"))
        .or_else(|| item.get("time_length"))
        .or_else(|| item.get("HQDuration"))
        .or_else(|| item.get("timelength"))
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
        })
        .unwrap_or(0);
    let duration = if raw_duration > 10000 {
        raw_duration / 1000
    } else {
        raw_duration
    };

    // 6. 提取封面 URL（酷狗占位符 {size} 替换为 400 像素）
    let raw_cover = item
        .get("Image")
        .or_else(|| item.get("ImgUrl"))
        .or_else(|| item.get("img"))
        .or_else(|| item.get("pic"))
        .or_else(|| item.get("cover"))
        .or_else(|| item.get("sizable_cover"))
        .or_else(|| item.get("album_sizable_cover"))
        .or_else(|| item.pointer("/album_info/sizable_cover"))
        .or_else(|| item.pointer("/album_info/cover"))
        .or_else(|| item.pointer("/audio_info/img"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let cover_url = if raw_cover.contains("{size}") {
        raw_cover.replace("{size}", "400")
    } else {
        raw_cover.to_string()
    };

    Some(json!({
        "id": id,
        "title": title,
        "artist": artist_name,
        "album": album_name,
        "duration": duration,
        "coverUrl": cover_url
    }))
}

fn handle_search(params: &Value, ctx: &Ctx) -> Value {
    let keyword = params
        .get("keywords")
        .or_else(|| params.get("keyword"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let page = params
        .get("page")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .or_else(|| v.as_i64())
        })
        .unwrap_or(1);

    let limit = params
        .get("limit")
        .or_else(|| params.get("pagesize"))
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .or_else(|| v.as_i64())
        })
        .unwrap_or(30);

    let mut q = json!({
        "keyword": keyword,
        "page": page,
        "pagesize": limit,
    });
    if let Some(cookie) = params.get("cookie") {
        q["cookie"] = cookie.clone();
    }

    match crate::modules::search::handle(&q, ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            let mut tracks = Vec::new();
            let mut total = 0;

            if let Some(data) = body.get("data") {
                if let Some(t) = data.get("total").and_then(|v| v.as_i64()) {
                    total = t;
                }
                let list = data
                    .get("lists")
                    .or_else(|| data.get("info"))
                    .or_else(|| data.get("songs"))
                    .and_then(|v| v.as_array());

                if let Some(arr) = list {
                    for item in arr {
                        if let Some(track) = transform_kugou_song(item) {
                            tracks.push(track);
                        }
                    }
                }
            }

            json!({
                "code": 0,
                "data": {
                    "tracks": tracks,
                    "total": total
                }
            })
        }
        Err(err_resp) => json!({
            "code": 500,
            "msg": format!("Kugou search failed with status {}", err_resp.status),
            "data": {
                "tracks": [],
                "total": 0
            }
        }),
    }
}

fn handle_song_url(params: &Value, ctx: &Ctx) -> Value {
    let id = params
        .get("id")
        .or_else(|| params.get("hash"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if id.is_empty() {
        return json!({
            "code": 500,
            "msg": "Missing id/hash parameter",
            "data": { "url": null }
        });
    }

    let level = params
        .get("level")
        .and_then(|v| v.as_str())
        .unwrap_or("standard");

    let quality = match level {
        "standard" => "128",
        "higher" | "exhigh" => "320",
        "lossless" | "hires" | "jymaster" | "sky" | "dolby" => "flac",
        other => other,
    };

    let mut q = json!({
        "hash": id,
        "quality": quality,
    });
    if let Some(cookie) = params.get("cookie") {
        q["cookie"] = cookie.clone();
    }

    match crate::modules::song_url::handle(&q, ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            let mut play_url: Option<String> = None;
            let mut format = "mp3".to_string();

            if let Some(data) = body.get("data") {
                if let Some(url_str) = data.get("url").and_then(|v| v.as_str()) {
                    if !url_str.is_empty() {
                        play_url = Some(url_str.to_string());
                    }
                } else if let Some(url_arr) = data.get("url").and_then(|v| v.as_array()) {
                    for u in url_arr {
                        if let Some(s) = u.as_str() {
                            if !s.is_empty() {
                                play_url = Some(s.to_string());
                                break;
                            }
                        }
                    }
                }

                if let Some(ext) = data
                    .get("extName")
                    .or_else(|| data.get("format"))
                    .and_then(|v| v.as_str())
                {
                    format = ext.to_string();
                } else if quality == "flac" {
                    format = "flac".to_string();
                }
            }

            if let Some(url) = play_url {
                json!({
                    "code": 0,
                    "data": {
                        "url": url,
                        "format": format,
                        "quality": level,
                        "headers": {
                            "User-Agent": crate::request::DEFAULT_UA
                        }
                    }
                })
            } else {
                json!({
                    "code": 0,
                    "data": {
                        "url": null,
                        "quality": level
                    }
                })
            }
        }
        Err(err) => json!({
            "code": 500,
            "msg": format!("Failed to get song url: status {}", err.status),
            "data": { "url": null }
        }),
    }
}

fn handle_lyric(params: &Value, ctx: &Ctx) -> Value {
    let id = params
        .get("id")
        .or_else(|| params.get("hash"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let keyword = params
        .get("keyword")
        .or_else(|| params.get("songName"))
        .or_else(|| params.get("title"))
        .or_else(|| params.get("keywords"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut q_search = json!({
        "hash": id,
        "keywords": keyword,
        "man": "yes",
    });
    if let Some(cookie) = params.get("cookie") {
        q_search["cookie"] = cookie.clone();
    }

    let mut candidate_id: Option<Value> = None;
    let mut candidate_accesskey: Option<Value> = None;

    if let Ok(search_resp) = crate::modules::search_more::handle_lyric(&q_search, ctx) {
        let body = search_resp.body.to_json();
        if let Some(candidates) = body.get("candidates").and_then(|v| v.as_array()) {
            if let Some(first) = candidates.first() {
                candidate_id = first.get("id").cloned();
                candidate_accesskey = first.get("accesskey").cloned();
            }
        }
    }

    if let (Some(cid), Some(ackey)) = (candidate_id, candidate_accesskey) {
        let q_download = json!({
            "id": cid,
            "accesskey": ackey,
            "decode": true,
            "fmt": "lrc",
        });

        if let Ok(dl_resp) = crate::modules::lyric::handle(&q_download, ctx) {
            let body = dl_resp.body.to_json();
            if let Some(lrc) = body.get("decodeContent").and_then(|v| v.as_str()) {
                return json!({
                    "code": 0,
                    "data": {
                        "lyric": lrc
                    }
                });
            }
        }
    }

    json!({
        "code": 0,
        "data": {
            "lyric": ""
        }
    })
}

fn handle_song_detail(params: &Value, ctx: &Ctx) -> Value {
    let ids = params
        .get("ids")
        .or_else(|| params.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut q = json!({
        "hash": ids,
    });
    if let Some(cookie) = params.get("cookie") {
        q["cookie"] = cookie.clone();
    }

    match crate::modules::audio::handle(&q, ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            let mut tracks = Vec::new();
            if let Some(arr) = body.get("data").and_then(|v| v.as_array()) {
                for item in arr {
                    if let Some(track) = transform_kugou_song(item) {
                        tracks.push(track);
                    }
                }
            } else if let Some(data_obj) = body.get("data").and_then(|v| v.as_object()) {
                if let Some(track) = transform_kugou_song(&Value::Object(data_obj.clone())) {
                    tracks.push(track);
                }
            }

            json!({
                "code": 0,
                "data": {
                    "tracks": tracks
                }
            })
        }
        Err(_) => json!({
            "code": 500,
            "msg": "Failed to get song detail",
            "data": {
                "tracks": []
            }
        }),
    }
}

fn handle_search_hot(params: &Value, ctx: &Ctx) -> Value {
    match crate::modules::search_more::handle_hot(params, ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            let mut items = Vec::new();
            if let Some(list) = body.pointer("/data/list").and_then(|v| v.as_array()) {
                for group in list {
                    if let Some(keywords) = group.get("keywords").and_then(|v| v.as_array()) {
                        for kw in keywords {
                            if let Some(word) = kw.get("keyword").and_then(|v| v.as_str()) {
                                items.push(json!({
                                    "searchWord": word,
                                    "content": "",
                                    "score": 10000
                                }));
                            }
                        }
                    }
                }
            }
            json!({
                "code": 0,
                "data": items
            })
        }
        Err(_) => json!({ "code": 500, "msg": "Failed to get hot search" }),
    }
}

fn handle_search_suggest(params: &Value, ctx: &Ctx) -> Value {
    match crate::modules::search_suggest::handle(params, ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            let mut suggests = Vec::new();
            if let Some(data) = body.get("data").and_then(|v| v.as_array()) {
                for item in data {
                    if let Some(rec) = item.get("RecordDatas").and_then(|v| v.as_array()) {
                        for r in rec {
                            if let Some(hint) = r.get("HintInfo").and_then(|v| v.as_str()) {
                                suggests.push(json!({ "keyword": hint }));
                            }
                        }
                    }
                }
            }
            json!({
                "code": 0,
                "result": {
                    "allMatch": suggests
                }
            })
        }
        Err(_) => json!({ "code": 500, "msg": "Failed to get suggestions" }),
    }
}

fn handle_toplist(params: &Value, ctx: &Ctx) -> Value {
    match crate::modules::rank::handle_list(params, ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            let mut list = Vec::new();
            if let Some(arr) = body.pointer("/data/info").and_then(|v| v.as_array()) {
                for item in arr {
                    let id = item
                        .get("rankid")
                        .and_then(|v| v.as_i64())
                        .map(|n| n.to_string())
                        .unwrap_or_default();
                    let name = item
                        .get("rankname")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    let raw_cover = item
                        .get("imgurl")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    let cover = if raw_cover.contains("{size}") {
                        raw_cover.replace("{size}", "400")
                    } else {
                        raw_cover.to_string()
                    };
                    list.push(json!({
                        "id": id,
                        "name": name,
                        "coverUrl": cover
                    }));
                }
            }
            json!({
                "code": 0,
                "data": {
                    "list": list
                }
            })
        }
        Err(_) => json!({ "code": 500, "msg": "Failed to get toplist" }),
    }
}

fn handle_qr_key(params: &Value, ctx: &Ctx) -> Value {
    match crate::modules::login::handle_qr_key(params, ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            let key = body
                .pointer("/data/qrcode")
                .or_else(|| body.pointer("/data/key"))
                .cloned()
                .unwrap_or(Value::String("".into()));
            json!({
                "code": 200,
                "data": {
                    "unikey": key
                }
            })
        }
        Err(_) => json!({ "code": 500, "msg": "Failed to get qr key" }),
    }
}

fn handle_qr_create(params: &Value, ctx: &Ctx) -> Value {
    match crate::modules::login::handle_qr_create(params, ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            json!({
                "code": 200,
                "data": body.get("data").cloned().unwrap_or(json!({}))
            })
        }
        Err(_) => json!({ "code": 500, "msg": "Failed to create qr" }),
    }
}

fn handle_qr_check(params: &Value, ctx: &Ctx) -> Value {
    match crate::modules::login::handle_qr_check(params, ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            let status = body.get("status").and_then(|v| v.as_i64()).unwrap_or(0);
            let cookie_str = resp.cookie.join("; ");
            if status == 2 || status == 4 {
                json!({
                    "code": 803,
                    "cookie": cookie_str,
                    "message": "登录成功"
                })
            } else if status == 1 {
                json!({
                    "code": 802,
                    "message": "等待确认"
                })
            } else if status == 3 {
                json!({
                    "code": 800,
                    "message": "二维码已过期"
                })
            } else {
                json!({
                    "code": 801,
                    "message": "等待扫码"
                })
            }
        }
        Err(_) => json!({ "code": 500, "msg": "Failed to check qr" }),
    }
}

fn handle_login_status(params: &Value, ctx: &Ctx) -> Value {
    match crate::modules::user::handle_vip_detail(params, ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            json!({
                "code": 200,
                "data": body
            })
        }
        Err(_) => json!({
            "code": 200,
            "data": null
        }),
    }
}

fn handle_fallback(method: &str, params: &Value, ctx: &Ctx) -> Value {
    let path = if method.starts_with('/') {
        method.to_string()
    } else {
        format!("/{}", method)
    };

    let mut routes: Vec<(&'static str, ModuleFn)> = Vec::new();
    crate::modules::register(&mut routes);

    for (route, module_fn) in routes {
        if *route == path {
            return match module_fn(params, ctx) {
                Ok(resp) => resp.body.to_json(),
                Err(err) => json!({
                    "code": err.status,
                    "msg": format!("Request failed with status {}", err.status)
                }),
            };
        }
    }

    json!({
        "code": -1,
        "msg": format!("Unsupported method: {}", method)
    })
}
