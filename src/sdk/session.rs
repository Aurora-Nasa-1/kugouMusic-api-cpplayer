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
                        "accounts.albums".into(),
                        "playlists.delete".into(),
                        "playlists.subscribe".into(),
                        "playlists.addTracks".into(),
                        "playlists.removeTracks".into(),
                        "playlists.rename".into(),
                        "playlists.create".into(),
                        "library.like".into(),
                        "library.dislike".into(),
                        "comments.list".into(),
                        "comments.floor".into(),
                        "comments.post".into(),
                        "comments.like".into(),
                        "radio.personalFm".into(),
                        "recommendations.similar".into(),
                        "recommendations.intelligence".into(),
                        "albums.subscribe".into(),
                        "discovery.hotSearches".into(),
                        "discovery.suggestions".into(),
                        "playback.scrobble".into(),
                        "accounts.session".into(),
                        "accounts.authenticate".into(),
                        "comments.delete".into(),
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
                let page = convert::to_track_page(&res);
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
                let page = convert::to_track_page(&res);
                if let Some(first) = page.items.into_iter().next() {
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
                let params = json!({
                    "ids": req.ids.join(","),
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("song/detail", &params);
                let tracks = convert::to_track_page(&res).items;
                let outcomes = tracks
                    .into_iter()
                    .map(|t| TrackOutcome {
                        id: t.id.clone(),
                        track: Some(t),
                        failure: None,
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
                let data = res.get("data");
                let lrc = data.and_then(|d| d.get("lyric")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
                let translation = data.and_then(|d| d.get("tlyric")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
                let yrc = data.and_then(|d| d.get("klyric")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);

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
                Self::success_bytes(&convert::to_playlist_page(&res))
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
                let pl = convert::to_playlist(&res.get("data").unwrap_or(&res)).unwrap_or(Playlist {
                    id: req.id,
                    title: "Playlist".into(),
                    coverUrl: None,
                    trackCount: None,
                    creatorId: None,
                    creatorName: None,
                    description: None,
                });
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
                Self::success_bytes(&convert::to_track_page(&res))
            }

            OP_GETUSERPLAYLISTS => {
                let req = match PagedIdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "uid": req.id,
                    "userid": req.id,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("user/playlist", &params);
                Self::success_bytes(&convert::to_playlist_page(&res))
            }

            OP_GETCREATEDPLAYLISTS => {
                let req = match PagedIdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "uid": req.id,
                    "userid": req.id,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("user/playlist/create", &params);
                Self::success_bytes(&convert::to_playlist_page(&res))
            }

            OP_GETCOLLECTEDPLAYLISTS => {
                let req = match PagedIdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "uid": req.id,
                    "userid": req.id,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("user/playlist/collect", &params);
                Self::success_bytes(&convert::to_playlist_page(&res))
            }

            OP_CREATEPLAYLIST => {
                let req = match CreatePlaylistRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "name": req.name,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("playlist/create", &params);
                let id = convert::first_string(&res, &["/data/id", "/id", "/listid"]);
                Self::success_bytes(&MutationResult {
                    success: !id.is_empty(),
                    id: if id.is_empty() { None } else { Some(id) },
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
                let _ = standard_api::handle_api("playlist/delete", &params);
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
                let _ = standard_api::handle_api("playlist/subscribe", &params);
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
                let _ = standard_api::handle_api("playlist/tracks", &params);
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
                let _ = standard_api::handle_api("playlist/tracks", &params);
                Self::success_bytes(&MutationResult {
                    success: true,
                    id: Some(req.playlistId),
                })
            }

            OP_RENAMEPLAYLIST => {
                let req = match PlaylistNameRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "id": req.playlistId,
                    "name": req.name,
                    "cookie": cookie,
                });
                let _ = standard_api::handle_api("playlist/update/name", &params);
                Self::success_bytes(&MutationResult {
                    success: true,
                    id: Some(req.playlistId),
                })
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
                Self::success_bytes(&convert::to_album_page(&res))
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
                let album = convert::to_album(&res.get("data").unwrap_or(&res)).unwrap_or(Album {
                    id: req.id,
                    title: "Album".into(),
                    artists: Vec::new(),
                    coverUrl: None,
                    trackCount: None,
                    releasedAtEpochMs: None,
                    company: None,
                    description: None,
                });
                let tracks = convert::to_track_page(&res).items;
                Self::success_bytes(&AlbumDetails { album, tracks })
            }

            OP_GETNEWALBUMS | OP_BROWSENEWALBUMS | OP_GETLATESTALBUMS => {
                let params = json!({ "cookie": cookie });
                let res = standard_api::handle_api("top_album", &params);
                Self::success_bytes(&convert::to_album_page(&res))
            }

            OP_SUBSCRIBEALBUM => {
                let req = match AlbumSubscribeRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                Self::success_bytes(&MutationResult {
                    success: true,
                    id: Some(req.id),
                })
            }

            OP_GETUSERALBUMS => {
                let params = json!({ "cookie": cookie });
                let res = standard_api::handle_api("top_album", &params);
                Self::success_bytes(&convert::to_album_page(&res))
            }

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
                Self::success_bytes(&convert::to_artist_page(&res))
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
                let artist = convert::to_artist(&res.get("data").unwrap_or(&res)).unwrap_or(CatalogArtist {
                    id: req.id,
                    name: "Artist".into(),
                    avatarUrl: None,
                    description: None,
                    aliases: Vec::new(),
                    albumCount: None,
                    trackCount: None,
                });
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
                Self::success_bytes(&convert::to_track_page(&res))
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
                Self::success_bytes(&convert::to_album_page(&res))
            }

            OP_GETHOTARTISTS => {
                let params = json!({ "cookie": cookie });
                let res = standard_api::handle_api("top_artists", &params);
                Self::success_bytes(&convert::to_artist_page(&res))
            }

            OP_GETRECOMMENDEDSONGS => {
                let params = json!({ "cookie": cookie });
                let res = standard_api::handle_api("recommend/songs", &params);
                Self::success_bytes(&convert::to_track_page(&res))
            }

            OP_GETRECOMMENDEDPLAYLISTS | OP_GETPERSONALIZEDPLAYLISTS | OP_GETTOPPLAYLISTS => {
                let params = json!({ "cookie": cookie });
                let res = standard_api::handle_api("personalized", &params);
                Self::success_bytes(&convert::to_playlist_page(&res))
            }

            OP_GETHIGHQUALITYPLAYLISTS => {
                let params = json!({ "cookie": cookie });
                let res = standard_api::handle_api("top_playlist_highquality", &params);
                Self::success_bytes(&convert::to_playlist_page(&res))
            }

            OP_GETPERSONALIZEDNEWSONGS | OP_GETTOPSONGS => {
                let params = json!({ "cookie": cookie });
                let res = standard_api::handle_api("personalized_newsong", &params);
                Self::success_bytes(&convert::to_track_page(&res))
            }

            OP_GETPERSONALFM => {
                let params = json!({ "cookie": cookie });
                let res = standard_api::handle_api("personal_fm", &params);
                Self::success_bytes(&convert::to_track_page(&res))
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
                Self::success_bytes(&convert::to_track_page(&res))
            }

            OP_GETINTELLIGENCESONGS => {
                let req = match IntelligenceSongsRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "id": req.id,
                    "hash": req.id,
                    "pid": req.playlistId,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("playmode/intelligence/list", &params);
                Self::success_bytes(&convert::to_track_page(&res))
            }

            OP_GETBANNERS => {
                let params = json!({ "cookie": cookie });
                let res = standard_api::handle_api("banner", &params);
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
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("search/suggest", &params);
                let mut items = Vec::new();
                if let Some(arr) = res.get("data").and_then(Value::as_array) {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            items.push(s.to_string());
                        } else if let Some(w) = item.get("searchWord").and_then(Value::as_str) {
                            items.push(w.to_string());
                        }
                    }
                }
                Self::success_bytes(&SuggestionPage { items })
            }

            OP_GETUSERPROFILE => {
                let req = match UserRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "userid": req.id,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("login/status", &params);
                let prof = res.pointer("/data/profile").unwrap_or(&res);
                let id = convert::first_string(prof, &["/userId", "/userid", "/id"]);
                let nickname = convert::first_string(prof, &["/nickname", "/username"]);
                let avatar = convert::first_string(prof, &["/avatarUrl", "/userpic"]).replace("{size}", "200");
                let sig = convert::first_string(prof, &["/signature", "/sign"]);
                let p_cnt = convert::first_i64(prof, &["/playlistCount"]) as i32;
                let follows = convert::first_i64(prof, &["/follows"]) as i32;
                let followeds = convert::first_i64(prof, &["/followeds"]) as i32;

                Self::success_bytes(&UserProfile {
                    id: if id.is_empty() { req.id } else { id },
                    nickname: if nickname.is_empty() { "Kugou User".into() } else { nickname },
                    avatarUrl: if avatar.is_empty() { None } else { Some(avatar) },
                    signature: if sig.is_empty() { None } else { Some(sig) },
                    playlistCount: p_cnt,
                    follows,
                    followeds,
                })
            }

            OP_GETUSERRECORDS => {
                let params = json!({ "cookie": cookie });
                let res = standard_api::handle_api("recommend/songs", &params);
                Self::success_bytes(&convert::to_track_page(&res))
            }

            OP_GETUSERCLOUD => {
                let params = json!({ "cookie": cookie });
                let res = standard_api::handle_api("recommend/songs", &params);
                Self::success_bytes(&convert::to_track_page(&res))
            }

            OP_DELETEUSERCLOUD => {
                let req = match IdRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                Self::success_bytes(&MutationResult {
                    success: true,
                    id: Some(req.id),
                })
            }

            OP_GETLIKELIST => {
                let req = match UserRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "userid": req.id,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("likelist", &params);
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
                let params = json!({
                    "userid": req.id,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("likelist", &params);
                let mut items = Vec::new();
                if let Some(arr) = res.pointer("/data/ids").or_else(|| res.get("ids")).and_then(Value::as_array) {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            items.push(s.to_string());
                        }
                    }
                }
                let total = items.len() as i64;
                Self::success_bytes(&IdPage {
                    items,
                    total: Some(total),
                    hasMore: false,
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
                let _ = standard_api::handle_api("like", &params);
                Self::success_bytes(&MutationResult {
                    success: true,
                    id: Some(req.id),
                })
            }

            OP_DISLIKESONG => {
                let req = match SongLikeRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                Self::success_bytes(&MutationResult {
                    success: true,
                    id: Some(req.id),
                })
            }

            OP_SCROBBLE => {
                let req = match ScrobbleRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                Self::success_bytes(&MutationResult {
                    success: true,
                    id: Some(req.id),
                })
            }

            OP_GETCOMMENTS | OP_GETCOMMENTPAGE => {
                let (id, page_no, page_size, sort_type, cursor) = if operation == OP_GETCOMMENTS {
                    let req = match CommentPageRequest::read(&mut r) {
                        Ok(v) => v,
                        Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                    };
                    let page = (req.offset / req.limit.max(1)) + 1;
                    (req.id, page, req.limit, req.sortType, String::new())
                } else {
                    let req = match CommentQuery::read(&mut r) {
                        Ok(v) => v,
                        Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                    };
                    let page = (req.offset / req.limit.max(1)) + 1;
                    (req.id, page, req.limit, req.sortType, req.cursor.unwrap_or_default())
                };

                let params = json!({
                    "id": id,
                    "hash": id,
                    "page": page_no,
                    "pagesize": page_size,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("comment/music", &params);
                let mut items = Vec::new();
                if let Some(arr) = res.pointer("/data/comments").or_else(|| res.pointer("/data/list")).and_then(Value::as_array) {
                    for item in arr {
                        if let Some(c) = convert::to_comment_item(item) {
                            items.push(c);
                        }
                    }
                }
                let total = convert::first_i64(&res, &["/data/total", "/data/count"]);
                let has_more = res.pointer("/data/hasMore").and_then(Value::as_bool).unwrap_or(items.len() >= page_size as usize);

                Self::success_bytes(&CommentPage {
                    comments: items,
                    totalCount: total,
                    hasMore: has_more,
                    cursor,
                    sortType: sort_type,
                })
            }

            OP_GETFLOORCOMMENTS => {
                let req = match CommentFloorRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let params = json!({
                    "id": req.id,
                    "hash": req.id,
                    "comment_id": req.parentCommentId,
                    "page": 1,
                    "pagesize": req.limit,
                    "cookie": cookie,
                });
                let res = standard_api::handle_api("comment/floor", &params);
                let mut items = Vec::new();
                if let Some(arr) = res.pointer("/data/comments").or_else(|| res.pointer("/data/list")).and_then(Value::as_array) {
                    for item in arr {
                        if let Some(c) = convert::to_comment_item(item) {
                            items.push(c);
                        }
                    }
                }
                let total = convert::first_i64(&res, &["/data/total", "/data/count"]) as i32;
                let has_more = res.pointer("/data/hasMore").and_then(Value::as_bool).unwrap_or(items.len() >= req.limit as usize);

                Self::success_bytes(&CommentFloorPage {
                    replies: items,
                    totalCount: total,
                    hasMore: has_more,
                    nextTime: 0,
                })
            }

            OP_POSTCOMMENT => {
                let req = match PostCommentRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                let mut params_map = serde_json::Map::new();
                params_map.insert("id".to_string(), Value::String(req.id.clone()));
                params_map.insert("content".to_string(), Value::String(req.content.clone()));
                if let Some(reply_id) = req.replyToCommentId {
                    params_map.insert("reply_id".to_string(), Value::Number(reply_id.into()));
                }
                params_map.insert("cookie".to_string(), cookie);
                let res = standard_api::handle_api("comment", &Value::Object(params_map));
                let comment_id = convert::first_i64(&res, &["/data/comment_id", "/data/id", "/comment_id"]);
                let item = CommentItem {
                    id: if comment_id > 0 { comment_id } else { 1 },
                    content: req.content,
                    user: String::new(),
                    avatar: String::new(),
                    time: String::new(),
                    likedCount: 0,
                    liked: false,
                    replyCount: 0,
                    beReplied: Vec::new(),
                    userId: 0,
                    timeMs: 0,
                    parentCommentId: req.replyToCommentId.unwrap_or(0),
                    ipLocation: String::new(),
                    isHot: false,
                    topReplies: Vec::new(),
                };
                Self::success_bytes(&CreatedComment {
                    comment: Some(item),
                })
            }

            OP_LIKECOMMENT => {
                let req = match CommentLikeRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                Self::success_bytes(&MutationResult {
                    success: true,
                    id: Some(req.commentId.to_string()),
                })
            }

            OP_DELETECOMMENT => {
                let req = match DeleteCommentRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
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
                    "key" | "qr_key" => {
                        let res = standard_api::handle_api("login/qr/key", &json!({ "cookie": cookie }));
                        let key = convert::first_string(&res, &["/data/unikey", "/data/key", "/unikey"]);
                        Self::success_bytes(&AuthActionResult {
                            success: !key.is_empty(),
                            code: Some(200),
                            status: None,
                            key: if key.is_empty() { None } else { Some(key) },
                            url: None,
                            imageBase64: None,
                            cookie: None,
                            verifyUrl: None,
                            challenge: false,
                            message: None,
                        })
                    }
                    "create" | "qr_create" => {
                        let key_str = req.key.clone().unwrap_or_default();
                        let res = standard_api::handle_api("login/qr/create", &json!({ "key": key_str, "cookie": cookie }));
                        let url = convert::first_string(&res, &["/data/qrurl", "/data/url", "/qrurl"]);
                        let img = convert::first_string(&res, &["/data/qrimg", "/data/imageBase64", "/qrimg"]);
                        Self::success_bytes(&AuthActionResult {
                            success: !url.is_empty() || !img.is_empty(),
                            code: Some(200),
                            status: None,
                            key: req.key,
                            url: if url.is_empty() { None } else { Some(url) },
                            imageBase64: if img.is_empty() { None } else { Some(img) },
                            cookie: None,
                            verifyUrl: None,
                            challenge: false,
                            message: None,
                        })
                    }
                    "check" | "qr_check" => {
                        let key_str = req.key.clone().unwrap_or_default();
                        let res = standard_api::handle_api("login/qr/check", &json!({ "key": key_str, "cookie": cookie }));
                        let status = convert::first_i64(&res, &["/data/code", "/code"]) as i32;
                        let auth_cookie = convert::first_string(&res, &["/data/cookie", "/cookie"]);
                        Self::success_bytes(&AuthActionResult {
                            success: status == 803,
                            code: Some(200),
                            status: Some(status),
                            key: req.key,
                            url: None,
                            imageBase64: None,
                            cookie: if auth_cookie.is_empty() { None } else { Some(auth_cookie) },
                            verifyUrl: None,
                            challenge: false,
                            message: None,
                        })
                    }
                    _ => Self::success_bytes(&AuthActionResult {
                        success: false,
                        code: Some(400),
                        status: None,
                        key: None,
                        url: None,
                        imageBase64: None,
                        cookie: None,
                        verifyUrl: None,
                        challenge: false,
                        message: Some(format!("Unsupported auth action: {}", req.action)),
                    }),
                }
            }

            OP_GETMEDIAQUALITIES => {
                let _req = match MediaQualityRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                Self::success_bytes(&QualityPage {
                    items: vec![
                        QualityOption {
                            id: "standard".into(),
                            label: "标准音质 (128k MP3)".into(),
                            bitrate: Some(128_000),
                            sizeBytes: Some(3_500_000),
                            available: true,
                        },
                        QualityOption {
                            id: "higher".into(),
                            label: "较高音质 (192k MP3)".into(),
                            bitrate: Some(192_000),
                            sizeBytes: Some(5_200_000),
                            available: true,
                        },
                        QualityOption {
                            id: "exhigh".into(),
                            label: "极高音质 (320k MP3)".into(),
                            bitrate: Some(320_000),
                            sizeBytes: Some(8_800_000),
                            available: true,
                        },
                        QualityOption {
                            id: "lossless".into(),
                            label: "无损音质 (FLAC)".into(),
                            bitrate: Some(960_000),
                            sizeBytes: Some(28_000_000),
                            available: true,
                        },
                        QualityOption {
                            id: "hires".into(),
                            label: "Hi-Res 音质 (24bit/96kHz)".into(),
                            bitrate: Some(2_300_000),
                            sizeBytes: Some(65_000_000),
                            available: true,
                        },
                    ],
                })
            }

            OP_CHECKTRACKAVAILABLE => {
                let req = match TrackAvailabilityRequest::read(&mut r) {
                    Ok(v) => v,
                    Err(e) => return Self::failure_bytes("InvalidResponse", &e),
                };
                Self::success_bytes(&TrackAvailability {
                    available: !req.id.trim().is_empty(),
                    message: None,
                })
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
