use crate::crypto::crypto_md5_str;
use crate::util::{calculate_mid, get_guid, random_string};
use serde_json::{json, Value};
use std::sync::Mutex;

/// DeviceConfig replicates `device_config.js`.
/// Fields use interior mutability so the static OnceLock instance can be updated
/// after server startup (dfid/mid are set by /register/dev).
#[derive(Debug)]
pub struct DeviceConfig {
    inner: Mutex<DeviceInner>,
}

#[derive(Debug, Clone)]
struct DeviceInner {
    dfid: Option<String>,
    mid: Option<String>,
    uuid: Option<String>,
    guid: Option<String>,
    server_dev: Option<String>,
    mac: Option<String>,
}

impl Default for DeviceInner {
    fn default() -> Self {
        DeviceInner {
            dfid: None,
            mid: None,
            uuid: None,
            guid: None,
            server_dev: None,
            mac: None,
        }
    }
}

impl DeviceConfig {
    pub fn new() -> Self {
        DeviceConfig {
            inner: Mutex::new(DeviceInner::default()),
        }
    }

    /// Global singleton (same storage as config.rs DEVICE_CONFIG).
    pub fn instance() -> &'static DeviceConfig {
        crate::config::DEVICE_CONFIG.get_or_init(DeviceConfig::new)
    }

    pub fn init_device_info(&self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.guid.is_none() {
            inner.guid = Some(get_guid());
        }
        if inner.mid.is_none() {
            let guid = inner.guid.clone().unwrap_or_default();
            inner.mid = Some(calculate_mid(&guid));
        }
        if inner.uuid.is_none() {
            inner.uuid = match (&inner.dfid, &inner.mid) {
                (Some(dfid), Some(mid)) => Some(crypto_md5_str(&format!("{}{}", dfid, mid))),
                _ => Some("-".to_string()),
            };
        }
        if inner.server_dev.is_none() {
            inner.server_dev = Some(random_string(10).to_uppercase());
        }
        if inner.mac.is_none() {
            inner.mac = Some("02:00:00:00:00:00".to_string());
        }
    }

    pub fn set_dfid(&self, new_dfid: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.dfid = Some(new_dfid.to_string());
        if let Some(mid) = &inner.mid {
            inner.uuid = Some(crypto_md5_str(&format!("{}{}", new_dfid, mid)));
        }
    }

    pub fn set_mid(&self, new_mid: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.mid = Some(new_mid.to_string());
        if let Some(dfid) = &inner.dfid {
            inner.uuid = Some(crypto_md5_str(&format!("{}{}", dfid, new_mid)));
        }
    }

    /// Restore a previously persisted guid. Since mid is derived from guid
    /// (calculate_mid), recompute it so the whole device identity stays
    /// consistent across process restarts — otherwise every launch would
    /// generate a brand-new guid and register a brand-new device upstream.
    pub fn set_guid(&self, new_guid: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.guid = Some(new_guid.to_string());
        inner.mid = Some(calculate_mid(new_guid));
        if let Some(dfid) = &inner.dfid {
            inner.uuid = Some(crypto_md5_str(&format!("{}{}", dfid, calculate_mid(new_guid))));
        }
    }

    pub fn set_server_dev(&self, new_dev: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.server_dev = Some(new_dev.to_string());
    }

    pub fn get_dfid(&self) -> String {
        self.init_device_info();
        self.inner.lock().unwrap().dfid.clone().unwrap_or_else(|| "-".to_string())
    }

    pub fn get_mid(&self) -> String {
        self.init_device_info();
        self.inner.lock().unwrap().mid.clone().unwrap_or_else(|| "-".to_string())
    }

    pub fn get_uuid(&self) -> String {
        self.init_device_info();
        self.inner.lock().unwrap().uuid.clone().unwrap_or_else(|| "-".to_string())
    }

    pub fn get_guid(&self) -> String {
        self.init_device_info();
        self.inner.lock().unwrap().guid.clone().unwrap_or_else(|| "-".to_string())
    }

    pub fn get_server_dev(&self) -> String {
        self.init_device_info();
        self.inner.lock().unwrap().server_dev.clone().unwrap_or_else(|| "-".to_string())
    }

    pub fn get_mac(&self) -> String {
        self.init_device_info();
        self.inner.lock().unwrap().mac.clone().unwrap_or_else(|| "-".to_string())
    }

    pub fn get_device_info(&self) -> Value {
        self.init_device_info();
        let inner = self.inner.lock().unwrap();
        json!({
            "dfid": inner.dfid.clone().unwrap_or_else(|| "-".to_string()),
            "mid": inner.mid.clone().unwrap_or_else(|| "-".to_string()),
            "uuid": inner.uuid.clone().unwrap_or_else(|| "-".to_string()),
            "guid": inner.guid.clone().unwrap_or_else(|| "-".to_string()),
            "serverDev": inner.server_dev.clone().unwrap_or_else(|| "-".to_string()),
            "mac": inner.mac.clone().unwrap_or_else(|| "-".to_string()),
        })
    }

    /// Load cached device_info.json (replicating server.js loadCachedDeviceInfo).
    pub fn load_cached(&self, data_dir: &str) -> bool {
        let path = format!("{}/device_info.json", data_dir);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => return false,
        };
        let data: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let dfid = data.get("dfid").and_then(|v| v.as_str()).unwrap_or("");
        if dfid.is_empty() {
            return false;
        }
        self.set_dfid(dfid);
        if let Some(mid) = data.get("mid").and_then(|v| v.as_str()) {
            if !mid.is_empty() {
                self.set_mid(mid);
            }
        }
        if let Some(guid) = data.get("guid").and_then(|v| v.as_str()) {
            if !guid.is_empty() {
                self.set_guid(guid);
            }
        }
        if let Some(server_dev) = data.get("serverDev").and_then(|v| v.as_str()) {
            if !server_dev.is_empty() {
                self.set_server_dev(server_dev);
            }
        }
        self.init_device_info();
        true
    }

    /// Persist device_info.json (replicating server.js registerDeviceAndGetDfid write).
    pub fn save(&self, data_dir: &str, dfid: &str, mid: Option<&str>) {
        let mid = match mid {
            Some(m) if !m.is_empty() => m.to_string(),
            _ => self.get_mid(),
        };
        let info = json!({
            "dfid": dfid,
            "mid": mid,
            "uuid": self.get_uuid(),
            "guid": self.get_guid(),
            "serverDev": self.get_server_dev(),
            "mac": self.get_mac(),
        });
        let path = format!("{}/device_info.json", data_dir);
        if let Ok(text) = serde_json::to_string_pretty(&info) {
            let _ = std::fs::write(path, text);
        }
    }
}

