//! Model pool, speech gating, preview segmentation and the `auto` language decision.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use log::{debug, info};
use parking_lot::Mutex;

use crate::config::{Config, ModelConfig, AUTO_LANG};
use crate::sherpa::{ModelFiles, Recognizer, RecognizerConfig, Transcript, Vad};

/// Peak amplitude (full scale = 1.0) below which a capture counts as silence.
const SILENCE_PEAK: f32 = 0.002;
/// Context kept around the VAD speech span before decoding.
const SPEECH_PAD_SECONDS: f32 = 0.3;
/// Context kept before the first speech of a part that starts at a segment
/// cut. The VAD reports onsets late, and with only SPEECH_PAD_SECONDS the
/// model drops the part's first word. The start of a capture keeps the short
/// pad, which leaves the trigger key's click out.
const CUT_LEAD_SECONDS: f32 = 1.0;

fn pick(dir: &Path, patterns: &[&str]) -> Option<PathBuf> {
    for pattern in patterns {
        let exact = dir.join(pattern);
        if exact.is_file() {
            return Some(exact);
        }
        if let Some((prefix, suffix)) = pattern.split_once('*') {
            let mut matches: Vec<PathBuf> = std::fs::read_dir(dir)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with(prefix) && n.ends_with(suffix))
                        .unwrap_or(false)
                })
                .collect();
            matches.sort();
            if let Some(m) = matches.into_iter().next() {
                return Some(m);
            }
        }
    }
    None
}

/// Resolves the model files, preferring int8 exports when both are present.
pub fn model_files(cfg: &ModelConfig) -> Result<ModelFiles> {
    if !cfg.dir.is_dir() {
        bail!("model directory missing: {}", cfg.dir.display());
    }
    let transducer = (
        pick(&cfg.dir, &["encoder.int8.onnx", "encoder*.onnx"]),
        pick(&cfg.dir, &["decoder.int8.onnx", "decoder*.onnx"]),
        pick(&cfg.dir, &["joiner.int8.onnx", "joiner*.onnx"]),
    );
    let ctc = pick(&cfg.dir, &["model.int8.onnx", "model*.onnx"]);
    let kind = match cfg.kind.as_str() {
        "auto" => match (&transducer, &ctc) {
            ((Some(_), Some(_), Some(_)), _) => "nemo_transducer",
            (_, Some(_)) => "nemo_ctc",
            _ => bail!("no sherpa-onnx model files in {}", cfg.dir.display()),
        },
        other => other,
    };
    match kind {
        "nemo_transducer" => match transducer {
            (Some(encoder), Some(decoder), Some(joiner)) => Ok(ModelFiles::NemoTransducer {
                encoder,
                decoder,
                joiner,
            }),
            _ => bail!("encoder/decoder/joiner missing in {}", cfg.dir.display()),
        },
        "nemo_ctc" => match ctc {
            Some(model) => Ok(ModelFiles::NemoCtc { model }),
            None => bail!("model.onnx missing in {}", cfg.dir.display()),
        },
        other => bail!("unsupported model kind {other:?} for {}", cfg.lang),
    }
}

/// A loaded model; decodes are serialised per model, different models run in parallel.
pub struct Model {
    recognizer: Mutex<Recognizer>,
    sample_rate: i32,
}

impl Model {
    pub fn transcribe(&self, samples: &[f32]) -> Result<Transcript> {
        self.recognizer.lock().transcribe(samples, self.sample_rate)
    }
}

/// Lazy, thread-safe model loader.
pub struct Pool {
    configs: BTreeMap<String, ModelConfig>,
    sample_rate: i32,
    warmup: bool,
    /// One slot per language; the slot's mutex serialises loading.
    slots: BTreeMap<String, Mutex<Option<Arc<Model>>>>,
}

impl Pool {
    pub fn new(cfg: &Config) -> Self {
        Self {
            configs: cfg.models.clone(),
            sample_rate: cfg.sample_rate,
            warmup: cfg.warmup,
            slots: cfg
                .models
                .keys()
                .map(|k| (k.clone(), Mutex::new(None)))
                .collect(),
        }
    }

