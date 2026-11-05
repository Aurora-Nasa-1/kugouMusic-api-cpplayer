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

mod discovery;
mod library;

// Positive, JavaScript-safe IDs keep rank IDs separate from public and account-local playlists.
const RANK_PLAYLIST_BASE: i64 = 1 << 52;
const RANK_PLAYLIST_SPAN: i64 = 1 << 32;

/// 统一入口：根据 Standard v3 方法名进行分派转换
pub fn handle_api(method: &str, params: &Value) -> Value {
    let params = normalize_standard_params(params);
    let params = &params;
    let ctx = Ctx {
        ip: String::new(),
        body_bytes: None,
    };

    let normalized_method = method.trim_start_matches('/');

    match normalized_method {
        "provider/info" => json!({
            "code": 0,
            "data": {
                "id": "kugou-music-rs",
                "version": env!("CARGO_PKG_VERSION"),
                "contract": "standard",
                "apiVersion": 3
            }
        }),
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
        "toplist" | "toplist/detail" | "toplist_detail" | "rank/list" => {
            handle_toplist(params, &ctx)
        }

        // 8. 扫码登录
        "login/qr/key" => handle_qr_key(params, &ctx),
        "login/qr/create" => handle_qr_create(params, &ctx),
        "login/qr/check" => handle_qr_check(params, &ctx),

        // 9. 登录状态
        "login/status" => handle_login_status(params, &ctx),

        // 10. 歌手
        "artist/detail" => handle_artist_detail(params, &ctx),
        "artist/songs" | "artist/top/song" | "artist/audios" => handle_artist_songs(params, &ctx),
        "artist/album" | "artist/albums" => handle_artist_albums(params, &ctx),

        // 11. 专辑
        "album" | "album/detail" => handle_album_detail(params, &ctx),

        // 12. 歌单
        "user/playlist" => handle_user_playlist(params, &ctx),
        "playlist/detail" => handle_playlist_detail(params, &ctx),
        "playlist/track/all" => handle_playlist_tracks(params, &ctx),
        "playlist/create" => handle_playlist_create(params, &ctx),
        "user/playlist/create" => handle_user_playlist_kind(params, &ctx, false),
        "user/playlist/collect" => handle_user_playlist_kind(params, &ctx, true),
        "like" | "likelist" | "playlist/tracks" | "playlist/delete" | "playlist/subscribe" => {
            library::handle(normalized_method, params, &ctx)
        }

        // 13. 每日推荐
        "recommend/songs" => handle_recommend_songs(params, &ctx),
        "recommend/resource" => handle_recommend_resource(params, &ctx),
        "personalized" | "top_playlist" | "top/playlist" => discovery::playlists(params, &ctx, false),
        "top_playlist_highquality" | "top/playlist/highquality" => discovery::playlists(params, &ctx, true),
        "top_album" | "top/album" | "album/new" | "album/newest" => discovery::albums(params, &ctx),
        "top_artists" | "top/artists" => discovery::artists(params, &ctx),
        "top_song" | "top/song" | "personalized_newsong" => discovery::new_songs(params, &ctx),
        "personal_fm" | "personal/fm" => discovery::personal_fm(params, &ctx),
        "banner" => discovery::banners(params, &ctx),
        "simi_song" | "simi/song" => discovery::similar_songs(params, &ctx),
        "playmode/intelligence/list" => discovery::intelligence(params, &ctx),

        // 14. 评论
        "comment/music" | "comment/new" => handle_comments(params, &ctx),
        "comment/floor" => handle_comment_floor(params, &ctx),
        "comment" => handle_comment_create(params, &ctx),

        // 15. 回退：透传匹配原生路由
        other => handle_fallback(other, params, &ctx),
    }
}

fn normalize_standard_params(params: &Value) -> Value {
    let mut normalized = params.clone();
    if let Some(cookie) = params.get("cookie").and_then(Value::as_str) {
        let mut values = serde_json::Map::new();
        for part in cookie.split(';') {
            if let Some((name, value)) = part.trim().split_once('=') {
                let name = name.trim();
                if !name.is_empty() {
                    values.insert(name.to_string(), json!(value.trim()));
                }
            }
        }
        normalized["cookie"] = Value::Object(values);
    }
    normalized
}

/// 转换单首酷狗歌曲为 Standard v3 规范结构
pub fn transform_kugou_song(item: &Value) -> Option<Value> {
    // 1. 提取唯一 ID（优先使用音频 Hash；详情接口有时只给 numeric id）
    let hash = first_string(item, &[
        "/FileHash", "/hash", "/hash_128", "/HQFileHash", "/SQFileHash", "/sd_hash",
        "/audio_info/hash", "/audio_info/hash_128", "/deprecated/hash",
        "/audio_id", "/album_audio_id", "/song_id", "/id",
    ]);

    if hash.is_empty() {
        return None;
    }
    let id = hash.to_lowercase();

    // 2. 提取歌名
    let raw_name = item
        .get("SongName")
        .or_else(|| item.get("audio_name"))
        .or_else(|| item.get("songname"))
        .or_else(|| item.get("FileName"))
        .or_else(|| item.get("filename"))
        .or_else(|| item.get("ori_audio_name"))
        .or_else(|| item.get("name"))
        .or_else(|| item.pointer("/base/audio_name"))
        .or_else(|| item.pointer("/base/songname"))
        .and_then(value_to_string)
        .unwrap_or_default();

    // 3. 提取歌手名（支持数组 Singers 与标量 SingerName/author_name）
    let mut artist_name = String::new();
    if let Some(singers) = item
        .get("Singers")
        .or_else(|| item.get("singers"))
        .or_else(|| item.get("singerinfo"))
        .and_then(|v| v.as_array())
    {
        let names: Vec<String> = singers
            .iter()
            .filter_map(|s| s.get("name").and_then(value_to_string))
            .filter(|n| !n.is_empty())
            .collect();
        if !names.is_empty() {
            artist_name = names.join(" / ");
        }
    }
    if artist_name.is_empty() {
        artist_name = first_string(item, &["/base/author_name"]);
    }
    if artist_name.is_empty() {
        if let Some(authors) = item.get("authors").and_then(|v| v.as_array()) {
            let names: Vec<String> = authors
                .iter()
                .filter_map(|s| s.get("author_name").and_then(value_to_string))
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
            .and_then(value_to_string)
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
        .and_then(value_to_string)
        .unwrap_or_default();

    // 5. 提取时长（归一化为秒）
    let raw_duration = item
        .get("Duration")
        .or_else(|| item.get("duration"))
        .or_else(|| item.get("time_length"))
        .or_else(|| item.get("HQDuration"))
        .or_else(|| item.get("timelength"))
        .or_else(|| item.pointer("/audio_info/duration_128"))
        .or_else(|| item.pointer("/audio_info/duration"))
        .or_else(|| item.pointer("/audio_info/timelength"))
        .or_else(|| item.pointer("/deprecated/duration"))
        .and_then(value_to_i64);
    let duration = raw_duration
        .map(|duration| if duration >= 10000 { duration / 1000 } else { duration })
        .unwrap_or_else(|| first_i64(item, &["/timelen"]) / 1000);

    // 6. 提取封面 URL（酷狗占位符 {size} 替换为 400 像素）
    let cover_url = standard_cover(item, &[
        "/Image", "/ImgUrl", "/img", "/pic", "/cover", "/coverUrl",
        "/sizable_cover", "/album_sizable_cover", "/album_info/sizable_cover",
        "/album_info/cover", "/albuminfo/sizable_cover", "/audio_info/img",
        "/trans_param/union_cover", "/info/image", "/base/sizable_cover",
    ]);

    Some(json!({
        "id": id,
        "title": title,
        "artist": artist_name,
        "album": album_name,
        "duration": duration,
        "coverUrl": cover_url
    }))
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn value_to_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

fn first_array<'a>(root: &'a Value, paths: &[&str]) -> Option<&'a Vec<Value>> {
    paths
        .iter()
        .filter_map(|path| root.pointer(path))
        .find_map(Value::as_array)
}

fn first_string(root: &Value, paths: &[&str]) -> String {
    paths
        .iter()
        .filter_map(|path| root.pointer(path).and_then(value_to_string))
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

fn standard_cover(root: &Value, paths: &[&str]) -> String {
    paths.iter()
        .filter_map(|path| root.pointer(path).and_then(value_to_string))
        .find_map(|raw| {
            let raw = raw.trim();
            let url = if raw.starts_with("//") { format!("https:{raw}") } else { raw.to_string() };
            if !url.starts_with("https://") && !url.starts_with("http://") {
                return None;
            }
            let url = url.replace("{size}", "400")
                .replace("%7Bsize%7D", "400").replace("%7bsize%7d", "400");
            if let Some(rest) = url.strip_prefix("http://") {
                let host = rest.split('/').next().unwrap_or("").to_ascii_lowercase();
                if host == "kgimg.com" || host.ends_with(".kgimg.com")
                    || host == "kugou.com" || host.ends_with(".kugou.com") {
                    return Some(format!("https://{rest}"));
                }
            }
            Some(url)
        })
        .unwrap_or_default()
}

fn first_i64(root: &Value, paths: &[&str]) -> i64 {
    paths
        .iter()
        .filter_map(|path| root.pointer(path).and_then(value_to_i64))
        .next()
        .unwrap_or(0)
}

fn with_page_params(params: &Value) -> Value {
    let mut query = params.clone();
    if query.get("pagesize").is_none() {
        if let Some(limit) = params.get("limit") {
            query["pagesize"] = limit.clone();
        }
    }
    if query.get("page").is_none() {
        let limit = params.get("limit").and_then(value_to_i64).unwrap_or(30).max(1);
        let offset = params.get("offset").and_then(value_to_i64).unwrap_or(0).max(0);
        query["page"] = json!((offset / limit) + 1);
    }
    query
}

fn transform_kugou_playlist(item: &Value) -> Option<Value> {
    let id = first_i64(
        item,
        &[
            "/listid",
            "/global_collection_id",
            "/special_id",
            "/specialid",
            "/id",
        ],
    );
    let title = first_string(item, &["/name", "/title", "/listname", "/specialname"])
        .replace("<em>", "")
        .replace("</em>", "");
    if id <= 0 || title.is_empty() {
        return None;
    }
    let cover = standard_cover(item, &[
        "/flexible_cover",
        "/pic",
        "/coverImgUrl",
        "/coverUrl",
        "/cover",
        "/img",
        "/sizable_cover",
        "/imgurl",
        "/trans_param/union_cover",
        "/picUrl",
        "/first_audio_cover",
        "/first_song_cover",
    ]);
    let total = first_i64(item, &["/trackCount", "/count", "/total", "/song_count", "/songcount"]);
    let author = first_string(item, &["/author", "/creator/nickname", "/creator/name", "/username", "/nickname", "/list_create_username"]);
    let userid = first_i64(item, &["/userid", "/uid", "/creator/userId", "/creator/userid", "/creator/id", "/create_userid"]);
    let subscribed = item.get("subscribed").and_then(Value::as_bool).unwrap_or(false);
    Some(json!({
        "id": id,
        "name": title.clone(),
        "title": title,
        "coverImgUrl": cover.clone(),
        "coverUrl": cover.clone(),
        "picUrl": cover,
        "total": total,
        "trackCount": total,
        "author": author.clone(),
        "creator": {
            "nickname": author,
            "userId": userid
        },
        "subscribed": subscribed
    }))
}

fn transform_kugou_artist(item: &Value) -> Option<Value> {
    let id = first_i64(item, &["/author_id", "/singerid", "/artist_id", "/id"]);
    let name = first_string(item, &["/author_name", "/singername", "/artist_name", "/name", "/title"])
        .replace("<em>", "").replace("</em>", "");
    if id <= 0 || name.is_empty() {
        return None;
    }
    let avatar = standard_cover(item, &["/avatar", "/sizable_avatar", "/imgurl", "/pic", "/img", "/cover"]);
    let fans = first_i64(item, &["/fans", "/fans_count", "/fanscount", "/fansnums", "/followeds"]);
    Some(json!({
        "id": id,
        "name": name,
        "avatarUrl": avatar,
        "fans": fans
    }))
}

fn transform_kugou_album(item: &Value) -> Option<Value> {
    let id = first_i64(item, &["/album_id", "/albumid", "/id"]);
    let name = first_string(item, &["/album_name", "/albumname", "/name", "/title"])
        .replace("<em>", "").replace("</em>", "");
    if id <= 0 || name.is_empty() {
        return None;
    }
    let cover = standard_cover(item, &["/sizable_cover", "/coverUrl", "/cover", "/pic", "/imgurl"]);
    let artist = first_string(item, &["/author_name", "/singername", "/artist", "/artist_name"])
        .replace("<em>", "").replace("</em>", "");
    let publish_date = first_i64(item, &["/publish_date", "/publish_time", "/publishDate"]);
    Some(json!({
        "id": id,
        "name": name,
        "coverUrl": cover,
        "artist": artist,
        "publishDate": publish_date
    }))
}

fn search_query(params: &Value) -> Value {
    let keyword = params
        .get("keywords")
        .or_else(|| params.get("keyword"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let limit = params
        .get("limit")
        .or_else(|| params.get("pagesize"))
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .or_else(|| v.as_i64())
        })
        .unwrap_or(30)
        .clamp(1, 100);

    let offset = params
        .get("offset")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .or_else(|| v.as_i64())
        });

    let page = if let Some(off) = offset {
        (off.max(0) / limit) + 1
    } else {
        params
            .get("page")
            .and_then(|v| {
                v.as_str()
                    .and_then(|s| s.parse::<i64>().ok())
                    .or_else(|| v.as_i64())
            })
            .unwrap_or(1)
            .max(1)
    };

    let mut q = json!({
        "keyword": keyword,
        "page": page,
        "pagesize": limit,
    });
    if let Some(search_type) = params.get("type") {
        q["type"] = search_type.clone();
    }
    if let Some(cookie) = params.get("cookie") {
        q["cookie"] = cookie.clone();
    }
    q
}

