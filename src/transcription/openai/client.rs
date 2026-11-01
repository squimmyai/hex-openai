//! One `POST {base_url}/audio/transcriptions`: a multipart form with the
//! clip as a 16 kHz 16-bit mono WAV, through `net` (the computer's proxy and
//! trusted roots). Failures become a sentence a person can act on; logs name
//! the host, status, duration, and audio length, never the key or the text.

use std::io::Read;
use std::time::{Duration, Instant};

use color_eyre::eyre::{Result, WrapErr, bail, eyre};
use serde::Deserialize;

use super::SAMPLE_RATE;
use super::config::{Config, FILE};
use crate::transcription::models::AUTO_LANGUAGE;

const MAX_KEYWORDS: usize = 100;
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
/// The most of OpenAI's own error message a failure repeats.
const MAX_DETAIL_CHARS: usize = 240;

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

/// The text OpenAI hears in 16 kHz mono `samples` in `language` (Hex's
/// code, or `auto`), biased toward `vocabulary`.
pub(super) fn transcribe(
    config: &Config,
    language: &str,
    vocabulary: &[String],
    samples: &[f32],
) -> Result<String> {
    if config.api_key().is_none() && config.is_default_host() {
        bail!(
            "No OpenAI API key is set. Paste one in Settings › General › Advanced, or set \
             HEX_OPENAI_API_KEY, then dictate again."
        );
    }
    let url = config.endpoint();
    let boundary = format!("hex-{:016x}", random_u64());
    let body = multipart_body(
        &boundary,
        &form_fields(config, language, vocabulary),
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
    if let Some(key) = config.api_key() {
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
        audio_ms = super::duration_ms(samples),
        elapsed_ms = started.elapsed().as_millis(),
        model = %config.model,
        "OpenAI transcription request"
    );
    if !(200..300).contains(&status) {
        let detail = serde_json::from_str::<ErrorResponse>(&text)
            .ok()
            .map(|error| redacted(&error.error.message));
        bail!("{}", failure(status, detail.as_deref(), config));
    }
    serde_json::from_str::<TranscriptionResponse>(&text)
        .map(|reply| reply.text)
        .wrap_err("OpenAI's transcription reply had no text")
}

/// What to tell the person about an HTTP failure.
fn failure(status: u16, detail: Option<&str>, config: &Config) -> String {
    let detail = detail
        .filter(|detail| !detail.is_empty())
        .map(|detail| format!(" ({detail})"))
        .unwrap_or_default();
    match status {
        401 => format!(
            "OpenAI didn't accept the API key{detail}. Check the key in Settings › General › \
             Advanced or {FILE}."
        ),
        403 => format!(
            "OpenAI refused the request{detail}. The key's project may not allow audio \
             transcription."
        ),
        404 => format!(
            "OpenAI has no transcription model \"{}\" at {}{detail}. Check HEX_OPENAI_MODEL or \
             \"model\" in {FILE}.",
            config.model,
            crate::net::host(&config.base_url)
        ),
        413 => format!("OpenAI refused the recording as too large{detail}."),
        429 => format!(
            "OpenAI is limiting requests or the account is out of credit{detail}. Wait a moment \
             and dictate again."
        ),
        500..=599 => {
            format!("OpenAI had a problem (HTTP {status}){detail}. Dictate again in a moment.")
        }
        _ => format!("OpenAI transcription failed (HTTP {status}){detail}."),
    }
}

/// OpenAI's message without anything shaped like a key (its 401 repeats a
/// masked one), cut to a line.
fn redacted(message: &str) -> String {
    let words: Vec<String> = message
        .split_whitespace()
        .map(|word| {
            if word.starts_with("sk-") || word.starts_with("sess-") {
                "[key]".to_owned()
            } else {
                word.to_owned()
            }
        })
        .collect();
    let line = words.join(" ");
    if line.chars().count() > MAX_DETAIL_CHARS {
        let cut: String = line.chars().take(MAX_DETAIL_CHARS).collect();
        format!("{}…", cut.trim_end())
    } else {
        line
    }
}

/// Upload time grows with the audio: 60 s plus 1 s per 10 s of audio. The
/// connection itself gives up after `net`'s 15 s.
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
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    use super::*;
    use crate::transcription::openai::config::for_test;

    fn config(model: &str) -> Config {
        for_test(Some("key"), "https://api.openai.com/v1", model)
    }

    /// One request a mock server received: its head and body.
    struct Received {
        head: String,
        body: String,
    }

    /// Answers one request on a local port with `status` and `body`, and
    /// hands back what it received. Tests never reach OpenAI.
    fn mock_server(status: u16, body: &'static str) -> (String, mpsc::Receiver<Received>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                head.push_str(&line);
            }
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            let mut request_body = vec![0; length];
            reader.read_exact(&mut request_body).unwrap();
            let mut stream = stream;
            write!(
                stream,
                "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            let _ = sender.send(Received {
                head,
                body: String::from_utf8_lossy(&request_body).into_owned(),
            });
        });
        (base_url, receiver)
    }

    fn speech() -> Vec<f32> {
        vec![0.25; 1_600]
    }

    #[test]
    fn a_reply_comes_back_as_text_and_the_request_carries_key_model_and_audio() {
        let (base_url, received) = mock_server(200, r#"{"text": "Hello from the mock."}"#);
        let config = for_test(Some("sk-test-key"), &base_url, "gpt-transcribe");
        let text = transcribe(&config, "de", &["Hex".into()], &speech()).unwrap();
        assert_eq!(text, "Hello from the mock.");
        let request = received.recv().unwrap();
        assert!(
            request.head.starts_with("POST /v1/audio/transcriptions "),
            "{}",
            request.head
        );
        assert!(
            request
                .head
                .to_ascii_lowercase()
                .contains("authorization: bearer sk-test-key")
        );
        assert!(
            request
                .body
                .contains("name=\"model\"\r\n\r\ngpt-transcribe\r\n")
        );
        assert!(request.body.contains("name=\"languages[]\"\r\n\r\nde\r\n"));
        assert!(request.body.contains("name=\"keywords[]\"\r\n\r\nHex\r\n"));
        assert!(request.body.contains("filename=\"dictation.wav\""));
    }

    #[test]
    fn a_proxy_without_a_key_gets_no_authorization_header() {
        let (base_url, received) = mock_server(200, r#"{"text": "ok"}"#);
        let config = for_test(None, &base_url, "whisper-1");
        assert_eq!(
            transcribe(&config, AUTO_LANGUAGE, &[], &speech()).unwrap(),
            "ok"
        );
        let request = received.recv().unwrap();
        assert!(!request.head.to_ascii_lowercase().contains("authorization"));
        assert!(!request.body.contains("name=\"language"));
    }

    #[test]
    fn a_rejected_key_says_where_to_fix_it_without_repeating_the_key() {
        let (base_url, _received) = mock_server(
            401,
            r#"{"error": {"message": "Incorrect API key provided: sk-abc***wxyz. You can find your API key at https://platform.openai.com/account/api-keys."}}"#,
        );
        let config = for_test(Some("sk-abc-secret-wxyz"), &base_url, "gpt-transcribe");
        let error = transcribe(&config, "en", &[], &speech())
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("OpenAI didn't accept the API key"),
            "{error}"
        );
        assert!(error.contains("Settings › General › Advanced"), "{error}");
        assert!(!error.contains("sk-abc"), "{error}");
    }

    #[test]
    fn a_rate_limit_asks_to_wait_and_a_server_error_to_try_again() {
        let (base_url, _received) = mock_server(
            429,
            r#"{"error": {"message": "You exceeded your current quota."}}"#,
        );
        let config = for_test(Some("key"), &base_url, "gpt-transcribe");
        let error = transcribe(&config, "en", &[], &speech())
            .unwrap_err()
            .to_string();
        assert!(error.contains("out of credit"), "{error}");
        assert!(
            error.contains("You exceeded your current quota."),
            "{error}"
        );

        let (base_url, _received) = mock_server(503, "upstream unavailable");
        let config = for_test(Some("key"), &base_url, "gpt-transcribe");
        let error = transcribe(&config, "en", &[], &speech())
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "OpenAI had a problem (HTTP 503). Dictate again in a moment."
        );
    }

    #[test]
    fn an_unreachable_server_names_the_host() {
        // A port that was free a moment ago refuses the connection.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let config = for_test(
            Some("key"),
            &format!("http://127.0.0.1:{port}/v1"),
            "gpt-transcribe",
        );
        let error = transcribe(&config, "en", &[], &speech())
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("Couldn't reach 127.0.0.1"), "{error}");
    }

    #[test]
    fn a_missing_key_says_where_to_put_one() {
        let config = for_test(None, "https://api.openai.com/v1", "gpt-transcribe");
        let error = transcribe(&config, "en", &[], &speech())
            .unwrap_err()
            .to_string();
        assert!(error.contains("Settings › General › Advanced"), "{error}");
        assert!(error.contains("HEX_OPENAI_API_KEY"), "{error}");
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
        assert!(body.contains(
            "name=\"file\"; filename=\"dictation.wav\"\r\nContent-Type: audio/wav\r\n\r\nWAV\r\n--b--\r\n"
        ));
    }

    #[test]
    fn long_audio_gets_more_time() {
        assert_eq!(request_timeout(0), Duration::from_secs(60));
        assert_eq!(
            request_timeout(10 * 60 * SAMPLE_RATE as usize),
            Duration::from_secs(120)
        );
    }
}
