//! One-time settings changes. A file that predates one lacks its ID in
//! `migrations`, so the change runs once on the next load and its ID is
//! recorded; new settings start with every ID because they need none.

#[cfg(target_os = "macos")]
use color_eyre::eyre::Result;

use super::{AppSettings, MeetingRetention};
use crate::transcription::models::{
    self, AUTO_LANGUAGE, TranscriptionModelId, TranscriptionSelection,
};
use crate::transcription::openai;

/// Moves English dictation to the streaming English model once.
pub(super) const STREAMING_ENGLISH_MIGRATION: &str = "streaming-english-model";
/// Annotate became a default feature: on for everyone once.
#[cfg(hex_capture)]
pub(super) const ANNOTATE_DEFAULT_MIGRATION: &str = "annotate-on-by-default";
/// Settings saved by an `anomaly.hex` build. Up to 2.3.1 Hex was
/// `com.kitlangton.hex2` and used the same folder, so a file without this
/// ID may carry onboarding finished under grants macOS no longer gives the
/// new identity; the macOS launch checks setup once
/// (`onboarding::recheck_after_identity_change`).
pub(super) const ANOMALY_IDENTITY_MIGRATION: &str = "anomaly-hex-identity";
#[cfg(hex_capture)]
pub(super) const MIGRATIONS: &[&str] = &[
    STREAMING_ENGLISH_MIGRATION,
    ANNOTATE_DEFAULT_MIGRATION,
    ANOMALY_IDENTITY_MIGRATION,
    MEETING_RETENTION_DEFAULT_MIGRATION,
    OPENAI_ONLY_MIGRATION,
];
#[cfg(not(hex_capture))]
pub(super) const MIGRATIONS: &[&str] = &[
    STREAMING_ENGLISH_MIGRATION,
    ANOMALY_IDENTITY_MIGRATION,
    MEETING_RETENTION_DEFAULT_MIGRATION,
    OPENAI_ONLY_MIGRATION,
];
/// Scrawl's settings were read into `capture` once. Left out of
/// `MIGRATIONS` so new settings import them too.
#[cfg(hex_capture)]
pub(super) const SCRAWL_IMPORT_MIGRATION: &str = "scrawl-capture-settings";
/// Scrawl's router settings were read into `capture.routing` once: its own
/// migration, as it came after the first import.
#[cfg(hex_capture)]
pub(super) const SCRAWL_ROUTING_IMPORT_MIGRATION: &str = "scrawl-routing-settings";
/// Meeting retention first saved `forever` as its default; that default is
/// now unchosen (24 hours for new meetings, older ones kept).
pub(super) const MEETING_RETENTION_DEFAULT_MIGRATION: &str = "meeting-retention-default";
/// This build transcribes only through OpenAI (`openai::ONLY`): a model the
/// public Hex saved moves to OpenAI once, keeping its language where OpenAI
/// takes it. The first save keeps the original file beside the new one.
pub(super) const OPENAI_ONLY_MIGRATION: &str = "openai-only-model";

impl AppSettings {
    /// Applies each one-time change this file has not seen, then records it
    /// so a later choice, such as switching back, sticks.
    pub(super) fn apply_one_time_migrations(&mut self) {
        self.apply_one_time_migrations_with(|model| models::is_installed(model, "en"));
    }

    pub(super) fn apply_one_time_migrations_with(
        &mut self,
        installed: impl Fn(&models::ModelDefinition) -> bool,
    ) {
        if !self.has_migration(STREAMING_ENGLISH_MIGRATION) {
            self.switch_english_to_streaming_model(installed);
            self.migrations.push(STREAMING_ENGLISH_MIGRATION.into());
        }
        if !self.has_migration(MEETING_RETENTION_DEFAULT_MIGRATION) {
            if self.meetings.retention == Some(MeetingRetention::Forever) {
                self.meetings.retention = None;
            }
            self.migrations
                .push(MEETING_RETENTION_DEFAULT_MIGRATION.into());
        }
        #[cfg(hex_capture)]
        if !self.has_migration(ANNOTATE_DEFAULT_MIGRATION) {
            self.capture.enabled = true;
            self.migrations.push(ANNOTATE_DEFAULT_MIGRATION.into());
        }
    }

