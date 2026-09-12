//! Thin safe wrappers over the sherpa-onnx C API (offline recognizer + Silero VAD).

use std::ffi::{CStr, CString};
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};

#[allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code
)]
mod ffi {
    include!(concat!(env!("OUT_DIR"), "/sherpa_bindings.rs"));
}

pub fn version() -> String {
    // SAFETY: the library returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(ffi::SherpaOnnxGetVersionStr()) }
        .to_string_lossy()
        .into_owned()
}

/// Keeps the CStrings referenced by a config struct alive for the FFI call.
struct CStrings(Vec<CString>);

impl CStrings {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn push(&mut self, s: impl AsRef<str>) -> *const libc::c_char {
        let c = CString::new(s.as_ref()).expect("config strings never contain NUL");
        let p = c.as_ptr();
        self.0.push(c);
        p
    }

    fn path(&mut self, p: &Path) -> *const libc::c_char {
        self.push(p.to_string_lossy())
    }
}

#[derive(Debug, Clone)]
pub enum ModelFiles {
    NemoTransducer {
        encoder: std::path::PathBuf,
        decoder: std::path::PathBuf,
        joiner: std::path::PathBuf,
    },
    NemoCtc {
        model: std::path::PathBuf,
    },
}

#[derive(Debug, Clone)]
pub struct RecognizerConfig {
    pub files: ModelFiles,
    pub tokens: std::path::PathBuf,
    pub num_threads: i32,
    pub sample_rate: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    pub text: String,
    /// Mean per-token log-probability of the greedy path; None when the model
    /// does not report it (NeMo CTC) or nothing was emitted.
    pub confidence: Option<f32>,
}

pub struct Recognizer {
    ptr: *const ffi::SherpaOnnxOfflineRecognizer,
}

// The recognizer is only ever driven from one thread at a time (callers wrap
// it in a Mutex); moving it between threads is fine.
unsafe impl Send for Recognizer {}

impl Recognizer {
    pub fn new(cfg: &RecognizerConfig) -> Result<Self> {
        let mut keep = CStrings::new();
        let mut c = ffi::SherpaOnnxOfflineRecognizerConfig::default();
        c.feat_config.sample_rate = cfg.sample_rate;
        c.feat_config.feature_dim = 80;
        match &cfg.files {
            ModelFiles::NemoTransducer {
                encoder,
                decoder,
                joiner,
            } => {
                c.model_config.transducer.encoder = keep.path(encoder);
                c.model_config.transducer.decoder = keep.path(decoder);
                c.model_config.transducer.joiner = keep.path(joiner);
                c.model_config.model_type = keep.push("nemo_transducer");
            }
            ModelFiles::NemoCtc { model } => {
                c.model_config.nemo_ctc.model = keep.path(model);
            }
        }
        c.model_config.tokens = keep.path(&cfg.tokens);
        c.model_config.num_threads = cfg.num_threads;
        c.model_config.provider = keep.push("cpu");
        c.decoding_method = keep.push("greedy_search");
        c.max_active_paths = 4;

        // SAFETY: `c` and every string it points to outlive the call; the
        // library copies what it needs.
        let ptr = unsafe { ffi::SherpaOnnxCreateOfflineRecognizer(&c) };
        if ptr.is_null() {
            bail!(
                "sherpa-onnx refused the recognizer config for {:?}",
                cfg.files
            );
        }
        Ok(Self { ptr })
    }

    pub fn transcribe(&self, samples: &[f32], sample_rate: i32) -> Result<Transcript> {
        // SAFETY: stream belongs to this recognizer and is destroyed below.
        unsafe {
            let stream = ffi::SherpaOnnxCreateOfflineStream(self.ptr);
            if stream.is_null() {
                bail!("cannot create offline stream");
            }
            ffi::SherpaOnnxAcceptWaveformOffline(
                stream,
                sample_rate,
                samples.as_ptr(),
                samples.len() as i32,
            );
            ffi::SherpaOnnxDecodeOfflineStream(self.ptr, stream);
            let json = ffi::SherpaOnnxGetOfflineStreamResultAsJson(stream);
            let parsed = if json.is_null() {
                Err(anyhow!("no result json"))
            } else {
                let text = CStr::from_ptr(json).to_string_lossy().into_owned();
                ffi::SherpaOnnxDestroyOfflineStreamResultJson(json);
                parse_result(&text)
            };
            ffi::SherpaOnnxDestroyOfflineStream(stream);
            parsed
        }
    }
}

impl Drop for Recognizer {
    fn drop(&mut self) {
        // SAFETY: pointer came from SherpaOnnxCreateOfflineRecognizer.
        unsafe { ffi::SherpaOnnxDestroyOfflineRecognizer(self.ptr) };
    }
}

