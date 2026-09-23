//! Automatic word-level timing via forced alignment - runs a wav2vec2-CTC
//! speech model natively (via `ort`/ONNX Runtime) against each
//! already-timed line's own tapped `[start, end)` window, instead of
//! shelling out to `aeneas` (a separate Python package needing eSpeak/
//! eSpeak-NG and `ffmpeg` on the system).
//!
//! Design: rather than aligning the whole song's lyrics against the whole
//! file in one call (harder to keep accurate over minutes of audio, and an
//! all-or-nothing failure), each already-timed *line* gets its own call,
//! restricted to that line's own `[start, end)` window (padded a little,
//! see [`WINDOW_PAD_SECS`]). This directly builds on the line-level taps
//! the user already did: they're what defines each alignment call's
//! search window, so a mistimed *line* would need re-tapping regardless,
//! but a correctly-timed line gets much more reliable word-level results
//! than a single whole-song pass would.
//!
//! The actual alignment math (the CTC forced-alignment trellis, tokenizing
//! lyric text against a language's vocab) lives in `ctc.rs`, kept separate
//! so it's unit-testable without any ONNX/model dependency. This module is
//! the orchestration: decoding/resampling audio, running the ONNX model to
//! get per-frame emissions, and turning `ctc.rs`'s frame-level token spans
//! into absolute (whole-song) word timestamps.
//!
//! **Language coverage**: one bundled model per language below - NOT the
//! ~100 languages aeneas+eSpeak covered via TTS+DTW. Each is a
//! wav2vec2-large-xlsr-53 CTC fine-tune (Apache-2.0), ONNX-converted; the
//! conversion's fidelity to the original checkpoint was verified directly
//! (not just trusted) by comparing the converted model's own vocabulary
//! against the original PyTorch checkpoint's `vocab.json` - an exact,
//! entry-for-entry match across every one of these nine, which would be
//! essentially impossible for an unrelated or incorrectly-converted model
//! to produce by chance. Sources:
//! - English: `Xenova/wav2vec2-large-xlsr-53-english` (base:
//!   `jonatasgrosman/wav2vec2-large-xlsr-53-english`)
//! - Spanish/French/German/Italian/Portuguese/Japanese/Chinese:
//!   `FinDIT-Studio/wav2vec2-large-xlsr-53-<language>-onnx` (base: the
//!   matching `jonatasgrosman/wav2vec2-large-xlsr-53-<language>`)
//! - Korean: `FinDIT-Studio/wav2vec2-large-xlsr-53-korean-onnx` (base:
//!   `kresnik/wav2vec2-large-xlsr-korean`)
//!
//! Each model is ~1.2GB (full fp32 precision, for best accuracy) - far too
//! large to bundle all nine into every installer the way the ~65MB vocal
//! separation model is (see `vocals.rs`). Auto-align instead downloads the
//! selected language's model on first use and caches it (see
//! `model_assets.rs`), the same mechanism a dev build already uses as a
//! fallback for the separation model.

use crate::ctc::{self, Vocab};
use crate::model_assets;
use anyhow::{Context, Result};
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use rodio::{Decoder, Source};
use rubato::audioadapter::Adapter;
use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler, WindowFunction};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// How much extra audio to analyze on either side of a line's own tapped
/// `[start, end)` window - tapping is rarely frame-accurate, so a small pad
/// keeps the very first/last word from being clipped if the actual singing
/// starts slightly earlier or trails slightly later than tapped. The
/// caller is responsible for clamping this so it never eats into a
/// neighboring line's own window.
pub const WINDOW_PAD_SECS: f64 = 0.4;

/// wav2vec2's fixed input rate - every bundled model was trained on this.
const MODEL_SAMPLE_RATE: u32 = 16_000;
/// The conv feature encoder's total downsampling factor (product of its
/// `conv_stride` config, `[5,2,2,2,2,2,2]` = 320) - identical across all
/// nine bundled models' own `config.json`, checked directly rather than
/// assumed. Each output frame/logit therefore covers
/// `HOP_SAMPLES / MODEL_SAMPLE_RATE` = 20ms of audio.
const HOP_SAMPLES: usize = 320;

/// One word's aligned window, in the same absolute (whole-song) timeline as
/// everything else in the app.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WordAlignment {
    pub start: f64,
    pub end: f64,
}

