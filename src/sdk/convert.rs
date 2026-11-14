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

fn explicit_i64(root: &Value, paths: &[&str]) -> Option<i64> {
    paths
        .iter()
        .find_map(|path| root.pointer(path).and_then(value_to_i64))
}

fn explicit_bool(root: &Value, paths: &[&str]) -> Option<bool> {
    paths.iter().find_map(|path| {
        root.pointer(path).and_then(|value| match value {
            Value::Bool(value) => Some(*value),
            Value::Number(value) => value.as_i64().map(|value| value != 0),
            Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
                "true" | "1" => Some(true),
                "false" | "0" => Some(false),
                _ => None,
            },
            _ => None,
        })
    })
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let end = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= max_bytes)
        .last()
        .unwrap_or(0);
    &value[..end]
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
    if trimmed.is_empty()
        || trimmed.chars().count() > 1024
        || trimmed.chars().any(|c| (c as u32) < 32 || c as u32 == 127)
    {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn sanitize_title(s: &str, fallback: Option<&str>) -> Option<String> {
    let trimmed = s.trim();
    if !trimmed.is_empty() {
        Some(truncate_utf8(trimmed, 4096).to_string())
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
                name: truncate_utf8(s, 4096).to_string(),
            })
            .collect()
    };

    if artists.len() > 64 {
        artists.truncate(64);
    }

    let raw_duration = explicit_i64(val, &["/duration"]);
    let mut duration_ms = if let Some(duration) = raw_duration {
        if duration >= 10_000 {
            duration
        } else {
            duration.saturating_mul(1000)
        }
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

    let total = explicit_i64(root, &["/songCount", "/total", "/count"]);
    let has_more = explicit_bool(
        root,
        &["/hasMore", "/has_more", "/hasNext", "/has_next", "/more"],
    );

    TrackPage {
        items,
        total,
        hasMore: has_more,
    }
}

pub fn clamp_track_page(mut page: TrackPage, limit: i32) -> TrackPage {
    if limit > 0 && page.items.len() > limit as usize {
        page.items.truncate(limit as usize);
    }
    page
}

pub fn to_playlist(val: &Value) -> Option<Playlist> {
    let raw_id = first_string(val, &["/id", "/specialid", "/listid", "/global_collection_id"]);
    let id = sanitize_id(&raw_id)?;
    let raw_title = first_string(val, &["/title", "/name", "/specialname", "/listname"]);
    let title = sanitize_title(&raw_title, None)?;

    let cover_url = {
        let s = first_string(val, &["/coverUrl", "/coverImgUrl", "/picUrl", "/pic", "/img", "/flexible_cover"]);
        sanitize_http_url(&s)
    };
    let track_count = explicit_i64(val, &["/trackCount", "/total", "/count", "/songcount"])
        .and_then(|count| i32::try_from(count).ok())
        .filter(|count| *count >= 0);
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
            Some(truncate_utf8(trimmed, 65536).to_string())
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

    let total = explicit_i64(root, &["/total", "/count"]);
    let has_more = explicit_bool(
        root,
        &["/hasMore", "/has_more", "/hasNext", "/has_next", "/more"],
    );

    PlaylistPage {
        items,
        total,
        hasMore: has_more,
    }
}

pub fn clamp_playlist_page(mut page: PlaylistPage, limit: i32) -> PlaylistPage {
    if limit > 0 && page.items.len() > limit as usize {
        page.items.truncate(limit as usize);
    }
    page
}

pub fn to_album(val: &Value) -> Option<Album> {
    let raw_id = first_string(val, &["/id", "/album_id", "/albumid"]);
    let id = sanitize_id(&raw_id)?;
    let raw_title = first_string(val, &["/name", "/title", "/album_name", "/albumname"]);
    let title = sanitize_title(&raw_title, None)?;

    let cover_url = {
        let s = first_string(val, &["/coverUrl", "/pic", "/img", "/cover", "/sizable_cover"]);
        sanitize_http_url(&s)
    };
    let track_count = explicit_i64(val, &["/trackCount", "/song_count", "/total"])
        .and_then(|count| i32::try_from(count).ok())
        .filter(|count| *count >= 0);
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
        Vec::new()
    } else {
        vec![Artist {
            id: None,
            name: truncate_utf8(&artist_str, 4096).to_string(),
        }]
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

    let total = explicit_i64(root, &["/total", "/count"]);
    let has_more = explicit_bool(
        root,
        &["/hasMore", "/has_more", "/hasNext", "/has_next", "/more"],
    );

    AlbumPage {
        items,
        total,
        hasMore: has_more,
    }
}

