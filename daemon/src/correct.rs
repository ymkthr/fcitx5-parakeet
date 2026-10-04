//! Homophone correction for Japanese transcripts.
//!
//! The CTC model hears the reading right but often picks the wrong kanji
//! (機会/機械). The transcript is cut into chunks, and each chunk's reading is
//! re-converted by the jinen-v2-small kana-kanji model with the text before
//! it as context. Every place where the conversion differs from the chunk is
//! an edit, and an edit is applied only when the general language model
//! TinySwallow-1.5B, given the same context, finds the chunk with that edit
//! more likely than the chunk as transcribed by the configured margin.
//! jinen's own likelihood does not decide: on 78 labelled edits it told
//! fixes from new errors with an AUC of 0.73, the judge with 0.97.
//!
//! Margin calibration: of those edits (28 fixes, 48 errors) the judge
//! scores 22 fixes above 2.0 nats and its highest error at 1.4 (体重を量って
//! -> 測って). On 96 synthesized short utterances, 22 long dictations and 12
//! plain-form paragraphs a 2.0 margin fixed 12 utterances and cut the char
//! errors of the long sets from 128 to 111 and 39 to 32 without worsening
//! any item; 1.5 let spelling variants through (子ども -> 子供).

use std::borrow::Cow;
use std::num::NonZeroU32;
use std::ops::Range;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
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

/// Left context the jinen-v2 model card uses; the judge sees the same.
const CONTEXT_CHARS: usize = 64;
const MAX_OUTPUT_TOKENS: usize = 60;
const BATCH_TOKENS: usize = 128;
/// IPADIC feature column holding the katakana reading of the surface form.
const READING_FIELD: usize = 7;
/// A chunk is closed at the first phrase boundary after this many chars,
/// because plain-form speech has no sentence end the chunker trusts. At 40
/// the plain-form paragraphs lost 7 of 39 char errors, at 25 six, and uncut
/// none; 40 also took 3.0 s per long dictation against 3.2 s at 25.
const CHUNK_CHARS: usize = 40;
/// Longest chunk handed to the models; longer ones (no boundary found) are
/// left as transcribed.
const MAX_CHUNK_CHARS: usize = 60;

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

/// Packages build llama.cpp for x86-64-v3 (AVX2, FMA, F16C, BMI2): without
/// those a correction takes seconds instead of a fraction of one, and on an
/// older CPU it would die with an illegal instruction.
pub fn cpu_supported() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        let ok = is_x86_feature_detected!("avx2")
            && is_x86_feature_detected!("fma")
            && is_x86_feature_detected!("f16c")
            && is_x86_feature_detected!("bmi2");
        if !ok {
            info!("Japanese correction disabled: the CPU lacks AVX2/FMA/F16C/BMI2");
        }
        ok
    }
    #[cfg(not(target_arch = "x86_64"))]
    true
}

struct Loaded {
    jinen: LlamaModel,
    judge: LlamaModel,
    segmenter: &'static Segmenter,
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

    /// Blocking: loads the models on first use.
    pub fn load(&self) -> Result<()> {
        self.with_loaded(|_| Ok(()))
    }

    /// Blocking. Never fails: any error keeps the transcript as it is.
    pub fn correct(&self, asr: &str, context: &str) -> String {
        match self.with_loaded(|loaded| self.correct_chunks(loaded, asr, context)) {
            Ok(text) => text,
            Err(e) => {
                warn!("correction skipped: {e:#}");
                asr.to_string()
            }
        }
    }

    /// Given a whole paragraph jinen returns a short, unrelated string, so
    /// each chunk is converted on its own, with the corrected text before it
    /// as context.
    fn correct_chunks(&self, loaded: &Loaded, asr: &str, context: &str) -> Result<String> {
        let mut out = String::with_capacity(asr.len());
        for chunk in chunks(loaded.segmenter, asr, CHUNK_CHARS)? {
            let fixed = if norm(chunk).chars().count() > MAX_CHUNK_CHARS {
                None
            } else {
                let before = format!("{context}{out}");
                self.fix(loaded, chunk, last_chars(&before, CONTEXT_CHARS))?
            };
            out.push_str(fixed.as_deref().unwrap_or(chunk));
        }
        Ok(out)
    }