/// A bundled forced-alignment language - see the module docs for how each
/// was sourced and verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignLanguage {
    Eng,
    Spa,
    Fra,
    Deu,
    Ita,
    Por,
    Jpn,
    Kor,
    Cmn,
}

impl AlignLanguage {
    pub const ALL: [AlignLanguage; 9] = [
        Self::Eng,
        Self::Spa,
        Self::Fra,
        Self::Deu,
        Self::Ita,
        Self::Por,
        Self::Jpn,
        Self::Kor,
        Self::Cmn,
    ];

    /// The ISO-639-2/3-style code this app already showed in its language
    /// picker before this phase (kept identical so existing projects/user
    /// habits don't change).
    pub const fn code(self) -> &'static str {
        match self {
            Self::Eng => "eng",
            Self::Spa => "spa",
            Self::Fra => "fra",
            Self::Deu => "deu",
            Self::Ita => "ita",
            Self::Por => "por",
            Self::Jpn => "jpn",
            Self::Kor => "kor",
            Self::Cmn => "cmn",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Eng => "English",
            Self::Spa => "Spanish",
            Self::Fra => "French",
            Self::Deu => "German",
            Self::Ita => "Italian",
            Self::Por => "Portuguese",
            Self::Jpn => "Japanese",
            Self::Kor => "Korean",
            Self::Cmn => "Mandarin",
        }
    }

    fn vocab_json(self) -> &'static str {
        match self {
            Self::Eng => include_str!("../assets/align_vocab/eng.json"),
            Self::Spa => include_str!("../assets/align_vocab/spa.json"),
            Self::Fra => include_str!("../assets/align_vocab/fra.json"),
            Self::Deu => include_str!("../assets/align_vocab/deu.json"),
            Self::Ita => include_str!("../assets/align_vocab/ita.json"),
            Self::Por => include_str!("../assets/align_vocab/por.json"),
            Self::Jpn => include_str!("../assets/align_vocab/jpn.json"),
            Self::Kor => include_str!("../assets/align_vocab/kor.json"),
            Self::Cmn => include_str!("../assets/align_vocab/cmn.json"),
        }
    }

    fn model_filename(self) -> &'static str {
        match self {
            Self::Eng => "wav2vec2-ctc-eng.onnx",
            Self::Spa => "wav2vec2-ctc-spa.onnx",
            Self::Fra => "wav2vec2-ctc-fra.onnx",
            Self::Deu => "wav2vec2-ctc-deu.onnx",
            Self::Ita => "wav2vec2-ctc-ita.onnx",
            Self::Por => "wav2vec2-ctc-por.onnx",
            Self::Jpn => "wav2vec2-ctc-jpn.onnx",
            Self::Kor => "wav2vec2-ctc-kor.onnx",
            Self::Cmn => "wav2vec2-ctc-cmn.onnx",
        }
    }

    fn model_download_url(self) -> &'static str {
        match self {
            Self::Eng => "https://huggingface.co/Xenova/wav2vec2-large-xlsr-53-english/resolve/main/onnx/model.onnx",
            Self::Spa => "https://huggingface.co/FinDIT-Studio/wav2vec2-large-xlsr-53-spanish-onnx/resolve/main/model.onnx",
            Self::Fra => "https://huggingface.co/FinDIT-Studio/wav2vec2-large-xlsr-53-french-onnx/resolve/main/model.onnx",
            Self::Deu => "https://huggingface.co/FinDIT-Studio/wav2vec2-large-xlsr-53-german-onnx/resolve/main/model.onnx",
            Self::Ita => "https://huggingface.co/FinDIT-Studio/wav2vec2-large-xlsr-53-italian-onnx/resolve/main/model.onnx",
            Self::Por => "https://huggingface.co/FinDIT-Studio/wav2vec2-large-xlsr-53-portuguese-onnx/resolve/main/model.onnx",
            Self::Jpn => "https://huggingface.co/FinDIT-Studio/wav2vec2-large-xlsr-53-japanese-onnx/resolve/main/model.onnx",
            Self::Kor => "https://huggingface.co/FinDIT-Studio/wav2vec2-large-xlsr-53-korean-onnx/resolve/main/model.onnx",
            Self::Cmn => "https://huggingface.co/FinDIT-Studio/wav2vec2-large-xlsr-53-chinese-zh-cn-onnx/resolve/main/model.onnx",
        }
    }
}

