//! CPPlayer 音源模块（JNI）入口 —— 酷狗音乐。
//!
//! 宿主 JniProvider 规范（`core/src/jvmMain/.../JniProvider.kt`）：
//!  - `Java_cp_player_core_provider_JniProvider_startNativeServer(String host, int port)`
//!  - `Java_cp_player_core_provider_JniProvider_nativeCallApi(String method, String paramsJson)`
//!  - `Java_cp_player_core_provider_JniProvider_analyzeAudioFile(String path)`
//!
//! 注意方法签名：`startNativeServer` 是 **(host, port) 两个参数**，不是旧版只有一个 port。
//! 签名不一致时 `System.load()` 仍然成功（它是合法的 PE/ELF），但首次调用抛
//! `UnsatisfiedLinkError` —— 宿主侧表现为"模块已加载，一调用就崩"。
//!
//! ## 稳定性约定
//!
//! 这三个符号跑在**宿主进程**里。Rust panic 在这里不是"当前这次请求失败"，
//! 而是 `panic=abort` 下的整进程 abort（用户侧看就是 App 闪退）。所以：
//! 1. release profile 必须是 `panic = "unwind"`（见 Cargo.toml）；
//! 2. 每个导出函数都套 `catch_unwind`，把 panic 收敛成本次调用的 JSON 错误。

use jni::objects::{JClass, JObject, JString, JValue};
use jni::sys::{jint, jstring};
use jni::JNIEnv;
use serde_json::Value;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// JVM 侧初始化（数据目录 / 设备注册）只做一次。
static JVM_INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();

/// 取 `System.getProperty("java.io.tmpdir")` 作为模块可写目录。
///
/// 为什么不用 `std::env::temp_dir()`：Android 上环境变量 `TMPDIR` 通常不存在，
/// 会退化成 `/tmp`（不存在且不可写）。而 JVM 的 `java.io.tmpdir` 在 Android 上
/// 指向应用私有 cache 目录，是宿主进程**一定有写权限**的位置。
fn jvm_tmp_dir(env: &mut JNIEnv) -> Option<String> {
    let system = env.find_class("java/lang/System").ok()?;
    let key = env.new_string("java.io.tmpdir").ok()?;
    let value = env
        .call_static_method(
            &system,
            "getProperty",
            "(Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(&key)],
        )
        .ok()?;
    let obj: JObject = value.l().ok()?;
    if obj.is_null() {
        return None;
    }
    let text: String = env.get_string(&JString::from(obj)).ok()?.into();
    if text.trim().is_empty() {
        None
    } else {
        Some(format!("{}/kugou-music-rs", text.trim_end_matches(['/', '\\'])))
    }
}

/// 首次 JNI 调用时初始化运行时：注册可写目录，并在后台把设备身份注册好。
///
/// 放后台线程：`startNativeServer` 在宿主切音源时同步调用，不能因为上游慢
/// （或离线）把 UI 卡住。注册失败不影响搜索，只影响歌曲直链，稍后会自动重试。
fn ensure_jvm_init(env: &mut JNIEnv) {
    JVM_INIT.get_or_init(|| {
        if let Some(dir) = jvm_tmp_dir(env) {
            let _ = std::fs::create_dir_all(&dir);
            crate::device::set_data_dir(&dir);
            log::info!("kugou module data dir: {dir}");
        }
        std::thread::spawn(|| {
            crate::device::ensure_registered();
        });
    });
}

/// 启动底层本地服务
fn start_native_server_impl(env: &mut JNIEnv, _class: JClass, host: JString, port: jint) {
    #[cfg(feature = "jni-android")]
    {
        let _ = android_logger::init_once(
            android_logger::Config::default().with_max_level(log::LevelFilter::Info),
        );
    }
    ensure_jvm_init(env);

    let host_str: String = match env.get_string(&host) {
        Ok(s) => s.into(),
        Err(_) => "127.0.0.1".to_string(),
    };
    log::info!("kugou start_native_server: host={}, port={}", host_str, port);

    if port > 0 {
        // 如果宿主指定了端口，同时启动进程内 HTTP 回环服务器，便于外部/调试直连
        let _ = crate::server::start(port as u16, crate::device::data_dir().to_string_lossy().into_owned());
    }
}

