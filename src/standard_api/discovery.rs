use super::*;
use crate::request::ModuleResponse;
use std::collections::HashSet;

const IP_PLAYLIST_BASE: i64 = 1 << 51;
const IP_PLAYLIST_SPAN: i64 = 1 << 32;

fn limit(params: &Value) -> usize {
    params
        .get("limit")
        .or_else(|| params.get("pagesize"))
        .and_then(value_to_i64)
        .unwrap_or(30)
        .clamp(1, 100) as usize
}

pub(super) fn query(params: &Value) -> Value {
    let mut query = with_page_params(params);
    let mut cookie = query.get("cookie").cloned().unwrap_or_else(|| json!({}));
    crate::device::merge_device_identity(&mut cookie);
    query["cookie"] = cookie;
    query
}

pub(super) fn read(result: Result<ModuleResponse, ModuleResponse>) -> Result<Value, Value> {
    match result {
        Ok(response) => {
            let body = response.body.to_json();
            let error = first_i64(&body, &["/error_code", "/errcode"]);
            if error != 0
                || body
                    .get("status")
                    .and_then(value_to_i64)
                    .is_some_and(|s| s != 1)
            {
                return Err(json!({"code": 502, "msg": "Kugou discovery request failed",
                    "data": {"upstreamCode": error,
                        "reason": first_string(&body, &["/errmsg", "/error", "/msg"])}}));
            }
            Ok(body)
        }
        Err(response) => {
            let body = response.body.to_json();
            Err(
                json!({"code": 502, "msg": "Kugou discovery request failed", "data": {
                    "upstreamStatus": response.status,
                    "upstreamCode": first_i64(&body, &["/error_code", "/errcode"]),
                    "reason": first_string(&body, &["/errmsg", "/error", "/msg"]),
                }}),
            )
        }
    }
}

pub(super) fn songs(body: &Value, params: &Value) -> Vec<Value> {
    first_array(
        body,
        &[
            "/data/song_list",
            "/data/songs",
            "/data/list",
            "/data/info",
            "/data",
            "/songs",
        ],
    )
    .into_iter()
    .flatten()
    .filter_map(transform_kugou_song)
    .take(limit(params))
    .collect()
}

pub(super) fn playlists(params: &Value, ctx: &Ctx, selected: bool) -> Value {
    let mut query = query(params);
    query["is_selected"] = json!(i64::from(selected));
    if first_string(params, &["/order"]) == "new" {
        query["sort"] = json!(2);
    }
    match read(crate::modules::top::handle_playlist(&query, ctx)) {
        Ok(body) => playlist_response(&body, params),
        Err(error) => error,
    }
}

fn playlist_response(body: &Value, params: &Value) -> Value {
    let mut seen = HashSet::new();
    let items: Vec<_> = first_array(
        body,
        &[
            "/data/special_list",
            "/data/playlists",
            "/data/list",
            "/data/info",
            "/data",
        ],
    )
    .into_iter()
    .flatten()
    .filter_map(|item| {
        let mut playlist = transform_kugou_playlist(item)?;
        // Recommendation `pic` is the creator avatar, not the playlist artwork.
        let cover = standard_cover(
            item,
            &[
                "/flexible_cover",
                "/imgurl",
                "/sizable_cover",
                "/coverUrl",
                "/cover",
                "/pic",
            ],
        );
        playlist["coverUrl"] = json!(cover.clone());
        playlist["coverImgUrl"] = json!(cover.clone());
        playlist["picUrl"] = json!(cover);
        playlist["name"] = playlist["title"].clone();
        playlist["trackCount"] = playlist["total"].clone();
        Some(playlist)
    })
    .filter(|item| seen.insert(item["id"].clone().to_string()))
    .take(limit(params))
    .collect();
    json!({"code": 0, "recommend": items.clone(), "result": items.clone(), "playlists": items.clone(), "data": items,
        "hasMore": first_i64(body, &["/data/has_next"]) == 1})
}