fn handle_search(params: &Value, ctx: &Ctx) -> Value {
    let q = search_query(params);
    let limit = first_i64(&q, &["/pagesize"]);
    let requested_type = params.get("type").and_then(value_to_i64).unwrap_or(1);
    let upstream = match requested_type {
        1000 => crate::modules::search_more::handle_special(&q, ctx),
        100 => crate::modules::search_more::handle_artist(&q, ctx),
        10 => crate::modules::search_more::handle_album(&q, ctx),
        _ => crate::modules::search::handle(&q, ctx),
    };

    match upstream {
        Ok(resp) => {
            let body = resp.body.to_json();
            let search_type = requested_type;
            let data = body.get("data").unwrap_or(&body);
            let list = first_array(
                &body,
                &[
                    "/data/lists",
                    "/data/info",
                    "/data/songs",
                    "/data/albums",
                    "/data/artists",
                    "/data/playlists",
                    "/lists",
                    "/info",
                    "/songs",
                ],
            )
            .or_else(|| data.as_array());
            let total = first_i64(
                &body,
                &[
                    "/data/total",
                    "/data/songcount",
                    "/data/albumcount",
                    "/data/singer_count",
                    "/data/specialcount",
                    "/total",
                ],
            );

            match search_type {
                1000 => {
                    let playlists = list
                        .into_iter()
                        .flatten()
                        .filter_map(transform_kugou_playlist)
                        .collect::<Vec<_>>();
                    json!({
                        "code": 0,
                        "data": { "playlists": playlists.clone(), "total": total },
                        "result": { "playlists": playlists, "playlistCount": total }
                    })
                }
                100 => {
                    let mut artists = list
                        .into_iter()
                        .flatten()
                        .filter_map(transform_kugou_artist)
                        .take(limit as usize)
                        .collect::<Vec<_>>();
                    enrich_artist_artwork_with(&mut artists, &q, |query| {
                        crate::modules::artist::handle_detail_with_timeout(query, ctx, 2)
                            .ok().map(|response| response.body.to_json())
                    });
                    json!({ "code": 0, "data": { "artists": artists, "total": total } })
                }
                10 => {
                    let albums = list
                        .into_iter()
                        .flatten()
                        .filter_map(transform_kugou_album)
                        .collect::<Vec<_>>();
                    json!({ "code": 0, "data": { "albums": albums, "total": total } })
                }
                _ => {
                    let tracks = list
                        .into_iter()
                        .flatten()
                        .filter_map(transform_kugou_song)
                        .collect::<Vec<_>>();
                    json!({ "code": 0, "data": { "tracks": tracks, "total": total } })
                }
            }
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
    crate::device::ensure_registered();

    let level = params
        .get("level")
        .and_then(|v| v.as_str())
        .unwrap_or("standard");

    let quality = match level {
        "standard" => "128",
        "higher" | "exhigh" => "320",
        "lossless" | "jymaster" | "sky" | "dolby" => "flac",
        "hires" => "high",
        other => other,
    };

    // Keep identity and album context instead of silently dropping parameters
    // such as album_audio_id, dfid, and the normalized login cookie.
    let mut q = params.clone();
    q["hash"] = json!(id);
    q["quality"] = json!(quality);

    match crate::modules::song_url::handle(&q, ctx) {
        Ok(resp) => song_url_response(&resp.body.to_json(), level, quality),
        Err(err) => {
            let mut response = song_url_response(&err.body.to_json(), level, quality);
            response["upstreamStatus"] = json!(err.status);
            response
        }
    }
}

fn song_url_response(body: &Value, level: &str, requested_quality: &str) -> Value {
    let info = pick_play_info(body);
    if let Some(url) = info.and_then(extract_url_field) {
        let info = info.unwrap();
        let quality = info
            .get("quality")
            .and_then(Value::as_str)
            .unwrap_or(requested_quality);
        let actual_level = match quality {
            "128" => "standard",
            "320" => "exhigh",
            "flac" => "lossless",
            "high" => "hires",
            other => other,
        };
        let format = info
            .get("extName")
            .or_else(|| info.get("format"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(if matches!(quality, "flac" | "high") {
                "flac"
            } else {
                "mp3"
            });
        let mut data = json!({
            "url": url,
            "format": format,
            "quality": actual_level,
            "requestedQuality": level,
            "headers": {"User-Agent": crate::request::DEFAULT_UA}
        });
        if let Some(size) = info
            .get("fileSize")
            .or_else(|| info.get("filesize"))
            .or_else(|| info.get("size"))
            .and_then(value_to_i64)
        {
            data["size"] = json!(size);
        }
        return json!({"code": 0, "data": data});
    }

    // Denials often contain only fail_process/priv_status, with no URL or
    // format keys. Inspect those objects even though none is playable.
    let candidates = play_info_candidates(body);
    let failures: Vec<String> = candidates
        .iter()
        .filter_map(|info| info.get("fail_process"))
        .flat_map(|failure| match failure {
            Value::Array(values) => values.iter().filter_map(value_to_string).collect(),
            _ => value_to_string(failure).into_iter().collect::<Vec<_>>(),
        })
        .filter(|reason| !reason.trim().is_empty())
        .collect();
    let denied = failures.iter().any(|reason| {
        reason
            .split(',')
            .any(|part| matches!(part.trim(), "pkg" | "buy"))
    }) || candidates.iter().any(|info| {
        info.get("priv_status").and_then(value_to_i64) == Some(0)
    });
    let reason = if !failures.is_empty() {
        format!("upstream fail_process: {}", failures.join(","))
    } else {
        candidates
            .iter()
            .find_map(|info| {
                ["error_msg", "error", "msg", "message"]
                    .iter()
                    .find_map(|key| info.get(*key).and_then(value_to_string))
            })
            .unwrap_or_else(|| {
                if denied {
                    "upstream denied playback (priv_status=0)".to_string()
                } else {
                    "upstream returned no playable HTTP URL".to_string()
                }
            })
    };
    let mut response = json!({
        "code": if denied { 403 } else { 502 },
        "msg": format!("no playable url ({reason})"),
        "data": {"url": null, "quality": level, "reason": reason}
    });
    if let Some(code) = candidates.iter().find_map(|info| {
        info.get("error_code")
            .or_else(|| info.get("errcode"))
            .and_then(value_to_i64)
    }) {
        response["data"]["upstreamErrorCode"] = json!(code);
    }
    response
}

fn play_info_candidates(body: &Value) -> Vec<&Value> {
    let mut candidates = vec![body];
    if let Some(items) = body.as_array() {
        candidates.extend(items);
    }
    if let Some(data) = body.get("data") {
        candidates.push(data);
        if let Some(items) = data.as_array() {
            candidates.extend(items);
        }
    }
    candidates
}

fn pick_play_info(body: &Value) -> Option<&Value> {
    play_info_candidates(body)
        .into_iter()
        .find(|info| extract_url_field(info).is_some())
}

/// `url` / `backupUrl` 既可能是字符串，也可能是字符串数组。
fn extract_url_field(info: &Value) -> Option<String> {
    fn http_url(value: &Value) -> Option<String> {
        let value = value.as_str()?.trim();
        if value.starts_with("http://") || value.starts_with("https://") {
            Some(value.to_string())
        } else {
            None
        }
    }
    for key in ["url", "backupUrl", "backup_url"] {
        match info.get(key) {
            Some(value @ Value::String(_)) => {
                if let Some(url) = http_url(value) {
                    return Some(url);
                }
            }
            Some(Value::Array(arr)) => {
                if let Some(url) = arr.iter().find_map(http_url) {
                    return Some(url);
                }
            }
            _ => {}
        }
    }
    None
}

fn handle_lyric(params: &Value, ctx: &Ctx) -> Value {
    crate::device::ensure_registered();
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
    crate::device::ensure_registered();
    let ids = params
        .get("ids")
        .or_else(|| params.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // /v1/audio/audio 从 cookie 里取 dfid / KUGOU_API_MID 参与签名；JNI 宿主
    // 不注入 cookie，必须补上设备身份，否则上游恒定 `20006 sign error`。
    let mut cookie = params.get("cookie").cloned().unwrap_or_else(|| json!({}));
    crate::device::merge_device_identity(&mut cookie);

    let q = json!({
        "hash": ids,
        "cookie": cookie,
    });

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
            enrich_song_artwork_with(&mut tracks, &q, |query| {
                crate::modules::images::handle_images(query, ctx)
                    .ok().map(|response| response.body.to_json())
            });

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

fn enrich_song_artwork_with(
    tracks: &mut [Value],
    params: &Value,
    fetch: impl FnOnce(&Value) -> Option<Value>,
) {
    let missing: Vec<usize> = tracks.iter().enumerate()
        .filter(|(_, track)| first_string(track, &["/coverUrl"]).is_empty())
        .filter(|(_, track)| {
            let id = first_string(track, &["/id"]);
            id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .map(|(index, _)| index)
        .collect();
    if missing.is_empty() {
        return;
    }
    let mut query = params.clone();
    query["hash"] = json!(missing.iter()
        .map(|&index| first_string(&tracks[index], &["/id"]))
        .collect::<Vec<_>>().join(","));
    let Some(body) = fetch(&query) else { return };
    if first_i64(&body, &["/status"]) != 1
        || first_i64(&body, &["/error_code", "/errcode"]) != 0 {
        return;
    }
    let Some(images) = body.get("data").and_then(Value::as_array) else { return };
    // This API aligns images with request hashes; an incomplete batch cannot be matched safely.
    if images.len() != missing.len() {
        return;
    }
    for (&index, image) in missing.iter().zip(images) {
        let albums = image.get("album").and_then(Value::as_array);
        let authors = image.get("author").and_then(Value::as_array);
        let cover = albums.into_iter().flatten()
            .map(|album| standard_cover(album, &["/sizable_cover", "/cover", "/pic"]))
            .chain(authors.into_iter().flatten()
                .map(|author| standard_cover(author, &["/sizable_avatar", "/avatar"])))
            .find(|cover| !cover.is_empty());
        if let Some(cover) = cover {
            tracks[index]["coverUrl"] = json!(cover);
        }
        if first_string(&tracks[index], &["/album"]).is_empty() {
            if let Some(album) = albums.into_iter().flatten()
                .map(|album| first_string(album, &["/album_name"]))
                .find(|name| !name.is_empty()) {
                tracks[index]["album"] = json!(album);
            }
        }
        if first_string(&tracks[index], &["/artist"]).is_empty() {
            let names: Vec<_> = authors.into_iter().flatten()
                .map(|author| first_string(author, &["/author_name"]))
                .filter(|name| !name.is_empty()).collect();
            if !names.is_empty() {
                tracks[index]["artist"] = json!(names.join(" / "));
            }
        }
    }
}

fn enrich_artist_artwork_with(
    artists: &mut [Value],
    params: &Value,
    fetch: impl Fn(&Value) -> Option<Value> + Sync,
) {
    let missing: Vec<usize> = artists.iter().enumerate()
        .filter(|(_, artist)| first_string(artist, &["/avatarUrl"]).is_empty())
        .map(|(index, _)| index).collect();
    // Optional portraits use at most four workers and stop scheduling after four seconds.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
    let results = std::thread::scope(|scope| {
        let artists = &*artists;
        let handles: Vec<_> = missing.chunks(missing.len().div_ceil(4).max(1))
            .map(|indices| {
                let fetch = &fetch;
                scope.spawn(move || {
                    let mut results = Vec::new();
                    for &index in indices {
                        if std::time::Instant::now() >= deadline {
                            break;
                        }
                        let mut query = params.clone();
                        query["id"] = artists[index]["id"].clone();
                        if let Some(body) = fetch(&query) {
                            if first_i64(&body, &["/status"]) != 1 {
                                continue;
                            }
                            if let Some(detail) = transform_kugou_artist(&body["data"]) {
                                if detail["id"] == artists[index]["id"] {
                                    results.push((index, detail));
                                }
                            }
                        }
                    }
                    results
                })
            }).collect();
        handles.into_iter().filter_map(|handle| handle.join().ok())
            .flatten().collect::<Vec<_>>()
    });
    for (index, detail) in results {
        artists[index]["avatarUrl"] = detail["avatarUrl"].clone();
        if first_i64(&artists[index], &["/fans"]) == 0 {
            artists[index]["fans"] = detail["fans"].clone();
        }
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
        Ok(resp) => toplist_response(&resp.body.to_json()),
        Err(resp) => playlist_request_error(&resp, "get rankings"),
    }
}

fn toplist_response(body: &Value) -> Value {
    let Some(items) = first_array(body, &["/data/info", "/data/list", "/list"]) else {
        return json!({"code": 502, "msg": "Upstream rankings contain no list"});
    };
    let list: Vec<_> = items.iter().filter_map(|item| {
        let rank_id = first_i64(item, &["/rankid", "/rank_id"]);
        let name = first_string(item, &["/rankname", "/name"]);
        if !(1..RANK_PLAYLIST_SPAN).contains(&rank_id) || name.is_empty() {
            return None;
        }
        Some(json!({
            "id": RANK_PLAYLIST_BASE + rank_id,
            "rankId": rank_id,
            "name": name,
            "coverUrl": standard_cover(item, &["/imgurl", "/img_cover", "/cover", "/pic"]),
            "updateFrequency": first_string(item, &["/update_frequency"]),
            "trackCount": first_i64(item, &["/songcount", "/count", "/total"]),
        }))
    }).collect();
    json!({"code": 0, "list": list, "data": {"list": list}})
}

fn rank_id_from_playlist_params(params: &Value) -> Option<i64> {
    let id = first_string(params, &["/id", "/ids", "/listid"]).parse::<i64>().ok()?;
    let rank_id = id.checked_sub(RANK_PLAYLIST_BASE)?;
    (1..RANK_PLAYLIST_SPAN).contains(&rank_id).then_some(rank_id)
}

fn rank_playlist_query(params: &Value, rank_id: i64) -> Value {
    let mut query = playlist_query(params).expect("encoded rank IDs are valid playlist IDs");
    let limit = first_i64(&query, &["/pagesize"]).clamp(1, 500);
    let offset = first_i64(&query, &["/offset"]);
    query["pagesize"] = json!(limit);
    query["page"] = json!(offset / limit + 1);
    query["rankid"] = json!(rank_id);
    query
}

fn handle_rank_playlist(params: &Value, ctx: &Ctx, rank_id: i64, detail: bool) -> Value {
    let query = rank_playlist_query(params, rank_id);
    let metadata = if detail {
        match crate::modules::rank::handle_info(&query, ctx) {
            Ok(response) => response.body.to_json()["data"].clone(),
            Err(response) => return playlist_request_error(&response, "get ranking detail"),
        }
    } else {
        json!({})
    };
    let resolved = ResolvedPlaylist { query, metadata, own: true };
    let body = match fetch_playlist_tracks_with(&resolved, |query| {
        crate::modules::rank::handle_audio(query, ctx)
            .map(|response| response.body.to_json())
            .map_err(|response| playlist_request_error(&response, "get ranking tracks"))
    }) {
        Ok(body) => body,
        Err(error) => return error,
    };
    let mut response = if detail {
        playlist_detail_response(&body, &resolved)
    } else {
        playlist_tracks_response(&body, &resolved.query)
    };
    response["data"]["rankId"] = json!(rank_id);
    response["data"]["readOnly"] = json!(true);
    response
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

fn extract_userid_from_sources(params: &Value, body: &Value) -> Option<i64> {
    for path in &[
        "/data/userid",
        "/data/userId",
        "/data/user_id",
        "/userid",
        "/userId",
        "/user_id",
    ] {
        if let Some(uid) = body.pointer(path) {
            if let Some(n) = uid
                .as_i64()
                .or_else(|| uid.as_str().and_then(|s| s.parse::<i64>().ok()))
            {
                if n > 0 {
                    return Some(n);
                }
            }
        }
    }
    for key in &["userid", "userId", "uid"] {
        if let Some(uid) = params.get(*key) {
            if let Some(n) = uid
                .as_i64()
                .or_else(|| uid.as_str().and_then(|s| s.parse::<i64>().ok()))
            {
                if n > 0 {
                    return Some(n);
                }
            }
        }
    }
    if let Some(cookie_val) = params.get("cookie") {
        if let Some(obj) = cookie_val.as_object() {
            for key in &["userid", "userId", "uid"] {
                if let Some(uid) = obj.get(*key) {
                    if let Some(n) = uid
                        .as_i64()
                        .or_else(|| uid.as_str().and_then(|s| s.parse::<i64>().ok()))
                    {
                        if n > 0 {
                            return Some(n);
                        }
                    }
                }
            }
        } else if let Some(cookie_str) = cookie_val.as_str() {
            for part in cookie_str.split(';') {
                let mut kv = part.trim().splitn(2, '=');
                if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
                    let k = k.trim().to_lowercase();
                    if k == "userid" || k == "uid" {
                        if let Ok(n) = v.trim().parse::<i64>() {
                            if n > 0 {
                                return Some(n);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn handle_qr_check(params: &Value, ctx: &Ctx) -> Value {
    match crate::modules::login::handle_qr_check(params, ctx) {
        Ok(resp) => qr_check_response(&resp.body.to_json(), &resp.cookie),
        Err(_) => json!({ "code": 500, "msg": "Failed to check qr" }),
    }
}

fn qr_check_response(body: &Value, cookies: &[String]) -> Value {
    // Outer status describes the request; data.status describes the QR flow.
    // Kugou: 0=expired, 1=waiting for scan, 2=waiting for confirmation, 4=authorized.
    let status = body
        .pointer("/data/status")
        .or_else(|| body.get("status"))
        .and_then(value_to_i64);
    match status {
        Some(4) => {
            let mut params = normalize_standard_params(&json!({"cookie": cookies.join("; ")}));
            let cookie = params["cookie"].as_object_mut().unwrap();
            for key in ["token", "userid", "vip_token", "vip_type"] {
                if let Some(value) = body
                    .get("data")
                    .and_then(|data| data.get(key))
                    .or_else(|| body.get(key))
                    .and_then(value_to_string)
                    .filter(|value| !matches!(value.trim(), "" | "null" | "undefined"))
                {
                    cookie.insert(key.to_string(), json!(value));
                }
            }
            let has_token = cookie
                .get("token")
                .and_then(Value::as_str)
                .is_some_and(|token| !matches!(token.trim(), "" | "null" | "undefined"));
            let has_userid = cookie
                .get("userid")
                .and_then(value_to_i64)
                .is_some_and(|userid| userid > 0);
            if !has_token || !has_userid {
                return json!({
                    "code": 502,
                    "msg": "QR authorized but login credentials are missing"
                });
            }
            let cookie = cookie
                .iter()
                .filter_map(|(name, value)| {
                    value_to_string(value).map(|value| format!("{name}={value}"))
                })
                .collect::<Vec<_>>()
                .join("; ");
            json!({"code": 803, "cookie": cookie, "message": "登录成功"})
        }
        Some(2) => json!({"code": 802, "message": "等待确认"}),
        Some(1) => json!({"code": 801, "message": "等待扫码"}),
        Some(0 | 3) => json!({"code": 800, "message": "二维码已过期"}),
        _ => json!({"code": 500, "msg": "Missing or unknown QR login status"}),
    }
}

fn handle_login_status(params: &Value, ctx: &Ctx) -> Value {
    let detail_res = crate::modules::user::handle_detail(params, ctx)
        .or_else(|_| crate::modules::user::handle_vip_detail(params, ctx));

    let body = match detail_res {
        Ok(resp) => resp.body.to_json(),
        Err(_) => json!({}),
    };

    let uid = extract_userid_from_sources(params, &body);
    if let Some(id) = uid {
        let nickname = body
            .pointer("/data/nickname")
            .or_else(|| body.pointer("/data/user_name"))
            .or_else(|| body.pointer("/data/username"))
            .or_else(|| body.get("nickname"))
            .and_then(|v| v.as_str())
            .unwrap_or("Kugou User");
        let avatar = body
            .pointer("/data/pic")
            .or_else(|| body.pointer("/data/avatar"))
            .or_else(|| body.get("pic"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        json!({
            "code": 200,
            "data": {
                "code": 200,
                "account": {
                    "id": id
                },
                "profile": {
                    "userId": id,
                    "nickname": nickname,
                    "avatarUrl": avatar
                },
                "raw": body
            }
        })
    } else {
        json!({
            "code": 200,
            "data": {
                "code": 200,
                "account": null,
                "profile": null
            }
        })
    }
}

fn handle_artist_detail(params: &Value, ctx: &Ctx) -> Value {
    match crate::modules::artist::handle_detail(params, ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            let data = body.get("data").unwrap_or(&body);
            let id = data
                .get("author_id")
                .or_else(|| data.get("id"))
                .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok())))
                .or_else(|| params.get("id").and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))))
                .unwrap_or(0);
            let name = data
                .get("author_name")
                .or_else(|| data.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let avatar = standard_cover(data, &["/avatar", "/sizable_avatar", "/pic"]);
            let desc = data
                .get("intro")
                .or_else(|| data.get("profile"))
                .or_else(|| data.get("desc"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let fans = data
                .get("fans_count")
                .or_else(|| data.get("fans"))
                .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok())))
                .unwrap_or(0);

            json!({
                "code": 0,
                "data": {
                    "id": id,
                    "name": name,
                    "avatarUrl": avatar,
                    "briefDesc": desc,
                    "fans": fans,
                    "alias": [],
                    "albumSize": data.get("album_count").and_then(value_to_i64).unwrap_or(0),
                    "musicSize": data.get("song_count").and_then(value_to_i64).unwrap_or(0)
                }
            })
        }
        Err(_) => json!({ "code": 500, "msg": "Failed to get artist detail" }),
    }
}

fn handle_artist_songs(params: &Value, ctx: &Ctx) -> Value {
    let query = with_page_params(params);
    match crate::modules::artist::handle_audios(&query, ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            let mut tracks = Vec::new();
            let list = body
                .pointer("/data/lists")
                .or_else(|| body.pointer("/data/songs"))
                .or_else(|| body.pointer("/data/info"))
                .or_else(|| body.get("data"))
                .and_then(|v| v.as_array());

            if let Some(arr) = list {
                for item in arr {
                    if let Some(track) = transform_kugou_song(item) {
                        tracks.push(track);
                    }
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
            "msg": "Failed to get artist songs",
            "data": { "tracks": [] }
        }),
    }
}

fn handle_artist_albums(params: &Value, ctx: &Ctx) -> Value {
    let query = with_page_params(params);
    match crate::modules::artist::handle_albums(&query, ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            let mut albums = Vec::new();
            let list = body
                .pointer("/data/lists")
                .or_else(|| body.pointer("/data/albums"))
                .or_else(|| body.pointer("/data/info"))
                .or_else(|| body.get("data"))
                .and_then(|v| v.as_array());

            if let Some(arr) = list {
                for item in arr {
                    let id = item
                        .get("album_id")
                        .or_else(|| item.get("id"))
                        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok())))
                        .unwrap_or(0);
                    let name = item
                        .get("album_name")
                        .or_else(|| item.get("name"))
                        .or_else(|| item.get("title"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let cover = standard_cover(item, &["/sizable_cover", "/cover", "/pic"]);
                    let pub_date = item
                        .get("publish_date")
                        .or_else(|| item.get("publish_time"))
                        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok())))
                        .unwrap_or(0);
                    if id > 0 && !name.is_empty() {
                        albums.push(json!({
                            "id": id,
                            "name": name,
                            "cover": cover,
                            "publishDate": pub_date
                        }));
                    }
                }
            }

            json!({
                "code": 0,
                "data": albums
            })
        }
        Err(_) => json!({ "code": 500, "msg": "Failed to get artist albums" }),
    }
}

fn handle_album_detail(params: &Value, ctx: &Ctx) -> Value {
    let detail_res = crate::modules::album::handle_album_detail(params, ctx);
    let songs_res = crate::modules::album::handle_album_songs(params, ctx);

    let detail_body = match detail_res {
        Ok(r) => r.body.to_json(),
        Err(_) => json!({}),
    };
    let songs_body = match songs_res {
        Ok(r) => r.body.to_json(),
        Err(_) => json!({}),
    };

    let album_data = detail_body
        .pointer("/data/0")
        .or_else(|| detail_body.pointer("/data/data/0"))
        .or_else(|| detail_body.get("data"))
        .unwrap_or(&detail_body);

    let id = album_data
        .get("album_id")
        .or_else(|| album_data.get("id"))
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok())))
        .or_else(|| params.get("id").and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))))
        .unwrap_or(0);

    let title = album_data
        .get("album_name")
        .or_else(|| album_data.get("name"))
        .or_else(|| album_data.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let artist = album_data
        .get("author_name")
        .or_else(|| album_data.get("artist"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let artist_id = album_data
        .get("author_id")
        .or_else(|| album_data.get("artist_id"))
        .and_then(value_to_i64)
        .unwrap_or(0);

    let cover = standard_cover(album_data, &["/sizable_cover", "/cover", "/pic"]);

    let desc = album_data
        .get("intro")
        .or_else(|| album_data.get("description"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let company = album_data
        .get("company")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let pub_date = album_data
        .get("publish_date")
        .or_else(|| album_data.get("publish_time"))
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok())))
        .unwrap_or(0);

    let mut tracks = Vec::new();
    let song_list = songs_body
        .pointer("/data/songs")
        .or_else(|| songs_body.pointer("/data/info"))
        .or_else(|| songs_body.get("data"))
        .and_then(|v| v.as_array());

    if let Some(arr) = song_list {
        for item in arr {
            if let Some(track) = transform_kugou_song(item) {
                tracks.push(track);
            }
        }
    }

    json!({
        "code": 0,
        "data": {
            "id": id,
            "title": title,
            "coverUrl": cover,
            "artist": artist,
            "artistId": artist_id,
            "publishDate": pub_date,
            "company": company,
            "description": desc,
            "tracks": tracks
        }
    })
}

fn handle_user_playlist(params: &Value, ctx: &Ctx) -> Value {
    if crate::modules::param_or_cookie_str(params, "token", "").is_empty() {
        return json!({"code": 401, "msg": "User playlists require login", "data": [], "playlist": []});
    }
    let mut query = with_page_params(params);
    let mut userid = crate::modules::param_or_cookie_num(params, "userid", 0);
    if userid <= 0 {
        userid = crate::modules::param_or_cookie_num(params, "uid", 0);
    }
    if userid > 0 {
        query["userid"] = json!(userid);
        query["uid"] = json!(userid);
    }
    match crate::modules::user::handle_playlist(&query, ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            let playlists: Vec<Value> = playlist_list(&body)
                .into_iter()
                .flatten()
                .filter_map(transform_kugou_playlist)
                .collect();

            json!({
                "code": 0,
                "status": 200,
                "data": playlists.clone(),
                "playlist": playlists
            })
        }
        Err(resp) => playlist_request_error(&resp, "get user playlists"),
    }
}

fn handle_user_playlist_kind(params: &Value, ctx: &Ctx, collected: bool) -> Value {
    let mut query = params.clone();
    query["type"] = json!(if collected { 1 } else { 0 });
    handle_user_playlist(&query, ctx)
}

fn playlist_create_query(params: &Value) -> Result<Value, Value> {
    let name = first_string(params, &["/name"]);
    if name.trim().is_empty() {
        return Err(json!({"code": 400, "msg": "Missing playlist name"}));
    }
    let mut userid = crate::modules::param_or_cookie_num(params, "userid", 0);
    if userid <= 0 {
        userid = crate::modules::param_or_cookie_num(params, "uid", 0);
    }
    let token = crate::modules::param_or_cookie_str(params, "token", "");
    if userid <= 0 || token.is_empty() {
        return Err(json!({"code": 401, "msg": "Playlist creation requires login"}));
    }
    let mut query = params.clone();
    query["name"] = json!(name.trim());
    query["userid"] = json!(userid);
    query["uid"] = json!(userid);
    query["token"] = json!(token);
    let mut cookie = params
        .get("cookie")
        .filter(|c| c.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    cookie["userid"] = json!(userid);
    cookie["uid"] = json!(userid);
    cookie["token"] = json!(token);
    query["cookie"] = cookie;
    query["type"] = json!(0);
    query["is_pri"] = json!(if first_i64(params, &["/privacy", "/is_pri"]) > 0 { 1 } else { 0 });
    Ok(query)
}

fn handle_playlist_create(params: &Value, ctx: &Ctx) -> Value {
    let query = match playlist_create_query(params) {
        Ok(query) => query,
        Err(error) => return error,
    };
    match crate::modules::playlist::handle_add(&query, ctx) {
        Ok(resp) => playlist_create_response(&resp.body.to_json(), &query),
        Err(resp) => playlist_request_error(&resp, "create playlist"),
    }
}

fn playlist_create_response(body: &Value, query: &Value) -> Value {
    let id = first_i64(body, &["/data/listid", "/data/id", "/listid", "/id"]);
    if id <= 0 {
        return json!({"code": 502, "msg": "Playlist creation returned no playlist id"});
    }
    let name = first_string(query, &["/name"]);
    let userid = first_i64(query, &["/userid", "/uid", "/cookie/userid", "/cookie/uid"]);
    let playlist_obj = json!({
        "id": id,
        "name": name.clone(),
        "title": name,
        "coverImgUrl": "",
        "coverUrl": "",
        "picUrl": "",
        "trackCount": 0,
        "total": 0,
        "userId": userid,
        "creator": {
            "userId": userid,
            "nickname": ""
        },
        "subscribed": false
    });
    json!({
        "code": 200,
        "status": 200,
        "id": id,
        "playlist": playlist_obj.clone(),
        "data": playlist_obj
    })
}

fn playlist_request_error(resp: &crate::request::ModuleResponse, action: &str) -> Value {
    let body = resp.body.to_json();
    let message = first_string(&body, &["/msg", "/message", "/errmsg", "/error", "/data/msg", "/data/message"]);
    json!({
        "code": resp.status.max(400),
        "msg": if message.is_empty() {
            format!("Failed to {action} (upstream HTTP {})", resp.status)
        } else {
            format!("Failed to {action}: {message}")
        },
        "upstreamCode": body.get("error_code").or_else(|| body.get("errcode")),
        "data": {"tracks": []}
    })
}

fn playlist_list(body: &Value) -> Option<&Vec<Value>> {
    first_array(body, &["/data/info", "/data/lists", "/data/list", "/data", "/info", "/lists"])
}

fn playlist_query(params: &Value) -> Result<Value, Value> {
    let id = first_string(params, &["/global_collection_id", "/listid", "/id", "/ids"]);
    if !id.starts_with("collection_") && id.parse::<i64>().ok().filter(|id| *id > 0).is_none() {
        return Err(json!({"code": 400, "msg": "Missing or invalid playlist id", "data": {"tracks": []}}));
    }
    let mut query = with_page_params(params);
    let limit = first_i64(params, &["/limit", "/pagesize"]).max(0);
    let limit = if limit == 0 { 30 } else { limit };
    let offset = params.get("offset").and_then(value_to_i64)
        .unwrap_or_else(|| (first_i64(&query, &["/page"]).max(1) - 1).saturating_mul(limit))
        .max(0);
    query["id"] = json!(id);
    query["ids"] = query["id"].clone();
    query["pagesize"] = json!(limit);
    query["offset"] = json!(offset);
    query["page"] = json!(offset / limit + 1);
    Ok(query)
}

struct ResolvedPlaylist {
    query: Value,
    metadata: Value,
    own: bool,
}

fn own_playlist_metadata(body: &Value, id: &str) -> Option<Value> {
    playlist_list(body)?.iter()
        .find(|item| first_string(item, &["/listid", "/id"]) == id)
        .cloned()
}

fn resolve_playlist(params: &Value, ctx: &Ctx) -> Result<ResolvedPlaylist, Value> {
    let mut query = playlist_query(params)?;
    let id = first_string(&query, &["/id"]);
    let logged_in = crate::modules::param_or_cookie_num(params, "userid", 0) > 0
        && !crate::modules::param_or_cookie_str(params, "token", "").is_empty();
    if params.get("listid").is_some() && params.get("global_collection_id").is_none() {
        if !logged_in {
            return Err(json!({"code": 401, "msg": "Personal playlists require login", "data": {"tracks": []}}));
        }
        query["listid"] = json!(id);
        query["type"] = json!(0);
        return Ok(ResolvedPlaylist { query, metadata: json!({}), own: true });
    }
    // Account-local listids and public specialids are different namespaces.
    // Check the account's lists before resolving a numeric public specialid.
    if logged_in && !id.starts_with("collection_") {
        let mut account_query = query.clone();
        account_query["page"] = json!(1);
        account_query["pagesize"] = json!(100);
        let mut previous_page = None;
        loop {
            let response = crate::modules::user::handle_playlist(&account_query, ctx)
                .map_err(|resp| playlist_request_error(&resp, "resolve personal playlist"))?;
            let body = response.body.to_json();
            if let Some(metadata) = own_playlist_metadata(&body, &id) {
                query["listid"] = json!(id);
                query["type"] = json!(0);
                return Ok(ResolvedPlaylist { query, metadata, own: true });
            }
            let count = playlist_list(&body).map_or(0, Vec::len);
            let page = first_i64(&account_query, &["/page"]);
            let total = ["/data/total", "/data/count", "/total"].iter()
                .find_map(|path| body.pointer(path).and_then(value_to_i64));
            if count < 100 || total.is_some_and(|total| page.saturating_mul(100) >= total) {
                break;
            }
            let current_page = playlist_list(&body).cloned();
            if current_page == previous_page {
                break;
            }
            previous_page = current_page;
            account_query["page"] = json!(page + 1);
        }
    }
    let gid = if id.starts_with("collection_") {
        id.clone()
    } else {
        if id.parse::<i64>().ok().filter(|id| *id > 0).is_none() {
            return Err(json!({"code": 400, "msg": "Invalid playlist id", "data": {"tracks": []}}));
        }
        let response = crate::modules::forward(
            &query, ctx, "GET", "/api/v3/special/info", None,
            Some(json!({"specialid": id, "plat": 0})), None, "android",
            &[("x-router", "mobilecdnbj.kugou.com")], false, false,
        ).map_err(|resp| playlist_request_error(&resp, "resolve public playlist"))?;
        let body = response.body.to_json();
        let gid = first_string(&body, &["/data/global_specialid", "/data/global_collection_id", "/data/gid"]);
        if gid.is_empty() {
            return Err(json!({"code": 404, "msg": "Playlist not found or not accessible", "data": {"tracks": []}}));
        }
        gid
    };
    query["ids"] = json!(gid);
    query["global_collection_id"] = json!(gid);
    let response = crate::modules::playlist::handle_detail(&query, ctx)
        .map_err(|resp| playlist_request_error(&resp, "get playlist detail"))?;
    let body = response.body.to_json();
    let metadata = playlist_metadata(&body).cloned().unwrap_or_else(|| json!({}));
    Ok(ResolvedPlaylist { query, metadata, own: false })
}

fn playlist_metadata(body: &Value) -> Option<&Value> {
    body.pointer("/data/list_info")
        .filter(|item| item.is_object())
        .or_else(|| body.get("data").and_then(Value::as_array).and_then(|list| list.first()))
        .or_else(|| body.pointer("/data/info").filter(|item| item.is_object()))
        .or_else(|| body.get("data").filter(|item| item.is_object()))
}

fn playlist_songs(body: &Value) -> Option<&Vec<Value>> {
    static EMPTY_SONGS: Vec<Value> = Vec::new();
    let paths = ["/data/songs", "/data/songlist", "/data/info", "/data/files", "/data/list", "/data", "/songs", "/info"];
    first_array(body, &paths).or_else(|| {
        // Cloudlist returns info:null (not []) for a successfully read empty list.
        // Only accept that documented shape when success and a zero total agree.
        let empty = body.get("status").and_then(value_to_i64) == Some(1)
            && playlist_total(body) == Some(0)
            && first_i64(body, &["/error_code", "/errcode"]) == 0
            && paths.iter().any(|path| body.pointer(path).is_some_and(Value::is_null));
        empty.then_some(&EMPTY_SONGS)
    })
}

fn fetch_playlist_tracks(resolved: &ResolvedPlaylist, ctx: &Ctx) -> Result<Value, Value> {
    fetch_playlist_tracks_with(resolved, |q: &Value| {
        let response = if resolved.own {
            crate::modules::playlist::handle_track_all_new(q, ctx)
        } else {
            crate::modules::playlist::handle_track_all(q, ctx)
        };
        response.map(|resp| resp.body.to_json())
            .map_err(|resp| playlist_request_error(&resp, "get playlist tracks"))
    })
}

fn fetch_playlist_tracks_with(
    resolved: &ResolvedPlaylist,
    mut fetch: impl FnMut(&Value) -> Result<Value, Value>,
) -> Result<Value, Value> {
    let query = &resolved.query;
    let mut body = fetch(query)?;
    let songs = playlist_songs(&body).ok_or_else(|| json!({
        "code": 502, "msg": "Upstream playlist response contains no song list", "data": {"tracks": []}
    }))?;
    let limit = first_i64(query, &["/pagesize"]).max(1) as usize;
    let skip = if resolved.own {
        (first_i64(query, &["/offset"]) % limit as i64) as usize
    } else { 0 };
    if skip > 0 {
        let mut combined = songs.clone();
        let total = playlist_total(&body);
        let page_end = first_i64(query, &["/page"]).saturating_mul(limit as i64);
        if combined.len() >= limit && total.is_none_or(|total| page_end < total) {
            let mut next_query = query.clone();
            next_query["page"] = json!(first_i64(query, &["/page"]) + 1);
            let next = fetch(&next_query)?;
            let next_songs = playlist_songs(&next).ok_or_else(|| json!({
                "code": 502, "msg": "Upstream playlist response contains no song list", "data": {"tracks": []}
            }))?;
            combined.extend(next_songs.iter().cloned());
        }
        if !body["data"].is_object() {
            body["data"] = json!({});
        }
        body["data"]["songs"] = json!(combined.into_iter().skip(skip).take(limit).collect::<Vec<_>>());
    }
    Ok(body)
}

fn playlist_total(body: &Value) -> Option<i64> {
    ["/data/count", "/data/total", "/data/list_info/count", "/total", "/count"]
        .iter().find_map(|path| body.pointer(path).and_then(value_to_i64))
}

fn playlist_tracks_response(body: &Value, query: &Value) -> Value {
    let Some(songs) = playlist_songs(body) else {
        return json!({"code": 502, "msg": "Upstream playlist response contains no song list", "data": {"tracks": []}});
    };
    let tracks: Vec<Value> = songs.iter().filter_map(transform_kugou_song).collect();
    let total = playlist_total(body);
    let offset = first_i64(query, &["/offset"]);
    let has_more = total.map(|total| offset.saturating_add(songs.len() as i64) < total)
        .or_else(|| ["/data/more", "/data/hasMore", "/more", "/hasMore"].iter()
            .find_map(|path| body.pointer(path).and_then(|v| v.as_bool().or_else(|| value_to_i64(v).map(|v| v != 0)))))
        .unwrap_or(songs.len() as i64 >= first_i64(query, &["/pagesize"]).max(1));
    // The host reads hasMore at the root; retain the nested field for v3 clients.
    json!({"code": 0, "hasMore": has_more, "data": {"tracks": tracks, "hasMore": has_more, "total": total}})
}

fn handle_playlist_detail(params: &Value, ctx: &Ctx) -> Value {
    if let Some(ip_id) = discovery::ip_id(params) {
        return discovery::ip_playlist(params, ctx, ip_id, true);
    }
    if let Some(rank_id) = rank_id_from_playlist_params(params) {
        return handle_rank_playlist(params, ctx, rank_id, true);
    }
    let resolved = match resolve_playlist(params, ctx) {
        Ok(resolved) => resolved,
        Err(error) => return error,
    };
    let body = match fetch_playlist_tracks(&resolved, ctx) {
        Ok(body) => body,
        Err(error) => return error,
    };
    playlist_detail_response(&body, &resolved)
}

fn playlist_detail_response(body: &Value, resolved: &ResolvedPlaylist) -> Value {
    let mut response = playlist_tracks_response(body, &resolved.query);
    let metadata = if resolved.metadata.as_object().is_some_and(|m| !m.is_empty()) {
        &resolved.metadata
    } else {
        playlist_metadata(body).unwrap_or(&resolved.metadata)
    };
    let title = first_string(metadata, &["/name", "/title", "/specialname", "/rankname"]);
    let mut cover = standard_cover(metadata, &[
        "/flexible_cover", "/pic", "/coverImgUrl", "/cover", "/coverUrl", "/img", "/imgurl", "/sizable_cover",
        "/trans_param/union_cover", "/picUrl",
    ]);
    if cover.is_empty() {
        if let Some(track) = response.pointer("/data/tracks/0") {
            cover = standard_cover(track, &["/coverUrl", "/picUrl", "/img"]);
        }
    }
    let id_str = first_string(&resolved.query, &["/id"]);
    let id_val = id_str.parse::<i64>().map(|id| json!(id)).unwrap_or_else(|_| json!(id_str));
    let desc = first_string(metadata, &["/intro", "/description"]);
    if response["data"]["total"].is_null() {
        response["data"]["total"] = metadata.get("count").cloned().unwrap_or_else(|| json!(response["data"]["tracks"].as_array().map_or(0, Vec::len)));
    }
    let total = response["data"]["total"].clone();
    let track_count = total.as_i64().unwrap_or(0);
    let tracks = response["data"]["tracks"].clone();
    let author = first_string(metadata, &["/author", "/creator/nickname", "/creator/name", "/username", "/nickname", "/list_create_username"]);
    let userid = first_i64(metadata, &["/userid", "/uid", "/creator/userId", "/creator/userid", "/creator/id", "/create_userid"]);

    response["status"] = json!(200);
    response["data"]["id"] = id_val.clone();
    response["data"]["name"] = json!(title.clone());
    response["data"]["title"] = json!(title.clone());
    response["data"]["coverImgUrl"] = json!(cover.clone());
    response["data"]["coverUrl"] = json!(cover.clone());
    response["data"]["picUrl"] = json!(cover.clone());
    response["data"]["description"] = json!(desc.clone());
    response["data"]["trackCount"] = json!(track_count);
    response["data"]["total"] = total.clone();

    let playlist_obj = json!({
        "id": id_val,
        "name": title.clone(),
        "title": title,
        "coverImgUrl": cover.clone(),
        "coverUrl": cover.clone(),
        "picUrl": cover,
        "description": desc,
        "trackCount": track_count,
        "total": total,
        "tracks": tracks,
        "creator": {
            "nickname": author,
            "userId": userid
        },
        "subscribed": false
    });
    response["playlist"] = playlist_obj;
    response
}

fn handle_playlist_tracks(params: &Value, ctx: &Ctx) -> Value {
    if let Some(ip_id) = discovery::ip_id(params) {
        return discovery::ip_playlist(params, ctx, ip_id, false);
    }
    if let Some(rank_id) = rank_id_from_playlist_params(params) {
        return handle_rank_playlist(params, ctx, rank_id, false);
    }
    let resolved = match resolve_playlist(params, ctx) {
        Ok(resolved) => resolved,
        Err(error) => return error,
    };
    match fetch_playlist_tracks(&resolved, ctx) {
        Ok(body) => playlist_tracks_response(&body, &resolved.query),
        Err(error) => error,
    }
}

fn handle_recommend_songs(params: &Value, ctx: &Ctx) -> Value {
    match discovery::read(crate::modules::everyday::handle_recommend_songs(params, ctx)) {
        Ok(body) => json!({"code": 0, "data": {"dailySongs": discovery::songs(&body, params)}}),
        Err(error) => error,
    }
}

fn handle_recommend_resource(params: &Value, ctx: &Ctx) -> Value {
    discovery::playlists(params, ctx, false)
}

fn normalize_epoch_millis(value: Option<&Value>) -> i64 {
    let timestamp = value.and_then(value_to_i64).unwrap_or(0);
    if timestamp > 0 && timestamp < 100_000_000_000 {
        timestamp.saturating_mul(1000)
    } else {
        timestamp
    }
}

fn standard_comment(item: &Value, parent_override: i64) -> Option<Value> {
    let id = first_i64(item, &["/commentId", "/cid", "/comment_id", "/id"]);
    if id <= 0 {
        return None;
    }
    let content = first_string(item, &["/content", "/comment", "/text"]);
    let user_id = first_i64(item, &["/user/userId", "/user/userid", "/userid", "/user_id"]);
    let user_name = first_string(
        item,
        &[
            "/user/nickname",
            "/user/name",
            "/user_name",
            "/username",
            "/nickname",
        ],
    );
    let avatar = first_string(
        item,
        &[
            "/user/avatarUrl",
            "/user/avatar",
            "/user_pic",
            "/avatar",
            "/pic",
        ],
    );
    let parent = first_i64(item, &["/parentCommentId", "/parent_comment_id", "/parent_id"])
        .max(parent_override);
    let reply_count = first_i64(item, &["/replyCount", "/reply_count", "/replynum"]);
    let liked_count = first_i64(item, &["/likedCount", "/like_count", "/praise_count", "/like"]);
    let liked = item
        .get("liked")
        .or_else(|| item.get("is_like"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let time = normalize_epoch_millis(
        item.get("time")
            .or_else(|| item.get("addtime"))
            .or_else(|| item.get("create_time")),
    );
    let time_str = first_string(item, &["/timeStr", "/time_string", "/addtime_str"]);

    Some(json!({
        "commentId": id,
        "content": content,
        "time": time,
        "timeStr": time_str,
        "likedCount": liked_count,
        "liked": liked,
        "replyCount": reply_count,
        "parentCommentId": parent,
        "user": {
            "userId": user_id,
            "nickname": user_name,
            "avatarUrl": avatar
        }
    }))
}

fn handle_comments(params: &Value, ctx: &Ctx) -> Value {
    let mut query = params.clone();
    if query.get("mixsongid").is_none() {
        if let Some(id) = params.get("id") {
            query["mixsongid"] = id.clone();
        }
    }
    if query.get("childrenid").is_none() {
        if let Some(id) = params.get("id") {
            query["childrenid"] = id.clone();
        }
    }
    if query.get("page").is_none() {
        if let Some(page) = params.get("pageNo") {
            query["page"] = page.clone();
        }
    }
    if query.get("pagesize").is_none() {
        if let Some(size) = params.get("pageSize") {
            query["pagesize"] = size.clone();
        }
    }
    let sort_type = params.get("sortType").and_then(value_to_i64).unwrap_or(1);
    let comment_result = match sort_type {
        2 => crate::modules::comment_more::handle_music_topliked(&query, ctx),
        3 => crate::modules::comment_more::handle_music_classify(&query, ctx),
        _ => crate::modules::comment_music::handle(&query, ctx),
    };
    match comment_result {
        Ok(resp) => {
            let body = resp.body.to_json();
            let mut comments = Vec::new();
            let list = first_array(&body, &["/data/list", "/data/comments", "/list", "/comments"]);
            if let Some(items) = list {
                comments.extend(items.iter().filter_map(|item| standard_comment(item, 0)));
            }
            let total = first_i64(&body, &["/data/totalCount", "/data/total", "/data/count", "/total"]);
            let page_size = params
                .get("pageSize")
                .or_else(|| params.get("pagesize"))
                .and_then(value_to_i64)
                .unwrap_or(30)
                .max(1);
            let has_more = body
                .pointer("/data/hasMore")
                .or_else(|| body.pointer("/data/more"))
                .and_then(|v| v.as_bool())
                .unwrap_or(comments.len() as i64 >= page_size);
            json!({
                "code": 0,
                "data": {
                    "totalCount": total,
                    "hasMore": has_more,
                    "cursor": first_string(&body, &["/data/cursor", "/cursor"]),
                    "sortType": params.get("sortType").and_then(value_to_i64).unwrap_or(1),
                    "comments": comments
                }
            })
        }
        Err(_) => json!({
            "code": 500,
            "msg": "Failed to get comments",
            "data": {
                "totalCount": 0,
                "hasMore": false,
                "comments": []
            }
        }),
    }
}

fn handle_comment_floor(params: &Value, ctx: &Ctx) -> Value {
    let mut query = params.clone();
    if let Some(id) = params.get("id") {
        query["special_id"] = id.clone();
    }
    if let Some(parent) = params.get("parentCommentId") {
        query["tid"] = parent.clone();
    }
    query["resource_type"] = json!("music");
    if query.get("page").is_none() {
        if let Some(page) = params.get("pageNo") {
            query["page"] = page.clone();
        }
    }
    if query.get("pagesize").is_none() {
        if let Some(size) = params.get("pageSize") {
            query["pagesize"] = size.clone();
        }
    }
    match crate::modules::comment_more::handle_floor(&query, ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            let parent = params
                .get("parentCommentId")
                .and_then(value_to_i64)
                .unwrap_or(0);
            let comments = first_array(&body, &["/data/list", "/data/comments", "/list", "/comments"])
                .into_iter()
                .flatten()
                .filter_map(|item| standard_comment(item, parent))
                .collect::<Vec<_>>();
            json!({
                "code": 0,
                "data": {
                    "comments": comments,
                    "totalCount": first_i64(&body, &["/data/totalCount", "/data/total", "/data/count"]),
                    "hasMore": body.pointer("/data/hasMore").and_then(|v| v.as_bool()).unwrap_or(false),
                    "time": first_i64(&body, &["/data/time", "/time"])
                }
            })
        }
        Err(_) => json!({
            "code": 500,
            "msg": "Failed to get comment floor",
            "data": { "comments": [], "totalCount": 0, "hasMore": false, "time": 0 }
        }),
    }
}

fn handle_comment_create(params: &Value, ctx: &Ctx) -> Value {
    let mut query = params.clone();
    if let Some(id) = params.get("id") {
        query["mixsongid"] = id.clone();
    }
    match crate::modules::comment_send::handle_music_send(&query, ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            let comment = body.get("data").and_then(|v| standard_comment(v, 0));
            json!({ "code": 0, "data": { "comment": comment } })
        }
        Err(_) => json!({ "code": 500, "msg": "Failed to create comment" }),
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

#[cfg(test)]
mod adapter_contract_tests {
    use super::*;

    #[test]
    fn rankings_expose_distinct_playlist_ids_and_canonical_list() {
        let response = toplist_response(&json!({"data": {"info": [
            {"rankid": "8888", "rankname": "Ranking", "imgurl": "http://imge.kugou.com/{size}/rank.jpg",
             "update_frequency": "daily"},
            {"rankid": 0, "rankname": "Category"}
        ]}}));
        let list = response["list"].as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(response["data"]["list"], response["list"]);
        assert_eq!(list[0]["id"], RANK_PLAYLIST_BASE + 8888);
        assert_eq!(list[0]["rankId"], 8888);
        assert_eq!(list[0]["coverUrl"], "https://imge.kugou.com/400/rank.jpg");
        assert_eq!(list[0]["updateFrequency"], "daily");
        assert!(list[0]["id"].as_i64().unwrap() < (1 << 53));
    }

    #[test]
    fn ranking_ids_do_not_intercept_real_playlists() {
        for id in ["8888", "198", "7235936", "collection_3_9876543210_198_0"] {
            assert_eq!(rank_id_from_playlist_params(&json!({"id": id})), None);
        }
        for rank_id in [1, 8888, RANK_PLAYLIST_SPAN - 1] {
            assert_eq!(rank_id_from_playlist_params(&json!({"id": (RANK_PLAYLIST_BASE + rank_id).to_string()})), Some(rank_id));
        }
        assert_eq!(rank_id_from_playlist_params(&json!({"id": RANK_PLAYLIST_BASE})), None);
        assert_eq!(rank_id_from_playlist_params(&json!({"id": RANK_PLAYLIST_BASE + RANK_PLAYLIST_SPAN})), None);
        assert_eq!(toplist_response(&json!({"data": {}}))["code"], 502);
    }

    #[test]
    fn rank_tracks_use_playable_nested_hash_before_numeric_resource_ids() {
        let track = transform_kugou_song(&json!({
            "hash": "", "songname": "Ranking track", "audio_id": 123, "album_audio_id": 456,
            "audio_info": {"hash_128": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "duration_128": 223763},
            "deprecated": {"hash": "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB", "duration": 223},
            "authors": [{"author_name": "Artist"}],
            "album_info": {"album_name": "Album", "sizable_cover": "https://image.test/{size}/track.jpg"}
        })).unwrap();
        assert_eq!(track["id"], "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(track["duration"], 223);
        assert_eq!(track["artist"], "Artist");
        assert_eq!(track["album"], "Album");
        assert_eq!(track["coverUrl"], "https://image.test/400/track.jpg");
    }

    #[test]
    fn ranking_pages_support_host_offsets_and_native_songlist() {
        let query = rank_playlist_query(&json!({
            "id": (RANK_PLAYLIST_BASE + 8888).to_string(), "limit": 3, "offset": 2,
            "cookie": {"token": "fixture"}
        }), 8888);
        let resolved = ResolvedPlaylist { query, metadata: json!({}), own: true };
        let mut pages = Vec::new();
        let body = fetch_playlist_tracks_with(&resolved, |query| {
            assert_eq!(query["rankid"], 8888);
            assert_eq!(query["cookie"]["token"], "fixture");
            let page = first_i64(query, &["/page"]);
            pages.push(page);
            let indices = if page == 1 { 0..3 } else { 3..5 };
            Ok(json!({"status": 1, "total": 5, "data": {"total": 5, "songlist": indices.map(|index| {
                json!({"songname": format!("Track {index}"), "audio_info": {"hash_128": format!("{index:032x}")}})
            }).collect::<Vec<_>>()}}))
        }).unwrap();
        assert_eq!(pages, vec![1, 2]);
        let response = playlist_tracks_response(&body, &resolved.query);
        let tracks = response["data"]["tracks"].as_array().unwrap();
        assert_eq!(tracks.len(), 3);
        assert_eq!(tracks[0]["id"], format!("{:032x}", 2));
        assert_eq!(tracks[2]["id"], format!("{:032x}", 4));
        assert_eq!(response["data"]["total"], 5);
        assert_eq!(response["hasMore"], false);
    }

    #[test]
    fn ranking_detail_retains_virtual_identity_name_artwork_and_total() {
        let resolved = ResolvedPlaylist {
            query: rank_playlist_query(&json!({"id": RANK_PLAYLIST_BASE + 8888, "limit": 3}), 8888),
            metadata: json!({"rankid": 8888, "rankname": "Ranking", "imgurl": "https://image.test/{size}/rank.jpg", "intro": "Description"}),
            own: true,
        };
        let response = playlist_detail_response(&json!({"status": 1, "data": {
            "total": 500, "songlist": [{"hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "songname": "Track"}]
        }}), &resolved);
        assert_eq!(response["code"], 0);
        assert_eq!(response["data"]["id"], RANK_PLAYLIST_BASE + 8888);
        assert_eq!(response["data"]["title"], "Ranking");
        assert_eq!(response["data"]["coverUrl"], "https://image.test/400/rank.jpg");
        assert_eq!(response["data"]["description"], "Description");
        assert_eq!(response["data"]["total"], 500);
        assert_eq!(response["hasMore"], true);
    }

    #[test]
    fn search_query_bounds_page_sizes_before_calculating_offsets() {
        for (limit, expected) in [(0, 1), (-1, 1), (3, 3), (1000, 100)] {
            let query = search_query(&json!({
                "keywords": "fixture", "limit": limit.to_string(), "offset": 0,
                "cookie": {"token": "fixture"}
            }));
            assert_eq!(query["pagesize"], expected);
            assert_eq!(query["page"], 1);
            assert_eq!(query["keyword"], "fixture");
            assert_eq!(query["cookie"]["token"], "fixture");
        }
        assert_eq!(search_query(&json!({"limit": 3, "offset": -10}))["page"], 1);
        assert_eq!(search_query(&json!({"limit": 3, "offset": 6}))["page"], 3);
    }

    #[test]
    fn album_search_accepts_native_lowercase_fields_and_highlights() {
        let album = transform_kugou_album(&json!({
            "albumid": "123", "albumname": "<em>Album</em>", "singername": "<em>Artist</em>",
            "imgurl": "http://imge.kugou.com/{size}/album.jpg"
        })).unwrap();
        assert_eq!(album["id"], 123);
        assert_eq!(album["name"], "Album");
        assert_eq!(album["artist"], "Artist");
        assert_eq!(album["coverUrl"], "https://imge.kugou.com/400/album.jpg");
    }

    #[test]
    fn song_detail_artwork_batches_only_missing_hashes_and_preserves_track_alignment() {
        let mut tracks = vec![
            json!({"id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "title": "First", "coverUrl": "", "artist": "", "album": ""}),
            json!({"id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "coverUrl": "https://image.test/existing.jpg", "artist": "Existing"}),
            json!({"id": "12345", "coverUrl": ""}),
            json!({"id": "cccccccccccccccccccccccccccccccc", "title": "Last", "coverUrl": "", "artist": "Keep", "album": ""}),
        ];
        enrich_song_artwork_with(&mut tracks, &json!({"cookie": {"token": "fixture"}}), |query| {
            assert_eq!(query["hash"], "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,cccccccccccccccccccccccccccccccc");
            assert_eq!(query["cookie"]["token"], "fixture");
            Some(json!({"status": 1, "error_code": 0, "data": [
                {"album": [{"album_name": "Album A", "sizable_cover": "http://c1.kgimg.com/{size}/a.jpg"}],
                 "author": [{"author_name": "Artist A"}]},
                {"album": [{"album_name": "Album C", "sizable_cover": "http://c1.kgimg.com/{size}/c.jpg"}],
                 "author": [{"author_name": "Artist C"}]}
            ]}))
        });
        assert_eq!(tracks[0]["coverUrl"], "https://c1.kgimg.com/400/a.jpg");
        assert_eq!(tracks[0]["artist"], "Artist A");
        assert_eq!(tracks[0]["album"], "Album A");
        assert_eq!(tracks[1]["coverUrl"], "https://image.test/existing.jpg");
        assert_eq!(tracks[2]["coverUrl"], "");
        assert_eq!(tracks[3]["coverUrl"], "https://c1.kgimg.com/400/c.jpg");
        assert_eq!(tracks[3]["artist"], "Keep");
    }

    #[test]
    fn song_artwork_failure_and_incomplete_batch_do_not_corrupt_tracks() {
        for response in [
            None,
            Some(json!({"status": 0, "data": [{"album": [{"sizable_cover": "https://image.test/wrong.jpg"}]}]})),
            Some(json!({"status": 1, "error_code": 20010, "data": []})),
            Some(json!({"status": 1, "data": []})),
            Some(json!({"status": 1, "data": [{"album": []}, {"album": []}]})),
        ] {
            let original = vec![json!({"id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "title": "Keep", "coverUrl": ""})];
            let mut tracks = original.clone();
            enrich_song_artwork_with(&mut tracks, &json!({}), |_| response);
            assert_eq!(tracks, original);
        }
    }

    #[test]
    fn existing_song_artwork_does_not_trigger_another_request() {
        let mut tracks = vec![json!({"id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "coverUrl": "https://image.test/a.jpg"})];
        enrich_song_artwork_with(&mut tracks, &json!({}), |_| panic!("unexpected image request"));
    }

    #[test]
    fn artist_search_artwork_matches_identity_and_keeps_existing_images() {
        let mut artists = vec![
            transform_kugou_artist(&json!({"singerid": 1, "singername": "<em>First</em>"})).unwrap(),
            transform_kugou_artist(&json!({"singerid": 2, "singername": "Second"})).unwrap(),
            transform_kugou_artist(&json!({"singerid": 3, "singername": "Third", "sizable_avatar": "https://image.test/keep.jpg"})).unwrap(),
        ];
        enrich_artist_artwork_with(&mut artists, &json!({"cookie": {"token": "fixture"}}), |query| {
            assert_eq!(query["cookie"]["token"], "fixture");
            match first_i64(query, &["/id"]) {
                1 => Some(json!({"status": 1, "data": {
                    "author_id": "1", "author_name": "First", "sizable_avatar": "http://c1.kgimg.com/{size}/first.jpg", "fansnums": "45"
                }})),
                2 => Some(json!({"status": 1, "data": {
                    "author_id": "99", "author_name": "Wrong", "sizable_avatar": "https://image.test/wrong.jpg"
                }})),
                _ => panic!("existing avatar should not be fetched"),
            }
        });
        assert_eq!(artists[0]["name"], "First");
        assert_eq!(artists[0]["avatarUrl"], "https://c1.kgimg.com/400/first.jpg");
        assert_eq!(artists[0]["fans"], 45);
        assert_eq!(artists[1]["avatarUrl"], "");
        assert_eq!(artists[2]["avatarUrl"], "https://image.test/keep.jpg");
    }

    #[test]
    fn artist_artwork_enrichment_bounds_concurrent_requests() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let mut artists: Vec<_> = (1..=13).map(|id| json!({"id": id, "avatarUrl": ""})).collect();
        enrich_artist_artwork_with(&mut artists, &json!({}), |_| {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(current, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(2));
            active.fetch_sub(1, Ordering::SeqCst);
            None
        });
        assert!(peak.load(Ordering::SeqCst) <= 4);
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn provider_info_reports_the_compiled_version_without_network_access() {
        let response = handle_api("provider/info", &json!({}));
        assert_eq!(response["code"], 0);
        assert_eq!(response["data"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(response["data"]["contract"], "standard");
    }

    #[test]
    fn successful_empty_cloud_playlist_with_null_info_is_not_an_error() {
        let body = json!({
            "status": 1, "error_code": 0,
            "data": {"pagesize": 3, "count": 0, "page": 1, "info": null, "listid": 1}
        });
        let resolved = ResolvedPlaylist {
            query: json!({"id": "1", "listid": "1", "pagesize": 3, "offset": 0, "page": 1}),
            metadata: json!({"listid": 1, "name": "Empty playlist", "pic": "", "count": 0}),
            own: true,
        };
        let fetched = fetch_playlist_tracks_with(&resolved, |_| Ok(body.clone())).unwrap();
        let tracks = playlist_tracks_response(&fetched, &resolved.query);
        assert_eq!(tracks["code"], 0);
        assert_eq!(tracks["data"]["tracks"], json!([]));
        assert_eq!(tracks["data"]["total"], 0);
        assert_eq!(tracks["hasMore"], false);
        let detail = playlist_detail_response(&fetched, &resolved);
        assert_eq!(detail["code"], 0);
        assert_eq!(detail["data"]["id"], 1);
        assert_eq!(detail["data"]["title"], "Empty playlist");
        assert_eq!(detail["data"]["total"], 0);
        assert_eq!(detail["data"]["coverUrl"], "");
    }

    #[test]
    fn failed_or_inconsistent_null_cloud_playlist_is_not_reported_as_empty() {
        for body in [
            json!({"status": 0, "error_code": 20010, "data": {"count": 0, "info": null}}),
            json!({"status": 1, "error_code": 20010, "data": {"count": 0, "info": null}}),
            json!({"status": 1, "data": {"count": 3, "info": null}}),
            json!({"status": 1, "data": {"count": 0}}),
            json!({"data": {"count": 0, "info": null}}),
        ] {
            assert_eq!(playlist_tracks_response(&body, &json!({"pagesize": 3}))["code"], 502);
        }
    }

    #[test]
    fn empty_cloud_playlist_with_nonzero_offset_does_not_request_another_page() {
        let resolved = ResolvedPlaylist {
            query: json!({"id": "1", "listid": "1", "pagesize": 3, "offset": 2, "page": 1}),
            metadata: json!({}),
            own: true,
        };
        let mut calls = 0;
        let body = fetch_playlist_tracks_with(&resolved, |_| {
            calls += 1;
            Ok(json!({"status": 1, "data": {"count": 0, "info": null}}))
        }).unwrap();
        assert_eq!(calls, 1);
        assert_eq!(playlist_tracks_response(&body, &resolved.query)["data"]["tracks"], json!([]));
    }

    #[test]
    fn song_cover_skips_empty_root_fields_and_uses_union_cover() {
        let track = transform_kugou_song(&json!({
            "hash": "abc", "name": "Track", "Image": "", "img": null, "pic": "undefined",
            "trans_param": {"union_cover": "http://imge.kugou.com/stdmusic/{size}/cover.jpg"}
        })).unwrap();
        assert_eq!(track["coverUrl"], "https://imge.kugou.com/stdmusic/400/cover.jpg");
    }

    #[test]
    fn cover_normalization_handles_protocol_relative_and_encoded_sizes() {
        for (raw, expected) in [
            ("//c1.kgimg.com/cover/{size}/image.jpg", "https://c1.kgimg.com/cover/400/image.jpg"),
            (" http://imge.kugou.com/%7Bsize%7D/cover.jpg ", "https://imge.kugou.com/400/cover.jpg"),
            ("http://example.test/image.jpg", "http://example.test/image.jpg"),
        ] {
            assert_eq!(standard_cover(&json!({"cover": raw}), &["/cover"]), expected);
        }
        assert_eq!(standard_cover(&json!({"cover": "null"}), &["/cover"]), "");
    }

    #[test]
    fn playlist_detail_can_use_actual_first_track_artwork_when_metadata_is_empty() {
        let resolved = ResolvedPlaylist {
            query: json!({"id": "1", "pagesize": 1, "offset": 0}),
            metadata: json!({"name": "Playlist", "pic": ""}),
            own: false,
        };
        let body = json!({"data": {"count": 1, "songs": [{
            "hash": "abc", "name": "Track", "cover": "http://c1.kgimg.com/{size}/cover.jpg"
        }]}});
        assert_eq!(playlist_detail_response(&body, &resolved)["data"]["coverUrl"],
            "https://c1.kgimg.com/400/cover.jpg");
    }

    #[test]
    fn playlist_ids_and_cookies_are_preserved_for_native_requests() {
        let params = normalize_standard_params(&json!({
            "id": 7235936, "limit": "30", "offset": "35",
            "cookie": "userid=9876543210; token=fixture=="
        }));
        let query = playlist_query(&params).unwrap();
        assert_eq!(query["id"], "7235936");
        assert_eq!(query["ids"], "7235936");
        assert_eq!(query["page"], 2);
        assert_eq!(query["pagesize"], 30);
        assert_eq!(query["offset"], 35);
        assert_eq!(query["cookie"]["token"], "fixture==");
        assert_eq!(crate::modules::param_or_cookie_num(&query, "userid", 0), 9876543210);
    }

    #[test]
    fn playlist_queries_accept_global_ids_and_native_page_parameters() {
        let query = playlist_query(&json!({
            "global_collection_id": "collection_3_9876543210_198_0",
            "pagesize": 20, "page": 3
        })).unwrap();
        assert_eq!(query["ids"], "collection_3_9876543210_198_0");
        assert_eq!(query["offset"], 40);
        assert_eq!(query["page"], 3);
        assert_eq!(playlist_query(&json!({"listid": 198})).unwrap()["id"], "198");
    }

    #[test]
    fn invalid_playlist_reads_fail_before_calling_upstream() {
        for method in ["playlist/detail", "playlist/track/all"] {
            for params in [json!({}), json!({"id": 0}), json!({"id": -1}), json!({"ids": "1,2"}), json!({"id": true})] {
                assert_eq!(handle_api(method, &params)["code"], 400);
            }
            assert_eq!(handle_api(method, &json!({"listid": 198}))["code"], 401);
        }
    }

    #[test]
    fn public_playlist_search_uses_specialname_songcount_and_imgurl() {
        let item = json!({
            "specialid": 7235936, "specialname": "Study <em>music</em>",
            "songcount": 165, "imgurl": "https://image.test/{size}/cover.jpg",
            "nickname": "creator", "gid": "collection_3_9876543210_198_0"
        });
        let playlist = transform_kugou_playlist(&item).unwrap();
        assert_eq!(playlist["id"], 7235936);
        assert_eq!(playlist["title"], "Study music");
        assert_eq!(playlist["total"], 165);
        assert_eq!(playlist["coverUrl"], "https://image.test/400/cover.jpg");
        assert_eq!(playlist["author"], "creator");
    }

    #[test]
    fn personal_playlist_resolution_uses_local_listid_not_public_specialid() {
        let item = json!({
            "listid": 198, "specialid": 7235936, "name": "Personal",
            "global_collection_id": "collection_3_9876543210_198_0"
        });
        for body in [json!({"data": {"info": [item]}}), json!({"data": {"lists": [item]}}), json!({"data": [item]})] {
            assert_eq!(own_playlist_metadata(&body, "198").unwrap()["name"], "Personal");
            assert!(own_playlist_metadata(&body, "7235936").is_none());
        }
        assert_eq!(transform_kugou_playlist(&item).unwrap()["id"], 198);
    }

    #[test]
    fn playlist_metadata_reads_native_data_arrays_and_list_info() {
        let item = json!({"name": "Real title", "count": 165});
        for body in [json!({"data": [item]}), json!({"data": {"list_info": item}}), json!({"data": {"info": item}})] {
            assert_eq!(playlist_metadata(&body).unwrap()["name"], "Real title");
        }
    }

    #[test]
    fn playlist_tracks_keep_total_and_expose_host_pagination() {
        let body = json!({"data": {"count": 165, "songs": [
            {"hash": "ABCDEF", "name": "Artist - Title", "timelen": 92000, "singerinfo": [{"name": "Artist"}]},
            {"hash": "123456", "name": "Short", "timelen": 9000}
        ]}});
        let response = playlist_tracks_response(&body, &json!({"pagesize": 2, "offset": 0}));
        assert_eq!(response["code"], 0);
        assert_eq!(response["data"]["tracks"].as_array().unwrap().len(), 2);
        assert_eq!(response["data"]["tracks"][0]["id"], "abcdef");
        assert_eq!(response["data"]["tracks"][0]["duration"], 92);
        assert_eq!(response["data"]["tracks"][1]["duration"], 9);
        assert_eq!(response["data"]["total"], 165);
        assert_eq!(response["hasMore"], true);
        assert_eq!(response["data"]["hasMore"], true);
        let end = playlist_tracks_response(&body, &json!({"pagesize": 2, "offset": 163}));
        assert_eq!(end["hasMore"], false);
    }

    #[test]
    fn playlist_pagination_honors_numeric_more_without_total() {
        for (more, expected) in [(json!(1), true), (json!(0), false), (json!(false), false)] {
            let body = json!({"data": {"info": [], "more": more}});
            assert_eq!(playlist_tracks_response(&body, &json!({"pagesize": 2, "offset": 0}))["hasMore"], expected);
        }
    }

    #[test]
    fn personal_playlist_non_aligned_offset_combines_two_pages() {
        let resolved = ResolvedPlaylist {
            query: json!({"listid": "198", "pagesize": 3, "offset": 2, "page": 1}),
            metadata: json!({}),
            own: true,
        };
        let mut pages = Vec::new();
        let body = fetch_playlist_tracks_with(&resolved, |query| {
            let page = first_i64(query, &["/page"]);
            pages.push(page);
            assert_eq!(query["listid"], "198");
            let hashes = if page == 1 { vec!["0", "1", "2"] } else { vec!["3", "4"] };
            Ok(json!({"data": {"count": 5, "info": hashes.into_iter().map(|hash| json!({"hash": hash, "name": hash})).collect::<Vec<_>>()}}))
        }).unwrap();
        assert_eq!(pages, vec![1, 2]);
        let response = playlist_tracks_response(&body, &resolved.query);
        let ids: Vec<&str> = response["data"]["tracks"].as_array().unwrap()
            .iter().map(|track| track["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["2", "3", "4"]);
        assert_eq!(response["hasMore"], false);
        assert_eq!(response["data"]["total"], 5);
    }

    #[test]
    fn personal_playlist_last_page_does_not_fetch_beyond_total() {
        let resolved = ResolvedPlaylist {
            query: json!({"listid": "198", "pagesize": 3, "offset": 2, "page": 1}),
            metadata: json!({}),
            own: true,
        };
        let mut calls = 0;
        let body = fetch_playlist_tracks_with(&resolved, |_| {
            calls += 1;
            Ok(json!({"count": 3, "data": [
                {"hash": "0"}, {"hash": "1"}, {"hash": "2"}
            ]}))
        }).unwrap();
        assert_eq!(calls, 1);
        assert_eq!(playlist_songs(&body).unwrap(), &vec![json!({"hash": "2"})]);
        assert_eq!(playlist_tracks_response(&body, &resolved.query)["hasMore"], false);
    }

    #[test]
    fn public_playlist_offset_is_not_sliced_twice() {
        let resolved = ResolvedPlaylist {
            query: json!({"global_collection_id": "collection_3_9876543210_198_0", "pagesize": 3, "offset": 2, "page": 1}),
            metadata: json!({}),
            own: false,
        };
        let mut calls = 0;
        let body = fetch_playlist_tracks_with(&resolved, |query| {
            calls += 1;
            assert_eq!(query["offset"], 2);
            Ok(json!({"data": {"count": 5, "songs": [
                {"hash": "2"}, {"hash": "3"}, {"hash": "4"}
            ]}}))
        }).unwrap();
        assert_eq!(calls, 1);
        assert_eq!(playlist_songs(&body).unwrap().len(), 3);
        assert_eq!(playlist_tracks_response(&body, &resolved.query)["hasMore"], false);
    }

    #[test]
    fn playlist_detail_preserves_identity_title_and_full_total() {
        let resolved = ResolvedPlaylist {
            query: json!({"id": "7235936", "pagesize": 1, "offset": 0}),
            metadata: json!({"name": "Real title", "intro": "Description", "pic": "https://image.test/{size}/cover.jpg", "count": 165}),
            own: false,
        };
        let response = playlist_detail_response(&json!({"data": {"count": 165, "songs": [
            {"hash": "ABCDEF", "name": "Track"}
        ]}}), &resolved);
        assert_eq!(response["data"]["id"], 7235936);
        assert_eq!(response["data"]["title"], "Real title");
        assert_eq!(response["data"]["coverUrl"], "https://image.test/400/cover.jpg");
        assert_eq!(response["data"]["description"], "Description");
        assert_eq!(response["data"]["total"], 165);
        assert_eq!(response["data"]["tracks"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn malformed_playlist_is_not_reported_as_successfully_empty() {
        for body in [json!({}), json!({"data": {}}), json!({"data": null})] {
            assert_eq!(playlist_tracks_response(&body, &json!({"pagesize": 30}))["code"], 502);
        }
        let empty = playlist_tracks_response(&json!({"data": {"songs": [], "count": 0}}), &json!({"pagesize": 30}));
        assert_eq!(empty["code"], 0);
        assert_eq!(empty["hasMore"], false);
    }

    #[test]
    fn playlist_creation_routes_are_registered_and_validate_without_side_effects() {
        assert_eq!(handle_api("playlist/create", &json!({}))["code"], 400);
        assert_eq!(handle_api("playlist/create", &json!({"name": "New playlist"}))["code"], 401);
        for method in ["user/playlist/create", "user/playlist/collect"] {
            assert_eq!(handle_api(method, &json!({}))["code"], 401);
        }
        let manifest: Value = serde_json::from_str(include_str!("../packaging/manifest.json")).unwrap();
        assert_eq!(manifest["apiMap"]["user/playlist/create"], "user/playlist/create");
        assert_eq!(manifest["apiMap"]["user/playlist/collect"], "user/playlist/collect");
    }

    #[test]
    fn playlist_creation_maps_privacy_to_native_create_type() {
        for (privacy, expected) in [(json!(0), 0), (json!("10"), 1)] {
            let params = normalize_standard_params(&json!({
                "name": "  New playlist  ", "privacy": privacy, "type": 1,
                "cookie": "userid=9876543210; token=fixture=="
            }));
            let query = playlist_create_query(&params).unwrap();
            assert_eq!(query["name"], "New playlist");
            assert_eq!(query["type"], 0);
            assert_eq!(query["is_pri"], expected);
            assert_eq!(query["cookie"]["token"], "fixture==");
        }
    }

    #[test]
    fn playlist_creation_exposes_root_id_for_the_host_parser() {
        for body in [json!({"data": {"listid": 198}}), json!({"listid": 198})] {
            let response = playlist_create_response(&body, &json!({"name": "New playlist"}));
            assert_eq!(response["code"], 200);
            assert_eq!(response["id"], 198);
            assert_eq!(response["playlist"]["id"], 198);
            assert_eq!(response["playlist"]["name"], "New playlist");
            assert_eq!(response["playlist"]["title"], "New playlist");
        }
        assert_eq!(playlist_create_response(&json!({"data": {}}), &json!({"name": "New playlist"}))["code"], 502);
    }

    #[test]
    fn playlist_errors_preserve_upstream_errmsg_and_error_code() {
        let response = crate::request::ModuleResponse {
            status: 502,
            body: crate::request::BodyValue::Json(json!({
                "status": 0, "error_code": 20010, "errmsg": "get other list file fail"
            })),
            cookie: Vec::new(),
            headers: std::collections::HashMap::new(),
        };
        let error = playlist_request_error(&response, "get playlist tracks");
        assert_eq!(error["code"], 502);
        assert_eq!(error["upstreamCode"], 20010);
        assert!(error["msg"].as_str().unwrap().contains("get other list file fail"));
    }

    #[test]
    fn string_cookie_reaches_the_upstream_identity_helpers() {
        let params = normalize_standard_params(&json!({
            "id": "track",
            "cookie": " token=fixture==; userid=9876543210; vip_token=vip; vip_type=6; ignored "
        }));

        assert_eq!(params["id"], "track");
        assert_eq!(crate::modules::c_str(&params, "token"), "fixture==");
        assert_eq!(crate::modules::c_str(&params, "vip_token"), "vip");
        assert_eq!(
            crate::modules::param_or_cookie_num(&params, "userid", 0),
            9876543210
        );
        assert_eq!(crate::modules::q_cookie(&params)["vip_type"], "6");
        assert!(params["cookie"].get("ignored").is_none());
    }

    #[test]
    fn object_cookie_is_preserved_without_changing_types() {
        let params = json!({"cookie": {"token": "fixture", "userid": 123, "vip_type": 6}});
        assert_eq!(normalize_standard_params(&params), params);
        assert_eq!(normalize_standard_params(&json!({})), json!({}));
        assert_eq!(
            normalize_standard_params(&json!({"cookie": ""}))["cookie"],
            json!({})
        );
    }

    #[test]
    fn qr_progress_uses_nested_status_and_the_kugou_state_machine() {
        for (upstream, expected) in [(0, 800), (1, 801), (2, 802), (3, 800)] {
            let response = qr_check_response(
                &json!({"status": 1, "data": {"status": upstream}}),
                &[],
            );
            assert_eq!(response["code"], expected, "QR state {upstream}");
            assert!(response.get("cookie").is_none());
        }
    }

    #[test]
    fn authorized_qr_returns_credentials_instead_of_waiting_for_confirmation() {
        let response = qr_check_response(
            &json!({
                "status": 1,
                "data": {
                    "status": 4,
                    "token": "fixture==",
                    "userid": 9876543210_i64,
                    "vip_token": "vip",
                    "vip_type": 6
                }
            }),
            &[],
        );

        assert_eq!(response["code"], 803);
        let params = normalize_standard_params(&response);
        assert_eq!(params["cookie"]["token"], "fixture==");
        assert_eq!(params["cookie"]["userid"], "9876543210");
        assert_eq!(params["cookie"]["vip_token"], "vip");
        assert_eq!(params["cookie"]["vip_type"], "6");
    }

    #[test]
    fn qr_accepts_string_status_and_cookie_credentials() {
        let response = qr_check_response(
            &json!({"status": 1, "data": {"status": "4"}}),
            &["token=fixture".into(), "userid=123".into()],
        );
        assert_eq!(response["code"], 803);
        assert!(response["cookie"].as_str().unwrap().contains("token=fixture"));
    }

    #[test]
    fn qr_never_reports_login_success_without_usable_credentials() {
        for data in [
            json!({"status": 4}),
            json!({"status": 4, "token": "", "userid": 123}),
            json!({"status": 4, "token": "undefined", "userid": 123}),
            json!({"status": 4, "token": "fixture", "userid": 0}),
            json!({"status": 4, "token": "fixture", "userid": "not-a-userid"}),
        ] {
            let response = qr_check_response(&json!({"status": 1, "data": data}), &[]);
            assert_eq!(response["code"], 502);
            assert!(response.get("cookie").is_none());
        }
    }

    #[test]
    fn qr_missing_or_unknown_status_is_not_masked_as_waiting() {
        for body in [
            json!({}),
            json!({"data": {"status": 99}}),
            json!({"status": 1, "data": {"status": "invalid"}}),
        ] {
            assert_eq!(qr_check_response(&body, &[])["code"], 500);
        }
    }

    #[test]
    fn playback_retains_paid_track_denials_without_url_keys() {
        let response = song_url_response(
            &json!({
                "status": 2,
                "priv_status": 0,
                "fail_process": ["pkg", "buy"],
                "auth_through": []
            }),
            "lossless",
            "flac",
        );

        assert_eq!(response["code"], 403);
        assert!(response["data"]["url"].is_null());
        assert_eq!(response["data"]["reason"], "upstream fail_process: pkg,buy");
    }

    #[test]
    fn playback_retains_nested_denials_and_upstream_error_codes() {
        let response = song_url_response(
            &json!({
                "error_code": 31833,
                "data": [{"fail_process": ["buy"], "priv_status": 0}]
            }),
            "lossless",
            "flac",
        );
        assert_eq!(response["code"], 403);
        assert_eq!(response["data"]["upstreamErrorCode"], 31833);
        assert!(response["data"]["reason"].as_str().unwrap().contains("buy"));
    }

    #[test]
    fn empty_playback_response_is_a_failure_with_a_reason() {
        for body in [json!({}), json!({"data": null}), json!({"url": []})] {
            let response = song_url_response(&body, "standard", "128");
            assert_eq!(response["code"], 502);
            assert!(response["data"]["url"].is_null());
            assert!(!response["data"]["reason"].as_str().unwrap().is_empty());
        }
    }

    #[test]
    fn playback_preserves_transport_error_message() {
        let response = song_url_response(
            &json!({"status": 0, "msg": "upstream timeout"}),
            "lossless",
            "flac",
        );
        assert_eq!(response["code"], 502);
        assert_eq!(response["data"]["reason"], "upstream timeout");
    }

    #[test]
    fn playback_accepts_root_nested_and_array_responses() {
        let info = json!({
            "url": "https://audio.example.test/song.mp3",
            "quality": "128",
            "extName": "mp3",
            "fileSize": "12345"
        });
        for body in [
            info.clone(),
            json!({"data": info}),
            json!([null, info]),
            json!({"data": [null, info]}),
            json!({"url": null, "extName": "", "data": info}),
        ] {
            let response = song_url_response(&body, "lossless", "flac");
            assert_eq!(response["code"], 0);
            assert_eq!(response["data"]["url"], "https://audio.example.test/song.mp3");
            assert_eq!(response["data"]["quality"], "standard");
            assert_eq!(response["data"]["requestedQuality"], "lossless");
            assert_eq!(response["data"]["format"], "mp3");
            assert_eq!(response["data"]["size"], 12345);
        }
    }

    #[test]
    fn playback_uses_valid_backup_urls_and_ignores_placeholder_strings() {
        let response = song_url_response(
            &json!({
                "url": ["", "undefined", "null"],
                "backupUrl": [" ", " https://audio.example.test/backup.flac "],
                "quality": "flac",
                "extName": "flac"
            }),
            "lossless",
            "flac",
        );
        assert_eq!(response["code"], 0);
        assert_eq!(response["data"]["url"], "https://audio.example.test/backup.flac");
        assert_eq!(response["data"]["quality"], "lossless");

        let invalid = song_url_response(&json!({"url": "undefined"}), "standard", "128");
        assert_eq!(invalid["code"], 502);
    }
}
