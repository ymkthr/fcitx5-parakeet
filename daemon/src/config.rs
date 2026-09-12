//! Daemon configuration: `$XDG_CONFIG_HOME/parakeetd/config.toml` (or
//! `$PARAKEETD_CONFIG`). Every key is optional; defaults run the two Parakeet
//! models that `scripts/download-models.sh` installs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

pub const DEFAULT_SAMPLE_RATE: i32 = 16000;
pub const AUTO_LANG: &str = "auto";

fn xdg_dir(env: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(env) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn default_socket_path() -> PathBuf {
    xdg_dir("XDG_RUNTIME_DIR", ".cache").join("parakeetd.sock")
}

pub fn default_models_dir() -> PathBuf {
    xdg_dir("XDG_DATA_HOME", ".local/share")
        .join("parakeetd")
        .join("models")
}

pub fn config_path() -> PathBuf {
    match std::env::var_os("PARAKEETD_CONFIG") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => xdg_dir("XDG_CONFIG_HOME", ".config")
            .join("parakeetd")
            .join("config.toml"),
    }
}

fn expand_home(p: &Path) -> PathBuf {
    match p.strip_prefix("~") {
        Ok(rest) => home().join(rest),
        Err(_) => p.to_path_buf(),
    }
}

#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub lang: String,
    pub dir: PathBuf,
    /// "auto" picks nemo_transducer when encoder/decoder/joiner files exist,
    /// nemo_ctc when a single model file exists.
    pub kind: String,
    pub num_threads: i32,
}

/// Language choice for the `auto` pseudo-language. Both models decode the
/// capture; the detector (a NeMo transducer, so it reports per-token
/// log-probabilities) wins when it is confident about its own transcript,
/// otherwise the fallback model's text is used. The English v3 model scores
/// about -0.02 on English speech and about -0.7 on Japanese.
#[derive(Debug, Clone)]
pub struct AutoConfig {
    pub detector: String,
    pub fallback: String,
    pub threshold: f32,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub socket_path: PathBuf,
    pub models: BTreeMap<String, ModelConfig>,
    pub preload: Vec<String>,
    /// PipeWire node name/serial for the capture stream target. None = default source.
    pub target: Option<String>,
    pub max_seconds: f32,
    pub sample_rate: i32,
    /// Silero VAD used to drop captures without speech; None disables gating.
    pub vad_model: Option<PathBuf>,
    /// Decode one second of silence after loading so the first real utterance
    /// does not pay onnxruntime's first-run cost.
    pub warmup: bool,
    pub auto: AutoConfig,
}

impl Config {
    pub fn auto_enabled(&self) -> bool {
        self.models.contains_key(&self.auto.detector)
            && self.models.contains_key(&self.auto.fallback)
    }

    /// Languages accepted by START/LOAD.
    pub fn languages(&self) -> Vec<String> {
        let mut v: Vec<String> = self.models.keys().cloned().collect();
        if self.auto_enabled() {
            v.push(AUTO_LANG.to_string());
        }
        v
    }