pub(super) fn albums(params: &Value, ctx: &Ctx) -> Value {
    match read(crate::modules::top::handle_album(
        &with_page_params(params),
        ctx,
    )) {
        Ok(body) => album_response(&body, params),
        Err(error) => error,
    }
}

fn album_response(body: &Value, params: &Value) -> Value {
    let area = first_string(params, &["/area"]).to_ascii_uppercase();
    let regions = match area.as_str() {
        "" | "ALL" => vec!["chn", "eur", "jpn", "kor"],
        "ZH" | "CHN" => vec!["chn"],
        "EA" | "EUR" => vec!["eur"],
        "JP" | "JPN" => vec!["jpn"],
        "KR" | "KOR" => vec!["kor"],
        _ => return json!({"code": 400, "msg": "Unsupported album region"}),
    };
    let groups: Vec<_> = regions
        .iter()
        .filter_map(|region| body["data"][*region].as_array())
        .collect();
    let mut seen = HashSet::new();
    // Interleave regions, otherwise the ALL wall contains only Chinese albums.
    let mut items = Vec::new();
    let now = (crate::cache::now_epoch_secs() * 1000.0) as i64;
    for index in 0..groups.iter().map(|g| g.len()).max().unwrap_or(0) {
        for group in &groups {
            if let Some(item) = group.get(index) {
                let release = album_release_time(item);
                // The new-album feed includes pre-releases whose song lists are still empty.
                if release.is_some_and(|time| time > now) {
                    continue;
                }
                if let Some(mut album) = transform_kugou_album(item) {
                    if seen.insert(album["id"].to_string()) {
                        album["trackCount"] =
                            json!(first_i64(item, &["/songcount", "/song_count", "/count"]));
                        if let Some(time) = release {
                            album["publishDate"] = json!(time);
                        }
                        items.push(album);
                    }
                }
            }
        }
    }
    items.truncate(limit(params));
    json!({"code": 0, "albums": items, "data": items})
}

pub(super) fn artists(params: &Value, ctx: &Ctx) -> Value {
    let mut query = params.clone();
    query["hotsize"] = json!(limit(params));
    match read(crate::modules::artist::handle_lists(&query, ctx)) {
        Ok(body) => artist_response(&body, params),
        Err(error) => error,
    }
}

fn artist_response(body: &Value, params: &Value) -> Value {
    let items: Vec<_> = first_array(body, &["/data/info/0/singer", "/data/singer", "/data/info"])
        .into_iter()
        .flatten()
        .filter_map(transform_kugou_artist)
        .take(limit(params))
        .collect();
    json!({"code": 0, "artists": items, "data": items})
}

pub(super) fn new_songs(params: &Value, ctx: &Ctx) -> Value {
    let mut query = with_page_params(params);
    // Standard region codes are not native Kugou rank IDs.
    let region = first_i64(params, &["/type"]);
    if region != 0 {
        return regional_new_songs(params, ctx, region);
    }
    query["type"] = json!(21608);
    match read(crate::modules::top::handle_song(&query, ctx)) {
        Ok(body) => {
            let items = songs(&body, params);
            json!({"code": 0, "data": items, "result": items, "songs": items})
        }
        Err(error) => error,
    }
}

pub(super) fn personal_fm(params: &Value, ctx: &Ctx) -> Value {
    match read(crate::modules::fm::handle_personal_fm(&query(params), ctx)) {
        Ok(body) => {
            let items = songs(&body, params);
            json!({"code": 0, "data": items, "songs": items})
        }
        Err(error) => error,
    }
}

