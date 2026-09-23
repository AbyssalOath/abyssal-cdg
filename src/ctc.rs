//! CTC forced alignment - the pure DP/tokenization math behind `align.rs`,
//! kept separate so it's testable without any ONNX Runtime/model
//! dependency (synthetic emission matrices in, known-correct alignments
//! out).
//!
//! This is the standard "CTC forced alignment" trellis algorithm (the same
//! one behind `torchaudio.functional.forced_align` and the widely-used
//! PyTorch forced-alignment tutorial), reimplemented from its underlying
//! math rather than transcribed line-by-line from any one reference, and
//! checked against hand-computable synthetic cases below.
//!
//! The idea: given a wav2vec2-CTC model's per-frame log-probabilities
//! (`emission`) and the *exact* target token sequence (no decoding, no
//! ambiguity about what was said - this app already knows the lyric
//! text), find the highest-probability way to explain that whole target
//! sequence as a monotonic walk through the frames, where each frame
//! either emits `blank` (stays at the same target position) or emits the
//! next real target token (advances one position). That walk's frame
//! boundaries are the alignment.

use std::collections::HashMap;

/// One language's character/word-delimiter vocabulary, loaded from that
/// model's own `vocab.json` (see `align.rs`) - not hardcoded, since token
/// IDs and even which special-token spelling is used (`<pad>` vs. `[PAD]`)
/// differ per model/author (see the module docs on why `<pad>`=0 can't be
/// assumed universally).
pub struct Vocab {
    char_to_id: HashMap<char, u32>,
    pub blank_id: u32,
    pub word_delim_id: u32,
    pub vocab_size: usize,
}

impl Vocab {
    /// Parses a HuggingFace-style `{"token": id, ...}` vocab JSON. Blank is
    /// whichever entry spells `<pad>`/`[pad]` (case-insensitively) - the
    /// CTC blank token, by the same convention `transformers` uses
    /// (`pad_token_id`). The word delimiter is the literal `"|"` entry,
    /// which every checked model (all 9 shipped languages) has.
    pub fn parse(json: &str) -> anyhow::Result<Self> {
        let raw: HashMap<String, u32> =
            serde_json::from_str(json).map_err(|e| anyhow::anyhow!("bad vocab JSON: {e}"))?;

        let blank_id = raw
            .iter()
            .find(|(k, _)| matches!(k.to_ascii_lowercase().as_str(), "<pad>" | "[pad]"))
            .map(|(_, &v)| v)
            .ok_or_else(|| anyhow::anyhow!("vocab has no <pad>/[PAD] (blank) entry"))?;
        let word_delim_id = *raw
            .get("|")
            .ok_or_else(|| anyhow::anyhow!("vocab has no \"|\" word-delimiter entry"))?;

        let mut char_to_id = HashMap::new();
        for (k, v) in &raw {
            // Only single-character entries are usable as alignment
            // tokens - multi-character special tokens (<s>, </s>, <unk>,
            // <pad>) don't correspond to anything in a lyric line's own
            // text and are never looked up by `tokenize`.
            let mut chars = k.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                char_to_id.insert(c, *v);
            }
        }

        Ok(Self {
            char_to_id,
            blank_id,
            word_delim_id,
            vocab_size: raw.len(),
        })
    }

    /// Vocab IDs for the characters of `word`, lowercased and Unicode
    /// (NFC) normalized first - the reference models are all trained on
    /// lowercase, precomposed text (verified directly against each
    /// model's own vocab: every letter entry is lowercase; Korean's
    /// syllable-block tokens only match precomposed NFC text, not
    /// decomposed Jamo). A character with no vocab entry (stray
    /// punctuation, an unsupported script) is dropped rather than mapped
    /// to `<unk>` - the model has near-zero real probability of emitting
    /// `<unk>` for an actual spoken syllable, so forcing one into the
    /// target sequence would only hurt the alignment, not help it.
    pub fn tokenize(&self, word: &str) -> Vec<u32> {
        use unicode_normalization::UnicodeNormalization;
        word.nfc()
            .flat_map(|c| c.to_lowercase())
            .filter_map(|c| self.char_to_id.get(&c).copied())
            .collect()
    }
}

/// `tokens[i]`'s aligned frame span, half-open `[start_frame, end_frame)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TokenSpan {
    pub start_frame: usize,
    pub end_frame: usize,
}

/// Natural log of a probability floor used in place of `ln(0)` - real
/// `-inf` propagates through the trellis's `max` comparisons just fine,
/// but two `-inf`s can't be told apart during backtracking (see
/// `backtrack`), so an effectively-impossible-but-finite floor is used
/// instead.
const NEG_INF: f32 = -1e30;