/// 核心分发：执行 API 调用并以 Standard v3 JSON 格式返回
fn native_call_api_impl(env: &mut JNIEnv, _class: JClass, method: JString, params_json: JString) -> jstring {
    ensure_jvm_init(env);

    let method_str: String = match env.get_string(&method) {
        Ok(s) => s.into(),
        Err(_) => String::new(),
    };
    let params_str: String = match env.get_string(&params_json) {
        Ok(s) => s.into(),
        Err(_) => "{}".to_string(),
    };

    let params_val: Value =
        serde_json::from_str(&params_str).unwrap_or_else(|_| serde_json::json!({}));
    let result_val = crate::standard_api::handle_api(&method_str, &params_val);
    let result_str = serde_json::to_string(&result_val)
        .unwrap_or_else(|_| "{\"code\":500,\"msg\":\"JSON serialization failed\"}".to_string());

    match env.new_string(result_str) {
        Ok(jstr) => jstr.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// 音频文件分析（当前音源暂不支持）
fn analyze_audio_file_impl(env: &mut JNIEnv, _class: JClass, _path: JString) -> jstring {
    let res = "{\"error\": \"Unsupported by this provider\"}";
    match env.new_string(res) {
        Ok(jstr) => jstr.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// 把 panic 收敛成一次调用失败：返回统一错误 JSON，绝不让宿主进程 abort。
fn panic_json(e: Box<dyn std::any::Any + Send>) -> String {
    let msg = if let Some(s) = e.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = e.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    };
    log::error!("kugou native panic: {msg}");
    serde_json::json!({
        "code": 500,
        "msg": format!("provider panic: {msg}")
    })
    .to_string()
}

/// 导出 JNI 符号宏
macro_rules! export_jni {
    ($start:ident, $call:ident, $analyze:ident) => {
        #[no_mangle]
        #[allow(unsafe_code)]
        pub unsafe extern "system" fn $start(
            env: jni::JNIEnv,
            class: jni::objects::JClass,
            host: jni::objects::JString,
            port: jni::sys::jint,
        ) {
            let mut env = env;
            let _ = catch_unwind(AssertUnwindSafe(|| {
                start_native_server_impl(&mut env, class, host, port)
            }));
        }

        #[no_mangle]
        #[allow(unsafe_code)]
        pub unsafe extern "system" fn $call(
            mut env: jni::JNIEnv,
            class: jni::objects::JClass,
            method: jni::objects::JString,
            params_json: jni::objects::JString,
        ) -> jni::sys::jstring {
            match catch_unwind(AssertUnwindSafe(|| {
                native_call_api_impl(&mut env, class, method, params_json)
            })) {
                Ok(ptr) => ptr,
                Err(payload) => match env.new_string(panic_json(payload)) {
                    Ok(s) => s.into_raw(),
                    Err(_) => std::ptr::null_mut(),
                },
            }
        }

        #[no_mangle]
        #[allow(unsafe_code)]
        pub unsafe extern "system" fn $analyze(
            env: jni::JNIEnv,
            class: jni::objects::JClass,
            path: jni::objects::JString,
        ) -> jni::sys::jstring {
            let mut env = env;
            match catch_unwind(AssertUnwindSafe(|| analyze_audio_file_impl(&mut env, class, path))) {
                Ok(ptr) => ptr,
                Err(_) => std::ptr::null_mut(),
            }
        }
    };
}

// 1. 当前宿主标准包名：cp.player.core.provider.JniProvider
export_jni!(
    Java_cp_player_core_provider_JniProvider_startNativeServer,
    Java_cp_player_core_provider_JniProvider_nativeCallApi,
    Java_cp_player_core_provider_JniProvider_analyzeAudioFile
);

// 2. 历史宿主包名支持（通过 --features legacy-jni-symbols 启用）
#[cfg(feature = "legacy-jni-symbols")]
export_jni!(
    Java_cp_cpplayer_core_provider_JniProvider_startNativeServer,
    Java_cp_cpplayer_core_provider_JniProvider_nativeCallApi,
    Java_cp_cpplayer_core_provider_JniProvider_analyzeAudioFile
);

#[cfg(feature = "legacy-jni-symbols")]
export_jni!(
    Java_cp_player_kmp_provider_JniProvider_startNativeServer,
    Java_cp_player_kmp_provider_JniProvider_nativeCallApi,
    Java_cp_player_kmp_provider_JniProvider_analyzeAudioFile
);

#[cfg(feature = "legacy-jni-symbols")]
export_jni!(
    Java_cp_player_provider_JniProvider_startNativeServer,
    Java_cp_player_provider_JniProvider_nativeCallApi,
    Java_cp_player_provider_JniProvider_analyzeAudioFile
);
