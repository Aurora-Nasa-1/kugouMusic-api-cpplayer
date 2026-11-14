use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use serde_json::{json, Value};

use crate::sdk::convert;
use crate::sdk::protocol::*;
use crate::sdk::wire::{WireReader, WireWriter};
use crate::sdk::wire_models::*;
use crate::standard_api;

pub struct KugouSession {
    pub provider_id: String,
    pub account_id: Option<String>,
    pub credential: Option<String>,
    pub settings: Option<String>,
    pub cancelled: Arc<AtomicBool>,
}

impl KugouSession {
    pub fn new(
        provider_id: String,
        account_id: Option<String>,
        credential: Option<String>,
        settings: Option<String>,
    ) -> Self {
        Self {
            provider_id,
            account_id,
            credential,
            settings,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn cookie_value(&self) -> Value {
        if let Some(cred) = &self.credential {
            if !cred.trim().is_empty() {
                if let Ok(Value::Object(map)) = serde_json::from_str(cred) {
                    return Value::Object(map);
                }
                return Value::String(cred.clone());
            }
        }
        json!({})
    }

    fn failure_bytes(category: &str, message: &str) -> Vec<u8> {
        let mut writer = WireWriter::new();
        writer.write_byte(1); // 1 = failure
        let failure = Failure {
            category: category.to_string(),
            message: message.to_string(),
        };
        failure.write(&mut writer);
        writer.finish()
    }

    fn success_bytes<T: WireWritable>(record: &T) -> Vec<u8> {
        let mut writer = WireWriter::new();
        writer.write_byte(0); // 0 = success
        record.write_to(&mut writer);
        writer.finish()
    }

    fn standard_failure(res: &Value) -> Option<(String, String)> {
        let code = res.get("code").and_then(convert::value_to_i64)?;
        if code == 0 || code == 200 {
            return None;
        }
        let message = convert::first_string(res, &["/msg", "/message", "/error"]);
        let message = if message.is_empty() {
            format!("Upstream request failed with code {code}")
        } else {
            message
        };
        let category = match code {
            -1 => "Unsupported",
            400 | 409 => "InvalidResponse",
            401 | 403 => "Authentication",
            404 => "NotFound",
            429 => "RateLimit",
            500..=599 => "Network",
            _ => "Runtime",
        };
        Some((category.to_string(), message))
    }

    fn upstream_mutation_ok(body: &Value) -> bool {
        let code = body
            .pointer("/code")
            .or_else(|| body.pointer("/data/code"))
            .and_then(convert::value_to_i64);
        if code.is_some_and(|code| code != 0 && code != 200) {
            return false;
        }
        let status = body
            .pointer("/status")
            .or_else(|| body.pointer("/data/status"))
            .and_then(convert::value_to_i64);
        if status.is_some_and(|status| status != 1 && status != 200) {
            return false;
        }
        let error = body
            .pointer("/error_code")
            .or_else(|| body.pointer("/errcode"))
            .or_else(|| body.pointer("/data/error_code"))
            .or_else(|| body.pointer("/data/errcode"))
            .and_then(convert::value_to_i64);
        if error.is_some_and(|error| error != 0) {
            return false;
        }
        code.is_some() || status == Some(1) || status == Some(200) || error == Some(0)
    }

    fn mutation_failure(body: &Value, action: &str) -> (String, String) {
        let message = convert::first_string(
            body,
            &[
                "/msg",
                "/message",
                "/error",
                "/errmsg",
                "/data/msg",
                "/data/message",
            ],
        );
        (
            "Runtime".to_string(),
            if message.is_empty() {
                format!("Upstream did not confirm {action}")
            } else {
                message
            },
        )
    }

    fn credential_value(&self, keys: &[&str]) -> Option<String> {
        let cookie = self.cookie_value();
        let object = cookie.as_object()?;
        keys.iter().find_map(|key| {
            object
                .get(*key)
                .and_then(convert::value_to_string)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty() && value != "0")
        })
    }

    fn current_user_id(&self) -> Option<String> {
        self.credential_value(&["userid", "userId", "uid"])
    }

    fn scoped_user_id(&self, requested: &str) -> Result<String, (String, String)> {
        let requested = requested.trim();
        if requested.is_empty() || requested.eq_ignore_ascii_case("self") {
            return self.current_user_id().ok_or_else(|| {
                (
                    "Authentication".to_string(),
                    "This operation requires an authenticated account".to_string(),
                )
            });
        }
        if let Some(current) = self.current_user_id() {
            if current != requested {
                return Err((
                    "Authentication".to_string(),
                    "Requested user does not match the current session".to_string(),
                ));
            }
        }
        Ok(requested.to_string())
    }

    fn page_params(limit: i32, offset: i32) -> (i64, usize) {
        let limit = if limit > 0 { limit } else { 20 };
        let page = (offset.max(0) / limit) as i64 + 1;
        (page, limit as usize)
    }

    fn page_slice<'a, T>(items: &'a [T], offset: i32, limit: usize) -> &'a [T] {
        let start = offset.max(0) as usize;
        if start >= items.len() {
            return &[];
        }
        let end = start.saturating_add(limit).min(items.len());
        &items[start..end]
    }

    fn optional_i64(value: Option<&Value>) -> Option<i64> {
        value.and_then(convert::value_to_i64)
    }

    fn optional_bool(value: Option<&Value>) -> Option<bool> {
        value.and_then(|value| match value {
            Value::Bool(value) => Some(*value),
            Value::Number(value) => value.as_i64().map(|value| value == 1),
            Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
                "true" | "1" => Some(true),
                "false" | "0" => Some(false),
                _ => None,
            },
            _ => None,
        })
    }

    fn page_has_more(
        res: &Value,
        offset: i64,
        count: i64,
        reported_total: Option<i64>,
    ) -> bool {
        Self::optional_bool(
            res.pointer("/data/hasMore")
                .or_else(|| res.pointer("/data/has_more"))
                .or_else(|| res.pointer("/data/more")),
        )
        .unwrap_or_else(|| {
            reported_total.is_some_and(|total| offset.saturating_add(count) < total)
        })
    }

    fn quality_for_bitrate(bitrate: i32) -> &'static str {
        match bitrate {
            i32::MIN..=128_000 => "standard",
            128_001..=192_000 => "higher",
            192_001..=320_000 => "exhigh",
            320_001..=1_000_000 => "lossless",
            _ => "hires",
        }
    }

    fn probe_quality(
        id: &str,
        quality: &str,
        cookie: &Value,
    ) -> Result<Option<(Option<i64>, Option<String>)>, (String, String)> {
        let params = json!({
            "id": id,
            "level": quality,
            "cookie": cookie,
        });
        let res = standard_api::handle_api("song/url", &params);
        let url = convert::first_string(&res, &["/data/url"]);
        if let Some(code) = res.get("code").and_then(convert::value_to_i64) {
            if code != 0 && code != 200 {
                // 403/404 mean this quality is not offered to the account, not
                // that the provider failed to answer.
                if matches!(code, 403 | 404) {
                    return Ok(None);
                }
                // 502 without a transport status is "upstream returned no
                // playable URL", which is an unavailable quality.
                if code == 502 && url.is_empty() && res.get("upstreamStatus").is_none() {
                    return Ok(None);
                }
                if let Some(failure) = Self::standard_failure(&res) {
                    return Err(failure);
                }
            }
        }
        if url.is_empty() {
            return Ok(None);
        }
        let actual = convert::first_string(&res, &["/data/quality"]);
        if !actual.is_empty() && actual != quality {
            return Ok(None);
        }
        let size = res
            .pointer("/data/size")
            .and_then(convert::value_to_i64)
            .filter(|size| *size > 0);
        Ok(Some((size, if actual.is_empty() { None } else { Some(actual) })))
    }

    pub fn dispatch(&self, operation: i32, request_bytes: &[u8]) -> Vec<u8> {
        if self.is_cancelled() {
            return Self::failure_bytes("Timeout", "Operation cancelled by host");
        }

        let mut r = WireReader::new(request_bytes);
        let cookie = self.cookie_value();

        match operation {
            OP_CAPABILITIES => {
                let caps = Capabilities {
                    supported: vec![
                        "tracks.search".into(),
                        "tracks.get".into(),
                        "tracks.getMany".into(),
                        "tracks.resolve".into(),
                        "lyrics.get".into(),
                        "playlists.search".into(),
                        "playlists.get".into(),
                        "playlists.tracks".into(),
                        "playlists.user".into(),
                        "albums.search".into(),
                        "albums.get".into(),
                        "artists.search".into(),
                        "artists.get".into(),
                        "artists.tracks".into(),
                        "artists.albums".into(),
                        "recommendations.songs".into(),
                        "recommendations.playlists".into(),
                        "recommendations.personalizedPlaylists".into(),
                        "recommendations.topPlaylists".into(),
                        "recommendations.highqualityPlaylists".into(),
                        "recommendations.newSongs".into(),
                        "discovery.banners".into(),
                        "discovery.rankings".into(),
                        "discovery.newAlbums".into(),
                        "discovery.topSongs".into(),
                        "discovery.hotArtists".into(),
                        "accounts.profile".into(),
                        "accounts.records".into(),
                        "accounts.cloud".into(),
                        "accounts.likes".into(),
                        "accounts.likes.page".into(),
                        "playlists.delete".into(),
                        "playlists.subscribe".into(),
                        "playlists.addTracks".into(),
                        "playlists.removeTracks".into(),
                        "playlists.created".into(),
                        "playlists.collected".into(),
                        "playlists.create".into(),
                        "library.like".into(),
                        "library.cloud.delete".into(),
                        "comments.list".into(),
                        "comments.floor".into(),
                        "comments.post".into(),
                        "recommendations.personalFm".into(),
                        "recommendations.similarSongs".into(),
                        "recommendations.intelligenceSongs".into(),
                        "discovery.hotSearches".into(),
                        "discovery.searchSuggestions".into(),
                        "accounts.session".into(),
                        "auth.actions".into(),
                        "comments.delete".into(),
                        "tracks.available".into(),
                        "media.qualities".into(),
                    ],
                };
                Self::success_bytes(&caps)
            }

            OP_SEARCH => {
                let req = match SearchRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "keywords": req.keyword,
                    "limit": req.limit,
                    "offset": req.offset,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("search", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_track_page(convert::to_track_page(&res), req.limit);
                Self::success_bytes(&page)
            }

            OP_GET => {
                let req = match IdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "hash": req.id,
                    "id": req.id,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("song/detail", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::to_track_page(&res);
                if let Some(mut first) = page.items.into_iter().next() {
                    first.id = req.id;
                    Self::success_bytes(&first)
                } else {
                    Self::failure_bytes("NotFound", &format!("Track not found: {}", req.id))
                }
            }

            OP_GETMANY => {
                let req = match BatchRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let mut unique = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for id in &req.ids {
                    let key = id.to_lowercase();
                    if seen.insert(key) {
                        unique.push(id.clone());
                    }
                }
                let mut track_map = std::collections::HashMap::new();
                let mut batch_failure = None;
                if !unique.is_empty() {
                    let params = json!({
                        "ids": unique.join(","),
                        "cookie": cookie,
                    });
                    let res = standard_api::handle_api("song/detail", &params);
                    if let Some((category, message)) = Self::standard_failure(&res) {
                        batch_failure = Some(Failure { category, message });
                    } else {
                        for track in convert::to_track_page(&res).items {
                            track_map.insert(track.id.to_lowercase(), track);
                        }
                    }
                }
                let outcomes = req
                    .ids
                    .into_iter()
                    .map(|id| {
                        if let Some(mut track) = track_map.get(&id.to_lowercase()).cloned() {
                            track.id = id.clone();
                            TrackOutcome {
                                id,
                                track: Some(track),
                                failure: None,
                            }
                        } else {
                            let failure = batch_failure.clone().unwrap_or(Failure {
                                category: "NotFound".into(),
                                message: format!("Track {id} not found"),
                            });
                            TrackOutcome {
                                id,
                                track: None,
                                failure: Some(failure),
                            }
                        }
                    })
                    .collect();
                Self::success_bytes(&TrackBatch { items: outcomes })
            }

            OP_RESOLVE => {
                let req = match ResolveRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "id": req.id,
                    "hash": req.id,
                    "level": req.quality,
                    "quality": req.quality,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("song/url", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let url = convert::first_string(&res, &["/data/url", "/data/0/url", "/url"]);
                if url.is_empty() {
                    return Self::failure_bytes("NotFound", "Failed to resolve track audio stream");
                }
                let data = res.get("data");
                let actual_quality = {
                    let q = convert::first_string(&res, &["/data/quality", "/data/0/quality", "/quality"]);
                    if q.is_empty() { req.quality } else { q }
                };
                let mut headers = Vec::new();
                if let Some(h) = data.and_then(|d| d.get("headers")).and_then(Value::as_object) {
                    for (k, v) in h {
                        if let Some(vs) = v.as_str() {
                            headers.push(Header {
                                name: k.clone(),
                                value: vs.to_string(),
                            });
                        }
                    }
                }
                let format = data.and_then(|d| d.get("format")).and_then(Value::as_str).map(String::from);
                let bitrate = data.and_then(|d| d.get("bitrate")).and_then(Value::as_i64).map(|n| n as i32);
                let size_bytes = data.and_then(|d| d.get("size")).and_then(Value::as_i64);

                Self::success_bytes(&MediaResource {
                    url: url.to_string(),
                    actualQuality: actual_quality,
                    headers,
                    expiresAtEpochMs: None,
                    format,
                    bitrate,
                    sizeBytes: size_bytes,
                })
            }

            OP_GETLYRICS => {
                let req = match IdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "hash": req.id,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("lyric", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let data = res.get("data");
                let lrc = data.and_then(|d| d.get("lyric")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
                let translation = data.and_then(|d| d.get("tlyric")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
                let yrc = data.and_then(|d| d.get("klyric")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);

                if lrc.is_none() && yrc.is_none() {
                    return Self::failure_bytes(
                        "NotFound",
                        &format!("Lyrics not found: {}", req.id),
                    );
                }
                Self::success_bytes(&Lyrics {
                    id: req.id,
                    lrc,
                    ttml: None,
                    yrc,
                    translation,
                    romanization: None,
                })
            }

            OP_SEARCHPLAYLISTS => {
                let req = match SearchRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let page_no = (req.offset / req.limit.max(1)) + 1;
                let params = json!({
                    "keywords": req.keyword,
                    "pagesize": req.limit,
                    "page": page_no,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("search/special", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_playlist_page(convert::to_playlist_page(&res), req.limit);
                Self::success_bytes(&page)
            }

            OP_GETPLAYLIST => {
                let req = match IdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "id": req.id,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("playlist/detail", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let Some(mut pl) = convert::to_playlist(&res.get("data").unwrap_or(&res)) else {
                    return Self::failure_bytes(
                        "NotFound",
                        &format!("Playlist not found: {}", req.id),
                    );
                };
                pl.id = req.id;
                Self::success_bytes(&pl)
            }

            OP_GETPLAYLISTTRACKS => {
                let req = match PagedIdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "id": req.id,
                    "limit": req.limit,
                    "offset": req.offset,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("playlist/track/all", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_track_page(convert::to_track_page(&res), req.limit);
                Self::success_bytes(&page)
            }

            OP_GETUSERPLAYLISTS => {
                let req = match PagedIdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let userid = match self.scoped_user_id(&req.id) {
                    Ok(userid) => userid,
                    Err((category, message)) => return Self::failure_bytes(&category, &message),
                };
                let params = json!({
                    "uid": userid,
                    "userid": userid,
                    "limit": req.limit,
                    "offset": req.offset,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("user/playlist", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_playlist_page(convert::to_playlist_page(&res), req.limit);
                Self::success_bytes(&page)
            }

            OP_GETCREATEDPLAYLISTS => {
                let req = match PagedIdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let userid = match self.scoped_user_id(&req.id) {
                    Ok(userid) => userid,
                    Err((category, message)) => return Self::failure_bytes(&category, &message),
                };
                let params = json!({
                    "uid": userid,
                    "userid": userid,
                    "limit": req.limit,
                    "offset": req.offset,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("user/playlist/create", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_playlist_page(convert::to_playlist_page(&res), req.limit);
                Self::success_bytes(&page)
            }

            OP_GETCOLLECTEDPLAYLISTS => {
                let req = match PagedIdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let userid = match self.scoped_user_id(&req.id) {
                    Ok(userid) => userid,
                    Err((category, message)) => return Self::failure_bytes(&category, &message),
                };
                let params = json!({
                    "uid": userid,
                    "userid": userid,
                    "limit": req.limit,
                    "offset": req.offset,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("user/playlist/collect", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_playlist_page(convert::to_playlist_page(&res), req.limit);
                Self::success_bytes(&page)
            }

            OP_CREATEPLAYLIST => {
                let req = match CreatePlaylistRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "name": req.name,
                    "privacy": req.privacy,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("playlist/create", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let id = convert::first_string(&res, &["/data/id", "/id", "/listid"]);
                if id.is_empty() {
                    return Self::failure_bytes(
                        "Runtime",
                        "Playlist creation returned no playlist id",
                    );
                }
                Self::success_bytes(&MutationResult {
                    success: true,
                    id: Some(id),
                })
            }

            OP_DELETEPLAYLIST => {
                let req = match IdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "id": req.id,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("playlist/delete", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                if !Self::upstream_mutation_ok(&res) {
                    let (category, message) = Self::mutation_failure(&res, "playlist deletion");
                    return Self::failure_bytes(&category, &message);
                }
                Self::success_bytes(&MutationResult {
                    success: true,
                    id: Some(req.id),
                })
            }

            OP_SUBSCRIBEPLAYLIST => {
                let req = match PlaylistSubscribeRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "id": req.playlistId,
                    "t": if req.subscribed { 1 } else { 2 },
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("playlist/subscribe", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                if !Self::upstream_mutation_ok(&res) {
                    let (category, message) =
                        Self::mutation_failure(&res, "playlist subscription update");
                    return Self::failure_bytes(&category, &message);
                }
                Self::success_bytes(&MutationResult {
                    success: true,
                    id: Some(req.playlistId),
                })
            }

            OP_ADDTRACKSTOPLAYLIST => {
                let req = match PlaylistTracksRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "op": "add",
                    "pid": req.playlistId,
                    "tracks": req.trackIds.join(","),
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("playlist/tracks", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                if !Self::upstream_mutation_ok(&res) {
                    let (category, message) =
                        Self::mutation_failure(&res, "adding tracks to the playlist");
                    return Self::failure_bytes(&category, &message);
                }
                Self::success_bytes(&MutationResult {
                    success: true,
                    id: Some(req.playlistId),
                })
            }

            OP_REMOVETRACKSFROMPLAYLIST => {
                let req = match PlaylistTracksRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "op": "del",
                    "pid": req.playlistId,
                    "tracks": req.trackIds.join(","),
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("playlist/tracks", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                if !Self::upstream_mutation_ok(&res) {
                    let (category, message) =
                        Self::mutation_failure(&res, "removing tracks from the playlist");
                    return Self::failure_bytes(&category, &message);
                }
                Self::success_bytes(&MutationResult {
                    success: true,
                    id: Some(req.playlistId),
                })
            }

            OP_RENAMEPLAYLIST => {
                if let Err(e) = PlaylistNameRequest::read(&mut r) {
                    return Self::failure_bytes("InvalidResponse", &e);
                }
                Self::failure_bytes(
                    "Unsupported",
                    "Kugou playlist renaming is not available through the public API",
                )
            }

            OP_SEARCHALBUMS => {
                let req = match SearchRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let page_no = (req.offset / req.limit.max(1)) + 1;
                let params = json!({
                    "keywords": req.keyword,
                    "pagesize": req.limit,
                    "page": page_no,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("search/album", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_album_page(convert::to_album_page(&res), req.limit);
                Self::success_bytes(&page)
            }

            OP_GETALBUM => {
                let req = match IdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "id": req.id,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("album/detail", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let Some(mut album) = convert::to_album(&res.get("data").unwrap_or(&res)) else {
                    return Self::failure_bytes(
                        "NotFound",
                        &format!("Album not found: {}", req.id),
                    );
                };
                album.id = req.id;
                let tracks = convert::to_track_page(&res).items;
                Self::success_bytes(&AlbumDetails { album, tracks })
            }

            OP_GETNEWALBUMS | OP_BROWSENEWALBUMS | OP_GETLATESTALBUMS => {
                let (area, limit) = if operation == OP_BROWSENEWALBUMS {
                    match AlbumPageRequest::read(&mut r) {
                        Ok(value) => (value.area, value.limit),
                        Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                    }
                } else if operation == OP_GETNEWALBUMS {
                    match AlbumBrowseRequest::read(&mut r) {
                        Ok(value) => (value.area, value.limit),
                        Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                    }
                } else {
                    if let Err(e) = Bytes::read(&mut r) {
                        return Self::failure_bytes("InvalidResponse", &e);
                    }
                    (String::new(), 20)
                };
                let limit = if limit > 0 { limit } else { 20 };
                let params = json!({
                    "area": area,
                    "limit": limit,
                    "cookie": cookie
                });
                let res = standard_api::handle_api("top_album", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_album_page(convert::to_album_page(&res), limit);
                Self::success_bytes(&page)
            }

            OP_SUBSCRIBEALBUM => {
                if let Err(e) = AlbumSubscribeRequest::read(&mut r) {
                    return Self::failure_bytes("InvalidResponse", &e);
                }
                Self::failure_bytes(
                    "Unsupported",
                    "Kugou album subscription is not available through the public API",
                )
            }

            OP_GETUSERALBUMS => Self::failure_bytes(
                "Unsupported",
                "Kugou user-collected album pagination is not available",
            ),

            OP_SEARCHARTISTS => {
                let req = match SearchRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let page_no = (req.offset / req.limit.max(1)) + 1;
                let params = json!({
                    "keywords": req.keyword,
                    "pagesize": req.limit,
                    "page": page_no,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("search/author", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_artist_page(convert::to_artist_page(&res), req.limit);
                Self::success_bytes(&page)
            }

            OP_GETARTIST => {
                let req = match IdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "id": req.id,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("artist/detail", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let Some(mut artist) = convert::to_artist(&res.get("data").unwrap_or(&res)) else {
                    return Self::failure_bytes(
                        "NotFound",
                        &format!("Artist not found: {}", req.id),
                    );
                };
                artist.id = req.id;
                Self::success_bytes(&artist)
            }

            OP_GETARTISTTRACKS => {
                let req = match PagedIdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "id": req.id,
                    "limit": req.limit,
                    "offset": req.offset,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("artist/songs", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_track_page(convert::to_track_page(&res), req.limit);
                Self::success_bytes(&page)
            }

            OP_GETARTISTALBUMS => {
                let req = match PagedIdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "id": req.id,
                    "limit": req.limit,
                    "offset": req.offset,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("artist/album", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_album_page(convert::to_album_page(&res), req.limit);
                Self::success_bytes(&page)
            }

            OP_GETHOTARTISTS => {
                let req = RecommendationsRequest::read(&mut r).unwrap_or(RecommendationsRequest { limit: 20 });
                let params = json!({ "limit": req.limit, "cookie": cookie });
                let res = standard_api::handle_api("top_artists", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_artist_page(convert::to_artist_page(&res), req.limit);
                Self::success_bytes(&page)
            }

            OP_GETRECOMMENDEDSONGS => {
                let req = RecommendationsRequest::read(&mut r).unwrap_or(RecommendationsRequest { limit: 20 });
                let params = json!({ "limit": req.limit, "cookie": cookie });
                let res = standard_api::handle_api("recommend/songs", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_track_page(convert::to_track_page(&res), req.limit);
                Self::success_bytes(&page)
            }

            OP_GETRECOMMENDEDPLAYLISTS | OP_GETPERSONALIZEDPLAYLISTS => {
                let req = RecommendationsRequest::read(&mut r).unwrap_or(RecommendationsRequest { limit: 20 });
                let params = json!({ "limit": req.limit, "cookie": cookie });
                let res = standard_api::handle_api("personalized", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_playlist_page(convert::to_playlist_page(&res), req.limit);
                Self::success_bytes(&page)
            }

            OP_GETTOPPLAYLISTS | OP_GETHIGHQUALITYPLAYLISTS => {
                let req = PlaylistBrowseRequest::read(&mut r).unwrap_or_default();
                let limit = if req.limit > 0 { req.limit } else { 20 };
                let api_name = if operation == OP_GETHIGHQUALITYPLAYLISTS {
                    "top_playlist_highquality"
                } else {
                    "top_playlist"
                };
                let params = json!({
                    "limit": limit,
                    "cat": if req.category.is_empty() { None } else { Some(req.category) },
                    "cookie": cookie
                });
                let res = standard_api::handle_api(api_name, &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_playlist_page(convert::to_playlist_page(&res), limit);
                Self::success_bytes(&page)
            }

            OP_GETPERSONALIZEDNEWSONGS => {
                let req = RecommendationsRequest::read(&mut r).unwrap_or(RecommendationsRequest { limit: 20 });
                let limit = if req.limit > 0 { req.limit } else { 20 };
                let params = json!({ "limit": limit, "cookie": cookie });
                let res = standard_api::handle_api("personalized_newsong", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_track_page(convert::to_track_page(&res), limit);
                Self::success_bytes(&page)
            }

            OP_GETTOPSONGS => {
                let req = TopSongsRequest::read(&mut r).unwrap_or_default();
                let limit = if req.limit > 0 { req.limit } else { 50 };
                let params = json!({ "type": req.region, "cookie": cookie });
                let res = standard_api::handle_api("top_song", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_track_page(convert::to_track_page(&res), limit);
                Self::success_bytes(&page)
            }

            OP_GETPERSONALFM => {
                let _ = Bytes::read(&mut r);
                let params = json!({ "cookie": cookie });
                let res = standard_api::handle_api("personal_fm", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_track_page(convert::to_track_page(&res), 30);
                Self::success_bytes(&page)
            }

            OP_GETSIMILARSONGS => {
                let req = match SimilarSongsRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "id": req.id,
                    "hash": req.id,
                    "limit": req.limit,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("simi_song", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_track_page(convert::to_track_page(&res), req.limit);
                Self::success_bytes(&page)
            }

            OP_GETINTELLIGENCESONGS => {
                let req = match IntelligenceSongsRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let limit = if req.limit > 0 { req.limit } else { 20 };
                let params = json!({
                    "id": req.id,
                    "hash": req.id,
                    "pid": req.playlistId,
                    "count": limit,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("playmode/intelligence/list", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                Self::success_bytes(&convert::clamp_track_page(
                    convert::to_track_page(&res),
                    limit,
                ))
            }

            OP_GETBANNERS => {
                let params = json!({ "cookie": cookie });
                let res = standard_api::handle_api("banner", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let mut items = Vec::new();
                if let Some(arr) = res.pointer("/data/banners").or_else(|| res.get("data")).and_then(Value::as_array) {
                    for item in arr {
                        if let Some(b) = convert::to_banner(item) {
                            items.push(b);
                        }
                    }
                }
                Self::success_bytes(&BannerPage { items })
            }

            OP_GETRANKINGS => {
                let params = json!({ "cookie": cookie });
                let res = standard_api::handle_api("toplist", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let mut items = Vec::new();
                if let Some(arr) = res.pointer("/data/list").or_else(|| res.get("data")).and_then(Value::as_array) {
                    for item in arr {
                        if let Some(rk) = convert::to_ranking(item) {
                            items.push(rk);
                        }
                    }
                }
                Self::success_bytes(&RankingPage { items })
            }

            OP_GETHOTSEARCHES => {
                let params = json!({ "cookie": cookie });
                let res = standard_api::handle_api("search/hot", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let mut items = Vec::new();
                if let Some(arr) = res.get("data").and_then(Value::as_array) {
                    for item in arr {
                        let kw = convert::first_string(item, &["/searchWord", "/keyword", "/word"]);
                        if !kw.is_empty() {
                            let desc = convert::first_string(item, &["/content", "/desc"]);
                            items.push(Keyword {
                                keyword: kw,
                                description: desc,
                            });
                        }
                    }
                }
                Self::success_bytes(&KeywordPage { items })
            }

            OP_GETSEARCHSUGGESTIONS => {
                let req = match SearchSuggestionRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "keywords": req.keyword,
                    "limit": req.limit,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("search/suggest", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let mut items = Vec::new();
                if let Some(arr) = res.get("data").and_then(Value::as_array) {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            let st = s.trim();
                            if !st.is_empty() { items.push(st.to_string()); }
                        } else if let Some(w) = item.get("searchWord").and_then(Value::as_str) {
                            let wt = w.trim();
                            if !wt.is_empty() { items.push(wt.to_string()); }
                        }
                    }
                }
                let limit = if req.limit > 0 { req.limit as usize } else { 30 };
                if items.len() > limit {
                    items.truncate(limit);
                }
                Self::success_bytes(&SuggestionPage { items })
            }

            OP_GETUSERPROFILE => {
                let req = match UserRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let userid = match req.id.trim().parse::<i64>() {
                    Ok(userid) if userid > 0 => userid,
                    _ => {
                        return Self::failure_bytes(
                            "InvalidResponse",
                            "User id must be a positive 64-bit integer",
                        )
                    }
                };
                let params = json!({
                    "userid": userid,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("user/detail", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let prof = res
                    .pointer("/data")
                    .filter(|data| data.is_object())
                    .unwrap_or(&res);
                let returned_id = convert::first_string(prof, &["/userid", "/userId", "/id"]);
                let nickname = convert::first_string(prof, &["/nickname", "/username"]);
                let avatar =
                    convert::first_string(prof, &["/pic", "/avatarUrl", "/userpic", "/avatar"]);
                let sig = convert::first_string(prof, &["/signature", "/sign"]);
                let p_cnt = convert::first_i64(prof, &["/playlistCount"]).max(0) as i32;
                let follows = convert::first_i64(prof, &["/follows"]).max(0) as i32;
                let followeds = convert::first_i64(prof, &["/followeds"]).max(0) as i32;
                if nickname.trim().is_empty()
                    && avatar.trim().is_empty()
                    && returned_id.is_empty()
                {
                    return Self::failure_bytes(
                        "NotFound",
                        &format!("User profile not found: {}", req.id),
                    );
                }

                Self::success_bytes(&UserProfile {
                    id: req.id,
                    nickname: nickname.trim().to_string(),
                    avatarUrl: convert::sanitize_http_url(&avatar),
                    signature: if sig.trim().is_empty() { None } else { Some(sig.trim().to_string()) },
                    playlistCount: p_cnt,
                    follows,
                    followeds,
                })
            }

            OP_GETUSERRECORDS => {
                let req = match UserRecordsRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                if !matches!(req.r#type, 0 | 1) {
                    return Self::failure_bytes("InvalidResponse", "History type must be 0 or 1");
                }
                let limit = if req.limit > 0 { req.limit } else { 50 };
                let userid = match self.scoped_user_id(&req.id) {
                    Ok(userid) => userid,
                    Err((category, message)) => return Self::failure_bytes(&category, &message),
                };
                let params = json!({
                    "userid": userid,
                    "type": req.r#type,
                    "pagesize": limit,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("user/history", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let page = convert::clamp_track_page(convert::to_track_page(&res), limit);
                Self::success_bytes(&page)
            }

            OP_GETUSERCLOUD => {
                let req = match UserListRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let userid = match self.scoped_user_id(&req.id) {
                    Ok(userid) => userid,
                    Err((category, message)) => return Self::failure_bytes(&category, &message),
                };
                let limit = if req.limit > 0 { req.limit } else { 20 };
                let (page_no, page_size) = Self::page_params(limit, req.offset);
                let params = json!({
                    "userid": userid,
                    "page": page_no,
                    "pagesize": limit,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("user/cloud", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let mut page = convert::to_track_page(&res);
                page.items = Self::page_slice(&page.items, req.offset, page_size).to_vec();
                Self::success_bytes(&page)
            }

            OP_DELETEUSERCLOUD => {
                let req = match BatchRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                if req.ids.is_empty() {
                    return Self::failure_bytes("InvalidResponse", "No cloud ids supplied");
                }
                let mut kv_ids = Vec::new();
                let mut hashes = Vec::new();
                for id in &req.ids {
                    let id = id.trim();
                    if id.is_empty() {
                        return Self::failure_bytes("InvalidResponse", "Cloud id must not be empty");
                    }
                    if id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                        hashes.push(id.to_string());
                    } else if id.parse::<i64>().is_ok_and(|id| id > 0) {
                        kv_ids.push(id.to_string());
                    } else {
                        return Self::failure_bytes(
                            "InvalidResponse",
                            &format!("Unsupported cloud id: {id}"),
                        );
                    }
                }
                if !hashes.is_empty() && !kv_ids.is_empty() {
                    return Self::failure_bytes(
                        "InvalidResponse",
                        "Cloud deletion cannot mix file hashes and numeric ids",
                    );
                }
                let mut params = json!({"cookie": cookie});
                if !hashes.is_empty() {
                    params["hashes"] = json!(hashes.join(","));
                } else {
                    params["fileids"] = json!(kv_ids.join(","));
                }
                let res = standard_api::handle_api("user/cloud/del", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                if !Self::upstream_mutation_ok(&res) {
                    let (category, message) = Self::mutation_failure(&res, "cloud deletion");
                    return Self::failure_bytes(&category, &message);
                }
                Self::success_bytes(&MutationResult {
                    success: true,
                    id: None,
                })
            }

            OP_GETLIKELIST => {
                let req = match UserRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let userid = match self.scoped_user_id(&req.id) {
                    Ok(userid) => userid,
                    Err((category, message)) => return Self::failure_bytes(&category, &message),
                };
                let params = json!({
                    "userid": userid,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("likelist", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let mut items = Vec::new();
                if let Some(arr) = res.pointer("/data/ids").or_else(|| res.get("ids")).and_then(Value::as_array) {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            items.push(s.to_string());
                        }
                    }
                }
                Self::success_bytes(&IdList { items })
            }

            OP_GETLIKELISTPAGE => {
                let req = match PagedIdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let userid = match self.scoped_user_id(&req.id) {
                    Ok(userid) => userid,
                    Err((category, message)) => return Self::failure_bytes(&category, &message),
                };
                let limit = if req.limit > 0 { req.limit as usize } else { 20 };
                let params = json!({
                    "userid": userid,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("likelist", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let mut all_items = Vec::new();
                if let Some(arr) = res.pointer("/data/ids").or_else(|| res.get("ids")).and_then(Value::as_array) {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            all_items.push(s.to_string());
                        }
                    }
                }
                let total = all_items.len() as i64;
                let items = Self::page_slice(&all_items, req.offset, limit).to_vec();
                let has_more = (req.offset.max(0) as i64 + items.len() as i64) < total;
                Self::success_bytes(&IdPage {
                    items,
                    total: Some(total),
                    hasMore: has_more,
                })
            }

            OP_LIKESONG => {
                let req = match SongLikeRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "id": req.id,
                    "like": req.liked,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("like", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                if !Self::upstream_mutation_ok(&res) {
                    let (category, message) =
                        Self::mutation_failure(&res, "liked-state update");
                    return Self::failure_bytes(&category, &message);
                }
                Self::success_bytes(&MutationResult {
                    success: true,
                    id: Some(req.id),
                })
            }

            OP_DISLIKESONG => {
                if let Err(e) = IdRequest::read(&mut r) {
                    return Self::failure_bytes("InvalidResponse", &e);
                }
                Self::failure_bytes(
                    "Unsupported",
                    "Kugou does not expose a reliable dislike operation",
                )
            }

            OP_SCROBBLE => {
                if let Err(e) = ScrobbleRequest::read(&mut r) {
                    return Self::failure_bytes("InvalidResponse", &e);
                }
                Self::failure_bytes(
                    "Unsupported",
                    "Kugou playback scrobbling is not available through the public API",
                )
            }

            OP_GETCOMMENTS | OP_GETCOMMENTPAGE => {
                let (id, resource_type, page_no, page_size, offset, sort_type, cursor) =
                    if operation == OP_GETCOMMENTS {
                    let req = match CommentPageRequest::read(&mut r) {
                        Ok(v) => v,
                        Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                    };
                    let page = (req.offset / req.limit.max(1)) + 1;
                    (
                        req.id,
                        req.r#type,
                        page,
                        req.limit.max(1),
                        req.offset.max(0),
                        req.sortType,
                        String::new(),
                    )
                } else {
                    let req = match CommentQuery::read(&mut r) {
                        Ok(v) => v,
                        Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                    };
                    let page = (req.offset / req.limit.max(1)) + 1;
                    (
                        req.id,
                        req.r#type,
                        page,
                        req.limit.max(1),
                        req.offset.max(0),
                        req.sortType,
                        req.cursor.unwrap_or_default(),
                    )
                };

                let params = json!({
                    "id": id,
                    "type": resource_type,
                    "page": page_no,
                    "pagesize": page_size,
                    "offset": offset,
                    "sortType": sort_type,
                    "cursor": cursor,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("comment/music", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let mut items = Vec::new();
                if let Some(arr) = res.pointer("/data/comments").or_else(|| res.pointer("/data/list")).and_then(Value::as_array) {
                    for item in arr {
                        if let Some(c) = convert::to_comment_item(item) {
                            items.push(c);
                        }
                    }
                }
                let reported_total = Self::optional_i64(
                    res.pointer("/data/totalCount")
                        .or_else(|| res.pointer("/data/total"))
                        .or_else(|| res.pointer("/data/count")),
                );
                let offset_i64 = offset as i64;
                let total = reported_total.map(|total| total.max(offset_i64 + items.len() as i64));
                let has_more =
                    Self::page_has_more(&res, offset_i64, items.len() as i64, reported_total);
                let effective_sort_type = Self::optional_i64(res.pointer("/data/sortType"))
                    .unwrap_or(sort_type as i64) as i32;
                let next_cursor = convert::first_string(&res, &["/data/cursor"]);

                Self::success_bytes(&CommentPage {
                    comments: items,
                    totalCount: total.unwrap_or(0),
                    hasMore: has_more,
                    cursor: next_cursor,
                    sortType: effective_sort_type,
                })
            }

            OP_GETFLOORCOMMENTS => {
                let req = match CommentFloorRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let page_no = if req.time > 0 { req.time } else { 1 };
                let page_size = req.limit.max(1);
                let params = json!({
                    "id": req.id,
                    "type": req.r#type,
                    "comment_id": req.parentCommentId,
                    "parentCommentId": req.parentCommentId,
                    "page": page_no,
                    "pagesize": page_size,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("comment/floor", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                let mut items = Vec::new();
                if let Some(arr) = res.pointer("/data/comments").or_else(|| res.pointer("/data/list")).and_then(Value::as_array) {
                    for item in arr {
                        if let Some(c) = convert::to_comment_item(item) {
                            items.push(c);
                        }
                    }
                }
                let reported_total = Self::optional_i64(
                    res.pointer("/data/totalCount")
                        .or_else(|| res.pointer("/data/total"))
                        .or_else(|| res.pointer("/data/count")),
                );
                let offset = (page_no - 1).saturating_mul(page_size as i64);
                let total = reported_total.map(|total| total.max(offset + items.len() as i64));
                let has_more =
                    Self::page_has_more(&res, offset, items.len() as i64, reported_total);
                let next_time = Self::optional_i64(res.pointer("/data/time")).unwrap_or_else(|| {
                    if has_more {
                        page_no.saturating_add(1)
                    } else {
                        0
                    }
                });

                Self::success_bytes(&CommentFloorPage {
                    replies: items,
                    totalCount: total.unwrap_or(0) as i32,
                    hasMore: has_more,
                    nextTime: next_time,
                })
            }

            OP_POSTCOMMENT => {
                let req = match PostCommentRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                if req.content.trim().is_empty() {
                    return Self::failure_bytes(
                        "InvalidResponse",
                        "Comment content must not be empty",
                    );
                }
                let mut params_map = serde_json::Map::new();
                params_map.insert("id".to_string(), Value::String(req.id.clone()));
                params_map.insert("type".to_string(), Value::String(req.r#type.clone()));
                params_map.insert("content".to_string(), Value::String(req.content.clone()));
                if let Some(reply_id) = req.replyToCommentId {
                    params_map.insert("reply_id".to_string(), Value::Number(reply_id.into()));
                }
                params_map.insert("cookie".to_string(), cookie);
                let res = standard_api::handle_api("comment", &Value::Object(params_map));
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                if !Self::upstream_mutation_ok(&res) {
                    let (category, message) = Self::mutation_failure(&res, "comment creation");
                    return Self::failure_bytes(&category, &message);
                }
                let item = res
                    .pointer("/data/comment")
                    .and_then(convert::to_comment_item)
                    .map(|mut comment| {
                        if comment.parentCommentId == 0 {
                            comment.parentCommentId = req.replyToCommentId.unwrap_or(0);
                        }
                        comment
                    });
                Self::success_bytes(&CreatedComment {
                    comment: item,
                })
            }

            OP_LIKECOMMENT => {
                if let Err(e) = CommentLikeRequest::read(&mut r) {
                    return Self::failure_bytes("InvalidResponse", &e);
                }
                Self::failure_bytes(
                    "Unsupported",
                    "Kugou does not expose a reliable comment-like operation",
                )
            }

            OP_DELETECOMMENT => {
                let req = match DeleteCommentRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                if req.commentId <= 0 {
                    return Self::failure_bytes("InvalidResponse", "Comment id must be positive");
                }
                let params = json!({
                    "id": req.id,
                    "type": req.r#type,
                    "comment_id": req.commentId,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("comment/delete", &params);
                if let Some((category, message)) = Self::standard_failure(&res) {
                    return Self::failure_bytes(&category, &message);
                }
                if !Self::upstream_mutation_ok(&res) {
                    let (category, message) = Self::mutation_failure(&res, "comment deletion");
                    return Self::failure_bytes(&category, &message);
                }
                Self::success_bytes(&MutationResult {
                    success: true,
                    id: Some(req.commentId.to_string()),
                })
            }

            OP_GETCURRENTACCOUNT => {
                let params = json!({ "cookie": cookie });
                let res = standard_api::handle_api("login/status", &params);
                let profile = res.pointer("/data/profile");
                if let Some(prof) = profile {
                    let uid = convert::first_string(prof, &["/userId", "/userid", "/id"]);
                    let nickname = convert::first_string(prof, &["/nickname", "/username"]);
                    let avatar = convert::first_string(prof, &["/avatarUrl", "/userpic"]).replace("{size}", "200");
                    Self::success_bytes(&AccountSession {
                        loggedIn: true,
                        userId: if uid.is_empty() { None } else { Some(uid) },
                        nickname: if nickname.is_empty() { None } else { Some(nickname) },
                        avatarUrl: if avatar.is_empty() { None } else { Some(avatar) },
                    })
                } else {
                    Self::success_bytes(&AccountSession {
                        loggedIn: false,
                        userId: None,
                        nickname: None,
                        avatarUrl: None,
                    })
                }
            }

            OP_AUTHENTICATE => {
                let req = match AuthActionRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let action = req.action.to_lowercase();
                match action.as_str() {
                    "qr.key" | "qr_key" | "key" => {
                        let res = standard_api::handle_api("login/qr/key", &json!({ "cookie": cookie }));
                        let key = convert::first_string(&res, &["/data/unikey", "/data/key", "/unikey"]);
                        let code = convert::first_i64(&res, &["/code"]) as i32;
                        let message = convert::first_string(&res, &["/message", "/msg"]);
                        Self::success_bytes(&AuthActionResult {
                            success: !key.is_empty() && code == 200,
                            code: Some(if code == 0 { 200 } else { code }),
                            status: None,
                            key: if key.is_empty() { None } else { Some(key) },
                            url: None,
                            imageBase64: None,
                            cookie: None,
                            verifyUrl: None,
                            challenge: false,
                            message: if message.is_empty() { None } else { Some(message) },
                        })
                    }
                    "qr.create" | "qr_create" | "create" => {
                        let key_str = req.key.clone().unwrap_or_default();
                        let res = standard_api::handle_api(
                            "login/qr/create",
                            &json!({ "key": key_str, "qrimg": true, "cookie": cookie }),
                        );
                        let url = convert::first_string(&res, &["/data/url", "/data/qrurl", "/url", "/qrurl"]);
                        let img = convert::first_string(
                            &res,
                            &["/data/base64", "/data/qrimg", "/data/imageBase64", "/base64", "/qrimg"],
                        );
                        let code = convert::first_i64(&res, &["/code"]) as i32;
                        let message = convert::first_string(&res, &["/message", "/msg"]);
                        Self::success_bytes(&AuthActionResult {
                            success: (!url.is_empty() || !img.is_empty()) && code == 200,
                            code: Some(if code == 0 { 200 } else { code }),
                            status: None,
                            key: req.key,
                            url: if url.is_empty() { None } else { Some(url) },
                            imageBase64: if img.is_empty() { None } else { Some(img) },
                            cookie: None,
                            verifyUrl: None,
                            challenge: false,
                            message: if message.is_empty() { None } else { Some(message) },
                        })
                    }
                    "qr.check" | "qr_check" | "check" => {
                        let key_str = req.key.clone().unwrap_or_default();
                        let res = standard_api::handle_api(
                            "login/qr/check",
                            &json!({ "key": key_str, "cookie": cookie }),
                        );
                        // Standard adapter returns the KuGou QR state (800/801/802/803)
                        // at the top-level `code`, plus the credential at `cookie`.
                        let status = convert::first_i64(
                            &res,
                            &["/status", "/data/status", "/code", "/data/code"],
                        ) as i32;
                        let auth_cookie = convert::first_string(&res, &["/cookie", "/data/cookie"]);
                        let message = convert::first_string(&res, &["/message", "/msg"]);
                        Self::success_bytes(&AuthActionResult {
                            success: status == 803 && !auth_cookie.is_empty(),
                            code: Some(status),
                            status: Some(status),
                            key: req.key,
                            url: None,
                            imageBase64: None,
                            cookie: if auth_cookie.is_empty() { None } else { Some(auth_cookie) },
                            verifyUrl: None,
                            challenge: false,
                            message: if message.is_empty() { None } else { Some(message) },
                        })
                    }
                    "login.phone" | "login_phone" | "phone" | "login.email" | "login_email"
                    | "email" => {
                        let phone = req.phone.clone().unwrap_or_default();
                        let email = req.email.clone().unwrap_or_default();
                        let captcha = req.captcha.clone().unwrap_or_default();
                        let password = req.password.clone().unwrap_or_default();
                        let username = if !phone.is_empty() { phone.clone() } else { email };

                        let ctx = crate::modules::Ctx {
                            ip: String::new(),
                            body_bytes: None,
                        };

                        let result = if !captcha.is_empty() {
                            let mut q_map = serde_json::Map::new();
                            q_map.insert("mobile".to_string(), json!(phone));
                            q_map.insert("code".to_string(), json!(captcha));
                            q_map.insert("cookie".to_string(), cookie);
                            crate::modules::login::handle_cellphone(&Value::Object(q_map), &ctx)
                        } else if !password.is_empty() {
                            let mut q_map = serde_json::Map::new();
                            q_map.insert("username".to_string(), json!(username));
                            q_map.insert("password".to_string(), json!(password));
                            q_map.insert("cookie".to_string(), cookie);
                            crate::modules::login::handle_login(&Value::Object(q_map), &ctx)
                        } else {
                            return Self::success_bytes(&AuthActionResult {
                                success: false,
                                code: Some(400),
                                status: Some(400),
                                key: None,
                                url: None,
                                imageBase64: None,
                                cookie: None,
                                verifyUrl: None,
                                challenge: false,
                                message: Some("Phone and captcha/password are required".to_string()),
                            });
                        };

                        match result {
                            Ok(resp) => {
                                let body = resp.body.to_json();
                                let status_ok = body.get("status").and_then(Value::as_i64) == Some(1)
                                    || body.get("code").and_then(Value::as_i64) == Some(0)
                                    || body.get("code").and_then(Value::as_i64) == Some(200);
                                let mut cookies = resp.cookie;
                                if let Some(token) = body.pointer("/data/token").and_then(Value::as_str) {
                                    cookies.push(format!("token={}", token));
                                }
                                if let Some(uid) = body.pointer("/data/userid").and_then(convert::value_to_string) {
                                    cookies.push(format!("userid={}", uid));
                                }
                                let auth_cookie = cookies.join("; ");
                                let msg = convert::first_string(&body, &["/msg", "/error", "/message"]);

                                Self::success_bytes(&AuthActionResult {
                                    success: status_ok && !auth_cookie.is_empty(),
                                    code: Some(200),
                                    status: Some(if status_ok { 200 } else { 400 }),
                                    key: None,
                                    url: None,
                                    imageBase64: None,
                                    cookie: if auth_cookie.is_empty() { None } else { Some(auth_cookie) },
                                    verifyUrl: None,
                                    challenge: false,
                                    message: if msg.is_empty() { None } else { Some(msg) },
                                })
                            }
                            Err(err) => {
                                let body = err.body.to_json();
                                let msg = convert::first_string(&body, &["/msg", "/error"]);
                                Self::success_bytes(&AuthActionResult {
                                    success: false,
                                    code: Some(err.status as i32),
                                    status: Some(err.status as i32),
                                    key: None,
                                    url: None,
                                    imageBase64: None,
                                    cookie: None,
                                    verifyUrl: None,
                                    challenge: false,
                                    message: Some(if msg.is_empty() {
                                        format!("Login failed with status {}", err.status)
                                    } else {
                                        msg
                                    }),
                                })
                            }
                        }
                    }
                    "captcha.send" | "captcha_send" => {
                        let phone = req.phone.clone().unwrap_or_default();
                        let ctx = crate::modules::Ctx {
                            ip: String::new(),
                            body_bytes: None,
                        };
                        let mut q_map = serde_json::Map::new();
                        q_map.insert("mobile".to_string(), json!(phone));
                        q_map.insert("cookie".to_string(), cookie);
                        let result = crate::modules::misc::handle_captcha_sent(&Value::Object(q_map), &ctx);

                        match result {
                            Ok(resp) => {
                                let body = resp.body.to_json();
                                let status_ok = body.get("status").and_then(Value::as_i64) == Some(1)
                                    || body.get("code").and_then(Value::as_i64) == Some(0)
                                    || body.get("code").and_then(Value::as_i64) == Some(200);
                                let msg = convert::first_string(&body, &["/msg", "/message"]);
                                Self::success_bytes(&AuthActionResult {
                                    success: status_ok,
                                    code: Some(200),
                                    status: Some(if status_ok { 200 } else { 400 }),
                                    key: None,
                                    url: None,
                                    imageBase64: None,
                                    cookie: None,
                                    verifyUrl: None,
                                    challenge: false,
                                    message: if msg.is_empty() { None } else { Some(msg) },
                                })
                            }
                            Err(err) => Self::success_bytes(&AuthActionResult {
                                success: false,
                                code: Some(err.status as i32),
                                status: Some(err.status as i32),
                                key: None,
                                url: None,
                                imageBase64: None,
                                cookie: None,
                                verifyUrl: None,
                                challenge: false,
                                message: Some(format!("Failed to send captcha: {}", err.status)),
                            }),
                        }
                    }
                    "logout" => Self::success_bytes(&AuthActionResult {
                        success: true,
                        code: Some(200),
                        status: Some(200),
                        key: None,
                        url: None,
                        imageBase64: None,
                        cookie: Some(String::new()),
                        verifyUrl: None,
                        challenge: false,
                        message: Some("Logged out".to_string()),
                    }),
                    _ => Self::failure_bytes(
                        "Unsupported",
                        &format!("Unsupported auth action: {}", req.action),
                    ),
                }
            }

            OP_GETMEDIAQUALITIES => {
                let req = match MediaQualityRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                if req.id.trim().is_empty() {
                    return Self::failure_bytes("InvalidResponse", "Missing media id");
                }
                let resource_type = req.r#type.trim().to_ascii_lowercase();
                if !matches!(resource_type.as_str(), "" | "song" | "audio" | "track") {
                    return Self::failure_bytes(
                        "Unsupported",
                        &format!(
                            "Kugou quality probing does not support resource type: {}",
                            req.r#type
                        ),
                    );
                }
                let catalog: [(&str, &str, i32); 5] = [
                    ("standard", "标准音质 (128k MP3)", 128_000),
                    ("higher", "较高音质 (192k MP3)", 192_000),
                    ("exhigh", "极高音质 (320k MP3)", 320_000),
                    ("lossless", "无损音质 (FLAC)", 960_000),
                    ("hires", "Hi-Res 音质 (24bit/96kHz)", 2_300_000),
                ];
                let mut items = Vec::new();
                for (id, label, bitrate) in catalog {
                    match Self::probe_quality(&req.id, id, &cookie) {
                        Ok(Some((size, _actual))) => items.push(QualityOption {
                            id: id.into(),
                            label: label.into(),
                            bitrate: Some(bitrate),
                            sizeBytes: size,
                            available: true,
                        }),
                        Ok(None) => items.push(QualityOption {
                            id: id.into(),
                            label: label.into(),
                            bitrate: Some(bitrate),
                            sizeBytes: None,
                            available: false,
                        }),
                        Err((category, message)) => {
                            return Self::failure_bytes(&category, &message)
                        }
                    }
                }
                Self::success_bytes(&QualityPage {
                    items,
                })
            }

            OP_CHECKTRACKAVAILABLE => {
                let req = match TrackAvailabilityRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                if req.id.trim().is_empty() {
                    return Self::failure_bytes("InvalidResponse", "Missing track id");
                }
                let quality = Self::quality_for_bitrate(req.bitrate);
                match Self::probe_quality(&req.id, quality, &cookie) {
                    Ok(Some((_size, _actual))) => Self::success_bytes(&TrackAvailability {
                        available: true,
                        message: None,
                    }),
                    Ok(None) => Self::success_bytes(&TrackAvailability {
                        available: false,
                        message: Some(format!(
                            "Track is not available at the requested quality ({quality})"
                        )),
                    }),
                    Err((category, message)) => Self::failure_bytes(&category, &message),
                }
            }

            _ => Self::failure_bytes("Unsupported", &format!("Operation {} is not supported by Kugou provider", operation)),
        }
    }
}

pub trait WireWritable {
    fn write_to(&self, w: &mut WireWriter);
}

macro_rules! impl_wire_writable {
    ($($t:ty),*) => {
        $(
            impl WireWritable for $t {
                fn write_to(&self, w: &mut WireWriter) {
                    self.write(w);
                }
            }
        )*
    };
}

impl_wire_writable!(
    Capabilities,
    TrackPage,
    Track,
    TrackBatch,
    MediaResource,
    Lyrics,
    PlaylistPage,
    Playlist,
    AlbumPage,
    AlbumDetails,
    ArtistPage,
    CatalogArtist,
    BannerPage,
    RankingPage,
    KeywordPage,
    SuggestionPage,
    UserProfile,
    IdList,
    IdPage,
    MutationResult,
    CommentPage,
    CommentFloorPage,
    CreatedComment,
    AccountSession,
    AuthActionResult,
    QualityPage,
    TrackAvailability
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capabilities_declaration_matches_contract() {
        let session = KugouSession::new("kugou".into(), None, None, None);
        let resp_bytes = session.dispatch(OP_CAPABILITIES, &[]);
        assert_eq!(resp_bytes[0], 0); // 0 = Success
        let mut r = WireReader::new(&resp_bytes[1..]);
        let caps = Capabilities::read(&mut r).expect("Capabilities wire decode failed");

        let expected_caps = [
            "recommendations.personalFm",
            "recommendations.similarSongs",
            "recommendations.intelligenceSongs",
            "discovery.searchSuggestions",
            "auth.actions",
            "tracks.available",
            "media.qualities",
            "recommendations.songs",
            "recommendations.playlists",
        ];

        for cap in expected_caps {
            assert!(
                caps.supported.contains(&cap.to_string()),
                "Missing declared capability: {}",
                cap
            );
        }
    }

    #[test]
    fn test_recommend_and_personal_fm_parsing() {
        // 每日推荐返回结构
        let daily_res = json!({
            "code": 0,
            "data": {
                "dailySongs": [
                    {
                        "id": "DAILY_HASH_1",
                        "title": "Daily Song 1",
                        "artist": "Daily Artist",
                        "duration": 200,
                        "album": "Daily Album",
                        "coverUrl": "https://img.kugou.com/cover/1.jpg"
                    }
                ]
            }
        });
        let page = convert::to_track_page(&daily_res);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, "DAILY_HASH_1");
        assert_eq!(page.items[0].title, "Daily Song 1");
        assert_eq!(page.items[0].artists[0].name, "Daily Artist");

        // 私人 FM 返回结构（data 为直接数组）
        let fm_res = json!({
            "code": 0,
            "data": [
                {
                    "hash": "FM_HASH_1",
                    "SongName": "FM Song 1",
                    "SingerName": "FM Artist",
                    "timelen": 180000,
                    "cover": "//img.kugou.com/fm/1.jpg"
                }
            ]
        });
        let fm_page = convert::to_track_page(&fm_res);
        assert_eq!(fm_page.items.len(), 1);
        assert_eq!(fm_page.items[0].id, "FM_HASH_1");
        assert_eq!(fm_page.items[0].title, "FM Song 1");
        assert_eq!(fm_page.items[0].coverUrl, Some("https://img.kugou.com/fm/1.jpg".to_string()));
    }

    #[test]
    fn test_page_clamping_strictness() {
        let mut tracks = Vec::new();
        for i in 0..30 {
            tracks.push(Track {
                id: format!("hash_{}", i),
                title: format!("Title {}", i),
                artists: vec![Artist { id: None, name: "Artist".into() }],
                durationMs: 1000,
                album: None,
                coverUrl: None,
            });
        }
        let raw_page = TrackPage {
            items: tracks,
            total: Some(10), // total 非法小于 items 长度
            hasMore: Some(true),
        };
        let clamped = convert::clamp_track_page(raw_page, 20);
        assert_eq!(clamped.items.len(), 20);
        assert_eq!(clamped.total, Some(10));
    }

    #[test]
    fn test_dislike_song_uses_id_request_and_reports_unsupported() {
        let session = KugouSession::new("kugou".into(), None, None, None);
        let mut writer = WireWriter::new();
        IdRequest {
            id: "aabbccddeeff00112233445566778899".into(),
        }
        .write(&mut writer);
        let resp = session.dispatch(OP_DISLIKESONG, &writer.finish());
        assert_eq!(resp[0], 1);
        let mut r = WireReader::new(&resp[1..]);
        let failure = Failure::read(&mut r).unwrap();
        assert_eq!(failure.category, "Unsupported");
    }

    #[test]
    fn test_media_qualities_reject_unprobeable_resource_types() {
        let session = KugouSession::new("kugou".into(), None, None, None);
        for resource_type in ["mv", "video"] {
            let mut writer = WireWriter::new();
            MediaQualityRequest {
                id: "123".into(),
                r#type: resource_type.into(),
            }
            .write(&mut writer);
            let resp = session.dispatch(OP_GETMEDIAQUALITIES, &writer.finish());
            assert_eq!(resp[0], 1, "{resource_type}");
            let mut r = WireReader::new(&resp[1..]);
            let failure = Failure::read(&mut r).unwrap();
            assert_eq!(failure.category, "Unsupported", "{resource_type}");
        }
    }

    #[test]
    fn test_check_track_available_requires_an_id() {
        let session = KugouSession::new("kugou".into(), None, None, None);
        let mut writer = WireWriter::new();
        TrackAvailabilityRequest {
            id: String::new(),
            bitrate: 128_000,
        }
        .write(&mut writer);
        let resp = session.dispatch(OP_CHECKTRACKAVAILABLE, &writer.finish());
        assert_eq!(resp[0], 1);
        let mut r = WireReader::new(&resp[1..]);
        let failure = Failure::read(&mut r).unwrap();
        assert_eq!(failure.category, "InvalidResponse");
    }

    #[test]
    fn test_comment_conversion_accepts_standard_shape() {
        let value = json!({
            "commentId": 42,
            "content": "hello",
            "user": {
                "userId": 7,
                "nickname": "listener",
                "avatarUrl": "https://img.kugou.com/avatar.jpg"
            },
            "time": 1700000000000_i64,
            "likedCount": 3,
            "replyCount": 1,
            "parentCommentId": 9
        });
        let item = convert::to_comment_item(&value).expect("standard comment must convert");
        assert_eq!(item.id, 42);
        assert_eq!(item.content, "hello");
        assert_eq!(item.user, "listener");
        assert_eq!(item.userId, 7);
        assert_eq!(item.avatar, "https://img.kugou.com/avatar.jpg");
        assert_eq!(item.likedCount, 3);
        assert_eq!(item.replyCount, 1);
        assert_eq!(item.parentCommentId, 9);
    }

    #[test]
    fn test_auth_actions_dispatch() {
        let session = KugouSession::new("kugou".into(), None, None, None);

        // 1. qr.key
        let mut writer = WireWriter::new();
        AuthActionRequest {
            action: "qr.key".into(),
            ..Default::default()
        }.write(&mut writer);
        let resp = session.dispatch(OP_AUTHENTICATE, &writer.finish());
        assert_eq!(resp[0], 0);

        // 2. qr.create
        let mut writer = WireWriter::new();
        AuthActionRequest {
            action: "qr.create".into(),
            key: Some("sample-key".into()),
            ..Default::default()
        }.write(&mut writer);
        let resp = session.dispatch(OP_AUTHENTICATE, &writer.finish());
        assert_eq!(resp[0], 0);
        let mut r = WireReader::new(&resp[1..]);
        let res = AuthActionResult::read(&mut r).unwrap();
        assert!(res.success);
        assert!(res.imageBase64.is_some(), "Expected imageBase64 to be populated for qr.create");

        // 3. logout
        let mut writer = WireWriter::new();
        AuthActionRequest {
            action: "logout".into(),
            ..Default::default()
        }.write(&mut writer);
        let resp = session.dispatch(OP_AUTHENTICATE, &writer.finish());
        assert_eq!(resp[0], 0);
        let mut r = WireReader::new(&resp[1..]);
        let res = AuthActionResult::read(&mut r).unwrap();
        assert!(res.success);
        assert_eq!(res.cookie, Some("".into()));
    }

    /// Host JNI / C ABI codec writes the native v1 layout, which has no trailing
    /// `countryCode` field. Decoding must not treat that as a truncated message.
    fn write_native_auth_request(req: &AuthActionRequest) -> Vec<u8> {
        let mut w = WireWriter::new();
        w.write_string(&req.action);
        w.write_optional(&req.email, |w, v| w.write_string(v));
        w.write_optional(&req.phone, |w, v| w.write_string(v));
        w.write_optional(&req.password, |w, v| w.write_string(v));
        w.write_optional(&req.captcha, |w, v| w.write_string(v));
        w.write_bool(req.md5);
        w.write_optional(&req.imageCaptcha, |w, v| w.write_string(v));
        w.write_optional(&req.challengeCookie, |w, v| w.write_string(v));
        w.write_optional(&req.key, |w, v| w.write_string(v));
        w.write_optional(&req.sessionId, |w, v| w.write_string(v));
        w.write_optional(&req.verifyId, |w, v| w.write_string(v));
        w.write_optional(&req.extraJson, |w, v| w.write_string(v));
        w.finish()
    }

    #[test]
    fn test_auth_actions_accept_host_native_layout() {
        let session = KugouSession::new("kugou".into(), None, None, None);

        let resp = session.dispatch(
            OP_AUTHENTICATE,
            &write_native_auth_request(&AuthActionRequest {
                action: "logout".into(),
                ..Default::default()
            }),
        );
        assert_eq!(resp[0], 0, "native auth request must decode as success");
        let mut r = WireReader::new(&resp[1..]);
        let res = AuthActionResult::read(&mut r).unwrap();
        assert!(res.success);

        let resp = session.dispatch(
            OP_AUTHENTICATE,
            &write_native_auth_request(&AuthActionRequest {
                action: "logout".into(),
                extraJson: Some(r#"{"countryCode":"86"}"#.into()),
                ..Default::default()
            }),
        );
        assert_eq!(resp[0], 0);
    }
}
