package cp.player.core.provider.kugou;

import java.io.ByteArrayOutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;

public class KugouSessionSmoke {
    private static int pass = 0;
    private static int fail = 0;

    public native long createNative(String providerId, String accountId, String credential, String settings);
    public native byte[] invokeNative(long handle, int operation, byte[] requestBytes);
    public native void cancelNative(long handle);
    public native void closeNative(long handle);

    public static void main(String[] args) throws Exception {
        if (args.length < 1) {
            System.err.println("usage: KugouSessionSmoke <path-to-kugou_music_rs.dll>");
            System.exit(2);
        }
        String libPath = args[0];
        System.load(libPath);

        KugouSessionSmoke runner = new KugouSessionSmoke();
        long handle = runner.createNative("kugou", null, null, null);
        check("createNative", handle != 0, "handle=" + handle);

        // 1. OP_CAPABILITIES (1)
        byte[] emptyReq = new byte[0];
        byte[] capResp = runner.invokeNative(handle, 1, emptyReq);
        check("invokeNative OP_CAPABILITIES (1)", capResp != null && capResp.length > 0 && capResp[0] == 0,
            "len=" + (capResp == null ? 0 : capResp.length));

        // 2. OP_SEARCH (2) -> SearchRequest { keyword: "纯音乐", limit: 10, offset: 0, filter: null }
        ByteArrayOutputStream searchReqBuf = new ByteArrayOutputStream();
        writeString(searchReqBuf, "纯音乐");
        writeInt(searchReqBuf, 10);
        writeInt(searchReqBuf, 0);
        writeOptionalNull(searchReqBuf); // filter = null
        byte[] searchResp = runner.invokeNative(handle, 2, searchReqBuf.toByteArray());
        check("invokeNative OP_SEARCH (2) [纯音乐]", searchResp != null && searchResp.length > 0 && searchResp[0] == 0,
            "respLen=" + (searchResp == null ? 0 : searchResp.length));

        java.util.List<String> trackIds = new java.util.ArrayList<>();
        if (searchResp != null && searchResp.length > 1 && searchResp[0] == 0) {
            ByteBuffer bb = ByteBuffer.wrap(searchResp, 1, searchResp.length - 1).order(ByteOrder.LITTLE_ENDIAN);
            int itemCount = bb.getInt();
            System.out.println("  -> Search returned " + itemCount + " tracks");
            for (int i = 0; i < itemCount; i++) {
                String tid = readString(bb);
                String ttitle = readString(bb);
                // read artists (list)
                int aCount = bb.getInt();
                for (int a = 0; a < aCount; a++) {
                    if (bb.get() != 0) readString(bb); // artist.id?
                    readString(bb); // artist.name
                }
                long dur = bb.getLong(); // durationMs
                if (bb.get() != 0) readString(bb); // album?
                if (bb.get() != 0) readString(bb); // coverUrl?
                trackIds.add(tid);
                if (i < 3) {
                    System.out.println("     Track " + i + ": id=" + tid + ", title=" + ttitle + ", dur=" + dur + "ms");
                }
            }
        }

        // 3. OP_GET (3) & OP_RESOLVE (5) & OP_GETLYRICS (10)
        String playableId = null;
        String playableUrl = null;
        for (String tid : trackIds) {
            // OP_RESOLVE (5)
            ByteArrayOutputStream resolveReqBuf = new ByteArrayOutputStream();
            writeString(resolveReqBuf, tid);
            writeString(resolveReqBuf, "standard");
            byte[] resolveResp = runner.invokeNative(handle, 5, resolveReqBuf.toByteArray());
            if (resolveResp != null && resolveResp.length > 1 && resolveResp[0] == 0) {
                ByteBuffer bb = ByteBuffer.wrap(resolveResp, 1, resolveResp.length - 1).order(ByteOrder.LITTLE_ENDIAN);
                String url = readString(bb);
                String quality = readString(bb);
                if (url.startsWith("http")) {
                    playableId = tid;
                    playableUrl = url;
                    System.out.println("  -> Resolved track " + tid + " [" + quality + "]: " + (url.length() > 60 ? url.substring(0, 60) + "..." : url));
                    break;
                }
            }
        }
        check("invokeNative OP_RESOLVE (5)", playableUrl != null,
            playableUrl == null ? "No free tracks found in sample" : "url=" + (playableUrl.length() > 40 ? playableUrl.substring(0, 40) + "..." : playableUrl));

        if (!trackIds.isEmpty()) {
            String testId = playableId != null ? playableId : trackIds.get(0);
            // OP_GET (3)
            ByteArrayOutputStream getReqBuf = new ByteArrayOutputStream();
            writeString(getReqBuf, testId);
            byte[] getResp = runner.invokeNative(handle, 3, getReqBuf.toByteArray());
            check("invokeNative OP_GET (3)", getResp != null && getResp.length > 0 && getResp[0] == 0,
                "respLen=" + (getResp == null ? 0 : getResp.length));

            // OP_GETLYRICS (10)
            ByteArrayOutputStream lyricReqBuf = new ByteArrayOutputStream();
            writeString(lyricReqBuf, testId);
            byte[] lyricResp = runner.invokeNative(handle, 10, lyricReqBuf.toByteArray());
            check("invokeNative OP_GETLYRICS (10)", lyricResp != null && lyricResp.length > 0 && lyricResp[0] == 0,
                "respLen=" + (lyricResp == null ? 0 : lyricResp.length));
        }

        // 4. OP_GETRANKINGS (28)
        byte[] rankResp = runner.invokeNative(handle, 28, emptyReq);
        check("invokeNative OP_GETRANKINGS (28)", rankResp != null && rankResp.length > 0 && rankResp[0] == 0,
            "respLen=" + (rankResp == null ? 0 : rankResp.length));

        // 5. OP_GETHOTSEARCHES (56)
        byte[] hotResp = runner.invokeNative(handle, 56, emptyReq);
        check("invokeNative OP_GETHOTSEARCHES (56)", hotResp != null && hotResp.length > 0 && hotResp[0] == 0,
            "respLen=" + (hotResp == null ? 0 : hotResp.length));

        // 6. OP_AUTHENTICATE (60) with "qr_key"
        ByteArrayOutputStream authReqBuf = new ByteArrayOutputStream();
        writeString(authReqBuf, "qr_key"); // action
        writeOptionalNull(authReqBuf);     // email
        writeOptionalNull(authReqBuf);     // phone
        writeOptionalNull(authReqBuf);     // password
        writeOptionalNull(authReqBuf);     // captcha
        authReqBuf.write(0);               // md5: false
        writeOptionalNull(authReqBuf);     // imageCaptcha
        writeOptionalNull(authReqBuf);     // challengeCookie
        writeOptionalNull(authReqBuf);     // key
        writeOptionalNull(authReqBuf);     // sessionId
        writeOptionalNull(authReqBuf);     // verifyId
        writeOptionalNull(authReqBuf);     // extraJson
        writeOptionalNull(authReqBuf);     // countryCode
        byte[] authResp = runner.invokeNative(handle, 60, authReqBuf.toByteArray());
        check("invokeNative OP_AUTHENTICATE (60) [qr_key]", authResp != null && authResp.length > 0 && authResp[0] == 0,
            "respLen=" + (authResp == null ? 0 : authResp.length));

        // 7. OP_CHECKTRACKAVAILABLE (126)
        ByteArrayOutputStream availReqBuf = new ByteArrayOutputStream();
        writeString(availReqBuf, "test_hash");
        writeInt(availReqBuf, 128000);
        byte[] availResp = runner.invokeNative(handle, 126, availReqBuf.toByteArray());
        printIfFailure(availResp);
        check("invokeNative OP_CHECKTRACKAVAILABLE (126)", availResp != null && availResp.length > 0 && availResp[0] == 0,
            "respLen=" + (availResp == null ? 0 : availResp.length));

        // 8. Unsupported operation returns status 1 (Failure)
        byte[] unsupportedResp = runner.invokeNative(handle, 9999, emptyReq);
        check("invokeNative OP_UNSUPPORTED (9999) returns Failure (status 1)",
            unsupportedResp != null && unsupportedResp.length > 0 && unsupportedResp[0] == 1,
            "status=" + (unsupportedResp == null ? -1 : unsupportedResp[0]));

        // 9. Close native session
        runner.closeNative(handle);
        check("closeNative", true, "closed");

        System.out.println("\n----------------------------------------");
        System.out.println("Smoke Results: PASS=" + pass + ", FAIL=" + fail);
        System.out.println("----------------------------------------");
        if (fail > 0) System.exit(1);
    }