    fn with_loaded<T>(&self, f: impl FnOnce(&Loaded) -> Result<T>) -> Result<T> {
        let mut guard = self.loaded.lock();
        if guard.is_none() {
            let started = Instant::now();
            *guard = Some(Loaded {
                jinen: load_model(&self.cfg.model)?,
                judge: load_model(&self.cfg.judge_model)?,
                segmenter: crate::punct::segmenter()?,
            });
            info!(
                "loaded correction models {} and {} in {:.1}s",
                self.cfg.model.display(),
                self.cfg.judge_model.display(),
                started.elapsed().as_secs_f32()
            );
        }
        f(guard.as_ref().expect("loaded above"))
    }

    /// `Some` with the chunk where each edit jinen proposes has, on its own,
    /// been preferred by the judge by the margin. Judging the edits one by
    /// one keeps a good fix (追及 -> 追究) from carrying a wrong one elsewhere
    /// in the same conversion (支社 -> 死者).
    fn fix(&self, loaded: &Loaded, chunk: &str, before: &str) -> Result<Option<String>> {
        let Some(conversion) = self.convert(loaded, chunk, before)? else {
            return Ok(None);
        };
        let mut edits: Vec<(Range<usize>, &str)> = Vec::new();
        let mut candidates = Vec::new();
        for (range, replacement) in diff_spans(chunk, &conversion) {
            if norm(&chunk[range.clone()]) == norm(replacement) {
                continue;
            }
            let candidate = format!(
                "{}{replacement}{}",
                &chunk[..range.start],
                &chunk[range.end..]
            );
            // A word the analyzer does not know gets a made-up reading (補証 ->
            // ホアカシ), and jinen then spells that reading out in kana.
            if kana_count(&candidate) > kana_count(chunk) {
                continue;
            }
            edits.push((range, replacement));
            candidates.push(candidate);
        }
        if edits.is_empty() {
            return Ok(None);
        }
        let gains = self.judge(&loaded.judge, before, chunk, &candidates)?;
        let margin = f64::from(self.cfg.judge_margin);
        let accepted: Vec<_> = edits
            .into_iter()
            .zip(gains)
            .filter(|(_, gain)| *gain > margin)
            .map(|(edit, _)| edit)
            .collect();
        if accepted.is_empty() {
            return Ok(None);
        }
        let mut text = chunk.to_string();
        for (range, replacement) in accepted.into_iter().rev() {
            text.replace_range(range, replacement);
        }
        Ok(Some(text))
    }

    /// jinen's greedy conversion of the chunk's reading; `None` when it is
    /// the chunk itself, or when it is cut off or empty, which the judge,
    /// preferring fewer tokens, would otherwise take for a fix.
    fn convert(&self, loaded: &Loaded, chunk: &str, before: &str) -> Result<Option<String>> {
        let model = &loaded.jinen;
        let vocab = model.vocab();
        let prompt = prompt(&reading(loaded.segmenter, chunk)?, before);
        let prompt_tokens = vocab.tokenize(prompt.as_bytes(), true, true);
        let mut ctx = self.context(model, prompt_tokens.len() + MAX_OUTPUT_TOKENS)?;
        let mut batch = LlamaBatch::new(BATCH_TOKENS, 1);
        let mut next = LlamaToken(0);
        feed(
            &mut ctx,
            &mut batch,
            &prompt_tokens,
            0,
            false,
            |_, logits| next = argmax(logits),
        )?;
        let mut generated = Vec::new();
        while !vocab.is_eog(next) {
            if generated.len() == MAX_OUTPUT_TOKENS {
                return Ok(None);
            }
            generated.push(next);
            let pos = prompt_tokens.len() + generated.len() - 1;
            feed(&mut ctx, &mut batch, &[next], pos, false, |_, logits| {
                next = argmax(logits)
            })?;
        }
        let bytes = vocab.detokenize(&generated, false, false);
        let out = String::from_utf8_lossy(&bytes).replace('\u{FFFD}', "");
        let out = out.trim();
        if out.is_empty() || norm(out) == norm(chunk) {
            return Ok(None);
        }
        Ok(Some(out.to_string()))
    }

