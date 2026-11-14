use crate::sdk::wire_models::*;
use serde_json::Value;

pub fn value_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

pub fn value_to_i64(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

pub fn first_string(root: &Value, paths: &[&str]) -> String {
    for path in paths {
        if let Some(val) = root.pointer(path).and_then(value_to_string) {
            return val;
        }
    }
    String::new()
}

pub fn first_i64(root: &Value, paths: &[&str]) -> i64 {
    for path in paths {
        if let Some(val) = root.pointer(path).and_then(value_to_i64) {
            return val;
        }
    }
    0
}

/// 宿主严苛校验要求：HTTP URL 必须以 http:// 或 https:// 开头，不含控制字符，长度不超过 16384
pub fn sanitize_http_url(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    let full = if trimmed.starts_with("//") {
        format!("https:{}", trimmed)
    } else {
        trimmed.to_string()
    };
    let replaced = full.replace("{size}", "400");
    let clean = replaced.trim();
    if (clean.starts_with("http://") || clean.starts_with("https://"))
        && clean.len() <= 16384
        && clean.chars().all(|c| (c as u32) >= 32)
    {
        Some(clean.to_string())
    } else {
        None
    }
}

pub fn sanitize_id(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.len() > 1024 || trimmed.chars().any(|c| (c as u32) < 32 || (c as u32) == 127) {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn sanitize_title(s: &str, fallback: Option<&str>) -> Option<String> {
    let trimmed = s.trim();
    if !trimmed.is_empty() {
        let truncated = if trimmed.len() > 4096 {
            &trimmed[..4096]
        } else {
            trimmed
        };
        Some(truncated.to_string())
    } else {
        fallback.map(|fb| fb.to_string())
    }
}

pub fn to_track(val: &Value) -> Option<Track> {
    let raw_id = first_string(val, &[
        "/id", "/hash", "/FileHash", "/audio_id", "/song_hash", "/audio_info/hash", "/hash_128", "/song_id"
    ]);
    let id = sanitize_id(&raw_id)?;

    let raw_title = first_string(val, &["/title", "/name", "/SongName", "/audio_name", "/filename"]);
    let title = sanitize_title(&raw_title, None)?;

    let artist_str = first_string(val, &["/artist", "/SingerName", "/author_name"]);
    let mut artists: Vec<Artist> = if artist_str.is_empty() {
        Vec::new()
    } else {
        artist_str
            .split('/')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| Artist {
                id: None,
                name: if s.len() > 4096 { s[..4096].to_string() } else { s.to_string() },
            })
            .collect()
    };

    if artists.is_empty() {
        artists.push(Artist {
            id: None,
            name: "未知歌手".to_string(),
        });
    }
    if artists.len() > 64 {
        artists.truncate(64);
    }

    let dur_sec = first_i64(val, &["/duration"]);
    let mut duration_ms = if dur_sec > 0 {
        dur_sec * 1000
    } else {
        first_i64(val, &["/timelen", "/durationMs"])
    };
    if duration_ms < 0 {
        duration_ms = 0;
    }

    let album = {
        let s = first_string(val, &["/album", "/AlbumName", "/album_name"]);
        sanitize_title(&s, None)
    };

    let cover_url = {
        let s = first_string(val, &["/coverUrl", "/coverImgUrl", "/picUrl", "/cover", "/img", "/pic", "/Image"]);
        sanitize_http_url(&s)
    };

    Some(Track {
        id,
        title,
        artists,
        durationMs: duration_ms,
        album,
        coverUrl: cover_url,
    })
}

pub fn to_track_page(val: &Value) -> TrackPage {
    let mut items = Vec::new();
    let root = if let Some(data) = val.get("data") { data } else { val };

    let arr = if let Some(arr) = root.as_array() {
        Some(arr)
    } else if let Some(arr) = val.as_array() {
        Some(arr)
    } else {
        root.get("tracks")
            .or_else(|| root.get("list"))
            .or_else(|| root.get("songs"))
            .or_else(|| root.get("dailySongs"))
            .or_else(|| root.get("song_list"))
            .or_else(|| root.get("items"))
            .or_else(|| root.get("info"))
            .and_then(Value::as_array)
    };

    if let Some(arr) = arr {
        for item in arr {
            if let Some(t) = to_track(item) {
                items.push(t);
            }
        }
    }

    let total = first_i64(root, &["/total", "/songCount", "/count"]);
    let total_opt = if total > 0 {
        Some(total.max(items.len() as i64))
    } else {
        None
    };
    let has_more = root.get("hasMore").and_then(Value::as_bool).or(Some(items.len() >= 20));

    TrackPage {
        items,
        total: total_opt,
        hasMore: has_more,
    }
}

pub fn clamp_track_page(mut page: TrackPage, limit: i32) -> TrackPage {
    if limit > 0 && page.items.len() > limit as usize {
        page.items.truncate(limit as usize);
    }
    if let Some(total) = page.total {
        if total < page.items.len() as i64 {
            page.total = Some(page.items.len() as i64);
        }
    }
    page
}

pub fn to_playlist(val: &Value) -> Option<Playlist> {
    let raw_id = first_string(val, &["/id", "/specialid", "/listid", "/global_collection_id"]);
    let id = sanitize_id(&raw_id)?;
    let raw_title = first_string(val, &["/title", "/name", "/specialname", "/listname"]);
    let title = sanitize_title(&raw_title, Some("歌单"))?;

    let cover_url = {
        let s = first_string(val, &["/coverUrl", "/coverImgUrl", "/picUrl", "/pic", "/img", "/flexible_cover"]);
        sanitize_http_url(&s)
    };
    let track_count = {
        let c = first_i64(val, &["/trackCount", "/total", "/count", "/songcount"]);
        if c >= 0 { Some(c as i32) } else { None }
    };
    let creator_id = {
        let s = first_string(val, &["/creator/userId", "/userid", "/uid"]);
        sanitize_id(&s)
    };
    let creator_name = {
        let s = first_string(val, &["/author", "/creator/nickname", "/nickname", "/username"]);
        sanitize_title(&s, None)
    };
    let description = {
        let s = first_string(val, &["/description", "/intro"]);
        if s.trim().is_empty() {
            None
        } else {
            let trimmed = s.trim();
            Some(if trimmed.len() > 65536 { trimmed[..65536].to_string() } else { trimmed.to_string() })
        }
    };

    Some(Playlist {
        id,
        title,
        coverUrl: cover_url,
        trackCount: track_count,
        creatorId: creator_id,
        creatorName: creator_name,
        description,
    })
}

pub fn to_playlist_page(val: &Value) -> PlaylistPage {
    let mut items = Vec::new();
    let root = if let Some(data) = val.get("data") { data } else { val };

    let arr = if let Some(arr) = root.as_array() {
        Some(arr)
    } else if let Some(arr) = val.as_array() {
        Some(arr)
    } else {
        root.get("playlists")
            .or_else(|| root.get("list"))
            .or_else(|| root.get("special_list"))
            .or_else(|| root.get("items"))
            .or_else(|| root.get("info"))
            .and_then(Value::as_array)
    };

    if let Some(arr) = arr {
        for item in arr {
            if let Some(p) = to_playlist(item) {
                items.push(p);
            }
        }
    }

    let total = first_i64(root, &["/total", "/count"]);
    let total_opt = if total > 0 {
        Some(total.max(items.len() as i64))
    } else {
        None
    };
    let has_more = root.get("hasMore").and_then(Value::as_bool).or(Some(items.len() >= 20));

    PlaylistPage {
        items,
        total: total_opt,
        hasMore: has_more,
    }
}

pub fn clamp_playlist_page(mut page: PlaylistPage, limit: i32) -> PlaylistPage {
    if limit > 0 && page.items.len() > limit as usize {
        page.items.truncate(limit as usize);
    }
    if let Some(total) = page.total {
        if total < page.items.len() as i64 {
            page.total = Some(page.items.len() as i64);
        }
    }
    page
}

pub fn to_album(val: &Value) -> Option<Album> {
    let raw_id = first_string(val, &["/id", "/album_id", "/albumid"]);
    let id = sanitize_id(&raw_id)?;
    let raw_title = first_string(val, &["/name", "/title", "/album_name", "/albumname"]);
    let title = sanitize_title(&raw_title, Some("专辑"))?;

    let cover_url = {
        let s = first_string(val, &["/coverUrl", "/pic", "/img", "/cover", "/sizable_cover"]);
        sanitize_http_url(&s)
    };
    let track_count = {
        let c = first_i64(val, &["/trackCount", "/song_count", "/total"]);
        if c >= 0 { Some(c as i32) } else { None }
    };
    let company = {
        let s = first_string(val, &["/company", "/publish_company"]);
        if s.trim().is_empty() { None } else { Some(s.trim().to_string()) }
    };
    let description = {
        let s = first_string(val, &["/description", "/intro"]);
        if s.trim().is_empty() { None } else { Some(s.trim().to_string()) }
    };
    let artist_str = first_string(val, &["/artist", "/author_name", "/singername"]);
    let artists = if artist_str.is_empty() {
        vec![Artist { id: None, name: "未知歌手".to_string() }]
    } else {
        vec![Artist { id: None, name: artist_str }]
    };

    Some(Album {
        id,
        title,
        artists,
        coverUrl: cover_url,
        trackCount: track_count,
        releasedAtEpochMs: None,
        company,
        description,
    })
}

pub fn to_album_page(val: &Value) -> AlbumPage {
    let mut items = Vec::new();
    let root = if let Some(data) = val.get("data") { data } else { val };

    let arr = if let Some(arr) = root.as_array() {
        Some(arr)
    } else if let Some(arr) = val.as_array() {
        Some(arr)
    } else {
        root.get("albums")
            .or_else(|| root.get("list"))
            .or_else(|| root.get("items"))
            .or_else(|| root.get("info"))
            .and_then(Value::as_array)
    };

    if let Some(arr) = arr {
        for item in arr {
            if let Some(a) = to_album(item) {
                items.push(a);
            }
        }
    }

    let total = first_i64(root, &["/total", "/count"]);
    let total_opt = if total > 0 {
        Some(total.max(items.len() as i64))
    } else {
        None
    };
    let has_more = root.get("hasMore").and_then(Value::as_bool).or(Some(items.len() >= 20));

    AlbumPage {
        items,
        total: total_opt,
        hasMore: has_more,
    }
}

pub fn clamp_album_page(mut page: AlbumPage, limit: i32) -> AlbumPage {
    if limit > 0 && page.items.len() > limit as usize {
        page.items.truncate(limit as usize);
    }
    if let Some(total) = page.total {
        if total < page.items.len() as i64 {
            page.total = Some(page.items.len() as i64);
        }
    }
    page
}

pub fn to_artist(val: &Value) -> Option<CatalogArtist> {
    let raw_id = first_string(val, &["/id", "/author_id", "/singerid", "/artist_id"]);
    let id = sanitize_id(&raw_id)?;
    let raw_name = first_string(val, &["/name", "/title", "/author_name", "/singername"]);
    let name = sanitize_title(&raw_name, Some("歌手"))?;

    let avatar_url = {
        let s = first_string(val, &["/avatarUrl", "/avatar", "/pic", "/img", "/cover"]);
        sanitize_http_url(&s)
    };
    let description = {
        let s = first_string(val, &["/description", "/intro", "/profile"]);
        if s.trim().is_empty() { None } else { Some(s.trim().to_string()) }
    };
    let album_count = {
        let c = first_i64(val, &["/albumCount", "/album_count", "/albumcount"]);
        if c >= 0 { Some(c as i32) } else { None }
    };
    let track_count = {
        let c = first_i64(val, &["/trackCount", "/songCount", "/song_count", "/songcount"]);
        if c >= 0 { Some(c as i32) } else { None }
    };

    Some(CatalogArtist {
        id,
        name,
        avatarUrl: avatar_url,
        description,
        aliases: Vec::new(),
        albumCount: album_count,
        trackCount: track_count,
    })
}

pub fn to_artist_page(val: &Value) -> ArtistPage {
    let mut items = Vec::new();
    let root = if let Some(data) = val.get("data") { data } else { val };

    let arr = if let Some(arr) = root.as_array() {
        Some(arr)
    } else if let Some(arr) = val.as_array() {
        Some(arr)
    } else {
        root.get("artists")
            .or_else(|| root.get("list"))
            .or_else(|| root.get("items"))
            .or_else(|| root.get("info"))
            .or_else(|| root.get("singers"))
            .and_then(Value::as_array)
    };

    if let Some(arr) = arr {
        for item in arr {
            if let Some(a) = to_artist(item) {
                items.push(a);
            }
        }
    }

    let total = first_i64(root, &["/total", "/count"]);
    let total_opt = if total > 0 {
        Some(total.max(items.len() as i64))
    } else {
        None
    };
    let has_more = root.get("hasMore").and_then(Value::as_bool).or(Some(items.len() >= 20));

    ArtistPage {
        items,
        total: total_opt,
        hasMore: has_more,
    }
}

pub fn clamp_artist_page(mut page: ArtistPage, limit: i32) -> ArtistPage {
    if limit > 0 && page.items.len() > limit as usize {
        page.items.truncate(limit as usize);
    }
    if let Some(total) = page.total {
        if total < page.items.len() as i64 {
            page.total = Some(page.items.len() as i64);
        }
    }
    page
}

pub fn to_banner(val: &Value) -> Option<Banner> {
    let raw_id = first_string(val, &["/id", "/bannerid", "/extra/id"]);
    let image_url = first_string(val, &["/imageUrl", "/pic", "/img", "/coverUrl", "/picUrl"]);
    let sanitized_img = sanitize_http_url(&image_url)?;
    let title = first_string(val, &["/title", "/name"]);
    let target_type = first_i64(val, &["/targetType", "/type", "/banner_type"]) as i32;
    let target_id = first_string(val, &["/targetId", "/extra/id", "/id"]);

    let id = if raw_id.trim().is_empty() {
        sanitized_img.clone()
    } else {
        raw_id
    };

    Some(Banner {
        id,
        imageUrl: sanitized_img,
        title,
        targetType: target_type,
        targetId: target_id,
    })
}

pub fn to_ranking(val: &Value) -> Option<Ranking> {
    let raw_id = first_string(val, &["/id", "/rankid", "/topid"]);
    let id = sanitize_id(&raw_id)?;
    let raw_title = first_string(val, &["/title", "/name", "/rankname"]);
    let title = sanitize_title(&raw_title, Some("榜单"))?;

    let cover_url = {
        let s = first_string(val, &["/coverUrl", "/pic", "/img", "/cover"]);
        sanitize_http_url(&s)
    };
    let update_frequency = {
        let s = first_string(val, &["/updateFrequency", "/update_frequency", "/period"]);
        if s.trim().is_empty() { None } else { Some(s.trim().to_string()) }
    };
    let track_count = first_i64(val, &["/trackCount", "/total", "/count"]).max(0) as i32;

    Some(Ranking {
        id,
        title,
        coverUrl: cover_url,
        updateFrequency: update_frequency,
        trackCount: track_count,
    })
}

pub fn to_comment_item(val: &Value) -> Option<CommentItem> {
    let id = first_i64(val, &["/id", "/comment_id", "/childrenid"]);
    let content = first_string(val, &["/content", "/msg"]);
    let user = first_string(val, &["/user", "/user_name", "/nickname", "/username"]);
    let avatar = {
        let s = first_string(val, &["/avatar", "/user_pic", "/userpic"]);
        sanitize_http_url(&s).unwrap_or_default()
    };
    let time = first_string(val, &["/time", "/addtime"]);
    let liked_count = first_i64(val, &["/likedCount", "/like_count", "/likenum"]).max(0) as i32;
    let liked = val.get("liked").and_then(Value::as_bool).unwrap_or(false);
    let reply_count = first_i64(val, &["/replyCount", "/reply_num"]).max(0) as i32;
    let user_id = first_i64(val, &["/userId", "/user_id", "/uid"]);
    let time_ms = first_i64(val, &["/timeMs", "/timestamp"]);

    Some(CommentItem {
        id: if id > 0 { id } else { 1 },
        content,
        user,
        avatar,
        time,
        likedCount: liked_count,
        liked,
        replyCount: reply_count,
        beReplied: Vec::new(),
        userId: user_id,
        timeMs: time_ms,
        parentCommentId: 0,
        ipLocation: first_string(val, &["/ipLocation", "/ip_location", "/location"]),
        isHot: val.get("isHot").and_then(Value::as_bool).unwrap_or(false),
        topReplies: Vec::new(),
    })
}
