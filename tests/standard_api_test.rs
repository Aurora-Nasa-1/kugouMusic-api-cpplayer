use kugou_music_rs::standard_api::transform_kugou_song;
use serde_json::json;

#[test]
fn test_transform_kugou_song_standard_fields() {
    let kugou_item = json!({
        "FileHash": "C138DA64149FA469E8020942FF903C72",
        "SongName": "晴天",
        "SingerName": "周杰伦",
        "AlbumName": "叶惠美",
        "Duration": 269,
        "Image": "http://imge.kugou.com/stdmusic/{size}/20200909/20200909134522432857.jpg"
    });

    let transformed = transform_kugou_song(&kugou_item).expect("Should transform song");

    assert_eq!(
        transformed["id"],
        "c138da64149fa469e8020942ff903c72"
    );
    assert_eq!(transformed["title"], "晴天");
    assert_eq!(transformed["artist"], "周杰伦");
    assert_eq!(transformed["album"], "叶惠美");
    assert_eq!(transformed["duration"], 269);
    assert_eq!(
        transformed["coverUrl"],
        "https://imge.kugou.com/stdmusic/400/20200909/20200909134522432857.jpg"
    );
}

#[test]
fn test_transform_kugou_song_with_merged_filename() {
    let kugou_item = json!({
        "hash": "aabbccddeeff00112233445566778899",
        "filename": "周杰伦 - 七里香",
        "albumname": "七里香",
        "time_length": 299000,
        "sizable_cover": "http://imge.kugou.com/stdmusic/{size}/cover.jpg"
    });

    let transformed = transform_kugou_song(&kugou_item).expect("Should transform song");

    assert_eq!(transformed["id"], "aabbccddeeff00112233445566778899");
    assert_eq!(transformed["title"], "七里香");
    assert_eq!(transformed["artist"], "周杰伦");
    assert_eq!(transformed["album"], "七里香");
    assert_eq!(transformed["duration"], 299);
    assert_eq!(transformed["coverUrl"], "https://imge.kugou.com/stdmusic/400/cover.jpg");
}

#[test]
fn test_transform_kugou_song_multiple_singers() {
    let kugou_item = json!({
        "hash": "11223344556677889900aabbccddeeff",
        "songname": "珊瑚海",
        "Singers": [
            { "name": "周杰伦", "id": 1 },
            { "name": "梁心颐", "id": 2 }
        ],
        "AlbumName": "十一月的萧邦",
        "duration": 256
    });

    let transformed = transform_kugou_song(&kugou_item).expect("Should transform song");

    assert_eq!(transformed["title"], "珊瑚海");
    assert_eq!(transformed["artist"], "周杰伦 / 梁心颐");
}

#[test]
fn test_transform_kugou_song_accepts_numeric_id_and_ms_boundary() {
    let transformed = transform_kugou_song(&json!({
        "id": 123456789,
        "name": "数字 ID 曲目",
        "artist_name": "歌手",
        "duration": 10000
    }))
    .expect("numeric song id should be accepted");

    assert_eq!(transformed["id"], "123456789");
    assert_eq!(transformed["duration"], 10);
}

#[test]
fn test_handle_unsupported_method() {
    let res = kugou_music_rs::standard_api::handle_api("some/unknown/method", &json!({}));
    assert_eq!(res["code"], -1);
}

#[test]
fn test_handle_login_status_with_cookie_uid() {
    let res = kugou_music_rs::standard_api::handle_api(
        "login/status",
        &json!({
            "cookie": "token=test_token; userid=987654321"
        }),
    );
    assert_eq!(res["code"], 200);
    assert_eq!(res["data"]["code"], 200);
    assert_eq!(res["data"]["account"]["id"], 987654321);
    assert_eq!(res["data"]["profile"]["userId"], 987654321);
}

#[test]
fn test_handle_login_status_unauthenticated() {
    let res = kugou_music_rs::standard_api::handle_api("login/status", &json!({}));
    assert_eq!(res["code"], 200);
    assert_eq!(res["data"]["code"], 200);
    assert!(res["data"]["account"].is_null());
    assert!(res["data"]["profile"].is_null());
}

#[test]
fn test_handle_song_url_missing_hash() {
    let res = kugou_music_rs::standard_api::handle_api("song/url", &json!({}));
    assert_eq!(res["code"], 500);
    assert!(res["data"]["url"].is_null());
}

#[test]
fn test_handle_artist_and_album_dispatch() {
    let res = kugou_music_rs::standard_api::handle_api("artist/detail", &json!({ "id": 0 }));
    assert!(res.is_object());

    let res_albums = kugou_music_rs::standard_api::handle_api("artist/albums", &json!({ "id": 0 }));
    assert!(res_albums.is_object());

    let res_album = kugou_music_rs::standard_api::handle_api("album", &json!({ "id": 0 }));
    assert!(res_album.is_object());
}

#[test]
fn test_handle_playlist_and_recommend_dispatch() {
    let res_pl = kugou_music_rs::standard_api::handle_api("user/playlist", &json!({ "userid": 0 }));
    assert!(res_pl.is_object());

    let res_track = kugou_music_rs::standard_api::handle_api("playlist/track/all", &json!({ "id": 0 }));
    assert!(res_track.is_object());

    let res_rec = kugou_music_rs::standard_api::handle_api("recommend/songs", &json!({}));
    assert!(res_rec.is_object());

    let res_cmt = kugou_music_rs::standard_api::handle_api("comment/music", &json!({ "id": 0 }));
    assert!(res_cmt.is_object());
}

#[test]
fn test_playlist_create_validates_authentication_and_name() {
    let missing_name = kugou_music_rs::standard_api::handle_api("playlist/create", &json!({}));
    assert_eq!(missing_name["code"], 400);

    let unauthenticated = kugou_music_rs::standard_api::handle_api(
        "playlist/create",
        &json!({ "name": "新歌单" }),
    );
    assert_eq!(unauthenticated["code"], 401);
}

#[test]
fn test_like_and_playlist_tracks_validation() {
    let missing_hash = kugou_music_rs::standard_api::handle_api("like", &json!({}));
    assert_eq!(missing_hash["code"], 400);

    let unauth_like = kugou_music_rs::standard_api::handle_api(
        "like",
        &json!({ "id": "c138da64149fa469e8020942ff903c72" }),
    );
    assert_eq!(unauth_like["code"], 401);

    let invalid_op = kugou_music_rs::standard_api::handle_api(
        "playlist/tracks",
        &json!({ "pid": "123", "tracks": "c138da64149fa469e8020942ff903c72" }),
    );
    assert_eq!(invalid_op["code"], 400);
}