fn regional_new_songs(params: &Value, ctx: &Ctx, region: i64) -> Value {
    let area = match region {
        7 => "ZH",
        96 => "EA",
        8 => "JP",
        16 => "KR",
        _ => return json!({"code": 400, "msg": "Unsupported new-song region"}),
    };
    let mut query = query(params);
    query["area"] = json!(area);
    query["limit"] = json!(limit(params).min(12));
    let response = albums(&query, ctx);
    if response["code"] != 0 {
        return response;
    }
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    let mut last_error = None;
    // The native new-song feed has no equivalent Standard region selector.
    // Use songs from actual new albums of that region, not unrelated regional charts.
    for chunk in response["albums"]
        .as_array()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .chunks(4)
    {
        let batches = std::thread::scope(|scope| {
            let tasks: Vec<_> = chunk
                .iter()
                .map(|album| {
                    let mut query = params.clone();
                    query["id"] = album["id"].clone();
                    query["pagesize"] = json!(4);
                    query["page"] = json!(1);
                    scope
                        .spawn(move || read(crate::modules::album::handle_album_songs(&query, ctx)))
                })
                .collect();
            tasks
                .into_iter()
                .map(|task| {
                    task.join().unwrap_or_else(|_| {
                        Err(json!({"code": 502, "msg": "Failed to read regional album songs"}))
                    })
                })
                .collect::<Vec<_>>()
        });
        for batch in batches {
            match batch {
                Ok(body) => items.extend(
                    songs(&body, params)
                        .into_iter()
                        .filter(|song| seen.insert(song["id"].to_string())),
                ),
                Err(error) => last_error = Some(error),
            }
        }
        if items.len() >= limit(params) {
            break;
        }
    }
    if items.is_empty() {
        if let Some(error) = last_error {
            return error;
        }
    }
    items.truncate(limit(params));
    json!({"code": 0, "data": items, "result": items, "songs": items, "source": "regional_new_albums"})
}

pub(super) fn banners(params: &Value, ctx: &Ctx) -> Value {
    match read(crate::modules::yueku::handle_banner(params, ctx)) {
        Ok(body) => banner_response(&body),
        Err(error) => error,
    }
}

fn banner_response(body: &Value) -> Value {
    let items: Vec<_> = first_array(body, &["/data/ads", "/data/banners", "/data"])
        .into_iter().flatten().filter_map(|ad| {
            let image = standard_cover(ad, &["/img_url", "/imageUrl", "/picUrl"]);
            if image.is_empty() { return None; }
            let url = first_string(ad, &["/extra/url", "/url"]);
            let link = crate::util::query_string_to_json(url.split_once('?').map_or("", |(_, query)| query));
            let ip_id = first_i64(&link, &["/ip_id"]);
            let (target_type, target_id) = if ip_id > 0 && ip_id < IP_PLAYLIST_SPAN {
                (1000, (IP_PLAYLIST_BASE + ip_id).to_string())
            } else {
                // Keep only explicit destinations that the player can actually open.
                let hash = first_string(ad, &["/hash", "/extra/hash"]);
                let playlist = first_i64(ad, &["/specialid", "/extra/specialid"]);
                if hash.len() == 32 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                    (1, hash.to_ascii_lowercase())
                } else if playlist > 0 {
                    (1000, playlist.to_string())
                } else { return None; }
            };
            Some(json!({"adid": first_string(ad, &["/id"]), "imageUrl": image,
                "title": first_string(ad, &["/title"]), "targetType": target_type, "targetId": target_id}))
        }).collect();
    json!({"code": 0, "banners": items, "data": items})
}

pub(super) fn ip_id(params: &Value) -> Option<i64> {
    let id = first_i64(params, &["/id"]);
    (id > IP_PLAYLIST_BASE && id < IP_PLAYLIST_BASE + IP_PLAYLIST_SPAN)
        .then_some(id - IP_PLAYLIST_BASE)
}

