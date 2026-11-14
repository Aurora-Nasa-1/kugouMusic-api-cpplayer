//! C ABI（`cpp_*`）导出 —— 供 GraalVM Native Image 宿主按符号直调。
//!
//! 规范在宿主仓库 `docs/dev/PROVIDER_NATIVE_ABI.md`。背景：JNI 绑定挂在类对象上，
//! 宿主做到「每包独立绑定」的办法是按包 `defineClass` 一份 `NativeSession`，而
//! native image 既不提供类文件资源，也不支持运行时定义类。所以同一个 DLL 里
//! **追加**一套普通 C 导出，JNI 那 7 组符号（含 `cp.player.sdk.jni.NativeSession`）
//! 原样保留，JVM / Android 行为一点不变。
//!
//! 两条硬约定（宿主侧也是按这个实现，改一边就要改另一边）：
//! 1. **内存全部由宿主分配**：本库不释放宿主的指针，也不返回需要宿主 `free` 的指针
//!    （Windows 下两个模块可能用不同的 CRT 堆，跨 DLL free 必崩）。
//! 2. **容量不足返回 `1`**，把所需长度写进 `*outLen`，宿主扩容后重试（幂等）。
//!
//! 响应字节与 JNI 完全一致：`KugouSession::dispatch` 的返回值本身就带状态首字节
//! （`0` 成功 / `1` 失败 + `Failure`），这里原样拷出去，不做二次封装。
#![allow(unsafe_code)]

