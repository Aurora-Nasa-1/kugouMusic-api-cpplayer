//! kugou_music_rs — CPPlayer-KMP 酷狗音源模块 (kmp-sdk-v1 原生 Provider)。
//!
//! 支持双通道调用：
//!  1. JNI / C ABI：宿主通过 `NativeSession` 的类型化 Wire 协议直接调用。
//!  2. 旧 HTTP/JSON 适配层：仅供调试与历史兼容，不是新的模块契约。

pub mod cache;
pub mod config;
pub mod crypto;
pub mod device;
pub mod helper;
pub mod mss_http;
pub mod modules;
pub mod request;
pub mod server;
pub mod simulate;
pub mod util;
pub mod standard_api;
pub mod sdk;

#[cfg(feature = "jni")]
pub mod jni;

use std::ffi::CStr;
use std::os::raw::{c_char, c_int};

// ---------------------------------------------------------------------------
// 纯 C FFI（供测试或 dart:ffi 直接调用）
// ---------------------------------------------------------------------------

/// 启动服务器。port==0 表示随机选端口（被占用则 1s 后换下一个，最多 10 次）。
/// data_dir 用于持久化 device_info.json。返回实际监听端口（0=失败）。
/// # Safety
/// `data_dir` 必须是指向 NUL 结尾 C 字符串的有效指针，调用者需保证生命周期覆盖本次调用。
#[no_mangle]
pub unsafe extern "C" fn start_server(port: c_int, data_dir: *const c_char) -> c_int {
    #[cfg(feature = "jni-android")]
    {
        let _ = android_logger::init_once(
            android_logger::Config::default().with_max_level(log::LevelFilter::Info),
        );
    }

    if data_dir.is_null() {
        return 0;
    }
    let dir = unsafe { CStr::from_ptr(data_dir) }
        .to_string_lossy()
        .into_owned();
    match server::start(port as u16, dir) {
        Some(p) => p as c_int,
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn stop_server() {
    server::stop();
}

#[no_mangle]
pub extern "C" fn is_server_running() -> c_int {
    if server::is_running() {
        1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn get_server_port() -> c_int {
    server::get_port() as c_int
}
