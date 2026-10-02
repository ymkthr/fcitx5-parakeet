//! Homophone correction for Japanese transcripts.
//!
//! The CTC model hears the reading right but often picks the wrong kanji
//! (機会/機械). The transcript's reading is re-converted by the jinen-v2-small
//! kana-kanji model with the text before the cursor as context, and the
//! conversion replaces the transcript only when the model scores it clearly
//! higher than the transcript itself. On 96 recorded utterances this fixed 6
//! and broke none at the default margin.

use std::borrow::Cow;
use std::num::NonZeroU32;
use std::sync::LazyLock;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::token::LlamaToken;
use log::{info, warn};
use parking_lot::Mutex;
use unicode_normalization::UnicodeNormalization;

use crate::config::CorrectionConfig;

/// Left context the jinen-v2 model card uses.
const CONTEXT_CHARS: usize = 64;
const MAX_OUTPUT_TOKENS: usize = 60;
const BATCH_TOKENS: usize = 128;
/// IPADIC feature column holding the katakana reading of the surface form.
const READING_FIELD: usize = 7;

const CONTEXT_MARK: char = '\u{EE02}';
const INPUT_MARK: char = '\u{EE00}';
const OUTPUT_MARK: char = '\u{EE01}';

/// llama.cpp refuses a second backend initialisation, while a failed model
/// load is retried on the next utterance.
static BACKEND: LazyLock<std::result::Result<LlamaBackend, String>> = LazyLock::new(|| {
    let mut backend = LlamaBackend::init().map_err(|e| e.to_string())?;
    backend.void_logs();
    Ok(backend)
});

fn backend() -> Result<&'static LlamaBackend> {
    BACKEND
        .as_ref()
        .map_err(|e| anyhow!("llama.cpp backend: {e}"))
}

struct Loaded {
    model: LlamaModel,
    segmenter: Segmenter,
}

/// Lazily loaded; the mutex also serialises corrections, which each use all
/// configured threads.
pub struct Corrector {
    cfg: CorrectionConfig,
    loaded: Mutex<Option<Loaded>>,
}

impl Corrector {
    pub fn new(cfg: CorrectionConfig) -> Self {
        Self {
            cfg,
            loaded: Mutex::new(None),
        }
    }

    /// Blocking: loads the model on first use.
    pub fn load(&self) -> Result<()> {
        self.with_loaded(|_| Ok(()))
    }

    /// Blocking. Never fails: any error keeps the transcript as it is.
    pub fn correct(&self, asr: &str, context: &str) -> String {
        match self.with_loaded(|loaded| self.convert(loaded, asr, context)) {
            Ok(Some(better)) => better,
            Ok(None) => asr.to_string(),
            Err(e) => {
                warn!("correction skipped: {e:#}");
                asr.to_string()
            }
        }
    }

    fn with_loaded<T>(&self, f: impl FnOnce(&Loaded) -> Result<T>) -> Result<T> {
        let mut guard = self.loaded.lock();
        if guard.is_none() {
            let started = Instant::now();
            let model = LlamaModel::load_from_file(
                backend()?,
                &self.cfg.model,
                &LlamaModelParams::default().with_n_gpu_layers(0),
            )
            .with_context(|| format!("loading {}", self.cfg.model.display()))?;
            let dictionary =
                load_dictionary("embedded://ipadic").map_err(|e| anyhow!("ipadic: {e}"))?;
            *guard = Some(Loaded {
                model,
                segmenter: Segmenter::new(Mode::Normal, dictionary, None),
            });
            info!(
                "loaded correction model from {} in {:.1}s",
                self.cfg.model.display(),
                started.elapsed().as_secs_f32()
            );
        }
        f(guard.as_ref().expect("loaded above"))
    }

    /// `Some` only when the model's conversion beats `asr` by the margin.
    fn convert(&self, loaded: &Loaded, asr: &str, context: &str) -> Result<Option<String>> {
        let model = &loaded.model;
        let vocab = model.vocab();
        let prompt = prompt(&reading(&loaded.segmenter, asr)?, context);
        let prompt_tokens = vocab.tokenize(prompt.as_bytes(), true, true);
        let asr_tokens = continuation(model, asr);
        let needed = prompt_tokens.len() + asr_tokens.len().max(MAX_OUTPUT_TOKENS + 1);
        let n_ctx_train = model.n_ctx_train() as usize;
        if needed > n_ctx_train {
            bail!("{needed} tokens exceed the model context of {n_ctx_train}");
        }
        // Headroom: re-tokenising the decoded conversion may not reproduce
        // the generated tokens exactly.
        let n_ctx = (needed + 64).min(n_ctx_train) as u32;
        let threads = self.cfg.num_threads;
        let mut ctx = model.new_context(
            backend()?,
            LlamaContextParams::default()
                .with_n_ctx(NonZeroU32::new(n_ctx))
                .with_n_batch(BATCH_TOKENS as u32)
                .with_n_ubatch(BATCH_TOKENS as u32)
                .with_n_threads(threads)
                .with_n_threads_batch(threads),
        )?;
        let mut batch = LlamaBatch::new(BATCH_TOKENS, 1);

        let mut first = Vec::new();
        feed(
            &mut ctx,
            &mut batch,
            &prompt_tokens,
            0,
            false,
            |_, logits| first = logits.to_vec(),
        )?;
        let mut generated = Vec::new();
        let mut next = argmax(&first);
        while !vocab.is_eog(next) && generated.len() < MAX_OUTPUT_TOKENS {
            generated.push(next);
            let pos = prompt_tokens.len() + generated.len() - 1;
            feed(&mut ctx, &mut batch, &[next], pos, false, |_, logits| {
                next = argmax(logits)
            })?;
        }
        let bytes = vocab.detokenize(&generated, false, false);
        let out = String::from_utf8_lossy(&bytes).replace('\u{FFFD}', "");
        let out = out.trim();
        if norm(out) == norm(asr) {
            return Ok(None);
        }

        let out_tokens = continuation(model, out);
        if prompt_tokens.len() + out_tokens.len() > n_ctx as usize {
            bail!("conversion does not fit the context");
        }
        let mut nll = |tokens: &[LlamaToken]| -> Result<f64> {
            ctx.clear_kv_cache_seq(Some(0), Some(prompt_tokens.len() as u32), None)?;
            let mut total = -log_prob(&first, tokens[0]);
            let fed = &tokens[..tokens.len() - 1];
            feed(
                &mut ctx,
                &mut batch,
                fed,
                prompt_tokens.len(),
                true,
                |i, logits| total -= log_prob(logits, tokens[i + 1]),
            )?;
            Ok(total)
        };
        let gain = nll(&asr_tokens)? - nll(&out_tokens)?;
        Ok((gain > f64::from(self.cfg.margin)).then(|| out.to_string()))
    }
}

