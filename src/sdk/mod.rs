pub mod protocol;
pub mod wire;
pub mod wire_models;
pub mod convert;
pub mod session;
/// C ABI（`cpp_*`）：给 GraalVM Native Image 宿主按符号直调，与 JNI 并存。
/// 不需要 `jni` feature —— 这条路完全不碰 JNIEnv。
pub mod cabi;
