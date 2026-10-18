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
        "http://imge.kugou.com/stdmusic/400/20200909/20200909134522432857.jpg"
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
    assert_eq!(transformed["coverUrl"], "http://imge.kugou.com/stdmusic/400/cover.jpg");
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
fn test_handle_unsupported_method() {
    let res = kugou_music_rs::standard_api::handle_api("some/unknown/method", &json!({}));
    assert_eq!(res["code"], -1);
}
