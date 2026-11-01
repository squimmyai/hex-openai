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
//! the transcript.

use std::io::Read;
use std::time::{Duration, Instant};

use color_eyre::eyre::{Result, WrapErr, bail, eyre};
use serde::Deserialize;

use crate::transcription::models::{AUTO_LANGUAGE, TranscriptionSelection};
use crate::transcription::transcript::{Transcript, TranscriptSegment};

/// This build transcribes only through OpenAI: the catalog offers no other
/// model, imports never show, and nothing downloads a speech model.
pub const ONLY: bool = true;

const DEFAULT_BASE_URL: &str = match option_env!("HEX_OPENAI_DEFAULT_BASE_URL") {
    Some(url) => url,
    None => "https://api.openai.com/v1",
};
const DEFAULT_MODEL: &str = match option_env!("HEX_OPENAI_DEFAULT_MODEL") {
    Some(model) => model,
    None => "gpt-transcribe",
};
const CONFIG_FILE: &str = "openai.json";

const SAMPLE_RATE: u32 = 16_000;
/// The API takes files up to 25 MB; 10 minutes of 16-bit 16 kHz mono is
/// 19.2 MB, so longer audio goes up in 10-minute parts.
const MAX_PART_SAMPLES: usize = 10 * 60 * SAMPLE_RATE as usize;
/// Below this peak a clip is silence (a prewarm, an empty chunk), which is
/// never uploaded: the API would bill it and may invent words for it.
const SILENCE_PEAK: f32 = 1.0e-4;
const MAX_KEYWORDS: usize = 100;
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, Default, Deserialize)]
struct FileConfig {
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    api_key: Option<String>,
    pub base_url: String,
    pub model: String,
}

impl Config {
    /// Reads the configuration now, so a changed `openai.json` or
    /// environment applies to the next request without a restart.
    pub fn load() -> Self {
        let file = read_file_config();
        Self::resolve(|name| std::env::var(name).ok(), file)
    }

    fn resolve(env: impl Fn(&str) -> Option<String>, file: FileConfig) -> Self {
        let nonempty = |value: Option<String>| {
            value
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let api_key = nonempty(env("HEX_OPENAI_API_KEY"))
            .or_else(|| nonempty(env("OPENAI_API_KEY")))
            .or_else(|| nonempty(file.api_key));
        let base_url = nonempty(env("HEX_OPENAI_BASE_URL"))
            .or_else(|| nonempty(file.base_url))
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        let model = nonempty(env("HEX_OPENAI_MODEL"))
            .or_else(|| nonempty(file.model))
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        Self {
            api_key,
            base_url: base_url.trim_end_matches('/').to_owned(),
            model,
        }
    }

    pub fn endpoint(&self) -> String {
        format!("{}/audio/transcriptions", self.base_url)
    }

    fn is_default_host(&self) -> bool {
        crate::net::host(&self.base_url) == "api.openai.com"
    }

    /// `gpt-transcribe` takes `languages[]` and `keywords[]`; the older
    /// models (`whisper-1`, `gpt-4o-transcribe`, …) take one `language` and
    /// read the vocabulary as their `prompt`.
    fn takes_language_lists(&self) -> bool {
        !(self.model.starts_with("whisper") || self.model.starts_with("gpt-4o"))
    }
}

fn read_file_config() -> FileConfig {
    let Ok(path) = crate::app_paths::support_dir().map(|dir| dir.join(CONFIG_FILE)) else {
        return FileConfig::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
            tracing::warn!(path = %path.display(), %error, "ignoring an unreadable OpenAI configuration");
            FileConfig::default()
        }),
        Err(_) => FileConfig::default(),
    }
}

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
        let mut texts = Vec::new();
        for part in samples.chunks(MAX_PART_SAMPLES) {
            if is_silent(part) {
                continue;
            }
            let text = request(&config, &self.selection.language, part)?;
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

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: ErrorBody,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
}

fn request(config: &Config, language: &str, samples: &[f32]) -> Result<String> {
    if config.api_key.is_none() && config.is_default_host() {
        bail!(
            "No OpenAI API key is set. Set HEX_OPENAI_API_KEY (or OPENAI_API_KEY), or put \
             {{\"api_key\": \"sk-…\"}} in {CONFIG_FILE} in Hex's data folder, then dictate again."
        );
    }
    let url = config.endpoint();
    let boundary = format!("hex-{:016x}", random_u64());
    let body = multipart_body(
        &boundary,
        &form_fields(config, language, &crate::dictation::vocabulary::terms()),
        &wav_bytes(samples),
    );
    let agent = crate::net::agent(&url, |agent| {
        agent
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(request_timeout(samples.len())))
    });
    let mut request = agent
        .post(&url)
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("User-Agent", format!("Hex/{}", crate::VERSION));
    if let Some(key) = &config.api_key {
        request = request.header("Authorization", format!("Bearer {key}"));
    }
    let started = Instant::now();
    let mut response = request
        .send(&body[..])
        .map_err(|error| eyre!("{}", crate::net::unreachable(&url, &error)))?;
    let status = response.status().as_u16();
    let mut text = String::new();
    response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .reader()
        .read_to_string(&mut text)
        .wrap_err("OpenAI's transcription reply could not be read")?;
    tracing::info!(
        host = crate::net::host(&url),
        status,
        audio_ms = duration_ms(samples),
        elapsed_ms = started.elapsed().as_millis(),
        model = %config.model,
        "OpenAI transcription request"
    );
    if !(200..300).contains(&status) {
        let detail = serde_json::from_str::<ErrorResponse>(&text)
            .map(|error| error.error.message)
            .unwrap_or_else(|_| format!("HTTP {status}"));
        bail!("OpenAI transcription failed (HTTP {status}): {detail}");
    }
    serde_json::from_str::<TranscriptionResponse>(&text)
        .map(|reply| reply.text)
        .wrap_err("OpenAI's transcription reply had no text")
}

