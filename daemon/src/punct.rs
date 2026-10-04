//! Punctuation for Japanese transcripts.
//!
//! The CTC model punctuates the end of a short decode but rarely anything in
//! a long one: four TTS sentences 0.7 s apart came back without a single mark
//! in one decode, while the same audio previewed in pieces showed 。 and ?.
//! So the pauses between tokens say where a mark goes, and the word before
//! the pause says which: 。 where a sentence can end, 、 elsewhere. The end
//! of the transcript gets 。 unless it stops on a particle that continues.

use std::borrow::Cow;
use std::sync::LazyLock;

use anyhow::{anyhow, Result};
use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;

use crate::asr::sanitize;
use crate::sherpa::Transcript;

/// Gap between the starts of two tokens that counts as a pause. On TTS
/// speech, tokens inside a sentence started at most 0.56 s apart (a spoken
/// comma included) and tokens across a 0.7 s pause 1.5 s apart.
const PAUSE_SECONDS: f32 = 0.9;
const MARKS: &[char] = &['、', '。', '，', '．', ',', '.', '!', '?', '！', '？'];

static SEGMENTER: LazyLock<std::result::Result<Segmenter, String>> = LazyLock::new(|| {
    let dictionary = load_dictionary("embedded://ipadic").map_err(|e| format!("ipadic: {e}"))?;
    Ok(Segmenter::new(Mode::Normal, dictionary, None))
});

/// The IPADIC segmenter, loaded on first use and shared with the corrector.
pub fn segmenter() -> Result<&'static Segmenter> {
    SEGMENTER.as_ref().map_err(|e| anyhow!("{e}"))
}

/// How the word before a pause or the end relates to what follows.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Ending {
    /// A sentence can end here: a conclusive or imperative form, or a
    /// sentence-final particle.
    Closes,
    /// A particle that needs more after it (を, の, ので); a te-form is not
    /// one, because a dictated request often ends on it.
    Dangles,
    Other,
}

/// `transcript.text` with marks at its pauses and its end.
pub fn punctuate(transcript: &Transcript) -> Result<String> {
    let segmenter = segmenter()?;
    let (text, mut pauses) = text_and_pauses(transcript);
    let words: Vec<_> = analyse(segmenter, &text)?.into_iter().map(|w| w.0).collect();
    pauses.retain(|at| words.contains(at) && *at < text.len());
    pauses.push(text.len());
    let mut out = String::with_capacity(text.len() + 3 * pauses.len());
    let mut from = 0;
    for at in pauses {
        let piece = &text[from..at];
        out.push_str(piece);
        from = at;
        if piece.trim().is_empty() || touches_mark(&text, at) {
            continue;
        }
        // The piece alone, because the words after a pause change the
        // analysis: ください before 昨日 is taken for a continuative form.
        let last = analyse(segmenter, piece)?.last().map_or(Ending::Other, |w| w.1);
        let end = at == text.len();
        match last {
            Ending::Closes => out.push('。'),
            Ending::Dangles if end => {}
            _ => out.push(if end { '。' } else { '、' }),
        }
    }
    Ok(out)
}

/// The transcript text and the byte offsets in it where the speaker paused.
/// Without tokens that spell the text, there are no pauses.
fn text_and_pauses(transcript: &Transcript) -> (String, Vec<usize>) {
    let tokens = &transcript.tokens;
    let mut text = String::new();
    let mut pauses = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        text.push_str(&token.text.replace('▁', " "));
        if tokens.get(i + 1).is_some_and(|next| next.start - token.start >= PAUSE_SECONDS) {
            pauses.push(text.trim_end().len());
        }
    }
    let joined = sanitize(&text);
    if joined != transcript.text {
        return (transcript.text.clone(), Vec::new());
    }
    let lead = text.len() - text.trim_start().len();
    (joined, pauses.into_iter().map(|p| p.saturating_sub(lead)).collect())
}

fn touches_mark(text: &str, at: usize) -> bool {
    text[..at].trim_end().ends_with(MARKS) || text[at..].trim_start().starts_with(MARKS)
}

