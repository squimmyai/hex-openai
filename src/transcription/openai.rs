//! Transcription through OpenAI's hosted API (`POST /v1/audio/transcriptions`).
//!
//! This build offers only this model ([`ONLY`]): every dictation, capture,
//! and meeting chunk is uploaded to OpenAI as a 16 kHz mono WAV, and nothing
//! is transcribed on the device. Requests go through `net` (the computer's
//! proxy and trusted roots), so a company proxy or TLS-inspecting firewall
//! works as it does for the browser.
//!
//! Configuration, first match wins for each value:
//!
//! - API key: `HEX_OPENAI_API_KEY`, then `OPENAI_API_KEY`, then `api_key` in
//!   `openai.json` in Hex's data folder (`%APPDATA%\hex` on Windows,
//!   `~/Library/Application Support/hex` on macOS, `~/.local/share/hex` on
//!   Linux). A company proxy that adds the key itself needs none.
//! - Base URL: `HEX_OPENAI_BASE_URL`, then `base_url` in `openai.json`, then
//!   the `HEX_OPENAI_DEFAULT_BASE_URL` the build was compiled with, then
//!   `https://api.openai.com/v1`.
//! - Model: `HEX_OPENAI_MODEL`, then `model` in `openai.json`, then the
//!   build's `HEX_OPENAI_DEFAULT_MODEL`, then `gpt-transcribe`.
//!
//! Logs name the host, status, duration, and audio length, never the key or
//! the transcript. `config` resolves these values; `client` sends one request.

use color_eyre::eyre::Result;

use crate::transcription::models::{TranscriptionModelId, TranscriptionSelection, definition};
use crate::transcription::transcript::{Transcript, TranscriptSegment};

mod client;
mod config;

pub use config::{Config, KeySource, masked, save_api_key};

/// This build transcribes only through OpenAI: the catalog offers no other
/// model, imports never show, and nothing downloads a speech model.
pub const ONLY: bool = true;

/// The languages the API takes: Whisper's ISO-639-1 codes, and Filipino,
/// which goes out as Tagalog (`tl`). Cantonese (`yue`) and Hawaiian (`haw`)
/// have no such code, so they dictate with the language detected.
#[rustfmt::skip]
pub const LANGUAGES: &[&str] = &[
    "en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar", "sv", "it",
    "id", "hi", "fi", "vi", "he", "uk", "el", "ms", "cs", "ro", "da", "hu", "ta", "no", "th", "ur",
    "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk", "te", "fa", "lv", "bn", "sr", "az", "sl", "kn",
    "et", "mk", "br", "eu", "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw", "gl", "mr", "pa", "si",
    "km", "sn", "yo", "so", "af", "oc", "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo",
    "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "ln", "ha", "ba",
    "jw", "su", "fil",
];

const SAMPLE_RATE: u32 = 16_000;
/// The API takes files up to 25 MB; 10 minutes of 16-bit 16 kHz mono is
/// 19.2 MB, so longer audio goes up in 10-minute parts.
const MAX_PART_SAMPLES: usize = 10 * 60 * SAMPLE_RATE as usize;
/// Below this peak a clip is silence (a prewarm, an empty chunk), which is
/// never uploaded: the API would bill it and may invent words for it.
const SILENCE_PEAK: f32 = 1.0e-4;

/// The OpenAI "model" the engine holds: a selection and nothing loaded.
pub struct OpenAiTranscriber {
    selection: TranscriptionSelection,
    /// What the Linux and Windows engine logs as the model's device.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    device_label: String,
}

impl OpenAiTranscriber {
    /// Nothing loads and nothing is sent: the first request checks the key.
    pub fn load(selection: &TranscriptionSelection) -> Result<Self> {
        let config = Config::load();
        Ok(Self {
            selection: selection.clone(),
            device_label: format!(
                "OpenAI {} ({})",
                config.model,
                crate::net::host(&config.base_url)
            ),
        })
    }

    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub fn selection(&self) -> &TranscriptionSelection {
        &self.selection
    }

    pub fn matches_selection(&self, selection: &TranscriptionSelection) -> bool {
        &self.selection == selection
    }

    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub fn device_label(&self) -> &str {
        &self.device_label
    }

    /// Text for a 16 kHz mono clip; silence comes back empty without a request.
    pub fn transcribe(&self, samples: &[f32]) -> Result<String> {
        if is_silent(samples) {
            return Ok(String::new());
        }
        let config = Config::load();
        let language =
            definition(TranscriptionModelId::OpenAi).runtime_language(&self.selection.language);
        let vocabulary = crate::dictation::vocabulary::terms();
        let mut texts = Vec::new();
        for part in samples.chunks(MAX_PART_SAMPLES) {
            if is_silent(part) {
                continue;
            }
            let text = client::transcribe(&config, language, &vocabulary, part)?;
            let text = text.trim();
            if !text.is_empty() {
                texts.push(text.to_owned());
            }
        }
        Ok(texts.join(" "))
    }

    /// The API gives no timings, so the text is one segment over the clip.
    pub fn transcribe_segments(&self, samples: &[f32]) -> Result<Transcript> {
        let text = self.transcribe(samples)?;
        let segments = if text.is_empty() {
            Vec::new()
        } else {
            vec![TranscriptSegment {
                start_ms: 0,
                end_ms: duration_ms(samples),
                text: text.clone(),
            }]
        };
        Ok(Transcript {
            text,
            segments,
            words: Vec::new(),
        })
    }
}

fn is_silent(samples: &[f32]) -> bool {
    samples.iter().all(|sample| sample.abs() < SILENCE_PEAK)
}

fn duration_ms(samples: &[f32]) -> i64 {
    (samples.len() as i64 * 1000) / i64::from(SAMPLE_RATE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription::models::AUTO_LANGUAGE;

    #[test]
    fn silence_is_never_uploaded() {
        let selection = TranscriptionSelection::default();
        let transcriber = OpenAiTranscriber::load(&selection).unwrap();
        assert_eq!(transcriber.transcribe(&vec![0.0; 24_000]).unwrap(), "");
        let transcript = transcriber.transcribe_segments(&[]).unwrap();
        assert!(transcript.text.is_empty() && transcript.segments.is_empty());
    }

    #[test]
    fn openai_takes_every_settings_language_but_cantonese_and_filipino_goes_as_tagalog() {
        let openai = definition(TranscriptionModelId::OpenAi);
        for (code, _) in crate::transcription::languages::LANGUAGES {
            assert_eq!(openai.supports_language(code), *code != "yue", "{code}");
        }
        assert!(openai.supports_language(AUTO_LANGUAGE));
        assert_eq!(openai.runtime_language("fil"), "tl");
        assert!(
            LANGUAGES
                .iter()
                .all(|code| code.len() == 2 || *code == "fil")
        );
    }
}
