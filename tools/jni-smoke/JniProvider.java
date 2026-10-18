// CPPlayer-KMP JNI 冒烟测试。
//
// 文件名/类名必须是 `JniProvider`、包必须是 `cp.player.core.provider`：JNI 按
// `Java_<包名下划线化>_<类名>_<方法名>` 查符号，换个名字就注定 UnsatisfiedLinkError。
//
// 这个类**故意**放在 `cp.player.core.provider` 包里、类名也故意叫 `JniProvider`：
// JNI 的符号名是按 `Java_<包名下划线化>_<类名>_<方法名>` 查的，只有包名/类名/方法
// 签名与宿主完全一致，才能验证导出符号、参数个数、返回值类型真的对得上。
//
// 用法（需要 JDK 17+）：
//   javac -encoding UTF-8 -d out tools/jni-smoke/JniProvider.java
//   java -cp out -Dfile.encoding=UTF-8 cp.player.core.provider.JniProvider \
//        target/release/kugou_music_rs.dll [测试关键词]
//
// 退出码 0 = 全部通过；1 = 有失败项。

package cp.player.core.provider;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

public class JniProvider {
    private static int pass = 0;
    private static int fail = 0;

    // 与宿主 core/src/jvmMain/.../JniProvider.kt 的声明逐字对应
    native void startNativeServer(String host, int port);
    native String nativeCallApi(String method, String paramsJson);
    native String analyzeAudioFile(String path);

    public static void main(String[] args) throws Exception {
        if (args.length < 1) {
            System.err.println("usage: JniProvider <path-to-kugou_music_rs.dll|.so> [keyword]");
            System.exit(2);
        }
        String libPath = args[0];
        String keyword = args.length > 1 ? args[1] : "纯音乐";
        System.load(libPath);

        JniProvider p = new JniProvider();

        // 宿主切音源时会先调 startNativeServer(host, port)
        p.startNativeServer("127.0.0.1", 18099);
        check("startNativeServer(host, port)", true, "port=18099");

        String search = p.nativeCallApi("cloudsearch",
            "{\"keywords\":\"" + keyword + "\",\"limit\":\"10\",\"type\":\"1\"}");
        check("search (cloudsearch)", search != null && search.contains("\"tracks\""),
            "len=" + len(search));

        List<String> ids = extractIds(search);

        // 找一首能拿到直链的（热门曲多数需要付费/会员，属于上游策略而非模块缺陷）
        String playable = null;
        String playUrl = null;
        for (String id : ids) {
            String r = p.nativeCallApi("song/url/v1",
                "{\"id\":\"" + id + "\",\"level\":\"standard\"}");
            int at = r.indexOf("\"url\":\"http");
            if (at >= 0) {
                int st = r.indexOf("\"url\":\"", at) + 7;
                playable = id;
                playUrl = r.substring(st, r.indexOf('"', st));
                break;
            }
        }
        check("song/url/v1 返回直链", playable != null,
            playable == null ? "候选曲目均无直链（付费/会员限制）" : playable);

        if (playUrl != null) {
            // 直链必须真的能下，否则"有 url 但放不出声"仍然算坏
            try {
                HttpURLConnection c = (HttpURLConnection) new URL(playUrl).openConnection();
                c.setRequestProperty("User-Agent", "Mozilla/5.0");
                c.setConnectTimeout(15000);
                c.setReadTimeout(15000);
                InputStream in = c.getInputStream();
                byte[] buf = new byte[65536];
                int n = in.read(buf);
                in.close();
                check("直链可下载", n > 4096, "read=" + n + " bytes, http=" + c.getResponseCode());
            } catch (Exception e) {
                check("直链可下载", false, e.toString());
            }
        }

        String id = playable != null ? playable : (ids.isEmpty() ? "" : ids.get(0));

        String lyric = p.nativeCallApi("lyric/new", "{\"id\":\"" + id + "\"}");
        check("lyric/new", lyric != null && lyric.contains("\"lyric\""), "len=" + len(lyric));

        String detail = p.nativeCallApi("song/detail", "{\"ids\":\"" + id + "\"}");
        check("song/detail", detail != null && detail.contains("\"title\":\"")
            && !detail.contains("\"title\":\"\""), head(detail));

        String toplist = p.nativeCallApi("toplist", "{}");
        check("toplist", toplist != null && toplist.contains("\"list\""), "len=" + len(toplist));

        String hot = p.nativeCallApi("search/hot/detail", "{}");
        check("search/hot/detail", hot != null && hot.contains("searchWord"), "len=" + len(hot));

        String suggest = p.nativeCallApi("search/suggest", "{\"keywords\":\"周杰\"}");
        check("search/suggest", suggest != null && suggest.length() > 20, "len=" + len(suggest));

        String qr = p.nativeCallApi("login/qr/key", "{}");
        check("login/qr/key", qr != null && qr.contains("unikey"), head(qr));

        // 未知路由必须优雅返回 JSON —— 崩溃/空指针都会让宿主以为音源坏了
        String unknown = p.nativeCallApi("definitely/not/a/route", "{}");
        check("未知路由不崩溃", unknown != null && unknown.contains("code"), head(unknown));

        // 内嵌 HTTP 服务（调试通道）也要活着
        try {
            HttpURLConnection c = (HttpURLConnection) new URL(
                "http://127.0.0.1:18099/search?keyword=" + urlEncode(keyword) + "&limit=3").openConnection();
            c.setConnectTimeout(5000);
            c.setReadTimeout(15000);
            int code = c.getResponseCode();
            ByteArrayOutputStream bos = new ByteArrayOutputStream();
            InputStream in = c.getInputStream();
            byte[] b = new byte[8192];
            int n;
            while ((n = in.read(b)) > 0) bos.write(b, 0, n);
            in.close();
            String body = new String(bos.toByteArray(), StandardCharsets.UTF_8);
            check("内置 HTTP /search", code == 200 && body.contains("data"),
                "http=" + code + " len=" + body.length());
        } catch (Exception e) {
            check("内置 HTTP /search", false, e.toString());
        }

        String analyze = p.analyzeAudioFile("C:/nonexistent.mp3");
        check("analyzeAudioFile", analyze != null && analyze.contains("error"), head(analyze));

        System.out.println();
        System.out.println("RESULT: pass=" + pass + " fail=" + fail);
        System.exit(fail == 0 ? 0 : 1);
    }

    private static void check(String name, boolean ok, String detail) {
        if (ok) {
            pass++;
            System.out.println("PASS  " + name + (detail.isEmpty() ? "" : "  [" + detail + "]"));
        } else {
            fail++;
            System.out.println("FAIL  " + name + "  [" + detail + "]");
        }
    }

    private static int len(String s) {
        return s == null ? -1 : s.length();
    }

    private static String head(String s) {
        if (s == null) return "null";
        String t = s.replace("\n", " ");
        return t.length() > 120 ? t.substring(0, 120) + "..." : t;
    }

    /** 从搜索响应里粗暴抠出 id（避免为了冒烟测试引入 JSON 依赖）。 */
    private static List<String> extractIds(String json) {
        List<String> ids = new ArrayList<>();
        if (json == null) return ids;
        int i = 0;
        while ((i = json.indexOf("\"id\":\"", i)) >= 0) {
            int st = i + 6;
            int en = json.indexOf('"', st);
            if (en < 0) break;
            ids.add(json.substring(st, en));
            i = en;
        }
        return ids;
    }

    private static String urlEncode(String s) throws Exception {
        return java.net.URLEncoder.encode(s, "UTF-8");
    }
}