    /// Model languages a request for `lang` needs; empty if unknown.
    pub fn models_for(&self, lang: &str) -> Vec<String> {
        if lang == AUTO_LANG {
            if self.auto_enabled() {
                return vec![self.auto.fallback.clone(), self.auto.detector.clone()];
            }
            return vec![];
        }
        if self.models.contains_key(lang) {
            vec![lang.to_string()]
        } else {
            vec![]
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawModel {
    dir: Option<PathBuf>,
    kind: Option<String>,
    num_threads: Option<i32>,
}

#[derive(Deserialize, Default)]
struct RawAuto {
    detector: Option<String>,
    fallback: Option<String>,
    threshold: Option<f32>,
}

#[derive(Deserialize, Default)]
struct Raw {
    socket_path: Option<PathBuf>,
    #[serde(default)]
    models: BTreeMap<String, RawModel>,
    preload: Option<Vec<String>>,
    target: Option<String>,
    max_seconds: Option<f32>,
    sample_rate: Option<i32>,
    /// A path, or "" / false to disable.
    vad_model: Option<toml::Value>,
    warmup: Option<bool>,
    #[serde(default)]
    auto: RawAuto,
}

fn default_models() -> BTreeMap<String, ModelConfig> {
    let base = default_models_dir();
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 4) as i32;
    let mk = |lang: &str, dir: &str| ModelConfig {
        lang: lang.to_string(),
        dir: base.join(dir),
        kind: "auto".into(),
        num_threads: threads,
    };
    BTreeMap::from([
        (
            "ja".to_string(),
            mk("ja", "sherpa-onnx-nemo-parakeet-tdt_ctc-0.6b-ja-35000-int8"),
        ),
        (
            "en".to_string(),
            mk("en", "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8"),
        ),
    ])
}

pub fn load(path: Option<&Path>) -> Result<Config> {
    let path = path.map(Path::to_path_buf).unwrap_or_else(config_path);
    let raw: Raw = if path.is_file() {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?
    } else {
        Raw::default()
    };

    let mut models = default_models();
    for (lang, spec) in raw.models {
        let base = models.get(&lang).cloned().unwrap_or_else(|| ModelConfig {
            lang: lang.clone(),
            dir: default_models_dir().join(&lang),
            kind: "auto".into(),
            num_threads: 4,
        });
        models.insert(
            lang.clone(),
            ModelConfig {
                lang: lang.clone(),
                dir: spec.dir.map(|d| expand_home(&d)).unwrap_or(base.dir),
                kind: spec.kind.unwrap_or(base.kind),
                num_threads: spec.num_threads.unwrap_or(base.num_threads),
            },
        );
    }

    let vad_model = match raw.vad_model {
        None => Some(default_models_dir().join("silero_vad.onnx")),
        Some(toml::Value::String(s)) if s.is_empty() => None,
        Some(toml::Value::String(s)) => Some(expand_home(Path::new(&s))),
        Some(toml::Value::Boolean(false)) => None,
        Some(other) => anyhow::bail!("vad_model must be a path string or \"\", got {other}"),
    };

    let max_seconds = raw.max_seconds.unwrap_or(120.0);
    if !max_seconds.is_finite() || max_seconds <= 0.0 {
        anyhow::bail!("max_seconds must be greater than zero");
    }
    let sample_rate = raw.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE);
    if sample_rate <= 0 {
        anyhow::bail!("sample_rate must be greater than zero");
    }

    Ok(Config {
        socket_path: raw
            .socket_path
            .map(|p| expand_home(&p))
            .unwrap_or_else(default_socket_path),
        preload: raw
            .preload
            .unwrap_or_else(|| models.keys().cloned().collect()),
        models,
        target: raw.target,
        max_seconds,
        sample_rate,
        vad_model,
        warmup: raw.warmup.unwrap_or(true),
        auto: AutoConfig {
            detector: raw.auto.detector.unwrap_or_else(|| "en".into()),
            fallback: raw.auto.fallback.unwrap_or_else(|| "ja".into()),
            threshold: raw.auto.threshold.unwrap_or(-0.35),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_overrides_parse() {
        let raw: Raw = toml::from_str(
            r#"
            target = "parakeet_src"
            max_seconds = 30
            vad_model = ""
            [models.ja]
            dir = "~/m/ja"
            num_threads = 2
            [auto]
            threshold = -0.5
            "#,
        )
        .unwrap();
        assert_eq!(raw.target.as_deref(), Some("parakeet_src"));
        assert_eq!(raw.models["ja"].num_threads, Some(2));
        assert_eq!(raw.auto.threshold, Some(-0.5));
        assert!(matches!(&raw.vad_model, Some(toml::Value::String(s)) if s.is_empty()));
    }
}