    private static void writeInt(ByteArrayOutputStream out, int v) {
        out.write(v & 0xFF);
        out.write((v >>> 8) & 0xFF);
        out.write((v >>> 16) & 0xFF);
        out.write((v >>> 24) & 0xFF);
    }

    private static void writeString(ByteArrayOutputStream out, String s) {
        byte[] bytes = s.getBytes(StandardCharsets.UTF_8);
        writeInt(out, bytes.length);
        out.write(bytes, 0, bytes.length);
    }

    private static void writeOptionalNull(ByteArrayOutputStream out) {
        out.write(0); // boolean false
    }

    private static String readString(ByteBuffer bb) {
        int len = bb.getInt();
        byte[] bytes = new byte[len];
        bb.get(bytes);
        return new String(bytes, StandardCharsets.UTF_8);
    }

    private static void check(String name, boolean ok, String detail) {
        if (ok) {
            pass++;
            System.out.println("[PASS] " + name + " (" + detail + ")");
        } else {
            fail++;
            System.out.println("[FAIL] " + name + " (" + detail + ")");
        }
    }

    private static void printIfFailure(byte[] resp) {
        if (resp != null && resp.length > 1 && resp[0] == 1) {
            ByteBuffer bb = ByteBuffer.wrap(resp, 1, resp.length - 1).order(ByteOrder.LITTLE_ENDIAN);
            String category = readString(bb);
            String message = readString(bb);
            System.out.println("  -> Failure [" + category + "]: " + message);
        }
    }
}