use std::ffi::{c_char, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::OnceLock;

use crate::sdk::session::KugouSession;

/// 本库实现的 ABI 版本。宿主先调 `cpp_abi_version`，不认就回落 JNI（JVM）或直接报错（原生构建）。
pub const ABI_VERSION: i32 = 1;

/// `cpp_invoke` 返回码，见规范第 4 节。
const INVOKE_OK: i32 = 0;
const INVOKE_BUFFER_TOO_SMALL: i32 = 1;
const INVOKE_FAILED: i32 = 2;

/// 原生路径的运行时初始化只做一次（数据目录）。
///
/// ⚠️ 这里**不能**像 JNI 路径那样 `std::thread::spawn(ensure_registered)`：宿主关闭
/// 最后一个会话时会 `Arena.close()`，FFM 据此 `FreeLibrary`。JNI 不会卸载 DLL，
/// 但原生宿主会 —— 有个游离线程还跑在本库的代码里，模块一卸载就是 0xC0000005
/// （2026-11-14 实测：两用例全绿，进程退出时崩在访问违例）。
/// 设备注册改由首次真正用到 dfid 的请求**同步**触发（`merge_device_identity` →
/// `ensure_registered`），本函数只留纯内存/文件系统操作。
static NATIVE_INIT: OnceLock<()> = OnceLock::new();

fn ensure_native_init() {
    NATIVE_INIT.get_or_init(|| {
        if let Some(dir) = native_data_dir() {
            let _ = std::fs::create_dir_all(&dir);
            crate::device::set_data_dir(&dir);
            log::info!("kugou module data dir (c abi): {dir}");
        }
    });
}

/// 原生宿主没有 `JNIEnv`，取不到 `java.io.tmpdir`，改按平台惯例找可写目录。
///
/// 顺序：`KUGOU_MUSIC_DATA_DIR` > Windows `LOCALAPPDATA` > macOS `~/Library/Caches`
/// > `XDG_CACHE_HOME` / `~/.cache`。全都取不到就返回 `None`，让
/// `device::data_dir()` 用它自己的临时目录兜底。
fn native_data_dir() -> Option<String> {
    if let Ok(dir) = std::env::var("KUGOU_MUSIC_DATA_DIR") {
        if !dir.trim().is_empty() {
            return Some(dir);
        }
    }
    let base = if cfg!(windows) {
        std::env::var("LOCALAPPDATA").ok()
    } else if cfg!(target_os = "macos") {
        std::env::var("HOME").ok().map(|home| format!("{home}/Library/Caches"))
    } else {
        std::env::var("XDG_CACHE_HOME")
            .ok()
            .or_else(|| std::env::var("HOME").ok().map(|home| format!("{home}/.cache")))
    };
    base.filter(|dir| !dir.trim().is_empty()).map(|dir| {
        format!(
            "{}/kugou-music-rs",
            dir.trim_end_matches(['/', '\\'])
        )
    })
}

/// 读一个 UTF-8 C 串；空指针 = None（`credential` / `settings` 允许为空）。
unsafe fn read_cstr(value: *const c_char) -> Option<String> {
    if value.is_null() {
        return None;
    }
    CStr::from_ptr(value)
        .to_str()
        .ok()
        .map(|text| text.to_owned())
}

/// 把错误原因写进宿主提供的缓冲（永远 NUL 结尾，超长则截断）。
unsafe fn write_error(err: *mut c_char, err_cap: i32, message: &str) {
    if err.is_null() || err_cap <= 0 {
        return;
    }
    let capacity = (err_cap as usize).saturating_sub(1);
    let bytes = message.as_bytes();
    let length = bytes.len().min(capacity);
    ptr::copy_nonoverlapping(bytes.as_ptr(), err as *mut u8, length);
    *((err as *mut u8).add(length)) = 0;
}

#[no_mangle]
pub extern "C" fn cpp_abi_version() -> i32 {
    ABI_VERSION
}

/// `int64_t cpp_create(providerId, accountId, credential, settingsJson, err, errCap)`
///
/// 返回 0 表示失败（原因写进 `err`）；非 0 是会话句柄，之后交给 `cpp_invoke`。
#[no_mangle]
pub unsafe extern "C" fn cpp_create(
    provider_id: *const c_char,
    account_id: *const c_char,
    credential: *const c_char,
    settings_json: *const c_char,
    err: *mut c_char,
    err_cap: i32,
) -> i64 {
    let created = catch_unwind(AssertUnwindSafe(|| {
        ensure_native_init();
        let pid = read_cstr(provider_id).unwrap_or_else(|| "kugou".to_string());
        let aid = read_cstr(account_id);
        let session = Box::new(KugouSession::new(
            pid,
            aid,
            read_cstr(credential),
            read_cstr(settings_json),
        ));
        Box::into_raw(session) as i64
    }));
    match created {
        Ok(handle) if handle != 0 => handle,
        Ok(_) => {
            write_error(err, err_cap, "native session creation returned no handle");
            0
        }
        Err(_) => {
            write_error(err, err_cap, "native session creation panicked");
            0
        }
    }
}

/// `int32_t cpp_invoke(handle, operation, req, reqLen, out, outCap, outLen)`
///
/// 返回 0 成功 / 1 容量不足（`*outLen` 是需要量）/ 2 ABI 层失败。
/// 业务错误不走返回码，而是 Wire 首字节（与 JNI 一致）。
#[no_mangle]
pub unsafe extern "C" fn cpp_invoke(
    handle: i64,
    operation: i32,
    request: *const u8,
    request_len: i32,
    out: *mut u8,
    out_cap: i32,
    out_len: *mut i32,
) -> i32 {
    if !out_len.is_null() {
        *out_len = 0;
    }
    if handle == 0 {
        return INVOKE_FAILED;
    }
    let request_bytes: &[u8] = if request.is_null() || request_len <= 0 {
        &[]
    } else {
        std::slice::from_raw_parts(request, request_len as usize)
    };

    let response = match catch_unwind(AssertUnwindSafe(|| {
        let session = &*(handle as *const KugouSession);
        session.dispatch(operation, request_bytes)
    })) {
        Ok(bytes) => bytes,
        // 与 JNI 路径的差别：那边把 panic 包成 Failure 帧返回，这里按规范回 2。
        // panic 属 ABI 层异常，宿主会抛出带 provider 名与 operation 的明确错误。
        Err(_) => return INVOKE_FAILED,
    };

    let length = response.len() as i32;
    if !out_len.is_null() {
        *out_len = length;
    }
    if length > 0 && (out.is_null() || out_cap < length) {
        return INVOKE_BUFFER_TOO_SMALL;
    }
    if length > 0 {
        ptr::copy_nonoverlapping(response.as_ptr(), out, response.len());
    }
    INVOKE_OK
}

/// `void cpp_cancel(int64_t handle)` —— 线程安全、立即返回，**不释放**句柄。
#[no_mangle]
pub unsafe extern "C" fn cpp_cancel(handle: i64) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if handle != 0 {
            let session = &*(handle as *const KugouSession);
            session.cancel();
        }
    }));
}

/// `void cpp_close(int64_t handle)` —— 等 in-flight 调用排空后释放一次。
///
/// 幂等由宿主保证（关闭后把句柄置 0，不会重复传进来）；这里只做空指针保护。
#[no_mangle]
pub unsafe extern "C" fn cpp_close(handle: i64) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if handle != 0 {
            let _ = Box::from_raw(handle as *mut KugouSession);
        }
    }));
}