pub(super) fn ip_playlist(params: &Value, ctx: &Ctx, id: i64, detail: bool) -> Value {
    let mut query = with_page_params(params);
    query["id"] = json!(id);
    query["type"] = json!("audios");
    query["sort"] = json!(1);
    let size = first_i64(&query, &["/pagesize"]).clamp(1, 100);
    let offset = first_i64(params, &["/offset"]).max(0);
    let skip = (offset % size) as usize;
    query["pagesize"] = json!(size);
    query["page"] = json!(offset / size + 1);
    let body = match read(crate::modules::ip::handle(&query, ctx)) {
        Ok(body) => body,
        Err(error) => return error,
    };
    let Some(raw) = first_array(&body, &["/data"]) else {
        return json!({"code": 502, "msg": "Kugou topic contains no song list"});
    };
    let total = topic_total(&body);
    let mut raw = raw.clone();
    if skip > 0 && raw.len() as i64 == size && offset - skip as i64 + size < total {
        query["page"] = json!(offset / size + 2);
        let next = match read(crate::modules::ip::handle(&query, ctx)) {
            Ok(body) => body,
            Err(error) => return error,
        };
        if let Some(items) = next["data"].as_array() {
            raw.extend(items.iter().cloned());
        }
    }
    let take = size.min(total.saturating_sub(offset).max(0)) as usize;
    let tracks: Vec<_> = raw
        .iter()
        .skip(skip)
        .take(take)
        .filter_map(transform_kugou_song)
        .collect();
    let has_more = offset.saturating_add(tracks.len() as i64) < total;
    let mut data = json!({"id": IP_PLAYLIST_BASE + id, "tracks": tracks,
        "total": total, "hasMore": has_more});
    if detail {
        let metadata = match read(crate::modules::ip_more::handle_dateil(&query, ctx)) {
            Ok(body) => body,
            Err(error) => return error,
        };
        let Some(info) = metadata.pointer("/data/0") else {
            return json!({"code": 502, "msg": "Kugou topic metadata is missing"});
        };
        data["title"] = json!(first_string(info, &["/base/name", "/config/title"]));
        data["name"] = data["title"].clone();
        data["coverUrl"] = json!(standard_cover(
            info,
            &["/pic/avatar_square", "/base/avatar", "/config/big_img"]
        ));
        data["description"] = info["intro"].clone();
    }
    json!({"code": 0, "hasMore": has_more, "songs": tracks, "playlist": data, "data": data})
}

fn topic_total(body: &Value) -> i64 {
    let total = first_i64(body, &["/total"]).max(0);
    // Large topics advertise their entire catalog but expose only page_total songs.
    body.pointer("/extra/page_total")
        .and_then(value_to_i64)
        .filter(|value| *value >= 0)
        .map_or(total, |available| total.min(available))
}

pub(super) fn similar_songs(params: &Value, ctx: &Ctx) -> Value {
    let id = first_string(params, &["/id", "/sid"]);
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return json!({"code": 400, "msg": "A Kugou song hash is required"});
    }
    let mut query = query(params);
    query["hash"] = json!(id);
    let body = match read(crate::modules::audio::handle(&query, ctx)) {
        Ok(body) => body,
        Err(error) => return error,
    };
    let mut audio_id = first_i64(
        &body,
        &[
            "/data/0/album_audio_id",
            "/data/0/mixsongid",
            "/data/0/base/album_audio_id",
            "/data/album_audio_id",
            "/data/mixsongid",
        ],
    );
    if audio_id <= 0 {
        // /audio returns audio_id, but recommendation requires album_audio_id.
        // Search by verified metadata and match the exact hash, never the first title match.
        let name = first_string(&body, &["/data/0/audio_name", "/data/audio_name"]);
        if !name.is_empty() {
            query["keyword"] = json!(name);
            query["pagesize"] = json!(30);
            query["page"] = json!(1);
            let search = match read(crate::modules::search::handle(&query, ctx)) {
                Ok(body) => body,
                Err(error) => return error,
            };
            audio_id = recommendation_seed(&search, &id);
        }
    }
    if audio_id <= 0 {
        return json!({"code": 502, "msg": "Kugou song metadata contains no recommendation seed"});
    }
    query["album_audio_id"] = json!(audio_id.to_string());
    match read(crate::modules::extras::handle_ai_recommend(&query, ctx)) {
        Ok(body) => {
            let mut seen = HashSet::new();
            let tracks: Vec<_> = songs(&body, params)
                .into_iter()
                .filter(|song| song["id"] != id.to_ascii_lowercase())
                .filter(|song| seen.insert(song["id"].to_string()))
                .collect();
            json!({"code": 0, "data": tracks, "songs": tracks})
        }
        Err(error) => error,
    }
}