    pub(super) fn has_migration(&self, id: &str) -> bool {
        self.migrations.iter().any(|applied| applied == id)
    }

    /// Whether these settings come from before the `anomaly.hex` identity.
    #[cfg(target_os = "macos")]
    pub(crate) fn predates_anomaly_identity(&self) -> bool {
        !self.has_migration(ANOMALY_IDENTITY_MIGRATION)
    }

    /// Records the identity check in the file at `path`, changing nothing else.
    #[cfg(target_os = "macos")]
    pub(crate) fn record_anomaly_identity(&mut self, path: &std::path::Path) -> Result<()> {
        if self.predates_anomaly_identity() {
            self.migrations.push(ANOMALY_IDENTITY_MIGRATION.into());
            self.write_to(path)?;
        }
        Ok(())
    }

    /// English dictation moves to Parakeet Unified English, which streams
    /// while the shortcut is held. The previous model stays downloaded and
    /// in the recent models, one click away. Other languages and Auto keep
    /// their model because Parakeet Unified is English-only. Only an installed
    /// model is switched to, so an upgrade never leaves dictation waiting on a
    /// download.
    fn switch_english_to_streaming_model(
        &mut self,
        installed: impl Fn(&models::ModelDefinition) -> bool,
    ) {
        if self.transcription.language != "en"
            || self.transcription.model == TranscriptionModelId::ParakeetUnifiedEnglish
        {
            return;
        }
        let model = models::definition(TranscriptionModelId::ParakeetUnifiedEnglish);
        if !installed(model) {
            return;
        }
        let selection = TranscriptionSelection {
            model: model.id,
            language: "en".into(),
        };
        if models::validate(&selection).is_err() {
            return;
        }
        tracing::info!(
            from = self.transcription.model.as_str(),
            "switched English dictation to the streaming English model"
        );
        self.remember_transcription(selection);
    }

    /// A model this build can't run falls back to the default, keeping its
    /// language where the default takes it. An import or Parakeet Ultra
    /// stays saved while Labs is off, except where only OpenAI runs.
    pub(super) fn migrate_disabled_transcription_model(&mut self) {
        let gated = self.transcription.model.is_labs_gated() && !openai::ONLY;
        if gated || models::definition(self.transcription.model).available() {
            return;
        }
        tracing::warn!(
            model = self.transcription.model.as_str(),
            "replaced a disabled transcription model"
        );
        self.transcription = default_model_for(&self.transcription.language);
    }

    /// Moves a model the public Hex saved, and the keyboard languages mapped
    /// to models, to OpenAI once ([`OPENAI_ONLY_MIGRATION`]).
    pub(super) fn migrate_to_openai_once(&mut self) {
        if self.has_migration(OPENAI_ONLY_MIGRATION) {
            return;
        }
        self.migrations.push(OPENAI_ONLY_MIGRATION.into());
        if !openai::ONLY {
            return;
        }
        if self.transcription.model != TranscriptionModelId::OpenAi {
            tracing::info!(
                from = self.transcription.model.as_str(),
                "moved dictation to OpenAI"
            );
            self.transcription = default_model_for(&self.transcription.language);
        }
        for choice in self.keyboard_model_switching.mappings.values_mut() {
            choice.model = TranscriptionModelId::OpenAi;
        }
    }
}

