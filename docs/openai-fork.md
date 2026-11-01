# OpenAI-only fork

This fork of Hex transcribes only through OpenAI's transcription API
(`POST /v1/audio/transcriptions`, default model `gpt-transcribe`). The
on-device models, imports, and Apple Speech are hidden, nothing downloads a
speech model, and the public Hex updater is off. `openai::ONLY` in
`src/transcription/openai.rs` is the switch every change below checks.

## Configuration

For each value, the first source that is set wins:

| Value    | Sources, in order |
|----------|-------------------|
| API key  | `HEX_OPENAI_API_KEY`, `OPENAI_API_KEY`, `api_key` in `openai.json` (Settings › General › Advanced › OpenAI API key writes it) |
| Base URL | `HEX_OPENAI_BASE_URL`, `base_url` in `openai.json`, build-time `HEX_OPENAI_DEFAULT_BASE_URL`, `https://api.openai.com/v1` |
| Model    | `HEX_OPENAI_MODEL`, `model` in `openai.json`, build-time `HEX_OPENAI_DEFAULT_MODEL`, `gpt-transcribe` |

Each dictation reads them again, so a changed file or key applies to the
next dictation without a restart.

`openai.json` lives in the data folder: `%APPDATA%\hex-openai` on Windows,
`~/Library/Application Support/hex-openai` on macOS,
`~/.local/share/hex-openai` on Linux.
Settings writes it owner-only (mode 0600 on macOS and Linux; on Windows it
inherits the per-user folder's ACL) and keeps any other values in it:

```json
{
  "api_key": "sk-…",
  "model": "gpt-transcribe"
}
```

`gpt-transcribe` gets `languages[]` and the vocabulary as `keywords[]`;
`whisper-1` and the `gpt-4o-*` models get one `language` and the vocabulary
as `prompt`.

### A company proxy instead of a key on every machine

Build with the proxy as the default base URL, and give no machine the real
key:

```sh
HEX_OPENAI_DEFAULT_BASE_URL=https://openai-proxy.corp.example/v1 scripts/build-windows.sh
```

With any base URL other than api.openai.com, Hex sends requests without an
`Authorization` header when no key is set, and Settings says "Requests go
through openai-proxy.corp.example, which adds the key" instead of showing the
key field. The proxy only has to forward `POST /v1/audio/transcriptions`
(multipart form data, up to 25 MB) to `https://api.openai.com` with the
company key added. A minimal nginx example:

```nginx
location /v1/audio/transcriptions {
    proxy_pass https://api.openai.com/v1/audio/transcriptions;
    proxy_set_header Authorization "Bearer $openai_key";
    proxy_set_header Host api.openai.com;
    proxy_ssl_server_name on;
    client_max_body_size 26m;
    proxy_read_timeout 180s;
}
```

Requests go through `net`, so the computer's proxy settings (Windows'
including PAC and WPAD, `HTTPS_PROXY`) and its trusted root certificates
apply, as they do in a browser.

## Name and identifiers

The fork is **Hex OpenAI**, and installs beside the public Hex without
sharing anything with it:

