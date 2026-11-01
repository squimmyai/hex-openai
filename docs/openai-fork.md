# OpenAI-only fork

This fork of Hex transcribes only through OpenAI's transcription API
(`POST /v1/audio/transcriptions`, default model `gpt-transcribe`). The on-device
models, imports, and Apple Speech are hidden, nothing downloads a speech model, and
the public Hex updater is off.

## Configuration

For each value, the first source that is set wins:

| Value    | Sources, in order |
|----------|-------------------|
| API key  | `HEX_OPENAI_API_KEY`, `OPENAI_API_KEY`, `api_key` in `openai.json` |
| Base URL | `HEX_OPENAI_BASE_URL`, `base_url` in `openai.json`, build-time `HEX_OPENAI_DEFAULT_BASE_URL`, `https://api.openai.com/v1` |
| Model    | `HEX_OPENAI_MODEL`, `model` in `openai.json`, build-time `HEX_OPENAI_DEFAULT_MODEL`, `gpt-transcribe` |

`openai.json` lives in Hex's data folder: `%APPDATA%\hex` on Windows,
`~/Library/Application Support/hex` on macOS, `~/.local/share/hex` on Linux.

```json
{ "api_key": "sk-…", "model": "gpt-transcribe" }
```

For company rollout, don't give every machine the real key. Build with
`HEX_OPENAI_DEFAULT_BASE_URL=https://your-proxy.example.com/v1` and point it at
an internal proxy that adds the key. With any base URL other than
api.openai.com, Hex sends requests without a key when none is set.

## What changed

- `src/transcription/openai.rs`: the API client. It encodes each clip as a
  16 kHz WAV, sends it as multipart form data through `net` (the computer's
  proxy and trusted roots), splits audio into 10-minute parts to stay under
  the 25 MB limit, and never uploads silence. Vocabulary terms go out as
  `keywords[]` for `gpt-transcribe`, or as `prompt` for `whisper-1` and the
  `gpt-4o-*` models.
- `src/transcription/models.rs`: adds `TranscriptionModelId::OpenAi` (the
  default) and `ModelRuntime::OpenAi`. `openai::ONLY` makes it the only
  available and recommended model, and it needs no download or verification.
- `src/transcription/local_transcriber.rs`: `LocalTranscriber` (Linux and
  Windows) is now an enum of `Gguf` (the old struct, renamed
  `GgufTranscriber`) or `OpenAi`.
- `src/transcription.rs`: adds `Transcriber::OpenAi` on macOS.
- `src/transcription/custom_models.rs`: imports are hidden.
- `src/updater.rs`: `managed_by_policy()` is always true, so the app shows
  "managed by your organization" and never updates itself to the public Hex.

## Status: work in progress, not compiled yet

1. Run `cargo check --bin hex` on each OS and fix what the compiler finds. Any
   `match` on `ModelRuntime` or `TranscriptionModelId` that I missed will show
   up here.
2. Saved settings from an existing Hex install (for example
   `parakeet_unified_en`) now fail `validate`. Check that startup falls back to
   `openai`, and add a migration in `app_settings` if it doesn't.
3. Check onboarding: the model step and its download page should be skipped or
   show OpenAI as ready. Consider a page that asks for the API key.
4. Hide the Labs rows that do nothing without a local model: Vocabulary biasing,
   Unload model when idle, Parakeet Ultra, and Live text (OpenAI doesn't stream
   here).
5. Settings: add an API key field, or document `openai.json` for IT.
6. The Silero speech detector still downloads from Hugging Face. It's optional,
   and dictation works without it.
7. Update the goldens (`HEX_UPDATE_GOLDENS=1 cargo test --locked --bin hex rune_ui`)
   once the picker shows only OpenAI.
8. Test on real hardware: Windows x64 (`scripts/build-windows.sh`) and macOS.
9. Rename the app and bundle ID before distributing, and sign the Windows build
   with your company's certificate.
