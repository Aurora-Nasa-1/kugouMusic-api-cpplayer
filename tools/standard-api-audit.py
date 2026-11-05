"""Read-only live checks for a built provider DLL. Never prints credentials."""

import argparse
import ctypes
import json
import os
from pathlib import Path
import tempfile
import urllib.error
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dll", type=Path)
    parser.add_argument("--cookie-env", help="Environment variable containing an optional login cookie")
    parser.add_argument("--home-only", action="store_true", help="Run homepage and click-through checks only")
    args = parser.parse_args()
    cookie = os.environ.get(args.cookie_env, "") if args.cookie_env else ""
    manifest = Path(__file__).resolve().parents[1] / "packaging" / "manifest.json"
    expected_version = json.loads(manifest.read_text(encoding="utf-8"))["version"]
    credentials = dict(
        part.strip().split("=", 1) for part in cookie.split(";") if "=" in part
    )
    library = ctypes.CDLL(str(args.dll.resolve()))
    library.start_server.argtypes = [ctypes.c_int, ctypes.c_char_p]
    library.start_server.restype = ctypes.c_int
    library.stop_server.argtypes = []
    library.stop_server.restype = None
    client = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    failures = []

    def check(label, passed, **details):
        print(("PASS" if passed else "FAIL"), label, json.dumps(details), flush=True)
        if not passed:
            failures.append(label)

    def call(method, **params):
        params["cookie"] = cookie
        request = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/{method}",
            data=json.dumps(params).encode("utf-8"),
            headers={"Content-Type": "application/json"},
        )
        try:
            with client.open(request, timeout=45) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            return json.load(error)

    def image(label, url):
        if not url:
            check(label, False, reason="missing artwork")
            return
        try:
            with client.open(url, timeout=20) as response:
                content_type = response.headers.get("Content-Type", "")
                data = response.read(8192)
                check(label, response.status == 200 and content_type.startswith("image/") and len(data) > 64,
                      status=response.status, contentType=content_type, bytes=len(data))
        except Exception as error:
            check(label, False, errorType=type(error).__name__)

    with tempfile.TemporaryDirectory(prefix="kugou-provider-audit-") as data_dir:
        port = library.start_server(0, data_dir.encode("utf-8"))
        if port <= 0:
            raise RuntimeError("Provider server failed to start")
        try:
            info = call("provider/info")
            check("provider/info", info.get("code") == 0 and info.get("data", {}).get("version") == expected_version,
                  version=info.get("data", {}).get("version"))
            for method, field in [
                ("recommend/resource", "recommend"), ("personalized", "result"),
                ("top_playlist", "playlists"), ("top_playlist_highquality", "playlists"),
                ("top_album", "albums"), ("top_artists", "artists"),
                ("top_song", "data"), ("personalized_newsong", "result"),
                ("personal_fm", "data"), ("banner", "banners"),
                ("recommend/songs", "data"),
            ]:
                response = call(method, limit="6")
                items = response.get(field, [])
                if method == "recommend/songs":
                    items = items.get("dailySongs", []) if isinstance(items, dict) else []
                check(f"home {method}", response.get("code") == 0 and isinstance(items, list) and bool(items)
                      and all(item.get("targetId") if method == "banner" else item.get("id") for item in items),
                      code=response.get("code"), count=len(items) if isinstance(items, list) else 0)
                if not isinstance(items, list) or not items:
                    continue
                first = items[0]
                cover = first.get("coverUrl", first.get("avatarUrl", first.get("imageUrl")))
                image(f"home {method} artwork", cover)
                if method in ("recommend/resource", "personalized", "top_playlist", "top_playlist_highquality"):
                    detail = call("playlist/detail", id=str(first["id"]), limit="3")
                    data = detail.get("data", {})
                    check(f"home {method} opens playlist", detail.get("code") == 0 and bool(data.get("title"))
                          and bool(data.get("tracks")), code=detail.get("code"), tracks=len(data.get("tracks", [])))
                if method == "top_album":
                    detail = call("album", id=str(first["id"]))
                    data = detail.get("data", {})
                    check("home new album opens", detail.get("code") == 0 and bool(data.get("title"))
                          and bool(data.get("tracks")), code=detail.get("code"), tracks=len(data.get("tracks", [])))
                if method == "top_artists":
                    detail = call("artist/detail", id=str(first["id"]))
                    songs = call("artist/songs", id=str(first["id"]), limit="3")
                    check("home hot artist opens", detail.get("code") == 0 and songs.get("code") == 0
                          and bool(songs.get("data", {}).get("tracks")))
                if method == "banner":
                    for index, banner in enumerate(items):
                        if banner.get("targetType") != 1000:
                            continue
                        detail = call("playlist/detail", id=banner["targetId"], limit="3")
                        tracks = detail.get("data", {}).get("tracks", [])
                        check(f"home banner #{index + 1} opens", detail.get("code") == 0 and bool(tracks)
                              and all(track.get("title") and track.get("artist") and track.get("duration", 0) > 0
                                      and len(track.get("id", "")) == 32 for track in tracks),
                              code=detail.get("code"), tracks=len(tracks))
                        image(f"home banner #{index + 1} detail artwork", detail.get("data", {}).get("coverUrl"))
                        first_page = call("playlist/track/all", id=banner["targetId"], limit="3")
                        next_page = call("playlist/track/all", id=banner["targetId"], limit="3", offset="2")
                        a = first_page.get("data", {}).get("tracks", [])
                        b = next_page.get("data", {}).get("tracks", [])
                        check(f"home banner #{index + 1} pagination", len(a) == 3 and len(b) == 3
                              and a[2]["id"] == b[0]["id"], firstTracks=len(a), nextTracks=len(b))
                        total = detail.get("data", {}).get("total", 0)
                        if total > 0:
                            final = call("playlist/track/all", id=banner["targetId"], limit="3", offset=str(total - 1))
                            check(f"home banner #{index + 1} last page", final.get("code") == 0
                                  and len(final.get("data", {}).get("tracks", [])) == 1 and final.get("hasMore") is False)
                if method == "top_song":
                    similar = call("simi_song", id=first["id"], limit="6")
                    check("home similar songs", similar.get("code") == 0 and bool(similar.get("songs")),
                          code=similar.get("code"), tracks=len(similar.get("songs", [])))
            for region in (7, 96, 8, 16):
                response = call("top_song", type=str(region), limit="6")
                items = response.get("data", [])
                check(f"home new songs region {region}", response.get("code") == 0 and isinstance(items, list) and bool(items)
                      and all(len(item.get("id", "")) == 32 and item.get("title") for item in items),
                      code=response.get("code"), count=len(items) if isinstance(items, list) else 0)
                if isinstance(items, list) and items:
                    image(f"home region {region} track artwork", items[0].get("coverUrl"))
            context = call("playlist/track/all", id="7235936", limit="1")
            seeds = context.get("data", {}).get("tracks", [])
            if seeds:
                response = call("playmode/intelligence/list", id=seeds[0]["id"], pid="7235936", count="6")
                check("home intelligence mode", response.get("code") == 0 and bool(response.get("data")),
                      code=response.get("code"), tracks=len(response.get("songs", [])))
            else:
                check("home intelligence seed playlist", False, code=context.get("code"))
            if args.home_only:
                print(f"RESULT: failures={len(failures)}", flush=True)
                return bool(failures)
            rankings = call("toplist")
            rank_items = rankings.get("list", rankings.get("data", {}).get("list", []))
            check("rankings list", rankings.get("code") == 0 and bool(rank_items)
                  and all(isinstance(rank.get("id"), int) and (1 << 52) < rank["id"] < (1 << 53) for rank in rank_items),
                  code=rankings.get("code"), rankings=len(rank_items))
            for index, rank in enumerate(rank_items[:2]):
                detail = call("playlist/detail", id=str(rank["id"]), limit=3)
                data = detail.get("data", {})
                check(f"ranking detail #{index + 1}", detail.get("code") == 0 and data.get("id") == rank["id"]
                      and data.get("title") == rank.get("name") and data.get("total", 0) > 0,
                      code=detail.get("code"), total=data.get("total"), tracks=len(data.get("tracks", [])))
                image(f"ranking cover #{index + 1}", data.get("coverUrl"))
                first = call("playlist/track/all", id=str(rank["id"]), limit=3, offset=0)
                next_page = call("playlist/track/all", id=str(rank["id"]), limit=3, offset=2)
                first_tracks = first.get("data", {}).get("tracks", [])
                next_tracks = next_page.get("data", {}).get("tracks", [])
                check(f"ranking offset pagination #{index + 1}", first.get("code") == 0 and next_page.get("code") == 0
                      and len(first_tracks) == 3 and len(next_tracks) == 3 and first_tracks[2]["id"] == next_tracks[0]["id"],
                      firstTracks=len(first_tracks), nextTracks=len(next_tracks))
                check(f"ranking playable hashes #{index + 1}", bool(first_tracks)
                      and all(len(track.get("id", "")) == 32 and all(character in "0123456789abcdef" for character in track["id"])
                              and track.get("duration", 0) > 0 for track in first_tracks))
                if first_tracks:
                    image(f"ranking track cover #{index + 1}", first_tracks[0].get("coverUrl"))
                total = data.get("total", 0)
                if total > 0:
                    final = call("playlist/track/all", id=str(rank["id"]), limit=3, offset=total - 1)
                    final_data = final.get("data", {})
                    check(f"ranking final page #{index + 1}", final.get("code") == 0 and len(final_data.get("tracks", [])) == 1
                          and final.get("hasMore") is False,
                          code=final.get("code"), tracks=len(final_data.get("tracks", [])), hasMore=final.get("hasMore"))
            for identifier in ["7235936", "collection_3_2061171118_198_0"]:
                detail = call("playlist/detail", id=identifier, limit=3)
                data = detail.get("data", {})
                check("public playlist/detail", detail.get("code") == 0 and bool(data.get("title")) and data.get("total", 0) > 0,
                      code=detail.get("code"), total=data.get("total"), tracks=len(data.get("tracks", [])))
                image("public playlist cover", data.get("coverUrl"))
                tracks = call("playlist/track/all", id=identifier, limit=3, offset=2)
                data = tracks.get("data", {})
                check("public playlist/track/all", tracks.get("code") == 0 and len(data.get("tracks", [])) == 3,
                      code=tracks.get("code"), total=data.get("total"), tracks=len(data.get("tracks", [])),
                      covers=sum(bool(track.get("coverUrl")) for track in data.get("tracks", [])))
                if data.get("tracks"):
                    image("playlist track cover", data["tracks"][0].get("coverUrl"))

            search = call("cloudsearch", keywords="\u7eaf\u97f3\u4e50", limit=3, type=1)
            tracks = search.get("data", {}).get("tracks", [])
            check("song search", search.get("code") == 0 and len(tracks) == 3,
                  code=search.get("code"), tracks=len(tracks))
            if tracks:
                image("song search cover", tracks[0].get("coverUrl"))
                detail = call("song/detail", ids=",".join(track["id"] for track in tracks[:2]))
                detail_tracks = detail.get("data", {}).get("tracks", [])
                check("batch song/detail", detail.get("code") == 0 and len(detail_tracks) == min(2, len(tracks)),
                      code=detail.get("code"), tracks=len(detail_tracks),
                      covers=sum(bool(track.get("coverUrl")) for track in detail_tracks))
                for index, track in enumerate(detail_tracks):
                    image(f"song/detail cover {index + 1}", track.get("coverUrl"))

            artists = call("cloudsearch", keywords="\u5468\u6770\u4f26", limit=3, type=100)
            artist_items = artists.get("data", {}).get("artists", [])
            check("artist search", artists.get("code") == 0 and 0 < len(artist_items) <= 3,
                  code=artists.get("code"), artists=len(artist_items),
                  avatars=sum(bool(artist.get("avatarUrl")) for artist in artist_items))
            if artist_items:
                image("artist search avatar", artist_items[0].get("avatarUrl"))
                identifier = artist_items[0]["id"]
                detail = call("artist/detail", id=identifier)
                data = detail.get("data", {})
                check("artist/detail", detail.get("code") == 0 and bool(data.get("name")),
                      code=detail.get("code"))
                songs = call("artist/songs", id=identifier, limit=3)
                data = songs.get("data", {})
                check("artist/songs", songs.get("code") == 0 and bool(data.get("tracks")),
                      code=songs.get("code"), tracks=len(data.get("tracks", [])))
                artist_albums = call("artist/albums", id=identifier, limit=3)
                data = artist_albums.get("data", [])
                check("artist/albums", artist_albums.get("code") == 0 and isinstance(data, list) and bool(data),
                      code=artist_albums.get("code"), albums=len(data) if isinstance(data, list) else 0)

            albums = call("cloudsearch", keywords="\u8303\u7279\u897f", limit=3, type=10)
            album_items = albums.get("data", {}).get("albums", [])
            check("album search", albums.get("code") == 0 and bool(album_items),
                  code=albums.get("code"), albums=len(album_items))
            if album_items:
                image("album search cover", album_items[0].get("coverUrl"))
                detail = call("album/detail", id=album_items[0]["id"])
                data = detail.get("data", {})
                check("album/detail", detail.get("code") == 0 and bool(data.get("title")) and bool(data.get("tracks")),
                      code=detail.get("code"), titlePresent=bool(data.get("title")), tracks=len(data.get("tracks", [])))
                image("album/detail cover", data.get("coverUrl"))

            if cookie:
                for method in ["user/playlist/create", "user/playlist/collect"]:
                    response = call(method, uid=credentials.get("userid", ""), limit=30)
                    items = response.get("playlist")
                    check(f"authenticated {method} is read-only list", response.get("code") == 0 and isinstance(items, list)
                          and items == response.get("data"),
                          code=response.get("code"), playlists=len(items) if isinstance(items, list) else 0)
                playlists = call("user/playlist", uid=credentials.get("userid", ""), limit=30)
                items = playlists.get("data", [])
                check("authenticated user/playlist", playlists.get("code") == 0 and isinstance(items, list),
                      code=playlists.get("code"), playlists=len(items) if isinstance(items, list) else 0)
                for index, item in enumerate(items if isinstance(items, list) else []):
                    for method in ["playlist/detail", "playlist/track/all"]:
                        response = call(method, id=str(item["id"]), limit=3)
                        data = response.get("data", {})
                        check(f"authenticated {method} #{index + 1}", response.get("code") == 0,
                              code=response.get("code"), total=data.get("total"),
                              tracks=len(data.get("tracks", [])), hasMore=data.get("hasMore", response.get("hasMore")))
        finally:
            library.stop_server()
    print(f"RESULT: failures={len(failures)}", flush=True)
    return bool(failures)


if __name__ == "__main__":
    raise SystemExit(main())
