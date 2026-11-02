# Hex OpenAI

Hex OpenAI is an internal build of [Hex](https://github.com/anomalyco/hex),
voice dictation for Windows and Mac, that transcribes only through OpenAI's
API. Hold a key, speak, and it pastes.

## How it works

- hold the shortcut, speak, then let go
- the recording goes to OpenAI (or your company's proxy), and the text is
  pasted into whatever app you're in
- nothing downloads a speech model, and the app never updates itself to the
  public Hex
- it installs beside the public Hex under its own name, folder
  (`hex-openai`), Windows installer ID, and macOS bundle ID
  (`com.squimmyai.hex-openai`), so the two never replace each other
- experimental features live in Labs, at the bottom of the sidebar

[`docs/openai-fork.md`](docs/openai-fork.md) lists every change from Hex.

## Configure

Hex OpenAI needs an OpenAI API key, unless your company's proxy adds it.
Paste one in **Settings › General › Advanced › OpenAI API key**, or set it for
everyone some other way. For each value the first source that is set wins:

| Value | Sources, in order |
| --- | --- |
| API key | `HEX_OPENAI_API_KEY`, `OPENAI_API_KEY`, `api_key` in `openai.json` |
| Base URL | `HEX_OPENAI_BASE_URL`, `base_url` in `openai.json`, the build's `HEX_OPENAI_DEFAULT_BASE_URL`, `https://api.openai.com/v1` |
| Model | `HEX_OPENAI_MODEL`, `model` in `openai.json`, the build's `HEX_OPENAI_DEFAULT_MODEL`, `gpt-transcribe` |

`openai.json` is in the data folder, `%APPDATA%\hex-openai` on Windows and
`~/Library/Application Support/hex-openai` on macOS. Keep it readable only by
its owner:

```json
{
  "api_key": "sk-…",
  "base_url": "https://api.openai.com/v1",
  "model": "gpt-transcribe"
}
```

### Through a company proxy

To keep the real key off every machine, build with a proxy as the default
base URL. With any base URL other than api.openai.com, Hex OpenAI sends no
key of its own, and Settings names the proxy instead of asking for one. The
proxy forwards `POST /v1/audio/transcriptions` (multipart form data, up to
25 MB) to OpenAI with the key added; `docs/openai-fork.md` has an nginx
example. Requests use the computer's proxy settings and trusted root
certificates, like a browser.

## Build

### Windows x64 installer

On a Windows x64 machine with Rust (MSVC), Git Bash, `jq`, CMake, and the
Vulkan SDK, from a clean checkout of `openai-transcription`, in Git Bash:

```sh
export HEX_OPENAI_DEFAULT_BASE_URL=https://openai-proxy.corp.example/v1  # optional
HEX_VERSION=3.2.0 scripts/build-windows.sh
```

That writes `dist/hex-3.2.0-windows-x64.exe`. Sign it before you wrap it,
since the installer embeds it byte for byte (PowerShell, with the Windows SDK's
`signtool` and your company's code-signing certificate):

```powershell
signtool sign /fd SHA256 /td SHA256 /tr http://timestamp.digicert.com `
  /f C:\certs\company-codesign.pfx /p $env:CODESIGN_PASSWORD `
  dist\hex-3.2.0-windows-x64.exe
scripts\build-windows-installer.ps1 -Executable dist\hex-3.2.0-windows-x64.exe `
  -Version 3.2.0 -Arch x86_64 -OutDir dist
signtool sign /fd SHA256 /td SHA256 /tr http://timestamp.digicert.com `
  /f C:\certs\company-codesign.pfx /p $env:CODESIGN_PASSWORD `
  dist\hex-openai-setup-3.2.0-windows-x64.exe
```

A certificate in a hardware token or the Windows certificate store signs with
`/sha1 <thumbprint>` (or `/n "<subject>"`) in place of `/f` and `/p`. Check
both files with `signtool verify /pa /v <file>`.

The installer is per-user (no administrator needed), installs into
`%APPDATA%\hex-openai`, and adds **Hex OpenAI** to the Start menu and Apps &
Features. The x64 build still links the Vulkan loader, so the PC needs a
graphics driver with Vulkan (any current NVIDIA, AMD, or Intel driver).

### macOS (Apple silicon)

For a local build to try: `HEX_LOCAL_BUILD=1 cargo run -- app`. For an app
to hand out, `HEX_TEAM_ID=<your Apple team> scripts/build-app.sh` signs
`target/app/Hex OpenAI.app` with your Developer ID; notarize it before you
distribute it.

More docs are in [`docs/`](docs/README.md).

Upstream Hex is MIT licensed, and so is this fork.