// ---------------------------------------------------------------------------
// 嵌入式（JNI）运行时的可写目录与设备注册
//
// 原 JS 版是长驻 HTTP 服务：带 data_dir 启动，前端在登录/播放前先调
// `/register/dev` 拿 dfid。CPPlayer-KMP 走进程内 JNI，**没有**这两个前提：
//   · 没有 data_dir（宿主只给 host/port），device_info.json 无处可落；
//   · 不会调 `/register/dev`（宿主的 API 方法表里没有这个端点）。
// 结果就是 dfid 恒为 "-"，`/v5/url` 一律 502 —— 现象是"能搜到歌，但一放就失败"。
// 这里补上：懒注册 + 落盘复用。
// ---------------------------------------------------------------------------

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

/// 模块可写目录（进程级）。JNI 侧会用 `java.io.tmpdir` 覆盖它 ——
/// Android 上那是应用私有的 cache 目录（可写），桌面上是系统临时目录。
static DATA_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// 注册串行锁：并发首调（搜索 + 播放同时打进来）只允许一个线程去注册，
/// 否则会向上游登记出多台"设备"。
static REGISTER_LOCK: Mutex<()> = Mutex::new(());
static LAST_ATTEMPT: Mutex<Option<Instant>> = Mutex::new(None);
/// 注册失败后的重试间隔（网络不好时别每次调用都去撞上游）。
const REGISTER_RETRY: Duration = Duration::from_secs(60);

/// 注册成功的进程级标记，命中后热路径只读一个原子变量。
static REGISTERED: AtomicBool = AtomicBool::new(false);

/// 覆盖模块数据目录（JNI 初始化时调用）。
pub fn set_data_dir(dir: &str) {
    if dir.trim().is_empty() {
        return;
    }
    let mut guard = DATA_DIR.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some(PathBuf::from(dir));
}

/// 模块数据目录。优先级：JNI 注入 > `KUGOU_MUSIC_DATA_DIR` > 系统临时目录。
pub fn data_dir() -> PathBuf {
    if let Some(dir) = DATA_DIR.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return dir;
    }
    if let Ok(dir) = std::env::var("KUGOU_MUSIC_DATA_DIR") {
        if !dir.trim().is_empty() {
            let p = PathBuf::from(dir);
            let _ = std::fs::create_dir_all(&p);
            return p;
        }
    }
    let p = std::env::temp_dir().join("kugou-music-rs");
    let _ = std::fs::create_dir_all(&p);
    p
}

fn is_valid_dfid(dfid: &str) -> bool {
    !dfid.is_empty() && dfid != "-"
}