    pub fn loaded(&self) -> Vec<String> {
        self.slots
            .iter()
            .filter(|(_, s)| s.lock().is_some())
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// Blocking: loads the model on first use (seconds), returns it afterwards.
    pub fn get(&self, lang: &str) -> Result<Arc<Model>> {
        let (cfg, slot) = match (self.configs.get(lang), self.slots.get(lang)) {
            (Some(c), Some(s)) => (c, s),
            _ => bail!(
                "unknown language {lang:?}; configured: {}",
                self.configs.keys().cloned().collect::<Vec<_>>().join(", ")
            ),
        };
        let mut guard = slot.lock();
        if let Some(model) = guard.as_ref() {
            return Ok(Arc::clone(model));
        }
        let started = Instant::now();
        let files = model_files(cfg)?;
        let tokens = cfg.dir.join("tokens.txt");
        if !tokens.is_file() {
            bail!("tokens.txt missing in {}", cfg.dir.display());
        }
        let recognizer = Recognizer::new(&RecognizerConfig {
            files: files.clone(),
            tokens,
            num_threads: cfg.num_threads,
            sample_rate: self.sample_rate,
        })
        .with_context(|| format!("loading {lang} model from {}", cfg.dir.display()))?;
        let model = Arc::new(Model {
            recognizer: Mutex::new(recognizer),
            sample_rate: self.sample_rate,
        });
        info!(
            "loaded {lang} model ({:?}) from {} in {:.1}s",
            kind_name(&files),
            cfg.dir.display(),
            started.elapsed().as_secs_f32()
        );
        if self.warmup {
            let started = Instant::now();
            let silence = vec![0.0f32; self.sample_rate as usize];
            model.transcribe(&silence)?;
            debug!(
                "{lang} warm-up decode took {:.2}s",
                started.elapsed().as_secs_f32()
            );
        }
        *guard = Some(Arc::clone(&model));
        Ok(model)
    }
}

fn kind_name(files: &ModelFiles) -> &'static str {
    match files {
        ModelFiles::NemoTransducer { .. } => "nemo_transducer",
        ModelFiles::NemoCtc { .. } => "nemo_ctc",
    }
}

pub fn pcm16_to_f32(pcm: &[i16]) -> Vec<f32> {
    pcm.iter().map(|&s| s as f32 / 32768.0).collect()
}

/// Silero VAD gate: models invent short fillers ("うん", "yeah") for noise-only
/// audio, so captures without detected speech are never decoded, and the
/// decoded span is trimmed to the speech plus a little context.
pub struct Gate {
    vad: Mutex<Vad>,
    sample_rate: i32,
}

impl Gate {
    pub fn new(model: &Path, sample_rate: i32, max_seconds: f32) -> Result<Self> {
        Ok(Self {
            vad: Mutex::new(Vad::new(model, sample_rate, max_seconds)?),
            sample_rate,
        })
    }

    /// Samples worth decoding, or None when the capture holds no speech.
    /// `after_cut` keeps CUT_LEAD_SECONDS before the first speech.
    pub fn speech(&self, samples: Vec<f32>, after_cut: bool) -> Option<Vec<f32>> {
        let segments = self.segments(&samples);
        let mut span = self.span(samples.len(), &segments)?;
        if after_cut {
            let lead = (CUT_LEAD_SECONDS * self.sample_rate as f32) as usize;
            span.start = span.start.min(segments[0].0.saturating_sub(lead));
        }
        Some(keep(samples, span))
    }

    pub fn segments(&self, samples: &[f32]) -> Vec<(usize, usize)> {
        self.vad.lock().segments(samples)
    }

    /// Range of `len` samples covering `segments` plus a little context.
    pub fn span(&self, len: usize, segments: &[(usize, usize)]) -> Option<Range<usize>> {
        let start = segments.iter().map(|s| s.0).min()?;
        let end = segments.iter().map(|s| s.1).max()?;
        let pad = (SPEECH_PAD_SECONDS * self.sample_rate as f32) as usize;
        Some(start.saturating_sub(pad)..(end + pad).min(len))
    }
}

/// `samples[range]`, reusing the allocation.
pub fn keep(mut samples: Vec<f32>, range: Range<usize>) -> Vec<f32> {
    samples.truncate(range.end);
    samples.drain(..range.start);
    samples
}

/// Pauses as `(length, middle)`: gaps between speech segments and after the
/// last one.
fn pauses(segments: &[(usize, usize)], len: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
    let next_starts = segments.iter().skip(1).map(|s| s.0).chain([len]);
    segments
        .iter()
        .map(|s| s.1)
        .zip(next_starts)
        .filter(|(end, next)| next > end)
        .map(|(end, next)| (next - end, (end + next) / 2))
}

