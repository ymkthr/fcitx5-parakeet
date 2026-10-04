//! Generates FFI bindings for the vendored sherpa-onnx C API header and links
//! against the prebuilt `libsherpa-onnx-c-api.so`.
//!
//! Environment:
//!   SHERPA_ONNX_LIB_DIR  directory holding libsherpa-onnx-c-api.so (+ onnxruntime)
//!                        at link time. Default: /usr/lib/voice-jad
//!   SHERPA_ONNX_RPATH    directory baked into the binary for runtime lookup.
//!                        Default: same as SHERPA_ONNX_LIB_DIR (packaging sets
//!                        /usr/lib/voice-jad while linking from $srcdir).

use std::env;
use std::path::PathBuf;

fn main() {
    let header = "vendor/sherpa-onnx/c-api.h";
    println!("cargo:rerun-if-changed={header}");
    println!("cargo:rerun-if-env-changed=SHERPA_ONNX_LIB_DIR");
    println!("cargo:rerun-if-env-changed=SHERPA_ONNX_RPATH");

    let lib_dir = env::var("SHERPA_ONNX_LIB_DIR").unwrap_or_else(|_| "/usr/lib/voice-jad".into());
    let rpath = env::var("SHERPA_ONNX_RPATH").unwrap_or_else(|_| lib_dir.clone());
    println!("cargo:rustc-link-search=native={lib_dir}");
    println!("cargo:rustc-link-lib=dylib=sherpa-onnx-c-api");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{rpath}");

    let bindings = bindgen::Builder::default()
        .header(header)
        .allowlist_function("SherpaOnnx(CreateOfflineRecognizer|DestroyOfflineRecognizer|CreateOfflineStream|DestroyOfflineStream|AcceptWaveformOffline|DecodeOfflineStream|GetOfflineStreamResultAsJson|DestroyOfflineStreamResultJson|CreateVoiceActivityDetector|DestroyVoiceActivityDetector|VoiceActivityDetector(AcceptWaveform|Empty|Pop|Front|Reset|Flush)|DestroySpeechSegment|GetVersionStr)")
        .allowlist_type("SherpaOnnx(OfflineRecognizerConfig|VadModelConfig|SpeechSegment)")
        .derive_default(true)
        .layout_tests(false)
        .generate()
        .expect("bindgen failed on sherpa-onnx c-api.h");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("sherpa_bindings.rs");
    bindings.write_to_file(out).expect("cannot write bindings");
}