#[derive(serde::Deserialize)]
struct ResultJson {
    #[serde(default)]
    text: String,
    #[serde(default)]
    ys_log_probs: Vec<f32>,
}

fn parse_result(json: &str) -> Result<Transcript> {
    let r: ResultJson = serde_json::from_str(json).context("unparsable recognizer result")?;
    let confidence = if r.ys_log_probs.is_empty() {
        None
    } else {
        Some(r.ys_log_probs.iter().sum::<f32>() / r.ys_log_probs.len() as f32)
    };
    Ok(Transcript {
        text: r.text,
        confidence,
    })
}

/// Silero VAD over a finished capture: where is the speech, if any?
pub struct Vad {
    ptr: *const ffi::SherpaOnnxVoiceActivityDetector,
    window: usize,
}

unsafe impl Send for Vad {}

impl Vad {
    pub fn new(model: &Path, sample_rate: i32, max_speech_seconds: f32) -> Result<Self> {
        if !model.is_file() {
            bail!("VAD model missing: {}", model.display());
        }
        let mut keep = CStrings::new();
        let mut c = ffi::SherpaOnnxVadModelConfig::default();
        c.silero_vad.model = keep.path(model);
        c.silero_vad.threshold = 0.5;
        c.silero_vad.min_silence_duration = 0.25;
        c.silero_vad.min_speech_duration = 0.2;
        c.silero_vad.max_speech_duration = max_speech_seconds;
        c.silero_vad.window_size = 512;
        c.sample_rate = sample_rate;
        c.num_threads = 1;
        c.provider = keep.push("cpu");
        // SAFETY: as in Recognizer::new.
        let ptr =
            unsafe { ffi::SherpaOnnxCreateVoiceActivityDetector(&c, max_speech_seconds + 30.0) };
        if ptr.is_null() {
            bail!("sherpa-onnx refused the VAD config");
        }
        Ok(Self { ptr, window: 512 })
    }

    /// Sample range `[start, end)` covering every detected speech segment.
    pub fn speech_span(&mut self, samples: &[f32]) -> Option<(usize, usize)> {
        // SAFETY: pointer valid for the lifetime of self; slices outlive the calls.
        unsafe {
            ffi::SherpaOnnxVoiceActivityDetectorReset(self.ptr);
            let full = samples.len() - samples.len() % self.window;
            for chunk in samples[..full].chunks_exact(self.window) {
                ffi::SherpaOnnxVoiceActivityDetectorAcceptWaveform(
                    self.ptr,
                    chunk.as_ptr(),
                    self.window as i32,
                );
            }
            if full < samples.len() {
                let mut tail = vec![0.0f32; self.window];
                tail[..samples.len() - full].copy_from_slice(&samples[full..]);
                ffi::SherpaOnnxVoiceActivityDetectorAcceptWaveform(
                    self.ptr,
                    tail.as_ptr(),
                    self.window as i32,
                );
            }
            ffi::SherpaOnnxVoiceActivityDetectorFlush(self.ptr);
            let mut span: Option<(usize, usize)> = None;
            while ffi::SherpaOnnxVoiceActivityDetectorEmpty(self.ptr) == 0 {
                let seg = ffi::SherpaOnnxVoiceActivityDetectorFront(self.ptr);
                if !seg.is_null() {
                    let start = (*seg).start.max(0) as usize;
                    let end = start + (*seg).n.max(0) as usize;
                    span = Some(match span {
                        None => (start, end),
                        Some((s, e)) => (s.min(start), e.max(end)),
                    });
                    ffi::SherpaOnnxDestroySpeechSegment(seg);
                }
                ffi::SherpaOnnxVoiceActivityDetectorPop(self.ptr);
            }
            span.map(|(s, e)| (s, e.min(samples.len())))
        }
    }
}

impl Drop for Vad {
    fn drop(&mut self) {
        // SAFETY: pointer came from SherpaOnnxCreateVoiceActivityDetector.
        unsafe { ffi::SherpaOnnxDestroyVoiceActivityDetector(self.ptr) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_is_mean_of_log_probs() {
        let t = parse_result(r#"{"text": " hi", "ys_log_probs": [-0.5, -1.5]}"#).unwrap();
        assert_eq!(t.text, " hi");
        assert_eq!(t.confidence, Some(-1.0));
    }

    #[test]
    fn ctc_results_have_no_confidence() {
        let t = parse_result(r#"{"text": "はい", "tokens": ["はい"]}"#).unwrap();
        assert_eq!(t.confidence, None);
    }
}