/// Katakana reading of `text`; symbols and unknown words keep their surface.
fn reading(segmenter: &Segmenter, text: &str) -> Result<String> {
    let text: String = text.nfkc().collect();
    let mut kana = String::with_capacity(text.len());
    let tokens = segmenter
        .segment(Cow::Borrowed(text.as_str()))
        .map_err(|e| anyhow!("morphological analysis: {e}"))?;
    for mut token in tokens {
        match token.get_detail(READING_FIELD) {
            Some(r) if r != "*" && !r.is_empty() => kana.push_str(r),
            _ => kana.extend(token.surface.chars().map(hiragana_to_katakana)),
        }
    }
    Ok(kana)
}

fn hiragana_to_katakana(c: char) -> char {
    match c {
        'ぁ'..='ゖ' | 'ゝ' | 'ゞ' => char::from_u32(c as u32 + 0x60).unwrap_or(c),
        _ => c,
    }
}

fn prompt(kana: &str, context: &str) -> String {
    let skip = context.chars().count().saturating_sub(CONTEXT_CHARS);
    let context: String = context.chars().skip(skip).collect();
    let lead = if context.is_empty() {
        String::new()
    } else {
        format!("{CONTEXT_MARK}{context}")
    };
    format!("{lead}{INPUT_MARK}{kana}{OUTPUT_MARK}")
        .nfkc()
        .collect()
}

/// Tokens the model must produce after the prompt to emit `text` and stop.
fn continuation(model: &LlamaModel, text: &str) -> Vec<LlamaToken> {
    let vocab = model.vocab();
    let mut tokens = vocab.tokenize(text.as_bytes(), false, false);
    tokens.push(vocab.eos());
    tokens
}

/// Decodes `tokens` from position `start` and hands `on_logits` the logits
/// after each token (`all`) or only after the last one.
fn feed(
    ctx: &mut LlamaContext,
    batch: &mut LlamaBatch,
    tokens: &[LlamaToken],
    start: usize,
    all: bool,
    mut on_logits: impl FnMut(usize, &[f32]),
) -> Result<()> {
    for (n, chunk) in tokens.chunks(BATCH_TOKENS).enumerate() {
        let base = n * BATCH_TOKENS;
        let wanted = |i: usize| all || base + i + 1 == tokens.len();
        batch.clear();
        for (i, &token) in chunk.iter().enumerate() {
            batch.add(token, (start + base + i) as i32, &[0], wanted(i))?;
        }
        ctx.decode(batch)?;
        for i in (0..chunk.len()).filter(|&i| wanted(i)) {
            on_logits(base + i, ctx.get_logits_ith(i as i32));
        }
    }
    Ok(())
}

fn argmax(logits: &[f32]) -> LlamaToken {
    let id = logits
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |(i, _)| i);
    LlamaToken(id as i32)
}

fn log_prob(logits: &[f32], token: LlamaToken) -> f64 {
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let sum: f64 = logits.iter().map(|&l| f64::from(l - max).exp()).sum();
    f64::from(logits[token.0 as usize] - max) - sum.ln()
}

/// Comparison key: conversions differing only in width, spacing or
/// punctuation count as the same text.
fn norm(s: &str) -> String {
    s.nfkc()
        .filter(|c| !c.is_whitespace() && !"、。，．,.!?！？「」".contains(*c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_ignores_width_spacing_and_punctuation() {
        assert_eq!(norm("今日は、ＡＢＣ　です。"), norm("今日はABCです"));
        assert_ne!(norm("機会です"), norm("機械です"));
    }

    #[test]
    fn prompt_keeps_last_64_context_chars() {
        let context: String = "あ".repeat(10) + &"い".repeat(CONTEXT_CHARS);
        let p = prompt("テスト", &context);
        assert_eq!(
            p,
            format!(
                "\u{EE02}{}\u{EE00}テスト\u{EE01}",
                "い".repeat(CONTEXT_CHARS)
            )
        );
    }
}
