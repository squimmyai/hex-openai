//! Where the API key, base URL, and model come from, first match winning:
//! the environment, then `openai.json` in Hex's data folder, then the build.
//! Settings saves the key to that file, owner-only like `local-api.json`,
//! and only ever shows it masked.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use color_eyre::eyre::{Result, eyre};
use serde::Deserialize;

const DEFAULT_BASE_URL: &str = match option_env!("HEX_OPENAI_DEFAULT_BASE_URL") {
    Some(url) => url,
    None => "https://api.openai.com/v1",
};
const DEFAULT_MODEL: &str = match option_env!("HEX_OPENAI_DEFAULT_MODEL") {
    Some(model) => model,
    None => "gpt-transcribe",
};
pub(super) const FILE: &str = "openai.json";
const KEY_VARIABLES: [&str; 2] = ["HEX_OPENAI_API_KEY", "OPENAI_API_KEY"];

#[derive(Clone, Debug, Default, Deserialize)]
struct FileConfig {
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    api_key: Option<String>,
    /// Which variable the key came from, when one did.
    key_variable: Option<&'static str>,
    pub base_url: String,
    pub model: String,
}

/// Never prints the key.
impl std::fmt::Debug for Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Config")
            .field("api_key", &self.api_key.as_ref().map(|_| "…"))
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .finish()
    }
}

/// Where Settings says the key comes from. It holds no key, only its last
/// characters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum KeySource {
    /// An environment variable sets it, which Settings can't change.
    Variable(&'static str),
    /// Requests go to a server other than OpenAI's (a company proxy), which
    /// adds the key itself.
    Proxy(String),
    /// Saved in `openai.json`: its masked form.
    Saved(String),
    #[default]
    Missing,
}

impl Config {
    /// Reads the configuration now, so a changed `openai.json` or
    /// environment applies to the next request without a restart.
    pub fn load() -> Self {
        let file = path().map(|path| read_file(&path)).unwrap_or_default();
        Self::resolve(|name| std::env::var(name).ok(), file)
    }

    fn resolve(env: impl Fn(&str) -> Option<String>, file: FileConfig) -> Self {
        let from_variable = KEY_VARIABLES
            .into_iter()
            .find_map(|name| nonempty(env(name)).map(|key| (name, key)));
        let key_variable = from_variable.as_ref().map(|(name, _)| *name);
        let api_key = from_variable
            .map(|(_, key)| key)
            .or_else(|| nonempty(file.api_key));
        let base_url = nonempty(env("HEX_OPENAI_BASE_URL"))
            .or_else(|| nonempty(file.base_url))
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        let model = nonempty(env("HEX_OPENAI_MODEL"))
            .or_else(|| nonempty(file.model))
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        Self {
            api_key,
            key_variable,
            base_url: base_url.trim_end_matches('/').to_owned(),
            model,
        }
    }

    pub fn endpoint(&self) -> String {
        format!("{}/audio/transcriptions", self.base_url)
    }

    pub(super) fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    pub(super) fn is_default_host(&self) -> bool {
        crate::net::host(&self.base_url) == "api.openai.com"
    }

    /// `gpt-transcribe` takes `languages[]` and `keywords[]`; the older
    /// models (`whisper-1`, `gpt-4o-transcribe`, …) take one `language` and
    /// read the vocabulary as their `prompt`.
    pub(super) fn takes_language_lists(&self) -> bool {
        !(self.model.starts_with("whisper") || self.model.starts_with("gpt-4o"))
    }

    pub fn key_source(&self) -> KeySource {
        if let Some(name) = self.key_variable {
            KeySource::Variable(name)
        } else if !self.is_default_host() {
            KeySource::Proxy(crate::net::host(&self.base_url))
        } else if let Some(key) = &self.api_key {
            KeySource::Saved(masked(key))
        } else {
            KeySource::Missing
        }
    }
}

fn nonempty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// The key's last four characters behind a mask: `••••wxyz`. Keys too short
/// to spare four show only the mask.
pub fn masked(key: &str) -> String {
    let characters: Vec<char> = key.chars().collect();
    let tail: String = if characters.len() >= 12 {
        characters[characters.len() - 4..].iter().collect()
    } else {
        String::new()
    };
    format!("••••{tail}")
}

pub(super) fn path() -> Result<PathBuf> {
    Ok(crate::app_paths::support_dir()?.join(FILE))
}

fn read_file(path: &Path) -> FileConfig {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
            tracing::warn!(path = %path.display(), %error, "ignoring an unreadable OpenAI configuration");
            FileConfig::default()
        }),
        Err(_) => FileConfig::default(),
    }
}

/// Saves `key` (or removes it, for `None`) in `openai.json`, keeping the
/// file's other values.
pub fn save_api_key(key: Option<&str>) -> Result<()> {
    save_api_key_at(&path()?, key)
}