/// A loaded aligner for one audio file/language pair - decodes the audio
/// once (mono, resampled to 16kHz) and loads the model once, then reuses
/// both across as many [`Aligner::align_line`] calls as needed (an
/// auto-align run typically makes one per already-timed multi-word line).
pub struct Aligner {
    session: Session,
    vocab: Vocab,
    /// Whole file, mono, resampled to [`MODEL_SAMPLE_RATE`].
    samples: Vec<f32>,
}

impl Aligner {
    pub fn load(audio_path: &Path, language: AlignLanguage) -> Result<Self> {
        crate::onnxrt::ensure_loaded()?;
        let vocab = Vocab::parse(language.vocab_json())
            .with_context(|| format!("failed to parse {} vocab", language.label()))?;

        let model_path =
            model_assets::resolve_model(language.model_filename(), language.model_download_url())
                .with_context(|| {
                format!(
                    "couldn't locate or download the {} alignment model",
                    language.label()
                )
            })?;
        let mut builder = Session::builder()
            .map_err(|e| anyhow::anyhow!("failed to create an ONNX Runtime session builder: {e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow::anyhow!("failed to configure ONNX Runtime session: {e}"))?;
        let session = builder.commit_from_file(&model_path).map_err(|e| {
            anyhow::anyhow!("failed to load ONNX model {}: {e}", model_path.display())
        })?;

        let samples = load_mono_16k(audio_path)?;

        Ok(Self {
            session,
            vocab,
            samples,
        })
    }

    /// Aligns `words` (in order) against this aligner's audio, restricting
    /// analysis to `[window_start, window_end)` seconds - returns one
    /// [`WordAlignment`] per word, in absolute (whole-song) time.
    pub fn align_line(
        &mut self,
        words: &[&str],
        window_start: f64,
        window_end: f64,
    ) -> Result<Vec<WordAlignment>> {
        if words.is_empty() {
            return Ok(Vec::new());
        }

        let window_start = window_start.max(0.0);
        let start_sample = (window_start * MODEL_SAMPLE_RATE as f64).round() as usize;
        let end_sample = ((window_end.max(window_start) * MODEL_SAMPLE_RATE as f64).round()
            as usize)
            .min(self.samples.len());
        anyhow::ensure!(
            end_sample > start_sample,
            "alignment window is empty or past the end of the audio"
        );
        let window = &self.samples[start_sample..end_sample];
        anyhow::ensure!(
            window.len() >= HOP_SAMPLES * 4,
            "alignment window is too short to analyze"
        );
        let normalized = normalize_zscore(window);

        let logits = self.run_model(&normalized)?;
        let log_probs = ctc::log_softmax(&logits);

        // Flatten every word's tokens into one target sequence, with the
        // vocab's word-delimiter token between adjacent words (matching
        // how these models were trained: continuous speech transcripts
        // use that token for the pause between words, so it's a real,
        // expected part of what the model emits, not just a separator we
        // invented).
        let mut tokens = Vec::new();
        let mut word_ranges = Vec::with_capacity(words.len());
        for (i, word) in words.iter().enumerate() {
            if i > 0 {
                tokens.push(self.vocab.word_delim_id);
            }
            let start = tokens.len();
            tokens.extend(self.vocab.tokenize(word));
            word_ranges.push(start..tokens.len());
        }
        anyhow::ensure!(
            tokens.iter().any(|&t| t != self.vocab.word_delim_id),
            "none of this line's words contain a character this language's model recognizes"
        );

        let (dedup_tokens, mapping) = ctc::separate_repeats(&tokens, self.vocab.blank_id);
        let spans = ctc::forced_align(&log_probs, &dedup_tokens, self.vocab.blank_id);

        let frame_to_time = |frame: usize| -> f64 {
            window_start + frame as f64 * (HOP_SAMPLES as f64 / MODEL_SAMPLE_RATE as f64)
        };

        let result = word_ranges
            .into_iter()
            .map(|range| {
                if range.is_empty() {
                    // No alignable characters in this word (pure
                    // punctuation, an unsupported script, ...) - anchor to
                    // wherever its neighbor left off rather than fail the
                    // whole line over one unalignable "word".
                    let anchor_frame = if range.start > 0 {
                        spans[mapping[range.start - 1]].end_frame
                    } else {
                        spans.first().map(|s| s.start_frame).unwrap_or(0)
                    };
                    let t = frame_to_time(anchor_frame);
                    WordAlignment { start: t, end: t }
                } else {
                    let first = spans[mapping[range.start]];
                    let last = spans[mapping[range.end - 1]];
                    WordAlignment {
                        start: frame_to_time(first.start_frame),
                        end: frame_to_time(last.end_frame),
                    }
                }
            })
            .collect();
        Ok(result)
    }

    /// Runs the ONNX model on one (already 16kHz-mono-normalized) window,
    /// returning raw logits as `[frames][vocab]`.
    fn run_model(&mut self, samples: &[f32]) -> Result<Vec<Vec<f32>>> {
        let input = ndarray::Array2::from_shape_vec((1, samples.len()), samples.to_vec())
            .map_err(|e| anyhow::anyhow!("failed to build input tensor: {e}"))?;
        let input_tensor = Tensor::from_array(input)
            .map_err(|e| anyhow::anyhow!("failed to build input tensor: {e}"))?;
        let outputs = self
            .session
            .run(ort::inputs!["input_values" => input_tensor])
            .map_err(|e| anyhow::anyhow!("ONNX Runtime inference failed: {e}"))?;
        let (shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("failed to read model output tensor: {e}"))?;
        anyhow::ensure!(shape.len() == 3, "unexpected model output shape {shape:?}");
        let frames = shape[1] as usize;
        let vocab_size = shape[2] as usize;
        anyhow::ensure!(
            vocab_size == self.vocab.vocab_size,
            "model output vocab size {vocab_size} doesn't match the loaded vocab ({})",
            self.vocab.vocab_size
        );

        Ok((0..frames)
            .map(|t| data[t * vocab_size..(t + 1) * vocab_size].to_vec())
            .collect())
    }
}

/// Zero-mean, unit-variance normalization of the raw waveform - what
/// `Wav2Vec2FeatureExtractor`'s `do_normalize=True` does before handing
/// audio to the model (confirmed in every bundled model's own
/// `preprocessor_config.json`), computed over the analyzed window only
/// (matching the feature extractor's own per-input-sequence behavior, not
/// a global/whole-file statistic).
fn normalize_zscore(samples: &[f32]) -> Vec<f32> {
    let n = samples.len() as f32;
    let mean = samples.iter().sum::<f32>() / n;
    let variance = samples
        .iter()
        .map(|&x| (x - mean) * (x - mean))
        .sum::<f32>()
        / n;
    let std_dev = variance.sqrt().max(1e-7);
    samples.iter().map(|&x| (x - mean) / std_dev).collect()
}

/// Decodes `path` fully to mono `f32` samples at [`MODEL_SAMPLE_RATE`].
fn load_mono_16k(path: &Path) -> Result<Vec<f32>> {
    let file = BufReader::new(
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?,
    );
    let source =
        Decoder::new(file).with_context(|| format!("failed to decode {}", path.display()))?;
    let channels = source.channels().max(1) as usize;
    let sample_rate = source.sample_rate().max(1);

    let mut mono = Vec::new();
    let mut frame_acc = 0f32;
    let mut pos = 0usize;
    for sample in source {
        frame_acc += sample as f32 / i16::MAX as f32;
        pos += 1;
        if pos < channels {
            continue;
        }
        mono.push(frame_acc / channels as f32);
        frame_acc = 0.0;
        pos = 0;
    }

    if sample_rate == MODEL_SAMPLE_RATE {
        Ok(mono)
    } else {
        resample_mono(mono, sample_rate)
    }
}

fn resample_mono(samples: Vec<f32>, rate_in: u32) -> Result<Vec<f32>> {
    let len = samples.len();
    let channel_data = [samples];
    let input = SequentialSliceOfVecs::new(&channel_data, 1, len)
        .map_err(|e| anyhow::anyhow!("failed to wrap input audio for resampling: {e}"))?;

    let mut resampler = Fft::<f32>::new_custom(
        rate_in as usize,
        MODEL_SAMPLE_RATE as usize,
        4096,
        1,
        1,
        WindowFunction::BlackmanHarris2,
        FixedSync::Both,
    )
    .map_err(|e| anyhow::anyhow!("failed to build resampler: {e}"))?;

    let output = resampler
        .process_all(&input, len, None)
        .map_err(|e| anyhow::anyhow!("resampling failed: {e}"))?;
    let frames = output.frames();
    let interleaved = output.take_data();
    Ok(interleaved[..frames].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end smoke test against the *real* English model and a real
    /// synthesized-speech WAV with a known transcript (generated via
    /// `flite`, CMU's open-source TTS engine - actual speech, not a
    /// synthetic tone the way `vocals.rs`'s equivalent test has to settle
    /// for, since a CTC model's output is meaningless on non-speech
    /// input). Not run by default - needs the real ~1.2GB model available
    /// (bundled/cached - see `model_assets.rs`) and real CPU time. Run
    /// with:
    ///   cargo test --release align::tests::real_model_smoke_test -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_model_smoke_test() {
        // Regenerate with:
        //   flite -t "the quick brown fox jumps over the lazy dog" -o test.wav
        let wav_path = std::env::var("ABYSSAL_CDG_ALIGN_TEST_WAV").expect(
            "set ABYSSAL_CDG_ALIGN_TEST_WAV to a flite-synthesized WAV of \
                     \"the quick brown fox jumps over the lazy dog\"",
        );
        let words = [
            "the", "quick", "brown", "fox", "jumps", "over", "the", "lazy", "dog",
        ];

        let mut aligner =
            Aligner::load(Path::new(&wav_path), AlignLanguage::Eng).expect("model should load");
        let duration = aligner.samples.len() as f64 / MODEL_SAMPLE_RATE as f64;
        eprintln!("audio duration: {duration:.2}s");

        let result = aligner
            .align_line(&words, 0.0, duration)
            .expect("alignment should succeed");

        assert_eq!(result.len(), words.len());
        for (w, a) in words.iter().zip(result.iter()) {
            eprintln!("{w:>8}: {:.3}s - {:.3}s", a.start, a.end);
            assert!(a.end >= a.start, "{w} has a negative-duration span");
            assert!(
                a.start >= 0.0 && a.end <= duration + 0.1,
                "{w} span out of bounds"
            );
        }
        // Words should appear in roughly non-decreasing order (forced
        // alignment is monotonic by construction, so this should hold
        // exactly - checked as a real assertion, not just eyeballed from
        // the printed output above).
        for pair in result.windows(2) {
            assert!(
                pair[1].start >= pair[0].start,
                "words are out of order: {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
        // The two "the"s should land at clearly different times, not
        // collapse onto each other - a real sanity check that this is
        // reading actual word-by-word structure, not just spreading 9
        // words evenly across the clip.
        let first_the_end = result[0].end;
        let second_the_start = result[6].start;
        assert!(
            second_the_start > first_the_end,
            "the two \"the\"s weren't distinguished: {:?} vs {:?}",
            result[0],
            result[6]
        );
    }

    #[test]
    fn every_bundled_vocab_parses_and_has_a_blank_and_delimiter() {
        for lang in AlignLanguage::ALL {
            let vocab = Vocab::parse(lang.vocab_json())
                .unwrap_or_else(|e| panic!("{} vocab failed to parse: {e}", lang.label()));
            assert!(vocab.vocab_size > 0, "{} vocab is empty", lang.label());
            assert_ne!(
                vocab.blank_id,
                vocab.word_delim_id,
                "{} blank and delimiter must differ",
                lang.label()
            );
        }
    }

    #[test]
    fn normalize_zscore_produces_zero_mean_unit_variance() {
        let samples = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let normalized = normalize_zscore(&samples);
        let n = normalized.len() as f32;
        let mean: f32 = normalized.iter().sum::<f32>() / n;
        let var: f32 = normalized
            .iter()
            .map(|&x| (x - mean) * (x - mean))
            .sum::<f32>()
            / n;
        assert!(mean.abs() < 1e-5);
        assert!((var - 1.0).abs() < 1e-4);
    }

    #[test]
    fn normalize_zscore_does_not_divide_by_zero_on_silence() {
        let samples = vec![0.0; 100];
        let normalized = normalize_zscore(&samples);
        assert!(normalized.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn language_codes_match_the_pre_existing_picker_list() {
        let codes: Vec<&str> = AlignLanguage::ALL.iter().map(|l| l.code()).collect();
        assert_eq!(
            codes,
            vec!["eng", "spa", "fra", "deu", "ita", "por", "jpn", "kor", "cmn"]
        );
    }
}