/// Builds the forward trellis: `trellis[t][j]` is the best (highest) total
/// log-probability of any path through emission frames `0..t` that has
/// consumed exactly `j` of `tokens` by then. `emission` is `[frames][vocab]`
/// **log-probabilities** (apply log-softmax before calling this, not raw
/// logits). Row/column 0 are the "before any frame/token" base case.
fn build_trellis(emission: &[Vec<f32>], tokens: &[u32], blank_id: u32) -> Vec<Vec<f32>> {
    let t_len = emission.len();
    let l_len = tokens.len();
    let mut trellis = vec![vec![NEG_INF; l_len + 1]; t_len + 1];
    trellis[0][0] = 0.0;

    for t in 1..=t_len {
        let blank_lp = emission[t - 1][blank_id as usize];
        trellis[t][0] = trellis[t - 1][0] + blank_lp;
        for j in 1..=l_len {
            let stay = trellis[t - 1][j] + blank_lp;
            let advance = trellis[t - 1][j - 1] + emission[t - 1][tokens[j - 1] as usize];
            trellis[t][j] = stay.max(advance);
        }
    }
    trellis
}

/// Backtracks the trellis from `(T, L)` (all frames consumed, all tokens
/// consumed) to `(0, 0)`, returning each token's `[start_frame, end_frame)`
/// span. At every step, "stay" wins ties: a frame that's equally
/// consistent with "still holding the current token" and "just started
/// the next one" (common when a token's own sound spans several loud
/// frames in a row) is credited to the token already established, not
/// pulled forward into starting the next one early - matching the
/// reference forced-alignment algorithm's strict `changed > stayed`
/// (never `>=`) convention.
fn backtrack(
    trellis: &[Vec<f32>],
    emission: &[Vec<f32>],
    tokens: &[u32],
    blank_id: u32,
) -> Vec<TokenSpan> {
    let t_len = emission.len();
    let l_len = tokens.len();
    let mut end_frame = vec![0usize; l_len];
    let mut start_frame = vec![0usize; l_len];

    let mut t = t_len;
    let mut j = l_len;
    // A token's end_frame is the first frame index (from the top) at which
    // we're still at state j - walking backward, that's simply the first
    // `t` we visit while `j` holds that value.
    if j > 0 {
        end_frame[j - 1] = t;
    }
    while j > 0 {
        debug_assert!(t > 0, "ran out of frames before consuming every token");
        let blank_lp = emission[t - 1][blank_id as usize];
        let token_lp = emission[t - 1][tokens[j - 1] as usize];
        let stay = trellis[t - 1][j] + blank_lp;
        let advance = trellis[t - 1][j - 1] + token_lp;

        if advance > stay + 1e-4 {
            // This frame is where token j-1 was newly emitted.
            start_frame[j - 1] = t - 1;
            t -= 1;
            j -= 1;
            if j > 0 {
                end_frame[j - 1] = t;
            }
        } else {
            t -= 1;
        }
    }

    tokens
        .iter()
        .enumerate()
        .map(|(i, _)| TokenSpan {
            start_frame: start_frame[i],
            end_frame: end_frame[i],
        })
        .collect()
}

/// Runs the full trellis + backtrack, so callers don't need to know the
/// two-step shape. `emission` must be `[frames][vocab]` log-probabilities
/// (see [`log_softmax`]).
pub fn forced_align(emission: &[Vec<f32>], tokens: &[u32], blank_id: u32) -> Vec<TokenSpan> {
    if tokens.is_empty() || emission.is_empty() {
        return Vec::new();
    }
    let trellis = build_trellis(emission, tokens, blank_id);
    backtrack(&trellis, emission, tokens, blank_id)
}

/// Log-softmax over the last axis of `[frames][vocab]` raw logits, turning
/// a model's raw output into the log-probabilities [`forced_align`] needs.
pub fn log_softmax(logits: &[Vec<f32>]) -> Vec<Vec<f32>> {
    logits
        .iter()
        .map(|frame| {
            let max = frame.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let sum: f32 = frame.iter().map(|&x| (x - max).exp()).sum();
            let log_sum = sum.ln();
            frame.iter().map(|&x| x - max - log_sum).collect()
        })
        .collect()
}