/// Upload time grows with the audio: 60 s plus 1 s per 10 s of audio.
fn request_timeout(samples: usize) -> Duration {
    Duration::from_secs(60 + (samples / (10 * SAMPLE_RATE as usize)) as u64)
}

fn form_fields(config: &Config, language: &str, vocabulary: &[String]) -> Vec<(String, String)> {
    let mut fields = vec![
        ("model".to_owned(), config.model.clone()),
        ("response_format".to_owned(), "json".to_owned()),
    ];
    let lists = config.takes_language_lists();
    if language != AUTO_LANGUAGE && !language.is_empty() {
        let name = if lists { "languages[]" } else { "language" };
        fields.push((name.to_owned(), language.to_owned()));
    }
    let terms = vocabulary
        .iter()
        .map(|term| term.trim())
        .filter(|term| !term.is_empty() && !term.contains(['<', '>', '\r', '\n']))
        .take(MAX_KEYWORDS);
    if lists {
        fields.extend(terms.map(|term| ("keywords[]".to_owned(), term.to_owned())));
    } else {
        let prompt = terms.collect::<Vec<_>>().join(", ");
        if !prompt.is_empty() {
            fields.push(("prompt".to_owned(), prompt));
        }
    }
    fields
}

fn multipart_body(boundary: &str, fields: &[(String, String)], wav: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(wav.len() + 1024);
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"dictation.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(wav);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// A 16-bit PCM mono WAV of 16 kHz float samples.
fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        wav.extend_from_slice(&value.to_le_bytes());
    }
    wav
}

fn random_u64() -> u64 {
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        return std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos() as u64);
    }
    u64::from_le_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(model: &str) -> Config {
        Config {
            api_key: Some("key".into()),
            base_url: "https://api.openai.com/v1".into(),
            model: model.into(),
        }
    }

    #[test]
    fn environment_overrides_the_file_and_defaults_fill_the_rest() {
        let env = |name: &str| match name {
            "OPENAI_API_KEY" => Some("from-env".to_owned()),
            "HEX_OPENAI_BASE_URL" => Some("https://proxy.example.com/openai/v1/".to_owned()),
            _ => None,
        };
        let file = FileConfig {
            api_key: Some("from-file".into()),
            base_url: Some("https://ignored.example.com".into()),
            model: Some("gpt-4o-mini-transcribe".into()),
        };
        let config = Config::resolve(env, file);
        assert_eq!(config.api_key.as_deref(), Some("from-env"));
        assert_eq!(
            config.endpoint(),
            "https://proxy.example.com/openai/v1/audio/transcriptions"
        );
        assert_eq!(config.model, "gpt-4o-mini-transcribe");

        let config = Config::resolve(|_| None, FileConfig::default());
        assert_eq!(config.api_key, None);
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.model, DEFAULT_MODEL);
    }

    #[test]
    fn gpt_transcribe_gets_language_and_keyword_lists() {
        let vocabulary = vec!["Hex".to_owned(), "bad<term".to_owned(), " Kit ".to_owned()];
        let fields = form_fields(&config("gpt-transcribe"), "en", &vocabulary);
        assert!(fields.contains(&("languages[]".into(), "en".into())));
        assert!(fields.contains(&("keywords[]".into(), "Hex".into())));
        assert!(fields.contains(&("keywords[]".into(), "Kit".into())));
        assert!(!fields.iter().any(|(_, value)| value.contains('<')));
        assert!(!fields.iter().any(|(name, _)| name == "language"));
    }

    #[test]
    fn older_models_get_one_language_and_a_prompt() {
        let fields = form_fields(&config("whisper-1"), "de", &["Hex".into(), "Kit".into()]);
        assert!(fields.contains(&("language".into(), "de".into())));
        assert!(fields.contains(&("prompt".into(), "Hex, Kit".into())));
        let fields = form_fields(&config("gpt-4o-transcribe"), AUTO_LANGUAGE, &[]);
        assert!(!fields.iter().any(|(name, _)| name.starts_with("language")));
    }

    #[test]
    fn wav_header_describes_16_bit_16_khz_mono() {
        let wav = wav_bytes(&[0.0, 1.0, -1.0]);
        assert_eq!(wav.len(), 44 + 6);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(i16::from_le_bytes([wav[46], wav[47]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([wav[48], wav[49]]), -i16::MAX);
    }

    #[test]
    fn multipart_body_carries_fields_then_the_file() {
        let body = multipart_body("b", &[("model".into(), "gpt-transcribe".into())], b"WAV");
        let body = String::from_utf8(body).unwrap();
        assert!(body.starts_with(
            "--b\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngpt-transcribe\r\n"
        ));
        assert!(body.contains("name=\"file\"; filename=\"dictation.wav\"\r\nContent-Type: audio/wav\r\n\r\nWAV\r\n--b--\r\n"));
    }

    #[test]
    fn silence_is_never_uploaded() {
        let selection = TranscriptionSelection::default();
        let transcriber = OpenAiTranscriber::load(&selection).unwrap();
        assert_eq!(transcriber.transcribe(&vec![0.0; 24_000]).unwrap(), "");
        let transcript = transcriber.transcribe_segments(&[]).unwrap();
        assert!(transcript.text.is_empty() && transcript.segments.is_empty());
    }

    #[test]
    fn a_missing_key_says_where_to_put_one() {
        let config = Config {
            api_key: None,
            ..config("gpt-transcribe")
        };
        let error = request(&config, "en", &[0.5; 1600])
            .unwrap_err()
            .to_string();
        assert!(error.contains("HEX_OPENAI_API_KEY"), "{error}");
    }
}
