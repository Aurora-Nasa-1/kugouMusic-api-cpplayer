//! Account library writes. Public playlist IDs, local listids, hashes and fileids
//! are deliberately resolved separately before any mutation is sent.

use super::*;
use std::collections::HashSet;

const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    Lists,
    Tracks,
    Audio,
    AddTracks,
    RemoveTracks,
    DeletePlaylist,
    AddPlaylist,
    PublicInfo,
    PublicDetail,
}

type ApiResult = Result<Value, Value>;

pub(super) fn handle(method: &str, params: &Value, ctx: &Ctx) -> Value {
    let mut call = |operation, query: &Value| {
        let response = match operation {
            Operation::Lists => crate::modules::user::handle_playlist(query, ctx),
            Operation::Tracks => crate::modules::playlist::handle_track_all_new(query, ctx),
            Operation::Audio => {
                crate::device::ensure_registered();
                let mut query = query.clone();
                let mut cookie = crate::modules::q_cookie(&query);
                crate::device::merge_device_identity(&mut cookie);
                query["cookie"] = cookie;
                crate::modules::audio::handle(&query, ctx)
            }
            Operation::AddTracks => crate::modules::playlist::handle_tracks_add(query, ctx),
            Operation::RemoveTracks => crate::modules::playlist::handle_tracks_del(query, ctx),
            Operation::DeletePlaylist => crate::modules::playlist::handle_del(query, ctx),
            Operation::AddPlaylist => crate::modules::playlist::handle_add(query, ctx),
            Operation::PublicInfo => crate::modules::forward(
                query,
                ctx,
                "GET",
                "/api/v3/special/info",
                None,
                Some(json!({"specialid": query["id"], "plat": 0})),
                None,
                "android",
                &[("x-router", "mobilecdnbj.kugou.com")],
                false,
                false,
            ),
            Operation::PublicDetail => crate::modules::playlist::handle_detail(query, ctx),
        };
        response
            .map(|response| response.body.to_json())
            .map_err(|response| playlist_request_error(&response, "update account library"))
    };
    dispatch_with(method, params, &mut call).unwrap_or_else(|error| error)
}

fn error(code: i64, message: &str) -> Value {
    json!({"code": code, "msg": message})
}