/// Where to cut a long preview tail of `len` samples: the middle of the last
/// pause that leaves at least `min_after` samples after it.
pub fn last_pause(segments: &[(usize, usize)], len: usize, min_after: usize) -> Option<usize> {
    pauses(segments, len)
        .map(|(_, cut)| cut)
        .filter(|cut| cut + min_after <= len)
        .last()
}

/// Where a capture is split into separately committed segments, in samples.
pub struct SegmentLimits {
    /// Uncommitted audio this long is committed up to the longest pause in
    /// its second half.
    pub commit_after: usize,
    /// Uncommitted audio this long without such a pause is cut at the
    /// quietest frame of its last `window`.
    pub max: usize,
    /// Audio a pause cut must leave after it, so a word just begun is not split.
    pub min_after: usize,
    pub window: usize,
    pub frame: usize,
}

impl SegmentLimits {
    pub fn new(cfg: &Config) -> Self {
        let samples = |seconds: f32| (seconds * cfg.sample_rate as f32) as usize;
        let max = samples(cfg.max_seconds);
        Self {
            commit_after: samples(cfg.commit_after_seconds),
            max,
            min_after: samples(1.0),
            window: samples(10.0).min(max),
            frame: samples(0.1),
        }
    }
}

/// End of the next segment of the uncommitted `samples`, or None to keep
/// listening. `segments` is the VAD's speech, None without a VAD.
pub fn segment_cut(
    samples: &[f32],
    segments: Option<&[(usize, usize)]>,
    limits: &SegmentLimits,
) -> Option<usize> {
    let len = samples.len();
    if len < limits.commit_after {
        return None;
    }
    // The longest pause rather than the last: cut at a short pause inside a
    // sentence, the model drops the clause before it.
    let pause = match segments {
        Some([]) => Some(len.saturating_sub(limits.min_after)),
        Some(segments) => pauses(segments, len)
            .filter(|&(_, cut)| cut >= len / 2 && cut + limits.min_after <= len)
            .max_by_key(|&(length, _)| length)
            .map(|(_, cut)| cut),
        None => None,
    };
    let cut = pause.or_else(|| {
        let from = len.checked_sub(limits.window).filter(|_| len >= limits.max)?;
        let quietest = samples[from..]
            .chunks_exact(limits.frame)
            .map(|frame| frame.iter().map(|s| s * s).sum::<f32>())
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(&b.1))?
            .0;
        Some(from + quietest * limits.frame + limits.frame / 2)
    });
    cut.filter(|&cut| cut > 0)
}

/// Joins preview pieces; only words written in ASCII need a space between them.
pub fn join(head: &str, tail: &str) -> String {
    let spaced = head.chars().last().is_some_and(|c| c.is_ascii())
        && tail.chars().next().is_some_and(|c| c.is_ascii());
    format!("{head}{}{tail}", if spaced { " " } else { "" })
}

/// Cheap pre-check shared by the gated and ungated paths.
pub fn is_silent(samples: &[f32]) -> bool {
    samples.iter().all(|s| s.abs() < SILENCE_PEAK)
}

pub fn sanitize(text: &str) -> String {
    text.replace(['\r', '\n'], " ").trim().to_string()
}

/// Outcome of the `auto` decision.
pub struct AutoResult {
    pub lang: String,
    pub text: String,
}