pub fn clamp_album_page(mut page: AlbumPage, limit: i32) -> AlbumPage {
    if limit > 0 && page.items.len() > limit as usize {
        page.items.truncate(limit as usize);
    }
    page
}

pub fn to_artist(val: &Value) -> Option<CatalogArtist> {
    let raw_id = first_string(val, &["/id", "/author_id", "/singerid", "/artist_id"]);
    let id = sanitize_id(&raw_id)?;
    let raw_name = first_string(val, &["/name", "/title", "/author_name", "/singername"]);
    let name = sanitize_title(&raw_name, None)?;

    let avatar_url = {
        let s = first_string(val, &["/avatarUrl", "/avatar", "/pic", "/img", "/cover"]);
        sanitize_http_url(&s)
    };
    let description = {
        let s = first_string(val, &["/description", "/intro", "/profile"]);
        if s.trim().is_empty() { None } else { Some(s.trim().to_string()) }
    };
    let album_count = explicit_i64(val, &["/albumCount", "/album_count", "/albumcount"])
        .and_then(|count| i32::try_from(count).ok())
        .filter(|count| *count >= 0);
    let track_count = explicit_i64(
        val,
        &["/trackCount", "/songCount", "/song_count", "/songcount"],
    )
    .and_then(|count| i32::try_from(count).ok())
    .filter(|count| *count >= 0);

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

    let total = explicit_i64(root, &["/total", "/count"]);
    let has_more = explicit_bool(
        root,
        &["/hasMore", "/has_more", "/hasNext", "/has_next", "/more"],
    );

    ArtistPage {
        items,
        total,
        hasMore: has_more,
    }
}

pub fn clamp_artist_page(mut page: ArtistPage, limit: i32) -> ArtistPage {
    if limit > 0 && page.items.len() > limit as usize {
        page.items.truncate(limit as usize);
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
    let id = first_i64(
        val,
        &["/commentId", "/comment_id", "/cid", "/id", "/childrenid"],
    );
    if id <= 0 {
        return None;
    }
    let content = first_string(val, &["/content", "/comment", "/text", "/msg"]);
    let user = first_string(
        val,
        &[
            "/user/nickname",
            "/user/name",
            "/user",
            "/user_name",
            "/nickname",
            "/username",
        ],
    );
    let avatar = {
        let s = first_string(
            val,
            &[
                "/user/avatarUrl",
                "/user/avatar",
                "/avatarUrl",
                "/avatar",
                "/user_pic",
                "/userpic",
            ],
        );
        sanitize_http_url(&s).unwrap_or_default()
    };
    let time = {
        let text = first_string(val, &["/timeStr", "/time_string", "/addtime_str"]);
        if text.is_empty() {
            first_string(val, &["/time", "/addtime", "/create_time"])
        } else {
            text
        }
    };
    let liked_count = first_i64(
        val,
        &["/likedCount", "/like_count", "/likenum", "/praise_count"],
    )
    .max(0) as i32;
    let liked = val
        .get("liked")
        .or_else(|| val.get("is_like"))
        .and_then(|value| match value {
            Value::Bool(value) => Some(*value),
            Value::Number(value) => value.as_i64().map(|value| value == 1),
            _ => None,
        })
        .unwrap_or(false);
    let reply_count = first_i64(
        val,
        &["/replyCount", "/reply_count", "/reply_num", "/replynum"],
    )
    .max(0) as i32;
    let user_id = first_i64(
        val,
        &["/user/userId", "/user/userid", "/userId", "/user_id", "/uid"],
    );
    let time_ms = first_i64(val, &["/timeMs", "/timestamp", "/time"]);
    let parent_comment_id = first_i64(
        val,
        &[
            "/parentCommentId",
            "/parent_comment_id",
            "/parent_id",
            "/pid",
        ],
    );

    Some(CommentItem {
        id,
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
        parentCommentId: parent_comment_id,
        ipLocation: first_string(val, &["/ipLocation", "/ip_location", "/location"]),
        isHot: val
            .get("isHot")
            .or_else(|| val.get("is_hot"))
            .and_then(|value| match value {
                Value::Bool(value) => Some(*value),
                Value::Number(value) => value.as_i64().map(|value| value == 1),
                _ => None,
            })
            .unwrap_or(false),
        topReplies: Vec::new(),
    })
}