| | Public Hex | Hex OpenAI |
|---|---|---|
| Data folder (and the Windows install) | `hex` | `hex-openai` (`app_paths::DATA_FOLDER`) |
| Personal commands workspace | `~/.config/hex` | `~/.config/hex-openai` |
| Window, menu bar, tray, Start menu, Apps & Features | Hex | Hex OpenAI (`app_paths::PRODUCT_NAME`) |
| Windows installer `AppId` (and the updater's uninstall key) | `{6ED48C92-…}` | `{F6D1BBE2-D2DD-476E-8BE3-4553C6DAC958}` |
| Windows launch-at-login value (`HKCU\…\Run`) | `Hex` | `Hex OpenAI` |
| Windows installer file | `hex-setup-<version>-windows-x64.exe` | `hex-openai-setup-<version>-windows-x64.exe` |
| macOS bundle ID (`scripts/build-app.sh`, `onboarding::MAC_BUNDLE_ID`) | `anomaly.hex` | `com.squimmyai.hex-openai` |
| macOS app and URL scheme | `Hex.app`, `hex://` | `Hex OpenAI.app`, `hex-openai://` |

Because the data folder is new, a computer that already runs the public Hex
starts Hex OpenAI with default settings and no history; nothing is copied
from `hex`. The UI copy still calls the app "Hex". The Linux app ID and
desktop entry are unchanged (Linux only has to compile). The macOS updater
still names `anomaly.hex`, but updates are off.

## What changed

- **Transcription** (`src/transcription/openai.rs`, `openai/config.rs`,
  `openai/client.rs`): each clip goes up as a 16 kHz 16-bit mono WAV in
  10-minute parts (the API's limit is 25 MB). Silence is never uploaded.
  Requests time out after 60 s plus 1 s per 10 s of audio; the connection
  gives up after 15 s. The dictation queues stay bounded at two jobs.
  Filipino goes out as `tl`; Cantonese (`yue`), which has no ISO-639-1 code,
  dictates with the language detected.
- **Errors**: no key, a rejected key (401), a refused project (403), an
  unknown model (404), rate limits or no credit (429), OpenAI's own failures
  (5xx), and network failures each fail the dictation with a sentence that
  says what to do, shown where Hex already shows a failed dictation, with
  the failure tone. Anything shaped like a key is removed from OpenAI's
  message. Logs name the host, status, model, and durations, never the key
  or the transcript.
- **Settings migration** (`src/app_settings/migrations.rs`): a
  `settings.json` from the public Hex with any other model (Parakeet, Whisper,
  an import, Parakeet Ultra, Apple Speech) moves to OpenAI once
  (`openai-only-model` in `migrations`), keeping its language where OpenAI
  takes it and detecting it otherwise; keyboard-language mappings move too.
  The first save keeps the original beside it as
  `settings.json.unreadable-<sha256>`. A model this build can't run falls
  back the same way on every load, so launch never fails.
- **Onboarding**: OpenAI counts as installed, so the first run opens on the
  demo and never shows a download. The language capsule switches only the
  language, and the setup page (reachable with Back) says "Dictating in
  English through OpenAI".
- **Settings**: General › Advanced › Dictation starts with an OpenAI API key
  row: a secure field and Save, or the saved key masked (`••••abcd`) with
  Change and Remove, or where an environment variable or proxy supplies it.
  The picker shows "Runs on: OpenAI's servers" for OpenAI.
- **Hidden**: Labs › Live text, Vocabulary biasing, Unload model when idle,
  Parakeet Ultra, Benchmark details, and Picker sort and filter
  (`labs::Feature::needs_local_model`); the Speech engine (GPU/CPU) row; the
  Hugging Face cache scan; and the picker's list of other models with its
  download, delete, details, and import actions. `hex model install` says
  there is nothing to install, and `hex model import` refuses.
- **Speech detector**: Silero VAD still downloads from Hugging Face in the
  background and keeps silence from being uploaded. When the download fails
  (a blocked network), dictation goes on without it, nothing shows, and the
  next try waits 30 minutes.
- **Updates**: `updater::managed_by_policy()` is always true, so the app
  shows "managed by your organization" and never updates itself to the
  public Hex.

## Tests

Tests never reach OpenAI: the client is tested against a local mock HTTP
server (`openai/client.rs`). Upstream tests that check behavior this fork
removed on purpose are `#[ignore = "openai-only: <reason>"]`:

| Test | Why |
|------|-----|
| `app_settings::tests::english_dictation_switches_to_the_installed_streaming_model_once` | no Parakeet Unified English to switch to |
| `app_settings::tests::model_switches_remember_language_without_changing_other_settings` | one model, so there is no other to switch to |
| `labs::tests::unload_idle_carries_its_minutes` | Unload when idle never turns on |
| `rune_ui::keyboard_model_tests::keyboard_model_switching_maps_enabled_sources_to_an_installed_model_and_language` | one model in the menu |
| `rune_ui::picker::tests::benchmark_details_are_off_by_default` | Labs › Benchmark details is hidden |
| `rune_ui::picker::tests::browsing_never_saves_or_asks_for_a_model` | Labs › Picker sort and filter is hidden |
| `desktop::tray_menu::tests::model_menu_only_lists_downloaded_models_with_their_remembered_language` | no models beside OpenAI (`model_menu_shows_openai_as_the_model_in_use` replaces it) |
| `transcription::models::tests::parakeet_ultra_is_offered_only_with_its_lab_and_a_saved_choice_survives` | a saved Parakeet Ultra moves to OpenAI |
| `transcription::models::tests::every_language_choice_supports_its_language_and_first_run_picks_it` | `first_run_picks_openai_in_every_language_it_takes` replaces it |
| `transcription::models::tests::the_picker_lists_every_model_with_the_language_s_recommendations_first` | `the_picker_lists_only_openai` replaces it |
| `transcription::models::tests::{hebrew_recommends_the_ivrit_fine_tune_with_its_hint, recommendations_are_language_specific, automatic_language_choices_only_include_detection_models, weak_qwen_and_parakeet_languages_route_to_stronger_shipped_models, bangla_is_transcribed_by_whisper_with_its_language_hint}` | no per-language recommendations between local models |
| `transcription::models::tests::{whisper_accepts_its_languages_beyond_the_settings_list, selections_require_model_language_compatibility, parakeet_v3_supports_portuguese_without_language_conditioning}` | the GGUF models they validate are unavailable |

The UI fixtures behind most scenes still select Parakeet Unified English, as
upstream's do; `onboarding-openai`, `openai-key`, `openai-key-saved`, and
`openai-key-proxy` show this build's own states. Goldens render on Linux
only (`HEX_UPDATE_GOLDENS=1 cargo test --locked --bin hex rune_ui`).
