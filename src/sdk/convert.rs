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

pub fn to_track(val: &Value) -> Option<Track> {
    let id = first_string(val, &["/id", "/hash", "/FileHash", "/audio_id"]);
    if id.is_empty() {
        return None;
    }
    let title = first_string(val, &["/title", "/name", "/SongName", "/audio_name", "/filename"]);
    let artist_str = first_string(val, &["/artist", "/SingerName", "/author_name"]);
    let artists = if artist_str.is_empty() {
        Vec::new()
    } else {
        artist_str
            .split(" / ")
            .map(|s| Artist {
                id: None,
                name: s.trim().to_string(),
            })
            .collect()
    };

    let dur_sec = first_i64(val, &["/duration"]);
    let duration_ms = if dur_sec > 0 {
        dur_sec * 1000
    } else {
        first_i64(val, &["/timelen", "/durationMs"])
    };

    let album = {
        let s = first_string(val, &["/album", "/AlbumName", "/album_name"]);
        if s.is_empty() { None } else { Some(s) }
    };

    let cover_url = {
        let s = first_string(val, &["/coverUrl", "/coverImgUrl", "/picUrl", "/cover", "/img", "/pic", "/Image"]);
        if s.is_empty() { None } else { Some(s.replace("{size}", "400")) }
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

    let arr = root
        .get("tracks")
        .or_else(|| root.get("list"))
        .or_else(|| root.get("songs"))
        .or_else(|| root.get("items"))
        .or_else(|| root.get("info"))
        .and_then(Value::as_array);

    if let Some(arr) = arr {
        for item in arr {
            if let Some(t) = to_track(item) {
                items.push(t);
            }
        }
    }

    let total = first_i64(root, &["/total", "/songCount", "/count"]);
    let total_opt = if total > 0 { Some(total) } else { None };
    let has_more = root.get("hasMore").and_then(Value::as_bool).or(Some(items.len() >= 20));

    TrackPage {
        items,
        total: total_opt,
        hasMore: has_more,
    }
}

pub fn to_playlist(val: &Value) -> Option<Playlist> {
    let id = first_string(val, &["/id", "/specialid", "/listid", "/global_collection_id"]);
    if id.is_empty() {
        return None;
    }
    let title = first_string(val, &["/title", "/name", "/specialname", "/listname"]);
    let cover_url = {
        let s = first_string(val, &["/coverUrl", "/coverImgUrl", "/picUrl", "/pic", "/img", "/flexible_cover"]);
        if s.is_empty() { None } else { Some(s.replace("{size}", "400")) }
    };
    let track_count = {
        let c = first_i64(val, &["/trackCount", "/total", "/count", "/songcount"]);
        if c > 0 { Some(c as i32) } else { None }
    };
    let creator_id = {
        let s = first_string(val, &["/creator/userId", "/userid", "/uid"]);
        if s.is_empty() { None } else { Some(s) }
    };
    let creator_name = {
        let s = first_string(val, &["/author", "/creator/nickname", "/nickname", "/username"]);
        if s.is_empty() { None } else { Some(s) }
    };
    let description = {
        let s = first_string(val, &["/description", "/intro"]);
        if s.is_empty() { None } else { Some(s) }
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

    let arr = root
        .get("playlists")
        .or_else(|| root.get("list"))
        .or_else(|| root.get("items"))
        .or_else(|| root.get("info"))
        .or_else(|| root.get("special_list"))
        .and_then(Value::as_array);

    if let Some(arr) = arr {
        for item in arr {
            if let Some(p) = to_playlist(item) {
                items.push(p);
            }
        }
    }

    let total = first_i64(root, &["/total", "/count"]);
    let total_opt = if total > 0 { Some(total) } else { None };
    let has_more = root.get("hasMore").and_then(Value::as_bool).or(Some(items.len() >= 20));

    PlaylistPage {
        items,
        total: total_opt,
        hasMore: has_more,
    }
}

pub fn to_album(val: &Value) -> Option<Album> {
    let id = first_string(val, &["/id", "/album_id", "/albumid"]);
    if id.is_empty() {
        return None;
    }
    let title = first_string(val, &["/name", "/title", "/album_name", "/albumname"]);
    let cover_url = {
        let s = first_string(val, &["/coverUrl", "/pic", "/img", "/cover", "/sizable_cover"]);
        if s.is_empty() { None } else { Some(s.replace("{size}", "400")) }
    };
    let track_count = {
        let c = first_i64(val, &["/trackCount", "/song_count", "/total"]);
        if c > 0 { Some(c as i32) } else { None }
    };
    let company = {
        let s = first_string(val, &["/company", "/publish_company"]);
        if s.is_empty() { None } else { Some(s) }
    };
    let description = {
        let s = first_string(val, &["/description", "/intro"]);
        if s.is_empty() { None } else { Some(s) }
    };
    let artist_str = first_string(val, &["/artist", "/author_name", "/singername"]);
    let artists = if artist_str.is_empty() {
        Vec::new()
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

    let arr = root
        .get("albums")
        .or_else(|| root.get("list"))
        .or_else(|| root.get("items"))
        .or_else(|| root.get("info"))
        .and_then(Value::as_array);

    if let Some(arr) = arr {
        for item in arr {
            if let Some(a) = to_album(item) {
                items.push(a);
            }
        }
    }

    let total = first_i64(root, &["/total", "/count"]);
    let total_opt = if total > 0 { Some(total) } else { None };
    let has_more = root.get("hasMore").and_then(Value::as_bool).or(Some(items.len() >= 20));

    AlbumPage {
        items,
        total: total_opt,
        hasMore: has_more,
    }
}

pub fn to_artist(val: &Value) -> Option<CatalogArtist> {
    let id = first_string(val, &["/id", "/author_id", "/singerid", "/artist_id"]);
    if id.is_empty() {
        return None;
    }
    let name = first_string(val, &["/name", "/title", "/author_name", "/singername"]);
    let avatar_url = {
        let s = first_string(val, &["/avatarUrl", "/avatar", "/pic", "/img", "/cover"]);
        if s.is_empty() { None } else { Some(s.replace("{size}", "400")) }
    };
    let description = {
        let s = first_string(val, &["/description", "/intro", "/profile"]);
        if s.is_empty() { None } else { Some(s) }
    };
    let album_count = {
        let c = first_i64(val, &["/albumCount", "/album_count", "/albumcount"]);
        if c > 0 { Some(c as i32) } else { None }
    };
    let track_count = {
        let c = first_i64(val, &["/trackCount", "/songCount", "/song_count", "/songcount"]);
        if c > 0 { Some(c as i32) } else { None }
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

    let arr = root
        .get("artists")
        .or_else(|| root.get("list"))
        .or_else(|| root.get("items"))
        .or_else(|| root.get("info"))
        .or_else(|| root.get("singers"))
        .and_then(Value::as_array);

    if let Some(arr) = arr {
        for item in arr {
            if let Some(a) = to_artist(item) {
                items.push(a);
            }
        }
    }

    let total = first_i64(root, &["/total", "/count"]);
    let total_opt = if total > 0 { Some(total) } else { None };
    let has_more = root.get("hasMore").and_then(Value::as_bool).or(Some(items.len() >= 20));

    ArtistPage {
        items,
        total: total_opt,
        hasMore: has_more,
    }
}

pub fn to_banner(val: &Value) -> Option<Banner> {
    let id = first_string(val, &["/id", "/bannerid", "/extra/id"]);
    let image_url = first_string(val, &["/imageUrl", "/pic", "/img", "/coverUrl", "/picUrl"]);
    if image_url.is_empty() {
        return None;
    }
    let title = first_string(val, &["/title", "/name"]);
    let target_type = first_i64(val, &["/targetType", "/type", "/banner_type"]) as i32;
    let target_id = first_string(val, &["/targetId", "/extra/id", "/id"]);

    Some(Banner {
        id: if id.is_empty() { image_url.clone() } else { id },
        imageUrl: image_url.replace("{size}", "750"),
        title,
        targetType: target_type,
        targetId: target_id,
    })
}

pub fn to_ranking(val: &Value) -> Option<Ranking> {
    let id = first_string(val, &["/id", "/rankid", "/topid"]);
    if id.is_empty() {
        return None;
    }
    let title = first_string(val, &["/title", "/name", "/rankname"]);
    let cover_url = {
        let s = first_string(val, &["/coverUrl", "/pic", "/img", "/cover"]);
        if s.is_empty() { None } else { Some(s.replace("{size}", "400")) }
    };
    let update_frequency = {
        let s = first_string(val, &["/updateFrequency", "/update_frequency", "/period"]);
        if s.is_empty() { None } else { Some(s) }
    };
    let track_count = first_i64(val, &["/trackCount", "/total", "/count"]) as i32;

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
    let avatar = first_string(val, &["/avatar", "/user_pic", "/userpic"]).replace("{size}", "200");
    let time = first_string(val, &["/time", "/addtime"]);
    let liked_count = first_i64(val, &["/likedCount", "/like_count", "/likenum"]) as i32;
    let liked = val.get("liked").and_then(Value::as_bool).unwrap_or(false);
    let reply_count = first_i64(val, &["/replyCount", "/reply_num"]) as i32;
    let user_id = first_i64(val, &["/userId", "/user_id", "/uid"]);
    let time_ms = first_i64(val, &["/timeMs", "/timestamp"]);

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
        parentCommentId: 0,
        ipLocation: first_string(val, &["/ipLocation", "/ip_location", "/location"]),
        isHot: val.get("isHot").and_then(Value::as_bool).unwrap_or(false),
        topReplies: Vec::new(),
    })
}