fn save_api_key_at(path: &Path, key: Option<&str>) -> Result<()> {
    let mut document = match fs::read(path) {
        Ok(data) => match serde_json::from_slice::<serde_json::Value>(&data) {
            Ok(serde_json::Value::Object(object)) => object,
            _ => return Err(eyre!("{FILE} isn't a JSON object; fix or delete it first")),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(error) => return Err(error.into()),
    };
    match key.map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => document.insert("api_key".into(), key.into()),
        None => document.remove("api_key"),
    };
    let mut data = serde_json::to_vec_pretty(&document)?;
    data.push(b'\n');
    write_owner_only(path, &data)
}

/// Replaces `path` with `data` that only the owner can read. On Windows the
/// file inherits the per-user data folder's ACL (the user, SYSTEM, and
/// Administrators).
fn write_owner_only(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| eyre!("{FILE} has no parent folder"))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    if let Err(error) = fs::remove_file(&temporary)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(error.into());
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&temporary)?;
    file.write_all(data)?;
    file.sync_all()?;
    #[cfg(unix)]
    fs::set_permissions(
        &temporary,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )?;
    fs::rename(&temporary, path)?;
    crate::durable::sync_directory(parent)?;
    Ok(())
}

#[cfg(test)]
pub(super) fn for_test(api_key: Option<&str>, base_url: &str, model: &str) -> Config {
    Config {
        api_key: api_key.map(str::to_owned),
        key_variable: None,
        base_url: base_url.into(),
        model: model.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own under the system's temporary folder, removed
    /// when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("hex-openai-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
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
        assert_eq!(config.api_key(), Some("from-env"));
        assert_eq!(
            config.endpoint(),
            "https://proxy.example.com/openai/v1/audio/transcriptions"
        );
        assert_eq!(config.model, "gpt-4o-mini-transcribe");
        assert_eq!(config.key_source(), KeySource::Variable("OPENAI_API_KEY"));

        let config = Config::resolve(|_| None, FileConfig::default());
        assert_eq!(config.api_key(), None);
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.model, DEFAULT_MODEL);
        assert_eq!(config.key_source(), KeySource::Missing);
    }

    #[test]
    fn hex_variable_wins_over_openai_variable_and_blank_values_fall_through() {
        let env = |name: &str| match name {
            "HEX_OPENAI_API_KEY" => Some("hex-key".to_owned()),
            "OPENAI_API_KEY" => Some("openai-key".to_owned()),
            "HEX_OPENAI_MODEL" => Some("   ".to_owned()),
            _ => None,
        };
        let file = FileConfig {
            model: Some("whisper-1".into()),
            ..FileConfig::default()
        };
        let config = Config::resolve(env, file);
        assert_eq!(config.api_key(), Some("hex-key"));
        assert_eq!(
            config.key_source(),
            KeySource::Variable("HEX_OPENAI_API_KEY")
        );
        assert_eq!(config.model, "whisper-1");
    }

    #[test]
    fn settings_learns_where_the_key_comes_from_without_seeing_it() {
        let saved = FileConfig {
            api_key: Some("sk-proj-abcdefghijklmnop".into()),
            ..FileConfig::default()
        };
        let config = Config::resolve(|_| None, saved.clone());
        assert_eq!(config.key_source(), KeySource::Saved("••••mnop".into()));
        assert!(!format!("{config:?}").contains("abcdef"));

        let proxied = FileConfig {
            base_url: Some("https://openai-proxy.corp.example/v1".into()),
            ..saved
        };
        assert_eq!(
            Config::resolve(|_| None, proxied).key_source(),
            KeySource::Proxy("openai-proxy.corp.example".into())
        );
        assert_eq!(masked("short"), "••••");
    }

    #[test]
    fn saving_a_key_keeps_the_other_values_and_only_the_owner_can_read_it() {
        let directory = TempDir::new("save");
        let path = directory.0.join(FILE);
        fs::write(&path, r#"{"model": "whisper-1", "api_key": "old"}"#).unwrap();
        save_api_key_at(&path, Some("  sk-new-key  ")).unwrap();
        let file = read_file(&path);
        assert_eq!(file.api_key.as_deref(), Some("sk-new-key"));
        assert_eq!(file.model.as_deref(), Some("whisper-1"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        save_api_key_at(&path, None).unwrap();
        let file = read_file(&path);
        assert_eq!(file.api_key, None);
        assert_eq!(file.model.as_deref(), Some("whisper-1"));
    }

    #[test]
    fn a_damaged_file_is_left_alone() {
        let directory = TempDir::new("damaged");
        let path = directory.0.join(FILE);
        fs::write(&path, "not json").unwrap();
        assert!(save_api_key_at(&path, Some("sk-key")).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "not json");
    }
}