pub(super) fn intelligence(params: &Value, ctx: &Ctx) -> Value {
    if first_i64(params, &["/pid"]) <= 0 {
        return json!({"code": 400, "msg": "A playlist id is required for intelligence mode"});
    }
    let mut query = params.clone();
    query["id"] = params.get("pid").cloned().unwrap_or(Value::Null);
    query["limit"] = json!(1);
    query["offset"] = json!(0);
    let playlist = super::handle_playlist_tracks(&query, ctx);
    if playlist["code"] != 0 {
        return playlist;
    }
    let seed = first_string(params, &["/id", "/sid"]).to_ascii_lowercase();
    let mut query = params.clone();
    query["id"] = json!(seed);
    query["limit"] = params.get("count").cloned().unwrap_or(json!(20));
    similar_songs(&query, ctx)
}

fn recommendation_seed(body: &Value, hash: &str) -> i64 {
    first_array(body, &["/data/lists", "/data/info", "/data"])
        .into_iter()
        .flatten()
        .flat_map(|item| {
            std::iter::once(item).chain(
                item.get("Group")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten(),
            )
        })
        .find_map(|item| {
            let matches = [
                "/FileHash",
                "/hash",
                "/HQFileHash",
                "/SQFileHash",
                "/hash_128",
                "/hash_320",
                "/hash_flac",
            ]
            .iter()
            .any(|path| first_string(item, &[*path]).eq_ignore_ascii_case(hash));
            let id = first_i64(
                item,
                &[
                    "/AlbumAudioID",
                    "/album_audio_id",
                    "/MixSongID",
                    "/mixsongid",
                ],
            );
            (matches && id > 0).then_some(id)
        })
        .unwrap_or(0)
}

