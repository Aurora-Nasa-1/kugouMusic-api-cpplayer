//! kugou_music_rs — CPPlayer-KMP 酷狗音源模块 (Standard v3 JNI Provider)。
//!
//! 支持双通道调用：
//!  1. JNI 模式：实现 CPPlayer-KMP JniProvider 规范，由宿主直接在进程内通过 JNI 调用 nativeCallApi，
//!     零 HTTP 回环开销，数据结构符合 Standard v3 (扁平 tracks、url、lyric 模型)。
//!  2. HTTP/FFI 模式：保留原有的 tiny_http 本地服务与 C FFI，支持独立运行和调试。

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