    /// Nats by which the judge prefers each candidate to `text` after
    /// `before`. The prefix and the tokens a candidate shares with `text` are
    /// decoded once; each candidate only decodes from where it diverges.
    fn judge(
        &self,
        model: &LlamaModel,
        before: &str,
        text: &str,
        candidates: &[String],
    ) -> Result<Vec<f64>> {
        let vocab = model.vocab();
        let mut seq = vocab.tokenize(before.as_bytes(), true, false);
        if seq.is_empty() {
            seq.push(vocab.bos());
        }
        let p = seq.len();
        let text_tokens = vocab.tokenize(text.as_bytes(), false, false);
        let candidate_tokens: Vec<_> = candidates
            .iter()
            .map(|c| vocab.tokenize(c.as_bytes(), false, false))
            .collect();
        let longest = candidate_tokens
            .iter()
            .map(Vec::len)
            .chain([text_tokens.len()])
            .max()
            .unwrap_or(0);
        let mut ctx = self.context(model, p + longest)?;
        let mut batch = LlamaBatch::new(BATCH_TOKENS, 1);

        seq.extend_from_slice(&text_tokens);
        feed(&mut ctx, &mut batch, &seq[..p - 1], 0, false, |_, _| {})?;
        // head[k]: NLL of the first k tokens of `text`.
        let mut head = vec![0.0];
        for nll in tail_nll(&mut ctx, &mut batch, &seq, p - 1)? {
            head.push(head[head.len() - 1] + nll);
        }
        let base = head[text_tokens.len()];
        let mut gains = Vec::with_capacity(candidates.len());
        for tokens in &candidate_tokens {
            let shared = text_tokens
                .iter()
                .zip(tokens)
                .take_while(|(a, b)| a == b)
                .count();
            seq.truncate(p);
            seq.extend_from_slice(tokens);
            let tail: f64 = tail_nll(&mut ctx, &mut batch, &seq, p + shared - 1)?
                .iter()
                .sum();
            gains.push(base - head[shared] - tail);
        }
        Ok(gains)
    }

    fn context<'m>(&self, model: &'m LlamaModel, needed: usize) -> Result<LlamaContext<'m>> {
        let n_ctx_train = model.n_ctx_train() as usize;
        if needed > n_ctx_train {
            bail!("{needed} tokens exceed the model context of {n_ctx_train}");
        }
        let threads = self.cfg.num_threads;
        Ok(model.new_context(
            backend()?,
            LlamaContextParams::default()
                .with_n_ctx(NonZeroU32::new(needed as u32))
                .with_n_batch(BATCH_TOKENS as u32)
                .with_n_ubatch(BATCH_TOKENS as u32)
                .with_n_threads(threads)
                .with_n_threads_batch(threads),
        )?)
    }
}

/// Without mmap: llama.cpp repacks quantised weights into an AVX2-friendly
/// layout at load, and with mmap the file pages stay resident beside the
/// repacked copy. For TinySwallow Q4_K_M that is 1.6 GB RSS instead of
/// 1.1 GB. llama-cpp-2 cannot turn repacking off, and running the original
/// layout from mmap would take the same 1.1 GB but decode about 10% slower.
fn load_model(path: &Path) -> Result<LlamaModel> {
    let params = LlamaModelParams::default()
        .with_n_gpu_layers(0)
        .with_use_mmap(false);
    LlamaModel::load_from_file(backend()?, path, &params)
        .with_context(|| format!("loading {}", path.display()))
}