fn login_query(params: &Value) -> ApiResult {
    let mut userid = crate::modules::param_or_cookie_num(params, "userid", 0);
    if userid <= 0 {
        userid = crate::modules::param_or_cookie_num(params, "uid", 0);
    }
    let token = crate::modules::param_or_cookie_str(params, "token", "");
    if userid <= 0 || token.trim().is_empty() || token == "undefined" || token == "null" {
        return Err(error(401, "Account library operations require login"));
    }
    // The native list reader prefers cookies, whereas writers prefer parameters.
    // Pin both to the same identity so uid cannot switch a write to another user.
    if let Some(uid) = params.get("uid") {
        if value_to_i64(uid) != Some(userid) {
            return Err(error(403, "Cannot access another user's account library"));
        }
    }
    let cookie_user = params.pointer("/cookie/userid")
        .or_else(|| params.pointer("/cookie/uid"))
        .and_then(value_to_i64);
    let cookie_token = params.pointer("/cookie/token").and_then(Value::as_str);
    if cookie_user.is_some_and(|id| id != userid)
        || cookie_token.is_some_and(|value| value != token)
    {
        return Err(error(400, "Conflicting account credentials"));
    }
    let mut query = params.clone();
    query["userid"] = json!(userid);
    query["uid"] = json!(userid);
    query["token"] = json!(token);
    let mut cookie = params
        .get("cookie")
        .filter(|cookie| cookie.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    cookie["userid"] = json!(userid);
    cookie["uid"] = json!(userid);
    cookie["token"] = json!(token);
    query["cookie"] = cookie;
    Ok(query)
}

fn boolean(params: &Value, key: &str, default: bool) -> Result<bool, Value> {
    match params.get(key) {
        None => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(Value::String(value)) if value == "true" || value == "1" => Ok(true),
        Some(Value::String(value)) if value == "false" || value == "0" => Ok(false),
        Some(Value::Number(value)) if value.as_i64() == Some(1) => Ok(true),
        Some(Value::Number(value)) if value.as_i64() == Some(0) => Ok(false),
        _ => Err(error(400, &format!("Invalid {key} value"))),
    }
}

fn local_id(params: &Value, paths: &[&str]) -> Result<String, Value> {
    let id = first_string(params, paths);
    if id.parse::<i64>().ok().filter(|id| *id > 0).is_none()
        || rank_id_from_playlist_params(&json!({"id": id})).is_some()
        || discovery::ip_id(&json!({"id": id})).is_some()
    {
        return Err(error(400, "A valid account-local playlist id is required"));
    }
    Ok(id)
}

fn hashes(value: Option<&Value>) -> Result<Vec<String>, Value> {
    let values: Vec<Value> = match value {
        Some(Value::String(value)) => value.split(',').map(|id| json!(id.trim())).collect(),
        Some(Value::Array(values)) => values.clone(),
        _ => return Err(error(400, "Missing song hashes")),
    };
    if values.is_empty() || values.len() > 1000 {
        return Err(error(400, "Expected between 1 and 1000 song hashes"));
    }
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for value in values {
        let Some(id) = value.as_str() else {
            return Err(error(400, "Song ids must be Kugou audio hashes"));
        };
        let id = id.trim().to_ascii_lowercase();
        if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(error(
                400,
                "Song ids must be 32-character Kugou audio hashes",
            ));
        }
        if seen.insert(id.clone()) {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn upstream_error(body: &Value) -> Option<Value> {
    let failure = [
        "/error_code",
        "/errcode",
        "/data/error_code",
        "/data/errcode",
    ]
    .iter()
    .find_map(|path| {
        body.pointer(path)
            .and_then(value_to_i64)
            .filter(|code| *code != 0)
            .map(|code| (*path, code))
    })
    .or_else(|| {
        ["/code", "/data/code"].iter().find_map(|path| {
            body.pointer(path)
                .and_then(value_to_i64)
                .filter(|code| *code != 0 && *code != 200)
                .map(|code| (*path, code))
        })
    })
    .or_else(|| {
        ["/status", "/data/status"].iter().find_map(|path| {
            body.pointer(path)
                .and_then(value_to_i64)
                .filter(|status| *status != 1)
                .map(|status| (*path, status))
        })
    })?;
    let message = first_string(
        body,
        &[
            "/errmsg",
            "/msg",
            "/message",
            "/error",
            "/data/errmsg",
            "/data/msg",
            "/data/message",
        ],
    );
    Some(json!({
        "code": 502,
        "msg": if message.is_empty() {
            format!("Upstream rejected account library operation ({}={})", failure.0, failure.1)
        } else { message },
        "upstreamCode": failure.1,
        "upstreamField": failure.0
    }))
}

fn read(
    call: &mut impl FnMut(Operation, &Value) -> ApiResult,
    operation: Operation,
    query: &Value,
) -> ApiResult {
    let body = call(operation, query)?;
    match upstream_error(&body) {
        Some(error) => Err(error),
        None => Ok(body),
    }
}

fn mutation(
    call: &mut impl FnMut(Operation, &Value) -> ApiResult,
    operation: Operation,
    query: &Value,
) -> ApiResult {
    let body = call(operation, query)?;
    let mut ambiguous = upstream_error(&body).map(|error| mutation_error(operation, error));
    if ambiguous
        .as_ref()
        .is_some_and(|error| error["_verifyState"] != true)
    {
        return Err(ambiguous.unwrap());
    }
    for path in ["/data", "/data/info", "/data/results"] {
        if let Some(items) = body.pointer(path).and_then(Value::as_array) {
            for (index, item) in items.iter().enumerate() {
                if let Some(mut error) = upstream_error(item) {
                    let field = first_string(&error, &["/upstreamField"]);
                    error["upstreamField"] = json!(format!("{path}/{index}{field}"));
                    error["partial"] = json!(true);
                    let error = mutation_error(operation, error);
                    if error["_verifyState"] != true {
                        return Err(error);
                    }
                    if ambiguous.is_none() {
                        ambiguous = Some(error);
                    }
                }
            }
        }
    }
    if let Some(error) = ambiguous {
        return Err(error);
    }
    let success = ["/status", "/data/status"]
        .iter()
        .any(|path| body.pointer(path).and_then(value_to_i64) == Some(1))
        || ["/code", "/data/code"].iter().any(|path| {
            body.pointer(path)
                .and_then(value_to_i64)
                .is_some_and(|code| code == 0 || code == 200)
        });
    if !success {
        return Err(error(
            502,
            "Upstream did not confirm the account library operation",
        ));
    }
    Ok(json!({"code": 200, "status": 200, "body": body}))
}

fn mutation_error(operation: Operation, mut error: Value) -> Value {
    error["upstreamOperation"] = json!(match operation {
        Operation::AddTracks => "playlist/tracks/add",
        Operation::RemoveTracks => "playlist/tracks/del",
        Operation::AddPlaylist => "playlist/add",
        Operation::DeletePlaylist => "playlist/del",
        _ => "account/library",
    });
    // A per-song code is not an HTTP status. Do not guess that code=1 means
    // success or failure: only a fresh playlist read can resolve the ambiguity.
    if matches!(operation, Operation::AddTracks | Operation::RemoveTracks)
        && error["upstreamCode"] == 1
    {
        error["_verifyState"] = json!(true);
        error["partial"] = json!(true);
    }
    error
}

fn verify_track_batch(
    query: &Value,
    operation: Operation,
    batch: &[Value],
    call: &mut impl FnMut(Operation, &Value) -> ApiResult,
) -> Result<usize, Value> {
    let add = operation == Operation::AddTracks;
    let mut confirmed = 0;
    for attempt in 0..3 {
        let items = all_pages(query, Operation::Tracks, call)?;
        let mut present = HashSet::new();
        for item in &items {
            let identity = if add {
                let hash = song_hash(item);
                if hash.len() != 32 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(error(
                        502,
                        "Playlist verification returned a song without a valid hash",
                    ));
                }
                hash
            } else {
                let fileid = first_i64(item, &["/fileid", "/file_id"]);
                if fileid <= 0 {
                    return Err(error(
                        502,
                        "Playlist verification returned a song without a valid fileid",
                    ));
                }
                fileid.to_string()
            };
            present.insert(identity);
        }
        confirmed = batch
            .iter()
            .filter(|item| {
                let identity = if add {
                    song_hash(item)
                } else {
                    item.to_string()
                };
                present.contains(&identity) == add
            })
            .count();
        if confirmed == batch.len() {
            return Ok(confirmed);
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(100 * (attempt + 1)));
        }
    }
    Ok(confirmed)
}

fn all_pages(
    query: &Value,
    operation: Operation,
    call: &mut impl FnMut(Operation, &Value) -> ApiResult,
) -> Result<Vec<Value>, Value> {
    let mut query = query.clone();
    query["pagesize"] = json!(PAGE_SIZE);
    query["offset"] = json!(0);
    let mut items = Vec::new();
    let mut previous = Vec::new();
    for page in 1..=MAX_PAGES {
        query["page"] = json!(page);
        let body = read(call, operation, &query)?;
        let list = if operation == Operation::Lists {
            playlist_list(&body)
        } else {
            playlist_songs(&body)
        }
        .ok_or_else(|| error(502, "Upstream returned no account library list"))?;
        if !list.is_empty() && *list == previous {
            return Err(error(502, "Upstream repeated an account library page"));
        }
        let total = playlist_total(&body);
        items.extend(list.iter().cloned());
        if total.is_some_and(|total| total >= 0 && items.len() as i64 >= total) {
            return Ok(items);
        }
        if list.len() < PAGE_SIZE {
            if total.is_some_and(|total| total > items.len() as i64) {
                return Err(error(
                    502,
                    "Upstream returned an incomplete account library list",
                ));
            }
            return Ok(items);
        }
        previous = list.clone();
    }
    Err(error(502, "Account library pagination limit exceeded"))
}

fn account_lists(
    query: &Value,
    kind: i64,
    call: &mut impl FnMut(Operation, &Value) -> ApiResult,
) -> Result<Vec<Value>, Value> {
    let mut query = query.clone();
    query["type"] = json!(kind);
    all_pages(&query, Operation::Lists, call)
}

fn owned_list(lists: &[Value], id: &str) -> ApiResult {
    lists
        .iter()
        .find(|item| {
            first_string(item, &["/listid", "/id"]) == id
                && item
                    .get("type")
                    .and_then(value_to_i64)
                    .is_none_or(|kind| kind == 0)
        })
        .cloned()
        .ok_or_else(|| error(403, "Playlist is not owned by the logged-in user"))
}

fn liked_list(lists: &[Value]) -> ApiResult {
    let matches: Vec<_> = lists
        .iter()
        .filter(|item| {
            let name = first_string(item, &["/name", "/listname", "/title"]);
            matches!(
                name.trim(),
                "我喜欢" | "我喜欢的音乐" | "我喜欢的歌曲" | "喜欢的音乐"
            ) && item
                .get("type")
                .and_then(value_to_i64)
                .is_none_or(|kind| kind == 0)
        })
        .collect();
    match matches.as_slice() {
        [item] => Ok((*item).clone()),
        [] => Err(error(
            404,
            "The account has no identifiable Kugou liked-songs playlist",
        )),
        _ => Err(error(
            409,
            "The account has multiple possible liked-songs playlists",
        )),
    }
}

fn song_hash(item: &Value) -> String {
    first_string(
        item,
        &[
            "/hash",
            "/FileHash",
            "/hash_128",
            "/audio_info/hash",
            "/audio_info/hash_128",
            "/base/hash",
        ],
    )
    .to_ascii_lowercase()
}

fn addition_resources(ids: &[String], items: &[Value]) -> Result<Vec<Value>, Value> {
    ids.iter().map(|id| {
        let item = items.iter().find(|item| song_hash(item) == *id)
            .ok_or_else(|| error(502, &format!("Song metadata is missing for {id}")))?;
        let name = first_string(item, &[
            "/audio_name", "/FileName", "/filename", "/songname", "/SongName", "/name",
            "/base/audio_name", "/base/songname",
        ]);
        if name.trim().is_empty() {
            return Err(error(502, &format!("Song metadata contains no name for {id}")));
        }
        Ok(json!({
            "number": 1, "name": name, "hash": id, "size": 0, "sort": 0,
            "timelen": 0, "bitrate": 0,
            "album_id": first_i64(item, &["/album_id", "/AlbumID", "/album_info/album_id", "/base/album_id"]),
            "mixsongid": first_i64(item, &["/album_audio_id", "/mixsongid", "/MixSongID", "/base/album_audio_id"])
        }))
    }).collect()
}

fn update_tracks(
    query: &Value,
    ids: &[String],
    add: bool,
    call: &mut impl FnMut(Operation, &Value) -> ApiResult,
) -> ApiResult {
    let mut query = query.clone();
    query["type"] = json!(0);
    let resources = if add {
        let mut items = Vec::new();
        for batch in ids.chunks(PAGE_SIZE) {
            query["hash"] = json!(batch.join(","));
            let body = read(call, Operation::Audio, &query)?;
            if let Some(list) = body.get("data").and_then(Value::as_array) {
                items.extend(list.iter().cloned());
            } else if let Some(item) = body.get("data").filter(|data| data.is_object()) {
                items.push(item.clone());
            }
        }
        addition_resources(ids, &items)?
    } else {
        let items = all_pages(&query, Operation::Tracks, call)?;
        let mut fileids = Vec::new();
        let mut seen = HashSet::new();
        for item in &items {
            if ids.contains(&song_hash(item)) {
                let fileid = first_i64(item, &["/fileid", "/file_id"]);
                if fileid <= 0 {
                    return Err(error(502, "Playlist song contains no valid fileid"));
                }
                if seen.insert(fileid) {
                    fileids.push(json!(fileid));
                }
            }
        }
        fileids
    };
    let mut completed = 0;
    let mut verified = false;
    for batch in resources.chunks(PAGE_SIZE) {
        let operation = if add {
            query["data"] = json!(batch);
            Operation::AddTracks
        } else {
            query["fileids"] = json!(batch
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(","));
            Operation::RemoveTracks
        };
        if let Err(mut error) = mutation(call, operation, &query) {
            let needs_verification = error["_verifyState"] == true;
            error.as_object_mut().unwrap().remove("_verifyState");
            if needs_verification {
                match verify_track_batch(&query, operation, batch, call) {
                    Ok(confirmed) if confirmed == batch.len() => {
                        completed += confirmed;
                        verified = true;
                        continue;
                    }
                    Ok(confirmed) => {
                        error["verification"] = json!("mismatch");
                        error["confirmedInBatch"] = json!(confirmed);
                        completed += confirmed;
                    }
                    Err(verification_error) => {
                        error["verification"] = json!("unavailable");
                        error["verificationError"] = verification_error;
                    }
                }
            }
            error["partial"] = json!(completed > 0 || error["partial"] == true);
            error["completed"] = json!(completed);
            return Err(error);
        }
        completed += batch.len();
    }
    let mut response = json!({
        "code": 200,
        "status": 200,
        "count": completed,
        "body": {
            "code": 200,
            "count": completed,
            "cloudCount": 0
        }
    });
    if verified {
        response["body"]["verified"] = json!(true);
    }
    Ok(response)
}

fn public_collection(
    query: &Value,
    call: &mut impl FnMut(Operation, &Value) -> ApiResult,
) -> ApiResult {
    let id = first_string(query, &["/global_collection_id", "/id"]);
    if id.starts_with("collection_") {
        return Ok(json!(id));
    }
    local_id(&json!({"id": id}), &["/id"])?;
    let body = read(call, Operation::PublicInfo, query)?;
    let gid = first_string(
        &body,
        &[
            "/data/global_specialid",
            "/data/global_collection_id",
            "/data/gid",
        ],
    );
    if gid.is_empty() {
        return Err(error(404, "Public playlist not found"));
    }
    Ok(json!(gid))
}

fn subscribe(
    query: &Value,
    subscribe: bool,
    call: &mut impl FnMut(Operation, &Value) -> ApiResult,
) -> ApiResult {
    let mut query = query.clone();
    let id = first_string(&query, &["/global_collection_id", "/id"]);
    if !id.starts_with("collection_") {
        local_id(&query, &["/id"])?;
    }
    if !subscribe {
        let lists = account_lists(&query, 1, call)?;
        // Hosts list collected playlists by their account-local ID. Prefer that
        // identity before trying to interpret a numeric ID as a public specialid.
        let mut found = lists
            .iter()
            .find(|item| first_string(item, &["/listid", "/id"]) == id)
            .cloned();
        if found.is_none() {
            let gid = public_collection(&query, call)?;
            found = lists.into_iter().find(|item| {
                first_string(item, &["/list_create_gid", "/global_collection_id", "/gid"])
                    == gid.as_str().unwrap_or("")
            });
        }
        let item = found.ok_or_else(|| error(404, "Playlist is not collected by this account"))?;
        query["listid"] = json!(local_id(&item, &["/listid", "/id"])?);
        query["type"] = json!(1);
        return mutation(call, Operation::DeletePlaylist, &query);
    }
    query["ids"] = public_collection(&query, call)?;
    let body = read(call, Operation::PublicDetail, &query)?;
    let metadata = playlist_metadata(&body)
        .ok_or_else(|| error(502, "Public playlist metadata is missing"))?;
    let gid = query["ids"].as_str().unwrap_or("");
    // collection_<source>_<creator-userid>_<creator-listid>_<version>
    let parts: Vec<_> = gid.split('_').collect();
    let creator = first_i64(
        metadata,
        &["/list_create_userid", "/create_userid", "/userid"],
    );
    let creator = if creator > 0 {
        creator
    } else {
        parts
            .get(2)
            .and_then(|id| id.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let listid = first_i64(metadata, &["/list_create_listid", "/create_listid"]);
    let listid = if listid > 0 {
        listid
    } else {
        parts
            .get(3)
            .and_then(|id| id.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let name = first_string(metadata, &["/name", "/listname", "/title"]);
    if creator <= 0 || listid <= 0 || name.is_empty() {
        return Err(error(
            502,
            "Public playlist metadata contains no collection identity",
        ));
    }
    query["name"] = json!(name);
    query["list_create_userid"] = json!(creator);
    query["list_create_listid"] = json!(listid);
    query["list_create_gid"] = query["ids"].clone();
    query["type"] = json!(1);
    mutation(call, Operation::AddPlaylist, &query)
}

fn dispatch_with(
    method: &str,
    params: &Value,
    call: &mut impl FnMut(Operation, &Value) -> ApiResult,
) -> ApiResult {
    // Validate the requested operation before performing any network requests.
    let (ids, add) = match method {
        "like" => (hashes(params.get("id"))?, boolean(params, "like", true)?),
        "playlist/tracks" => {
            let add = match params.get("op").and_then(Value::as_str) {
                Some("add") => true,
                Some("del") => false,
                _ => return Err(error(400, "Playlist tracks op must be add or del")),
            };
            local_id(params, &["/pid", "/listid"])?;
            (hashes(params.get("tracks"))?, add)
        }
        "playlist/delete" => {
            local_id(params, &["/id", "/listid"])?;
            (Vec::new(), false)
        }
        "playlist/subscribe" => {
            let t = params.get("t").and_then(value_to_i64);
            if !matches!(t, Some(1 | 2)) {
                return Err(error(400, "Playlist subscribe t must be 1 or 2"));
            }
            if first_string(params, &["/global_collection_id", "/id"]).is_empty() {
                return Err(error(400, "Missing playlist id"));
            }
            (Vec::new(), t == Some(1))
        }
        "likelist" => (Vec::new(), false),
        _ => return Err(error(-1, "Unsupported account library method")),
    };
    let mut query = login_query(params)?;
    if method == "playlist/subscribe" {
        return subscribe(&query, add, call);
    }
    let lists = account_lists(&query, 0, call)?;
    let metadata = if method == "like" || method == "likelist" {
        liked_list(&lists)?
    } else {
        let id = local_id(
            params,
            if method == "playlist/tracks" {
                &["/pid", "/listid"]
            } else {
                &["/id", "/listid"]
            },
        )?;
        owned_list(&lists, &id)?
    };
    query["listid"] = json!(local_id(&metadata, &["/listid", "/id"])?);
    query["type"] = json!(0);
    if method == "playlist/delete" {
        if liked_list(std::slice::from_ref(&metadata)).is_ok() {
            return Err(error(403, "Cannot delete the liked-songs playlist"));
        }
        // delete_list uses type=1 for deleting a list, not the list's own kind.
        query["type"] = json!(1);
        let mut resp = mutation(call, Operation::DeletePlaylist, &query)?;
        resp["id"] = params.get("id").or_else(|| params.get("listid")).cloned().unwrap_or(json!(0));
        return Ok(resp);
    }
    if method == "likelist" {
        let items = all_pages(&query, Operation::Tracks, call)?;
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        for item in &items {
            let id = song_hash(item);
            if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(error(502, "Liked song contains no valid audio hash"));
            }
            if seen.insert(id.clone()) {
                ids.push(id);
            }
        }
        return Ok(json!({
            "code": 200,
            "status": 200,
            "ids": ids.clone(),
            "data": ids,
            "checkPoint": 0
        }));
    }
    let ids = if method == "like" && add {
        let existing = all_pages(&query, Operation::Tracks, call)?;
        let existing: HashSet<_> = existing.iter().map(song_hash).collect();
        ids.into_iter()
            .filter(|id| !existing.contains(id))
            .collect()
    } else {
        ids
    };
    let mut response = update_tracks(&query, &ids, add, call)?;
    if method == "like" {
        let playlist_id = query["listid"]
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
        response["playlistId"] = json!(playlist_id);
        response["songs"] = json!(ids);
        response["tracks"] = json!(ids);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "aabbccddeeff00112233445566778899";
    const OTHER: &str = "11223344556677889900aabbccddeeff";

    fn authenticated(mut params: Value) -> Value {
        params["cookie"] = json!({"userid": "123", "token": "fixture=="});
        params
    }

    fn lists() -> Value {
        json!({"status": 1, "data": {"info": [
            {"listid": 198, "name": "Personal", "type": 0},
            {"listid": 276, "name": "我喜欢", "type": 0}
        ]}})
    }

    #[test]
    fn routes_are_registered_and_invalid_requests_never_access_the_network() {
        let manifest: Value =
            serde_json::from_str(include_str!("../../packaging/manifest.json")).unwrap();
        for (method, params) in [
            ("like", json!({"id": HASH})),
            ("likelist", json!({})),
            (
                "playlist/tracks",
                json!({"op": "add", "pid": "198", "tracks": HASH}),
            ),
            ("playlist/delete", json!({"id": 198})),
            ("playlist/subscribe", json!({"id": 7235936, "t": "1"})),
        ] {
            assert_eq!(manifest["apiMap"][method], method);
            assert_eq!(handle_api(method, &params)["code"], 401, "{method}");
        }
        for (method, params) in [
            ("like", json!({"id": HASH, "like": "no"})),
            ("like", json!({"id": "123"})),
            (
                "playlist/tracks",
                json!({"op": "remove", "pid": 198, "tracks": HASH}),
            ),
            (
                "playlist/tracks",
                json!({"op": "add", "pid": 198, "tracks": ""}),
            ),
            (
                "playlist/tracks",
                json!({"op": "add", "pid": "collection_3_123_198_0", "tracks": HASH}),
            ),
            (
                "playlist/tracks",
                json!({"op": "add", "pid": RANK_PLAYLIST_BASE + 8888, "tracks": HASH}),
            ),
            ("playlist/delete", json!({"id": -1})),
            ("playlist/subscribe", json!({"id": 7235936, "t": 0})),
        ] {
            assert_eq!(
                handle_api(method, &authenticated(params))["code"],
                400,
                "{method}"
            );
        }
    }

    #[test]
    fn identity_conflicts_are_rejected_before_any_call() {
        for (params, code) in [
            (json!({"uid": "456"}), 403),
            (json!({"userid": 456}), 400),
            (json!({"token": "different"}), 400),
        ] {
            let result = dispatch_with("likelist", &authenticated(params), &mut |_, _| {
                panic!("conflicting identity must not send requests")
            })
            .unwrap_err();
            assert_eq!(result["code"], code);
        }
        let query =
            login_query(&json!({"userid": "123", "token": "fixture==", "cookie": null})).unwrap();
        assert_eq!(query["cookie"]["userid"], 123);
    }

    #[test]
    fn like_finds_the_real_account_list_and_adds_structured_metadata() {
        let mut operations = Vec::new();
        let params = normalize_standard_params(&json!({
            "id": HASH.to_ascii_uppercase(), "like": "true",
            "cookie": "userid=123; token=fixture=="
        }));
        let result = dispatch_with("like", &params, &mut |op, query| {
            operations.push(op);
            assert_eq!(query["cookie"]["token"], "fixture==");
            match op {
                Operation::Lists => {
                    assert_eq!(query["type"], 0);
                    Ok(lists())
                }
                Operation::Tracks => Ok(json!({"status": 1, "data": {"count": 0, "info": []}})),
                Operation::Audio => {
                    assert_eq!(query["hash"], HASH);
                    Ok(json!({"status": 1, "data": [{
                        "hash": HASH, "audio_name": "Artist - Title, with | delimiters",
                        "album_info": {"album_id": 77}, "album_audio_id": 88
                    }]}))
                }
                Operation::AddTracks => {
                    assert_eq!(query["listid"], "276");
                    assert_eq!(
                        query["data"][0]["name"],
                        "Artist - Title, with | delimiters"
                    );
                    assert_eq!(query["data"][0]["hash"], HASH);
                    assert_eq!(query["data"][0]["album_id"], 77);
                    assert_eq!(query["data"][0]["mixsongid"], 88);
                    Ok(json!({"status": 1, "error_code": 0}))
                }
                _ => panic!("unexpected operation: {op:?}"),
            }
        })
        .unwrap();
        assert_eq!(result["code"], 200);
        assert_eq!(result["playlistId"], 276);
        assert_eq!(result["songs"], json!([HASH]));
        assert_eq!(
            operations,
            vec![
                Operation::Lists,
                Operation::Tracks,
                Operation::Audio,
                Operation::AddTracks
            ]
        );
    }

    #[test]
    fn liking_an_existing_song_does_not_add_a_duplicate() {
        let result =
            dispatch_with(
                "like",
                &authenticated(json!({"id": HASH})),
                &mut |op, _| match op {
                    Operation::Lists => Ok(lists()),
                    Operation::Tracks => {
                        Ok(json!({"data": {"songs": [{"hash": HASH.to_ascii_uppercase()}]}}))
                    }
                    _ => panic!("an already liked song needs no metadata request or write"),
                },
            )
            .unwrap();
        assert_eq!(result["code"], 200);
        assert_eq!(result["playlistId"], 276);
        assert_eq!(result["body"]["count"], 0);
    }

    #[test]
    fn liked_list_does_not_guess_listid_or_choose_an_ambiguous_name() {
        assert_eq!(
            liked_list(&[json!({"listid": 1, "name": "Other"})]).unwrap_err()["code"],
            404
        );
        assert_eq!(
            liked_list(&[
                json!({"listid": 1, "name": "我喜欢"}),
                json!({"listid": 2, "name": "我喜欢的音乐"}),
            ])
            .unwrap_err()["code"],
            409
        );
    }

    #[test]
    fn unliking_uses_real_fileids_for_all_matching_hashes() {
        for unlike in [json!(false), json!("false"), json!(0), json!("0")] {
            let mut removed = false;
            let result = dispatch_with(
                "like",
                &authenticated(json!({"id": HASH, "like": unlike})),
                &mut |op, query| match op {
                    Operation::Lists => Ok(lists()),
                    Operation::Tracks => {
                        assert_eq!(query["listid"], "276");
                        Ok(json!({"data": {"count": 3, "info": [
                            {"hash": OTHER, "fileid": 55},
                            {"hash": HASH.to_ascii_uppercase(), "fileid": "987654321012345"},
                            {"hash": HASH, "fileid": 66}
                        ]}}))
                    }
                    Operation::RemoveTracks => {
                        assert_eq!(query["fileids"], "987654321012345,66");
                        removed = true;
                        Ok(json!({"status": "1"}))
                    }
                    _ => panic!("unexpected operation: {op:?}"),
                },
            )
            .unwrap();
            assert_eq!(result["code"], 200);
            assert!(removed);
        }
    }

    #[test]
    fn likelist_reads_every_page_and_normalizes_hash_case() {
        let mut pages = Vec::new();
        let result = dispatch_with(
            "likelist",
            &authenticated(json!({"uid": "123", "page": 9, "limit": 1})),
            &mut |op, query| match op {
                Operation::Lists => Ok(lists()),
                Operation::Tracks => {
                    let page = first_i64(query, &["/page"]);
                    pages.push(page);
                    assert_eq!(query["listid"], "276");
                    assert_eq!(query["pagesize"], 100);
                    let songs: Vec<_> = if page == 1 {
                        (0..100)
                            .map(|id| json!({"hash": format!("{id:032X}")}))
                            .collect()
                    } else {
                        vec![
                            json!({"hash": HASH.to_ascii_uppercase()}),
                            json!({"hash": HASH}),
                        ]
                    };
                    Ok(json!({"status": 1, "data": {"info": songs, "count": 102}}))
                }
                _ => panic!("likelist must be read-only"),
            },
        )
        .unwrap();
        assert_eq!(pages, vec![1, 2]);
        assert_eq!(result["ids"].as_array().unwrap().len(), 101);
        assert_eq!(result["ids"][100], HASH);
    }

    #[test]
    fn empty_liked_list_and_already_absent_song_are_successful() {
        for method in ["like", "likelist"] {
            let result = dispatch_with(
                method,
                &authenticated(json!({"id": HASH, "like": false})),
                &mut |op, _| match op {
                    Operation::Lists => Ok(lists()),
                    Operation::Tracks => {
                        Ok(json!({"status": 1, "count": 0, "data": {"info": null}}))
                    }
                    _ => panic!("an empty list needs no mutation"),
                },
            )
            .unwrap();
            assert_eq!(result["code"], 200);
            if method == "likelist" {
                assert_eq!(result["ids"], json!([]));
            }
        }
    }

    #[test]
    fn playlist_addition_aligns_metadata_to_requested_ids_and_deduplicates() {
        let result = dispatch_with(
            "playlist/tracks",
            &authenticated(json!({
                "pid": "198", "op": "add", "tracks": [HASH, OTHER, HASH.to_ascii_uppercase()]
            })),
            &mut |op, query| match op {
                Operation::Lists => Ok(lists()),
                Operation::Audio => Ok(json!({"data": [
                    {"hash": OTHER, "audio_name": "Second"},
                    {"hash": HASH, "audio_name": "First"}
                ]})),
                Operation::AddTracks => {
                    assert_eq!(query["listid"], "198");
                    assert_eq!(query["data"].as_array().unwrap().len(), 2);
                    assert_eq!(query["data"][0]["name"], "First");
                    assert_eq!(query["data"][1]["name"], "Second");
                    Ok(json!({"status": 1}))
                }
                _ => panic!("unexpected operation"),
            },
        )
        .unwrap();
        assert_eq!(result["code"], 200);
        assert_eq!(result["body"]["count"], 2);
    }

    #[test]
    fn missing_or_mismatched_metadata_never_sends_a_write() {
        for body in [
            json!({}),
            json!({"data": [{"hash": OTHER, "name": "Wrong song"}]}),
            json!({"data": [{"hash": HASH}]}),
        ] {
            let result = dispatch_with(
                "playlist/tracks",
                &authenticated(json!({
                    "pid": 198, "op": "add", "tracks": HASH
                })),
                &mut |op, _| match op {
                    Operation::Lists => Ok(lists()),
                    Operation::Audio => Ok(body.clone()),
                    _ => panic!("incomplete metadata must not cause a write"),
                },
            )
            .unwrap_err();
            assert_eq!(result["code"], 502);
        }
    }

    #[test]
    fn deletion_resolves_hashes_on_later_pages_and_never_sends_hash_as_fileid() {
        let mut pages = Vec::new();
        let result = dispatch_with(
            "playlist/tracks",
            &authenticated(json!({
                "pid": 198, "op": "del", "tracks": HASH
            })),
            &mut |op, query| match op {
                Operation::Lists => Ok(lists()),
                Operation::Tracks => {
                    let page = first_i64(query, &["/page"]);
                    pages.push(page);
                    let songs: Vec<_> = if page == 1 {
                        (0..100)
                            .map(|id| json!({"hash": format!("{id:032x}"), "fileid": id + 1}))
                            .collect()
                    } else {
                        vec![json!({"hash": HASH, "fileid": 222})]
                    };
                    Ok(json!({"data": {"count": 101, "songs": songs}}))
                }
                Operation::RemoveTracks => {
                    assert_eq!(query["fileids"], "222");
                    Ok(json!({"status": 1}))
                }
                _ => panic!("unexpected operation"),
            },
        )
        .unwrap();
        assert_eq!(result["code"], 200);
        assert_eq!(pages, vec![1, 2]);
    }

    #[test]
    fn invalid_fileids_and_broken_pagination_stop_before_deletion() {
        for response in [
            json!({"data": {"songs": [{"hash": HASH, "fileid": 0}]}}),
            json!({"data": {"count": 10, "songs": [{"hash": HASH, "fileid": 1}]}}),
        ] {
            let result = dispatch_with(
                "playlist/tracks",
                &authenticated(json!({
                    "pid": 198, "op": "del", "tracks": HASH
                })),
                &mut |op, _| match op {
                    Operation::Lists => Ok(lists()),
                    Operation::Tracks => Ok(response.clone()),
                    _ => panic!("broken fileids or pagination must not cause a write"),
                },
            )
            .unwrap_err();
            assert_eq!(result["code"], 502);
        }
        let mut calls = 0;
        let result = all_pages(&json!({}), Operation::Tracks, &mut |_, _| {
            calls += 1;
            Ok(json!({"data": {"songs": (0..100).map(|id| json!({"hash": format!("{id:032x}")})).collect::<Vec<_>>()}}))
        }).unwrap_err();
        assert_eq!(result["code"], 502);
        assert_eq!(calls, 2);
    }

    #[test]
    fn ownership_is_checked_by_local_id_not_public_specialid() {
        let list = vec![json!({"listid": 198, "specialid": 7235936, "type": 0})];
        assert!(owned_list(&list, "198").is_ok());
        assert_eq!(owned_list(&list, "7235936").unwrap_err()["code"], 403);
        for method in ["playlist/delete", "playlist/tracks"] {
            let result = dispatch_with(
                method,
                &authenticated(json!({
                    "id": 7235936, "pid": 7235936, "op": "add", "tracks": HASH
                })),
                &mut |op, _| {
                    assert_eq!(op, Operation::Lists);
                    Ok(json!({"data": {"info": list}}))
                },
            )
            .unwrap_err();
            assert_eq!(result["code"], 403);
        }
    }

    #[test]
    fn deleting_owned_playlist_uses_delete_list_type_and_protects_liked_list() {
        for (id, expected) in [(198, 200), (276, 403)] {
            let result = dispatch_with(
                "playlist/delete",
                &authenticated(json!({"id": id})),
                &mut |op, query| match op {
                    Operation::Lists => Ok(lists()),
                    Operation::DeletePlaylist => {
                        assert_eq!(id, 198);
                        assert_eq!(query["listid"], "198");
                        assert_eq!(query["type"], 1);
                        Ok(json!({"status": 1}))
                    }
                    _ => panic!("unexpected operation"),
                },
            )
            .unwrap_or_else(|error| error);
            assert_eq!(result["code"], expected);
        }
    }

    #[test]
    fn collecting_public_playlist_uses_resolved_global_and_creator_ids() {
        let result = dispatch_with("playlist/subscribe", &authenticated(json!({"id": 7235936, "t": "1"})), &mut |op, query| match op {
            Operation::PublicInfo => Ok(json!({"data": {"global_specialid": "collection_3_999_87_0"}})),
            Operation::PublicDetail => {
                assert_eq!(query["ids"], "collection_3_999_87_0");
                Ok(json!({"data": [{"name": "Public playlist", "list_create_userid": 998, "list_create_listid": 86}]}))
            }
            Operation::AddPlaylist => {
                assert_eq!(query["type"], 1);
                assert_eq!(query["list_create_userid"], 998);
                assert_eq!(query["list_create_listid"], 86);
                assert_eq!(query["list_create_gid"], "collection_3_999_87_0");
                assert_eq!(query["name"], "Public playlist");
                Ok(json!({"status": 1}))
            }
            _ => panic!("unexpected operation"),
        }).unwrap();
        assert_eq!(result["code"], 200);
    }

    #[test]
    fn uncollecting_uses_account_local_id_before_public_namespace() {
        let result = dispatch_with(
            "playlist/subscribe",
            &authenticated(json!({"id": 198, "t": "2"})),
            &mut |op, query| match op {
                Operation::Lists => {
                    assert_eq!(query["type"], 1);
                    Ok(json!({"data": {"info": [{"listid": 198, "specialid": 7235936}]}}))
                }
                Operation::DeletePlaylist => {
                    assert_eq!(query["listid"], "198");
                    assert_eq!(query["type"], 1);
                    Ok(json!({"status": 1}))
                }
                _ => panic!("a collected local ID must not be looked up as a public ID"),
            },
        )
        .unwrap();
        assert_eq!(result["code"], 200);
    }

    #[test]
    fn upstream_failures_and_unconfirmed_writes_are_not_successes() {
        for body in [
            json!({"status": 0, "error_code": 20010, "errmsg": "denied"}),
            json!({"status": 1, "data": {"status": 0, "errmsg": "denied"}}),
            json!({"code": 403, "msg": "denied"}),
            json!({"status": 1, "data": [{"errcode": 20010, "errmsg": "denied"}]}),
            json!({}),
            json!({"data": null}),
            json!("encrypted response was not decrypted"),
        ] {
            let response = mutation(
                &mut |_, _| Ok(body.clone()),
                Operation::DeletePlaylist,
                &json!({}),
            )
            .unwrap_err();
            assert_eq!(response["code"], 502);
        }
        let failure =
            upstream_error(&json!({"status": 0, "error_code": 20010, "errmsg": "denied"})).unwrap();
        assert_eq!(failure["upstreamCode"], 20010);
        assert_eq!(failure["msg"], "denied");
    }

    #[test]
    fn ambiguous_song_code_is_confirmed_by_reading_the_target_playlist() {
        for data in [
            json!([{"hash": HASH, "code": 1}]),
            json!({"info": [{"hash": HASH, "code": "1"}]}),
            json!({"results": [{"hash": HASH, "errcode": 1}]}),
        ] {
            let mut writes = 0;
            let mut confirmations = 0;
            let result = update_tracks(
                &authenticated(json!({"listid": "198"})),
                &[HASH.to_string()],
                true,
                &mut |op, query| match op {
                    Operation::Audio => Ok(json!({"data": [{"hash": HASH, "name": "Track"}]})),
                    Operation::AddTracks => {
                        writes += 1;
                        Ok(json!({"status": 1, "data": data}))
                    }
                    Operation::Tracks => {
                        confirmations += 1;
                        assert_eq!(query["listid"], "198");
                        assert_eq!(query["type"], 0);
                        Ok(json!({"status": 1, "data": {
                            "count": 1, "info": [{"hash": HASH.to_ascii_uppercase(), "fileid": 77}]
                        }}))
                    }
                    _ => panic!("unexpected operation: {op:?}"),
                },
            )
            .unwrap();
            assert_eq!(result["code"], 200);
            assert_eq!(result["body"]["count"], 1);
            assert_eq!(result["body"]["verified"], true);
            assert_eq!(writes, 1, "confirmation must never repeat the write");
            assert_eq!(confirmations, 1);
        }
    }

    #[test]
    fn ambiguous_code_is_not_success_when_the_song_is_still_missing() {
        let mut writes = 0;
        let mut confirmations = 0;
        let result = update_tracks(
            &authenticated(json!({"listid": "198"})),
            &[HASH.to_string()],
            true,
            &mut |op, _| match op {
                Operation::Audio => Ok(json!({"data": [{"hash": HASH, "name": "Track"}]})),
                Operation::AddTracks => {
                    writes += 1;
                    Ok(json!({"status": 1, "data": {"info": [{"hash": HASH, "code": 1}]}}))
                }
                Operation::Tracks => {
                    confirmations += 1;
                    Ok(json!({"status": 1, "data": {"count": 0, "info": []}}))
                }
                _ => panic!("unexpected operation: {op:?}"),
            },
        )
        .unwrap_err();
        assert_eq!(result["code"], 502);
        assert_eq!(result["upstreamCode"], 1);
        assert_eq!(result["upstreamField"], "/data/info/0/code");
        assert_eq!(result["upstreamOperation"], "playlist/tracks/add");
        assert_eq!(result["verification"], "mismatch");
        assert_eq!(result["completed"], 0);
        assert_eq!(result["confirmedInBatch"], 0);
        assert!(result.get("_verifyState").is_none());
        assert_eq!(writes, 1);
        assert_eq!(confirmations, 3);
    }

    #[test]
    fn ambiguous_write_retries_only_reads_until_the_song_becomes_visible() {
        let mut writes = 0;
        let mut confirmations = 0;
        let result = update_tracks(
            &authenticated(json!({"listid": "198"})),
            &[HASH.to_string()],
            true,
            &mut |op, _| match op {
                Operation::Audio => Ok(json!({"data": [{"hash": HASH, "name": "Track"}]})),
                Operation::AddTracks => {
                    writes += 1;
                    Ok(json!({"status": 1, "data": [{"code": 1}]}))
                }
                Operation::Tracks => {
                    confirmations += 1;
                    if confirmations == 1 {
                        Ok(json!({"status": 1, "data": {"count": 0, "songs": []}}))
                    } else {
                        Ok(json!({"status": 1, "data": {"count": 1, "songs": [{"hash": HASH, "fileid": 77}]}}))
                    }
                }
                _ => panic!("unexpected operation: {op:?}"),
            },
        ).unwrap();
        assert_eq!(result["code"], 200);
        assert_eq!(result["body"]["verified"], true);
        assert_eq!(writes, 1);
        assert_eq!(confirmations, 2);
    }

    #[test]
    fn failed_or_malformed_confirmation_does_not_hide_the_original_error() {
        for verification in [
            Err(error(502, "Fixture read timeout")),
            Ok(json!({"status": 1, "data": {"count": 2, "songs": [{"hash": HASH}]}})),
            Ok(json!({"status": 1, "data": {"count": 1, "songs": [{"fileid": 77}]}})),
        ] {
            let mut writes = 0;
            let result = update_tracks(
                &authenticated(json!({"listid": "198"})),
                &[HASH.to_string()],
                true,
                &mut |op, _| match op {
                    Operation::Audio => Ok(json!({"data": [{"hash": HASH, "name": "Track"}]})),
                    Operation::AddTracks => {
                        writes += 1;
                        Ok(json!({"status": 1, "data": [{"errcode": 1}]}))
                    }
                    Operation::Tracks => verification.clone(),
                    _ => panic!("unexpected operation: {op:?}"),
                },
            )
            .unwrap_err();
            assert_eq!(result["code"], 502);
            assert_eq!(result["upstreamCode"], 1);
            assert_eq!(result["upstreamField"], "/data/0/errcode");
            assert_eq!(result["verification"], "unavailable");
            assert_eq!(result["verificationError"]["code"], 502);
            assert_eq!(result["completed"], 0);
            assert!(result.get("_verifyState").is_none());
            assert_eq!(writes, 1);
        }
    }

    #[test]
    fn partially_visible_batch_reports_confirmed_songs_without_repeating_writes() {
        let result = update_tracks(
            &authenticated(json!({"listid": "198"})),
            &[HASH.to_string(), OTHER.to_string()],
            true,
            &mut |op, _| match op {
                Operation::Audio => Ok(json!({"data": [
                    {"hash": HASH, "name": "First"}, {"hash": OTHER, "name": "Second"}
                ]})),
                Operation::AddTracks => Ok(json!({"status": 1, "data": [
                    {"hash": HASH, "code": 1}, {"hash": OTHER, "code": 1}
                ]})),
                Operation::Tracks => Ok(json!({"status": 1, "data": {
                    "count": 1, "songs": [{"hash": HASH, "fileid": 77}]
                }})),
                _ => panic!("unexpected operation: {op:?}"),
            },
        )
        .unwrap_err();
        assert_eq!(result["code"], 502);
        assert_eq!(result["partial"], true);
        assert_eq!(result["completed"], 1);
        assert_eq!(result["confirmedInBatch"], 1);
        assert_eq!(result["verification"], "mismatch");
    }

    #[test]
    fn ambiguous_removal_is_verified_by_fileid_without_removing_other_copies() {
        for gone in [true, false] {
            let mut reads = 0;
            let mut writes = 0;
            let result = update_tracks(
                &authenticated(json!({"listid": "198"})),
                &[HASH.to_string()],
                false,
                &mut |op, query| match op {
                    Operation::Tracks => {
                        reads += 1;
                        if reads == 1 || !gone {
                            Ok(json!({"status": 1, "data": {
                                "count": 1, "songs": [{"hash": HASH, "fileid": 77}]
                            }}))
                        } else {
                            // A concurrently added copy has a different fileid.
                            Ok(json!({"status": 1, "data": {
                                "count": 1, "songs": [{"hash": HASH, "fileid": 88}]
                            }}))
                        }
                    }
                    Operation::RemoveTracks => {
                        writes += 1;
                        assert_eq!(query["fileids"], "77");
                        Ok(json!({"status": 1, "data": [{"fileid": 77, "code": "1"}]}))
                    }
                    _ => panic!("unexpected operation: {op:?}"),
                },
            );
            if gone {
                let response = result.unwrap();
                assert_eq!(response["code"], 200);
                assert_eq!(response["body"]["verified"], true);
                assert_eq!(reads, 2);
            } else {
                let response = result.unwrap_err();
                assert_eq!(response["code"], 502);
                assert_eq!(response["verification"], "mismatch");
                assert_eq!(reads, 4);
            }
            assert_eq!(writes, 1);
        }
    }

    #[test]
    fn real_business_failures_are_not_reinterpreted_as_success() {
        let mut writes = 0;
        let result = update_tracks(
            &authenticated(json!({"listid": "198"})),
            &[HASH.to_string()],
            true,
            &mut |op, _| match op {
                Operation::Audio => Ok(json!({"data": [{"hash": HASH, "name": "Track"}]})),
                Operation::AddTracks => {
                    writes += 1;
                    Ok(json!({"status": 1, "data": [{"error_code": 20010, "errmsg": "denied"}]}))
                }
                Operation::Tracks => panic!("a definite business failure needs no confirmation"),
                _ => panic!("unexpected operation: {op:?}"),
            },
        )
        .unwrap_err();
        assert_eq!(result["code"], 502);
        assert_eq!(result["upstreamCode"], 20010);
        assert_eq!(result["upstreamField"], "/data/0/error_code");
        assert_eq!(result["msg"], "denied");
        assert!(result.get("verification").is_none());
        assert_eq!(writes, 1);
    }

    #[test]
    fn explicit_item_failures_take_precedence_over_ambiguous_codes() {
        for (body, field) in [
            (
                json!({"status": 1, "data": [
                    {"code": 1}, {"error_code": 20010, "errmsg": "denied"}
                ]}),
                "/data/1/error_code",
            ),
            (
                json!({"status": 1, "data": {"code": 1, "info": [
                    {"error_code": 20010, "errmsg": "denied"}
                ]}}),
                "/data/info/0/error_code",
            ),
            (
                json!({"status": 1, "data": {
                    "info": [{"code": 1}],
                    "results": [{"errcode": 20010, "errmsg": "denied"}]
                }}),
                "/data/results/0/errcode",
            ),
        ] {
            let result = mutation(
                &mut |_, _| Ok(body.clone()),
                Operation::AddTracks,
                &json!({}),
            )
            .unwrap_err();
            assert_eq!(result["upstreamCode"], 20010);
            assert_eq!(result["upstreamField"], field);
            assert_eq!(result["msg"], "denied");
            assert!(result.get("_verifyState").is_none());
        }
    }

    #[test]
    fn nested_error_codes_are_preserved_with_their_actual_field_paths() {
        for (body, code, field) in [
            (json!({"data": {"code": 403}}), 403, "/data/code"),
            (json!({"data": {"status": 0}}), 0, "/data/status"),
            (
                json!({"status": 0, "code": 1, "error_code": 20010}),
                20010,
                "/error_code",
            ),
        ] {
            let result = upstream_error(&body).unwrap();
            assert_eq!(result["upstreamCode"], code);
            assert_eq!(result["upstreamField"], field);
            assert!(result["msg"].as_str().unwrap().contains(field));
        }
    }

    #[test]
    fn batch_failure_reports_partial_completion_instead_of_success() {
        let ids: Vec<_> = (0..101).map(|id| format!("{id:032x}")).collect();
        let mut writes = 0;
        let result = update_tracks(
            &json!({"listid": "198"}),
            &ids,
            true,
            &mut |op, query| match op {
                Operation::Audio => Ok(
                    json!({"data": query["hash"].as_str().unwrap().split(',').map(|hash| {
                json!({"hash": hash, "name": "Track"})
            }).collect::<Vec<_>>()}),
                ),
                Operation::AddTracks => {
                    writes += 1;
                    if writes == 1 {
                        Ok(json!({"status": 1}))
                    } else {
                        Ok(json!({"status": 0, "errmsg": "denied"}))
                    }
                }
                _ => panic!("unexpected operation"),
            },
        )
        .unwrap_err();
        assert_eq!(result["code"], 502);
        assert_eq!(result["partial"], true);
        assert_eq!(result["completed"], 100);
        assert_eq!(writes, 2);
    }
}