/// 把设备身份并入 cookie 对象（宿主不注入 cookie 时的兜底）。
///
/// 部分模块（如 `/v1/audio/audio`）从 `cookie` 里读 `dfid` / `KUGOU_API_MID`，
/// JS 版由 App 注入；JNI 宿主没有这一步，缺了就签名校验失败。
pub fn merge_device_identity(cookie: &mut Value) {
    ensure_registered();
    let info = DeviceConfig::instance().get_device_info();
    let Some(map) = cookie.as_object_mut() else {
        return;
    };
    for (cookie_key, info_key) in [
        ("dfid", "dfid"),
        ("KUGOU_API_MID", "mid"),
        ("uuid", "uuid"),
        ("KUGOU_API_GUID", "guid"),
    ] {
        let empty = map
            .get(cookie_key)
            .map(|v| v.as_str().map(|s| s.is_empty()).unwrap_or(false) || v.is_null())
            .unwrap_or(true);
        if empty {
            if let Some(v) = info.get(info_key) {
                let s = v.as_str().unwrap_or("");
                if !s.is_empty() && s != "-" {
                    map.insert(cookie_key.to_string(), json!(s));
                }
            }
        }
    }
}

/// 确保拿到 dfid（设备身份）。幂等、带节流，失败也不向上抛 ——
/// 搜索等不依赖 dfid 的接口不应该因为注册失败而整体不可用。
pub fn ensure_registered() {
    if REGISTERED.load(AtomicOrdering::Acquire) {
        return;
    }
    if is_valid_dfid(&DeviceConfig::instance().get_dfid()) {
        REGISTERED.store(true, AtomicOrdering::Release);
        return;
    }

    // 串行化：并发首调只让一个线程真正去注册。
    let _guard = REGISTER_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    if is_valid_dfid(&DeviceConfig::instance().get_dfid()) {
        REGISTERED.store(true, AtomicOrdering::Release);
        return;
    }

    let dir = data_dir();

    // 1) 复用上次落盘的设备身份（冷启动最常见路径，零网络开销）。
    if DeviceConfig::instance().load_cached(&dir.to_string_lossy()) {
        if is_valid_dfid(&DeviceConfig::instance().get_dfid()) {
            log::info!("device identity restored from {}", dir.display());
            REGISTERED.store(true, AtomicOrdering::Release);
            return;
        }
    }

    // 2) 节流：失败后 REGISTER_RETRY 内不再重试。
    {
        let mut last = LAST_ATTEMPT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = *last {
            if t.elapsed() < REGISTER_RETRY {
                return;
            }
        }
        *last = Some(Instant::now());
    }

    // 3) 真正去上游注册。register_dev 内部会缓存/复用 dfid，重复调用是幂等的。
    let ctx = crate::modules::Ctx {
        ip: String::new(),
        body_bytes: None,
    };
    match crate::modules::register_dev::handle(&serde_json::json!({}), &ctx) {
        Ok(resp) => {
            let body = resp.body.to_json();
            match body
                .get("data")
                .and_then(|d| d.get("dfid"))
                .and_then(|v| v.as_str())
            {
                Some(dfid) if is_valid_dfid(dfid) => {
                    DeviceConfig::instance().set_dfid(dfid);
                    DeviceConfig::instance().save(&dir.to_string_lossy(), dfid, None);
                    REGISTERED.store(true, AtomicOrdering::Release);
                    log::info!("device registered: dfid={dfid} (saved to {})", dir.display());
                }
                _ => log::warn!(
                    "register/dev returned no dfid: {}",
                    body.to_string().chars().take(200).collect::<String>()
                ),
            }
        }
        Err(err) => log::warn!("register/dev failed: status {}", err.status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process;

    fn tmp_dir() -> String {
        let d = std::env::temp_dir()
            .join(format!("kugou_dev_test_{}", process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.to_str().unwrap().to_string()
    }

    /// Core regression test for the device-identity bug: every app cold start
    /// (a fresh local server process) used to generate a brand-new random guid,
    /// so the account kept registering "new devices". After the fix, guid (and
    /// the mid derived from it) MUST round-trip through device_info.json across
    /// independent instances — exactly what two separate processes observe.
    #[test]
    fn guid_stable_across_instances() {
        let dir = tmp_dir();
        let f = format!("{}/device_info.json", dir);
        let _ = std::fs::remove_file(&f);

        // "launch 1": fresh instance, obtain a device identity, then persist.
        let a = DeviceConfig::new();
        let dfid = "TESTDFID1234567890";
        a.set_dfid(dfid);
        a.init_device_info();
        let guid1 = a.get_guid();
        assert_ne!(guid1, "-", "a fresh instance must own a non-empty guid");
        a.save(&dir, dfid, Some(&a.get_mid()));
        drop(a);

        // "launch 2": an independent instance reads the file back. This is the
        // cross-process contract — shared statics play no part here.
        let b = DeviceConfig::new();
        assert!(b.load_cached(&dir), "device_info.json must be loadable");
        assert_eq!(b.get_dfid(), dfid, "dfid must be restored");
        assert_eq!(
            b.get_guid(),
            guid1,
            "guid MUST be restored across instances, not regenerated"
        );
        assert_eq!(
            b.get_mid(),
            calculate_mid(&guid1),
            "mid must stay consistent with the restored guid"
        );
        let _ = std::fs::remove_file(&f);
    }
}