/// Byte ranges of `a` that differ from `b`, each with its replacement in `b`,
/// from a longest common subsequence of characters.
fn diff_spans<'b>(a: &str, b: &'b str) -> Vec<(Range<usize>, &'b str)> {
    let ac: Vec<(usize, char)> = a.char_indices().collect();
    let bc: Vec<(usize, char)> = b.char_indices().collect();
    let (n, m) = (ac.len(), bc.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if ac[i].1 == bc[j].1 {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let byte_a = |k: usize| ac.get(k).map_or(a.len(), |c| c.0);
    let byte_b = |k: usize| bc.get(k).map_or(b.len(), |c| c.0);
    let mut spans = Vec::new();
    let mut open: Option<(usize, usize)> = None;
    let (mut i, mut j) = (0, 0);
    loop {
        let done = i == n && j == m;
        if done || (i < n && j < m && ac[i].1 == bc[j].1) {
            if let Some((ai, bj)) = open.take() {
                spans.push((byte_a(ai)..byte_a(i), &b[byte_b(bj)..byte_b(j)]));
            }
            if done {
                return spans;
            }
            (i, j) = (i + 1, j + 1);
        } else {
            open.get_or_insert((i, j));
            if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
                j += 1;
            } else {
                i += 1;
            }
        }
    }
}

fn kana_count(s: &str) -> usize {
    s.chars()
        .filter(|c| matches!(c, 'ぁ'..='ゖ' | 'ァ'..='ヺ'))
        .count()
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

/// A word of the transcript as the chunker sees it.
struct Word {
    /// Byte offset just past the word.
    end: usize,
    /// Sentence-final punctuation.
    stop: bool,
    /// Ends a polite sentence (ます, ました, でしょう, ですね) unless the
    /// next word continues it.
    final_word: bool,
    /// Particle, auxiliary verb or symbol: belongs to the phrase before it.
    continues: bool,
    /// Cannot start a phrase: `continues`, or a dependent word or suffix.
    attaches: bool,
}

/// Cuts `text` into sentences, and sentences into chunks of about `target`
/// chars (see `split`).
fn chunks<'a>(segmenter: &Segmenter, text: &'a str, target: usize) -> Result<Vec<&'a str>> {
    let mut words: Vec<Word> = Vec::new();
    let (mut prev_polite, mut prev_final) = (false, false);
    for mut t in segmenter
        .segment(Cow::Borrowed(text))
        .map_err(|e| anyhow!("morphological analysis: {e}"))?
    {
        let pos = t.get_detail(0).unwrap_or_default().to_string();
        let sub = t.get_detail(1).unwrap_or_default().to_string();
        let form = t.get_detail(5).unwrap_or_default().to_string();
        let polite = matches!(t.get_detail(6), Some("ます" | "です"));
        let final_word = (polite && form == "基本形")
            || (prev_polite && pos == "助動詞" && matches!(t.surface.as_ref(), "た" | "う"))
            || (prev_final && sub == "終助詞");
        let continues = matches!(pos.as_str(), "助詞" | "助動詞" | "記号");
        words.push(Word {
            end: t.byte_end,
            stop: ["。", "！", "？", "!", "?"].contains(&t.surface.as_ref()),
            final_word,
            continues,
            attaches: continues || matches!(sub.as_str(), "非自立" | "接尾"),
        });
        (prev_polite, prev_final) = (polite, final_word);
    }
    Ok(split(text, &words, target))
}