/// Detector wins when it emitted text and is confident; otherwise fallback.
pub fn decide_auto(cfg: &Config, detected: &Transcript, fallback: &Transcript) -> AutoResult {
    let auto = &cfg.auto;
    debug!(
        "{AUTO_LANG}: {}={:.3} {:?} | {} {:?}",
        auto.detector,
        detected.confidence.unwrap_or(f32::NAN),
        detected.text,
        auto.fallback,
        fallback.text
    );
    let confident = detected
        .confidence
        .map(|c| c >= auto.threshold)
        .unwrap_or(false);
    if !detected.text.is_empty() && confident {
        AutoResult {
            lang: auto.detector.clone(),
            text: detected.text.clone(),
        }
    } else {
        AutoResult {
            lang: auto.fallback.clone(),
            text: fallback.text.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    fn cfg() -> Config {
        config::load(Some(Path::new("/nonexistent/config.toml"))).unwrap()
    }

    fn t(text: &str, confidence: Option<f32>) -> Transcript {
        Transcript {
            text: text.into(),
            confidence,
        }
    }

    #[test]
    fn confident_english_wins() {
        let r = decide_auto(
            &cfg(),
            &t("Hello there.", Some(-0.02)),
            &t("ハローゼア", None),
        );
        assert_eq!((r.lang.as_str(), r.text.as_str()), ("en", "Hello there."));
    }

    #[test]
    fn low_confidence_or_empty_detector_falls_back() {
        let c = cfg();
        let r = decide_auto(
            &c,
            &t("Witchnut jökkä", Some(-0.78)),
            &t("うちの中学は", None),
        );
        assert_eq!(r.lang, "ja");
        let r = decide_auto(&c, &t("", None), &t("はい", None));
        assert_eq!((r.lang.as_str(), r.text.as_str()), ("ja", "はい"));
    }

    #[test]
    fn threshold_is_inclusive() {
        let c = cfg();
        let r = decide_auto(&c, &t("ok", Some(c.auto.threshold)), &t("おけ", None));
        assert_eq!(r.lang, "en");
    }

    #[test]
    fn silence_gate() {
        assert!(is_silent(&[0.0, 0.001, -0.0015]));
        assert!(!is_silent(&[0.0, 0.01]));
        assert_eq!(sanitize(" a\nb\r\n"), "a b");
    }

    #[test]
    fn long_tail_is_cut_at_the_last_pause_that_leaves_a_second() {
        let s = 16_000;
        // Pauses at 3-4 s, 8-9 s and 10.5-10.9 s; the last leaves under 1 s of a 11.5 s tail.
        let segments = [
            (0, 3 * s),
            (4 * s, 8 * s),
            (9 * s, 10 * s + s / 2),
            (10 * s + 9 * s / 10, 11 * s),
        ];
        assert_eq!(last_pause(&segments, 11 * s + s / 2, s), Some(17 * s / 2));
        // Speech followed by silence: the trailing gap is a pause too.
        assert_eq!(last_pause(&[(0, 9 * s)], 12 * s, s), Some(21 * s / 2));
    }

    #[test]
    fn continuous_speech_has_no_cut() {
        assert_eq!(last_pause(&[(0, 12 * 16_000)], 12 * 16_000, 16_000), None);
        assert_eq!(last_pause(&[], 12 * 16_000, 16_000), None);
    }

    fn limits() -> SegmentLimits {
        let s = 16_000;
        SegmentLimits {
            commit_after: 60 * s,
            max: 120 * s,
            min_after: s,
            window: 10 * s,
            frame: s / 10,
        }
    }

    #[test]
    fn segment_ends_at_the_longest_pause_of_its_second_half() {
        let s = 16_000;
        let speech = [(0, 20 * s), (22 * s, 45 * s), (46 * s, 55 * s), (55 * s + s / 4, 59 * s)];
        let samples = vec![0.1; 60 * s + s / 2];
        assert_eq!(segment_cut(&samples[..59 * s], Some(&speech), &limits()), None);
        // 45-46 s beats the later short pause at 55 s; 20-22 s is in the first
        // half, and the trailing silence would leave less than a second.
        assert_eq!(segment_cut(&samples, Some(&speech), &limits()), Some(45 * s + s / 2));
    }

    #[test]
    fn speech_without_a_pause_is_cut_at_the_quietest_frame_by_max() {
        let s = 16_000;
        let mut samples = vec![0.1; 120 * s];
        samples[115 * s..115 * s + s / 10].fill(0.01);
        let speech = [(0, 120 * s)];
        assert_eq!(segment_cut(&samples[..119 * s], Some(&speech), &limits()), None);
        assert_eq!(segment_cut(&samples, Some(&speech), &limits()), Some(115 * s + s / 20));
        // Without a VAD only the hard cut applies.
        assert_eq!(segment_cut(&samples[..90 * s], None, &limits()), None);
        assert_eq!(segment_cut(&samples, None, &limits()), Some(115 * s + s / 20));
    }

    #[test]
    fn audio_without_speech_is_cut_without_waiting_for_max() {
        let s = 16_000;
        assert_eq!(segment_cut(&vec![0.0; 60 * s], Some(&[]), &limits()), Some(59 * s));
    }

    #[test]
    fn pieces_join_with_a_space_only_between_ascii() {
        assert_eq!(join("今日は", "晴れです。"), "今日は晴れです。");
        assert_eq!(join("Hello there.", "How are you?"), "Hello there. How are you?");
        assert_eq!(join("", "はい"), "はい");
        assert_eq!(join("ok", ""), "ok");
    }
}