/// Byte offset past each word of `text` with how it ends.
fn analyse(segmenter: &Segmenter, text: &str) -> Result<Vec<(usize, Ending)>> {
    let mut out = Vec::new();
    for mut t in segmenter
        .segment(Cow::Borrowed(text))
        .map_err(|e| anyhow!("morphological analysis: {e}"))?
    {
        let pos = t.get_detail(0).unwrap_or_default().to_string();
        let sub = t.get_detail(1).unwrap_or_default().to_string();
        let form = t.get_detail(5).unwrap_or_default().to_string();
        let ending = match pos.as_str() {
            // か is tagged 副助詞／並立助詞／終助詞.
            "助詞" if sub.contains("終助詞") => Ending::Closes,
            "助詞" if sub == "接続助詞" && matches!(t.surface.as_ref(), "て" | "で") => Ending::Other,
            "助詞" | "接頭詞" => Ending::Dangles,
            "動詞" | "形容詞" | "助動詞" if form.starts_with("基本形") || form.starts_with("命令") => {
                Ending::Closes
            }
            _ => Ending::Other,
        };
        out.push((t.byte_end, ending));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sherpa::Token;

    /// `parts` are token texts, each with the seconds of silence before the next.
    fn transcript(parts: &[(&str, f32)]) -> Transcript {
        let mut start = 0.0;
        let mut tokens = Vec::new();
        for &(text, pause) in parts {
            tokens.push(Token {
                text: text.into(),
                start,
            });
            start += 0.2 + pause;
        }
        let text = sanitize(&tokens.iter().map(|t| t.text.replace('▁', " ")).collect::<String>());
        Transcript {
            text,
            confidence: None,
            tokens,
        }
    }

    #[test]
    fn the_word_before_a_pause_is_read_as_the_end_of_its_sentence() {
        let t = transcript(&[
            ("▁確認して", 0.0),
            ("ください", 1.0),
            ("昨日の", 0.0),
            ("会議は", 0.0),
            ("どうでしたか", 1.0),
            ("見てもらえますか", 0.0),
        ]);
        assert_eq!(
            punctuate(&t).unwrap(),
            "確認してください。昨日の会議はどうでしたか。見てもらえますか。"
        );
    }

    #[test]
    fn a_pause_after_a_sentence_end_gets_a_full_stop_and_after_a_clause_a_comma() {
        let t = transcript(&[
            ("▁ログを", 0.0),
            ("見る", 0.0),
            ("限り", 1.0),
            ("原因は", 0.0),
            ("タイムアウトだと", 0.0),
            ("思う", 1.0),
            ("明日", 0.0),
            ("直します", 0.0),
        ]);
        assert_eq!(punctuate(&t).unwrap(), "ログを見る限り、原因はタイムアウトだと思う。明日直します。");
    }

    #[test]
    fn short_gaps_get_no_mark() {
        let t = transcript(&[("▁テストを", 0.3), ("実行して", 0.3), ("結果を", 0.3), ("確認して", 0.0)]);
        assert_eq!(punctuate(&t).unwrap(), "テストを実行して結果を確認して。");
    }

    #[test]
    fn the_end_gets_no_full_stop_after_a_particle_that_continues() {
        for text in ["資料を", "雨なので"] {
            let t = transcript(&[(text, 0.0)]);
            assert_eq!(punctuate(&t).unwrap(), text);
        }
    }

    #[test]
    fn marks_the_model_emitted_are_not_doubled() {
        let t = transcript(&[("▁始めます。", 1.0), ("いいですか?", 0.0)]);
        assert_eq!(punctuate(&t).unwrap(), "始めます。いいですか?");
    }

    #[test]
    fn a_pause_inside_a_word_gets_no_mark() {
        let t = transcript(&[("▁共", 1.0), ("有しておきます", 0.0)]);
        assert_eq!(punctuate(&t).unwrap(), "共有しておきます。");
    }

    #[test]
    fn without_tokens_only_the_end_is_marked() {
        let t = Transcript {
            text: "明日は会議があります".into(),
            confidence: None,
            tokens: Vec::new(),
        };
        assert_eq!(punctuate(&t).unwrap(), "明日は会議があります。");
    }
}