/// Splits after sentence-final punctuation, and after a polite sentence end
/// that the next word does not continue, because the transcript often has no
/// punctuation at all. Plain-form endings are not used: IPADIC gives た and
/// ない the same form in 買った本 as at the end of a sentence. A plain-form
/// paragraph is instead cut once a chunk reaches `target` chars, after the
/// next particle or auxiliary that is followed by a word starting a new
/// phrase.
fn split<'a>(text: &'a str, words: &[Word], target: usize) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, w) in words.iter().enumerate() {
        let next = words.get(i + 1);
        let sentence_end = w.final_word && next.is_some_and(|n| !n.continues);
        let long_phrase = w.continues
            && next.is_some_and(|n| !n.attaches)
            && text[start..w.end].chars().count() >= target;
        if w.stop || sentence_end || long_phrase {
            out.push(&text[start..w.end]);
            start = w.end;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

fn hiragana_to_katakana(c: char) -> char {
    match c {
        'ぁ'..='ゖ' | 'ゝ' | 'ゞ' => char::from_u32(c as u32 + 0x60).unwrap_or(c),
        _ => c,
    }
}

fn last_chars(s: &str, n: usize) -> &str {
    let skip = s.chars().count().saturating_sub(n);
    s.char_indices().nth(skip).map_or("", |(i, _)| &s[i..])
}

fn prompt(kana: &str, context: &str) -> String {
    let context = last_chars(context, CONTEXT_CHARS);
    let lead = if context.is_empty() {
        String::new()
    } else {
        format!("{CONTEXT_MARK}{context}")
    };
    format!("{lead}{INPUT_MARK}{kana}{OUTPUT_MARK}")
        .nfkc()
        .collect()
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

/// NLL of each token of `seq` after position `from`, decoding from `from`
/// on; the KV cache must already hold `seq[..from]`.
fn tail_nll(
    ctx: &mut LlamaContext,
    batch: &mut LlamaBatch,
    seq: &[LlamaToken],
    from: usize,
) -> Result<Vec<f64>> {
    ctx.clear_kv_cache_seq(Some(0), Some(from as u32), None)?;
    let mut nll = Vec::with_capacity(seq.len() - from - 1);
    feed(
        ctx,
        batch,
        &seq[from..seq.len() - 1],
        from,
        true,
        |i, logits| nll.push(-log_prob(logits, seq[from + 1 + i])),
    )?;
    Ok(nll)
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

    fn segmenter() -> &'static Segmenter {
        crate::punct::segmenter().unwrap()
    }

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

    #[test]
    fn diff_spans_pair_each_changed_run_with_its_replacement() {
        let a = "この機械に新しい機会を導入";
        let b = "この機会に新しい機械を導入";
        let spans = diff_spans(a, b);
        let pairs: Vec<_> = spans.iter().map(|(r, s)| (&a[r.clone()], *s)).collect();
        assert_eq!(pairs, [("械", "会"), ("会", "械")]);
        let deletion = diff_spans("引き数を減らす", "引数を減らす");
        assert_eq!(deletion, [(3..6, "")]);
        assert!(diff_spans("同じ", "同じ").is_empty());
    }

    /// `parts` are surfaces tagged `w` (content word), `p` (particle or
    /// auxiliary), `s` (suffix), `.` (full stop) or `f` (polite end).
    fn words(parts: &[(&str, char)]) -> (String, Vec<Word>) {
        let mut text = String::new();
        let mut out = Vec::new();
        for &(surface, kind) in parts {
            text.push_str(surface);
            out.push(Word {
                end: text.len(),
                stop: kind == '.',
                final_word: kind == 'f',
                continues: matches!(kind, 'p' | '.' | 'f'),
                attaches: matches!(kind, 'p' | '.' | 'f' | 's'),
            });
        }
        (text, out)
    }

    #[test]
    fn split_cuts_long_chunks_after_a_phrase_that_the_next_word_does_not_continue() {
        let (text, w) = words(&[
            ("会議", 'w'),
            ("の", 'p'),
            ("資料", 'w'),
            ("を", 'p'),
            ("担当", 'w'),
            ("者", 's'),
            ("が", 'p'),
            ("確認", 'w'),
            ("し", 'w'),
            ("た", 'p'),
        ]);
        // Under the target nothing is cut; at it, the cut waits for a phrase
        // end whose next word starts a phrase, never before a suffix.
        assert_eq!(split(&text, &w, 100), [text.as_str()]);
        assert_eq!(
            split(&text, &w, 3),
            ["会議の", "資料を", "担当者が", "確認した"]
        );
        assert_eq!(split(&text, &w, 6), ["会議の資料を", "担当者が確認した"]);
    }

    #[test]
    fn split_cuts_at_sentence_ends_regardless_of_length() {
        let (text, w) = words(&[
            ("始め", 'w'),
            ("ます", 'f'),
            ("次", 'w'),
            ("。", '.'),
            ("終わり", 'w'),
        ]);
        assert_eq!(split(&text, &w, 100), ["始めます", "次。", "終わり"]);
    }

    #[test]
    fn chunks_split_at_sentence_ends_without_punctuation() {
        let text = "新しい機械を導入しましょう意外なことに彼以外は全員参加しました。試験もあるので早めに始めます";
        assert_eq!(
            chunks(&segmenter(), text, usize::MAX).unwrap(),
            [
                "新しい機械を導入しましょう",
                "意外なことに彼以外は全員参加しました。",
                "試験もあるので早めに始めます",
            ]
        );
    }

    #[test]
    fn chunks_do_not_split_inside_a_short_sentence() {
        for text in [
            "きかいを見てください",
            "関数の引き数を1つ減らして戻り値の型を変更します。",
            "昨日買った本を読みました",
        ] {
            assert_eq!(chunks(&segmenter(), text, CHUNK_CHARS).unwrap(), [text]);
        }
    }
}