/// Inserts an explicit blank between adjacent identical tokens in `tokens`,
/// required for CTC forced alignment to tell two consecutive occurrences
/// of the same symbol apart (e.g. the double "l" in "hello": without a
/// blank between them, the trellis has no way to represent "two l's" as
/// distinct from "one l held for longer", since both look identical to the
/// DP). Returns the new token list plus, for each *original* input token,
/// its index in that new list (so callers can map alignment results back
/// to the tokens they actually asked about).
pub fn separate_repeats(tokens: &[u32], blank_id: u32) -> (Vec<u32>, Vec<usize>) {
    let mut out = Vec::with_capacity(tokens.len());
    let mut mapping = Vec::with_capacity(tokens.len());
    for (i, &tok) in tokens.iter().enumerate() {
        if i > 0 && tokens[i - 1] == tok {
            out.push(blank_id);
        }
        mapping.push(out.len());
        out.push(tok);
    }
    (out, mapping)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ln(p: f32) -> f32 {
        p.ln()
    }

    #[test]
    fn vocab_parses_standard_layout_and_finds_blank_and_delimiter() {
        let json = r#"{"<pad>":0,"<s>":1,"</s>":2,"<unk>":3,"|":4,"a":5,"b":6}"#;
        let v = Vocab::parse(json).unwrap();
        assert_eq!(v.blank_id, 0);
        assert_eq!(v.word_delim_id, 4);
        assert_eq!(v.tokenize("ab"), vec![5, 6]);
    }

    #[test]
    fn vocab_parses_end_loaded_special_tokens_like_korean() {
        let json = r#"{"[UNK]":3,"[PAD]":4,"|":2,"a":0,"b":1}"#;
        let v = Vocab::parse(json).unwrap();
        assert_eq!(v.blank_id, 4);
        assert_eq!(v.word_delim_id, 2);
    }

    #[test]
    fn tokenize_lowercases_and_drops_unknown_characters() {
        let json = r#"{"<pad>":0,"|":1,"a":2,"b":3}"#;
        let v = Vocab::parse(json).unwrap();
        // 'A' isn't in the vocab as-is - lowercased to 'a', which is.
        // '!' has no entry at all and is dropped, not mapped to <unk>.
        assert_eq!(v.tokenize("A!b"), vec![2, 3]);
    }

    #[test]
    fn separate_repeats_inserts_blank_only_between_adjacent_duplicates() {
        // tokens: a, l, l, o (matches "hallo"-shaped input)
        let (out, mapping) = separate_repeats(&[1, 2, 2, 3], 0);
        assert_eq!(out, vec![1, 2, 0, 2, 3]);
        assert_eq!(mapping, vec![0, 1, 3, 4]);
    }

    #[test]
    fn separate_repeats_is_a_no_op_with_no_adjacent_duplicates() {
        let (out, mapping) = separate_repeats(&[1, 2, 3], 0);
        assert_eq!(out, vec![1, 2, 3]);
        assert_eq!(mapping, vec![0, 1, 2]);
    }

    #[test]
    fn log_softmax_rows_sum_to_one_in_probability_space() {
        let logits = vec![vec![1.0, 2.0, 0.5], vec![-1.0, 0.0, 3.0]];
        let lp = log_softmax(&logits);
        for row in &lp {
            let sum: f32 = row.iter().map(|&x| x.exp()).sum();
            assert!((sum - 1.0).abs() < 1e-5);
        }
    }

    /// A hand-built emission matrix with an obvious, unambiguous answer:
    /// 2 tokens (blank=0, 'a'=1, 'b'=2), 6 frames, where frames 0-1 are
    /// clearly blank, frames 2-3 clearly 'a', frames 4-5 clearly 'b'.
    /// Verifies the trellis/backtrack recovers exactly that segmentation,
    /// independent of any real model or vocab.
    #[test]
    fn forced_align_recovers_an_unambiguous_synthetic_alignment() {
        let blank = ln(0.98);
        let other = ln(0.01);
        let frame = |loud: usize| -> Vec<f32> {
            (0..3)
                .map(|i| if i == loud { ln(0.98) } else { other })
                .collect()
        };
        let _ = blank;
        let emission = vec![
            frame(0), // blank
            frame(0), // blank
            frame(1), // 'a'
            frame(1), // 'a'
            frame(2), // 'b'
            frame(2), // 'b'
        ];
        let tokens = vec![1u32, 2u32]; // 'a', 'b'
        let spans = forced_align(&emission, &tokens, 0);
        assert_eq!(spans.len(), 2);
        assert_eq!(
            spans[0],
            TokenSpan {
                start_frame: 2,
                end_frame: 4
            }
        );
        assert_eq!(
            spans[1],
            TokenSpan {
                start_frame: 4,
                end_frame: 6
            }
        );
    }

    #[test]
    fn forced_align_handles_a_single_token() {
        let loud = ln(0.9);
        let quiet = ln(0.05);
        let emission = vec![
            vec![loud, quiet],
            vec![loud, quiet],
            vec![quiet, loud],
            vec![quiet, loud],
        ];
        let spans = forced_align(&emission, &[1], 0);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].start_frame, 2);
        assert_eq!(spans[0].end_frame, 4);
    }

    #[test]
    fn forced_align_on_empty_tokens_returns_empty() {
        let emission = vec![vec![0.0, 0.0]];
        assert!(forced_align(&emission, &[], 0).is_empty());
    }
}
