//! CPPlayer 音源模块（JNI）入口 —— 酷狗音乐。
//!
//! 宿主 JniProvider 规范：
//!  - `Java_cp_player_core_provider_JniProvider_startNativeServer`
//!  - `Java_cp_player_core_provider_JniProvider_nativeCallApi`
//!  - `Java_cp_player_core_provider_JniProvider_analyzeAudioFile`

use jni::objects::{JClass, JString};
use jni::sys::{jint, jstring};
use jni::JNIEnv;
use serde_json::Value;

/// 启动底层本地服务
#[allow(unsafe_code)]
unsafe fn start_native_server_impl(mut env: JNIEnv, _class: JClass, host: JString, port: jint) {
    #[cfg(feature = "jni-android")]
    {
        let _ = android_logger::init_once(
            android_logger::Config::default().with_max_level(log::LevelFilter::Info),
        );
    }
    let host_str: String = match env.get_string(&host) {
        Ok(s) => s.into(),
        Err(_) => "127.0.0.1".to_string(),
    };
    log::info!("kugou start_native_server: host={}, port={}", host_str, port);

    if port > 0 {
        // 如果宿主指定了端口，同时启动进程内 HTTP 回环服务器，便于外部/调试直连
        let _ = crate::server::start(port as u16, String::new());
    }
}

/// 核心分发：执行 API 调用并以 Standard v3 JSON 格式返回
#[allow(unsafe_code)]
unsafe fn native_call_api_impl(
    mut env: JNIEnv,
    _class: JClass,
    method: JString,
    params_json: JString,
) -> jstring {
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
#[allow(unsafe_code)]
unsafe fn analyze_audio_file_impl(env: JNIEnv, _class: JClass, _path: JString) -> jstring {
    let res = "{\"error\": \"Unsupported by this provider\"}";
    match env.new_string(res) {
        Ok(jstr) => jstr.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
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
            start_native_server_impl(env, class, host, port)
        }

        #[no_mangle]
        #[allow(unsafe_code)]
        pub unsafe extern "system" fn $call(
            env: jni::JNIEnv,
            class: jni::objects::JClass,
            method: jni::objects::JString,
            params_json: jni::objects::JString,
        ) -> jni::sys::jstring {
            native_call_api_impl(env, class, method, params_json)
        }

        #[no_mangle]
        #[allow(unsafe_code)]
        pub unsafe extern "system" fn $analyze(
            env: jni::JNIEnv,
            class: jni::objects::JClass,
            path: jni::objects::JString,
        ) -> jni::sys::jstring {
            analyze_audio_file_impl(env, class, path)
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