/// The default model in `language` where it takes it, detecting a language
/// it has no code for (Cantonese for OpenAI), and in the default's own
/// language when `language` isn't one at all.
fn default_model_for(language: &str) -> TranscriptionSelection {
    let fallback = if models::is_known_language(language) {
        AUTO_LANGUAGE
    } else {
        ""
    };
    [language, fallback]
        .into_iter()
        .map(|language| TranscriptionSelection {
            language: language.into(),
            ..TranscriptionSelection::default()
        })
        .find(|selection| models::validate(selection).is_ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    /// A settings file in a folder of its own, removed when the test ends.
    struct Folder(PathBuf);

    impl Folder {
        fn with_settings(name: &str, json: &str) -> Self {
            let folder = std::env::temp_dir().join(format!(
                "hex-openai-migration-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&folder);
            fs::create_dir_all(&folder).unwrap();
            fs::write(folder.join("settings.json"), json).unwrap();
            Self(folder)
        }

        fn settings(&self) -> PathBuf {
            self.0.join("settings.json")
        }

        fn backups(&self) -> Vec<String> {
            fs::read_dir(&self.0)
                .unwrap()
                .filter_map(|entry| {
                    let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                    name.starts_with("settings.json.").then_some(name)
                })
                .collect()
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const UPSTREAM: &str = r#"{
  "transcription": {"model": "whisper_large_v3_turbo", "language": "de"},
  "keyboard_model_switching": {
    "enabled": true,
    "mappings": {"Russian": {"model": "parakeet_v3", "language": "ru"}}
  },
  "migrations": ["streaming-english-model", "anomaly-hex-identity", "meeting-retention-default"]
}"#;

    #[test]
    fn an_upstream_model_moves_to_openai_once_keeping_its_language_and_the_original() {
        let folder = Folder::with_settings("upstream", UPSTREAM);
        let settings = AppSettings::load_from(&folder.settings()).unwrap();
        assert_eq!(
            settings.transcription,
            TranscriptionSelection {
                model: TranscriptionModelId::OpenAi,
                language: "de".into(),
            }
        );
        assert!(settings.has_migration(OPENAI_ONLY_MIGRATION));
        assert_eq!(
            settings.keyboard_model_switching.mappings["Russian"].model,
            TranscriptionModelId::OpenAi
        );
        // The move is saved, and the original stays beside it, byte for byte.
        let saved = fs::read_to_string(folder.settings()).unwrap();
        assert!(saved.contains(r#""model": "openai""#), "{saved}");
        assert!(saved.contains(OPENAI_ONLY_MIGRATION), "{saved}");
        let backups = folder.backups();
        assert_eq!(backups.len(), 1, "{backups:?}");
        assert_eq!(
            fs::read_to_string(folder.0.join(&backups[0])).unwrap(),
            UPSTREAM
        );
        // A second launch changes nothing more.
        let again = AppSettings::load_from(&folder.settings()).unwrap();
        assert_eq!(again.transcription, settings.transcription);
        assert_eq!(fs::read_to_string(folder.settings()).unwrap(), saved);
        assert_eq!(folder.backups().len(), 1);
    }

    #[test]
    fn imports_parakeet_ultra_and_apple_speech_move_too() {
        for (name, model) in [
            ("ultra", "parakeet_ultra"),
            ("apple", "apple_speech"),
            ("import", "custom-0123456789abcdef"),
        ] {
            let json = format!(r#"{{"transcription": {{"model": "{model}", "language": "fr"}}}}"#);
            let folder = Folder::with_settings(name, &json);
            let settings = AppSettings::load_from(&folder.settings()).unwrap();
            assert_eq!(
                settings.transcription.model,
                TranscriptionModelId::OpenAi,
                "{model}"
            );
            assert_eq!(settings.transcription.language, "fr", "{model}");
        }
    }

    #[test]
    fn a_language_openai_lacks_falls_back_to_detection() {
        let folder = Folder::with_settings(
            "cantonese",
            r#"{"transcription": {"model": "sense_voice_small", "language": "yue"}}"#,
        );
        let settings = AppSettings::load_from(&folder.settings()).unwrap();
        assert_eq!(
            settings.transcription,
            TranscriptionSelection {
                model: TranscriptionModelId::OpenAi,
                language: AUTO_LANGUAGE.into(),
            }
        );
    }

    #[test]
    fn a_model_saved_after_the_move_still_falls_back_without_refusing_to_launch() {
        // Someone edits the file, or an older build saves over it.
        let folder = Folder::with_settings(
            "edited",
            r#"{"transcription": {"model": "parakeet_v3", "language": "it"},
               "migrations": ["openai-only-model"]}"#,
        );
        let settings = AppSettings::load_from(&folder.settings()).unwrap();
        assert_eq!(settings.transcription.model, TranscriptionModelId::OpenAi);
        assert_eq!(settings.transcription.language, "it");
    }
}