fn album_release_time(album: &Value) -> Option<i64> {
    let numeric = first_i64(album, &["/publish_time", "/publishDate"]);
    if numeric > 0 {
        return Some(if numeric < 100_000_000_000 {
            numeric.saturating_mul(1000)
        } else {
            numeric
        });
    }
    let date = first_string(album, &["/publishtime", "/publish_date"]);
    let date = date.split(' ').next()?;
    let mut parts = date.split('-');
    let mut year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<i64>().ok()?;
    let day = parts.next()?.parse::<i64>().ok()?;
    if parts.next().is_some() || !(1000..=9999).contains(&year) || !(1..=12).contains(&month) {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day < 1 || day > month_days[(month - 1) as usize] {
        return None;
    }
    // Gregorian date -> Unix days; upstream release dates use China Standard Time.
    year -= i64::from(month <= 2);
    let era = year / 400;
    let year_of_era = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let days = era * 146097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year
        - 719468;
    Some((days * 86400 - 8 * 3600) * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::BodyValue;
    use std::collections::HashMap;

    #[test]
    fn manifest_exposes_homepage_methods_with_the_host_spelling() {
        let manifest: Value =
            serde_json::from_str(include_str!("../../packaging/manifest.json")).unwrap();
        for method in [
            "recommend/songs",
            "recommend/resource",
            "personalized",
            "top_playlist",
            "top_playlist_highquality",
            "top_album",
            "top_artists",
            "top_song",
            "personalized_newsong",
            "personal_fm",
            "banner",
            "simi_song",
            "playmode/intelligence/list",
        ] {
            assert_eq!(manifest["apiMap"][method], method, "{method}");
        }
    }

    #[test]
    fn invalid_home_requests_fail_locally_without_touching_user_data() {
        assert_eq!(
            handle_api("simi_song", &json!({"id": "invalid"}))["code"],
            400
        );
        assert_eq!(
            handle_api("playmode/intelligence/list", &json!({"id": "invalid"}))["code"],
            400
        );
        assert_eq!(handle_api("top_song", &json!({"type": 999}))["code"], 400);
    }

    #[test]
    fn daily_and_fm_read_the_actual_song_list_and_keep_hashes_and_artwork() {
        let items = songs(
            &json!({"data": {"song_list": [{
                "hash": "AABB", "songname": "Track", "author_name": "Artist",
                "time_length": 210000, "sizable_cover": "http://imge.kugou.com/{size}/album.jpg"
            }]}}),
            &json!({"limit": "1"}),
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], "aabb");
        assert_eq!(items[0]["duration"], 210);
        assert_eq!(items[0]["coverUrl"], "https://imge.kugou.com/400/album.jpg");
    }

    #[test]
    fn discovery_playlists_use_public_ids_and_playlist_artwork_not_creator_avatars() {
        let response = playlist_response(
            &json!({"data": {"has_next": 1, "special_list": [
                {"specialid": 123, "specialname": "List", "global_collection_id": "collection_3_99_7_0",
                 "flexible_cover": "http://imge.kugou.com/{size}/list.jpg", "pic": "http://imge.kugou.com/avatar.jpg"},
                {"specialid": 123, "specialname": "Duplicate"}
            ]}}),
            &json!({"limit": 3}),
        );
        assert_eq!(response["playlists"].as_array().unwrap().len(), 1);
        assert_eq!(response["playlists"][0]["id"], 123);
        assert_eq!(
            response["playlists"][0]["coverUrl"],
            "https://imge.kugou.com/400/list.jpg"
        );
        assert_eq!(response["recommend"], response["result"]);
        assert_eq!(response["playlists"], response["data"]);
    }

    #[test]
    fn all_album_regions_are_interleaved_and_deduplicated() {
        let body = json!({"data": {
            "chn": [{"albumid": 1, "albumname": "CN1"}, {"albumid": 2, "albumname": "CN2"}],
            "eur": [{"albumid": 3, "albumname": "EU"}],
            "jpn": [{"albumid": 4, "albumname": "JP"}],
            "kor": [{"albumid": 4, "albumname": "Duplicate"}, {"albumid": 5, "albumname": "KR"}]
        }});
        let all = album_response(&body, &json!({"limit": 4}));
        assert_eq!(
            all["albums"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["id"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 3, 4, 2]
        );
        let japanese = album_response(&body, &json!({"area": "JP"}));
        assert_eq!(japanese["albums"][0]["id"], 4);
        assert_eq!(japanese["albums"].as_array().unwrap().len(), 1);
        assert_eq!(
            album_response(&body, &json!({"area": "unknown"}))["code"],
            400
        );
    }

    #[test]
    fn hot_artists_read_the_hot_group_not_alphabetical_groups() {
        let response = artist_response(
            &json!({"data": {"info": [
                {"singer": [{"singerid": 1, "singername": "Hot", "imgurl": "//singerimg.kugou.com/{size}/avatar.jpg"}]},
                {"singer": [{"singerid": 2, "singername": "Alphabetical"}]}
            ]}}),
            &json!({}),
        );
        assert_eq!(response["artists"].as_array().unwrap().len(), 1);
        assert_eq!(
            response["artists"][0]["avatarUrl"],
            "https://singerimg.kugou.com/400/avatar.jpg"
        );
    }

    #[test]
    fn banner_destinations_are_openable_and_use_a_separate_namespace() {
        let response = banner_response(&json!({"data": {"ads": [
            {"id": 1, "title": "Topic", "img_url": "//imge.kugou.com/banner.jpg",
                "extra": {"url": "https://h5.kugou.com/?ip_id=88104&test=1"}},
            {"id": 2, "img_url": "//imge.kugou.com/banner.jpg", "extra": {"url": "https://example.com/ad"}},
            {"id": 3, "img_url": "", "extra": {"url": "https://h5.kugou.com/?ip_id=99"}}
        ]}}));
        assert_eq!(response["banners"].as_array().unwrap().len(), 1);
        let target = response["banners"][0]["targetId"].clone();
        assert_eq!(ip_id(&json!({"id": target})), Some(88104));
        assert_eq!(ip_id(&json!({"id": 7235936})), None);
        assert_eq!(ip_id(&json!({"id": RANK_PLAYLIST_BASE + 8888})), None);
        assert_eq!(response["banners"][0]["targetType"], 1000);
    }

    #[test]
    fn nested_topic_songs_keep_real_hash_titles_artists_and_duration() {
        let track = transform_kugou_song(&json!({
            "base": {"songname": "Topic song", "author_name": "Artist"},
            "audio_info": {"hash": "ABCDEF", "timelength": 123000},
            "album_info": {"cover": "https://imge.kugou.com/album.jpg"}
        }))
        .unwrap();
        assert_eq!(track["id"], "abcdef");
        assert_eq!(track["title"], "Topic song");
        assert_eq!(track["artist"], "Artist");
        assert_eq!(track["duration"], 123);
    }

    #[test]
    fn topic_pagination_uses_the_upstream_accessible_limit_not_the_catalog_total() {
        assert_eq!(
            topic_total(&json!({"total": 9802, "extra": {"page_total": 1000}})),
            1000
        );
        assert_eq!(
            topic_total(&json!({"total": 158, "extra": {"page_total": 158}})),
            158
        );
        assert_eq!(
            topic_total(&json!({"total": 10, "extra": {"page_total": 1000}})),
            10
        );
        assert_eq!(topic_total(&json!({"total": 10})), 10);
    }

    #[test]
    fn recommendation_seed_requires_exact_hash_not_merely_the_same_title() {
        let body = json!({"data": {"lists": [
            {"FileHash": "WRONG", "AlbumAudioID": 1},
            {"FileHash": "ABCDEF", "AlbumAudioID": "2"},
            {"FileHash": "OTHER", "Group": [{"FileHash": "123ABC", "AlbumAudioID": 3}]},
            {"FileHash": "NATIVE", "MixSongID": "4", "Audioid": 999}
        ]}});
        assert_eq!(recommendation_seed(&body, "abcdef"), 2);
        assert_eq!(recommendation_seed(&body, "123abc"), 3);
        assert_eq!(recommendation_seed(&body, "native"), 4);
        assert_eq!(recommendation_seed(&body, "missing"), 0);
    }

    #[test]
    fn release_dates_have_real_timestamps_and_prereleases_are_not_openable_albums() {
        assert_eq!(
            album_release_time(&json!({"publishtime": "1970-01-02 00:00:00"})),
            Some(57_600_000)
        );
        assert_eq!(
            album_release_time(&json!({"publishtime": "2026-02-29"})),
            None
        );
        assert!(album_release_time(&json!({"publishtime": "2024-02-29"})).is_some());
        let response = album_response(
            &json!({"data": {"chn": [
                {"albumid": 1, "albumname": "Not released", "publishtime": "9999-12-31"},
                {"albumid": 2, "albumname": "Released", "publishtime": "2020-01-01"}
            ]}}),
            &json!({}),
        );
        assert_eq!(response["albums"].as_array().unwrap().len(), 1);
        assert_eq!(response["albums"][0]["id"], 2);
        assert!(response["albums"][0]["publishDate"].as_i64().unwrap() > 0);
    }

    #[test]
    fn discovery_does_not_mask_upstream_denial_as_an_empty_success() {
        let response = ModuleResponse {
            status: 200,
            body: BodyValue::Json(json!({"status": 0, "error_code": 20010, "errmsg": "denied"})),
            headers: HashMap::new(),
            cookie: vec![],
        };
        let error = read(Ok(response)).unwrap_err();
        assert_eq!(error["code"], 502);
        assert_eq!(error["data"]["upstreamCode"], 20010);
        assert_eq!(error["data"]["reason"], "denied");
    }
}
