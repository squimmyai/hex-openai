//! General › Advanced › OpenAI API key: where this build's transcription key
//! comes from, and a field to paste one. A pasted key is saved to
//! `openai.json` in Hex's data folder, owner-only, and only ever shown
//! masked; it never reaches `settings.json` or the log. A key an environment
//! variable sets, or a company proxy that adds it, shows instead of the field.

use rune::{TextEdit, edit_access, row, text};
use rune_kit::TextInput;

use super::Ui;
use super::controls::{ButtonKind, CardRow, Paint, button, icon_row, note};
use super::icons::LOCK;
use crate::desktop::i18n::{tr, tr_fill};
use crate::transcription::openai::{self, KeySource};

const FIELD_WIDTH: f32 = 240.0;

/// The row's state: where the key comes from (read live, never by a
/// fixture), and the field while one is typed.
#[derive(Default)]
pub(super) struct KeyRow {
    pub(super) source: KeySource,
    editing: bool,
    field: TextEdit,
    error: Option<String>,
}

impl KeyRow {
    /// Where the key comes from now: the environment, `openai.json`, or the
    /// build's base URL.
    pub(super) fn load() -> Self {
        Self {
            source: openai::Config::load().key_source(),
            ..Self::default()
        }
    }
}

/// Scenes: Windows' General › Advanced with no key, a saved one, and a
/// company proxy that adds it.
pub(super) fn scene(ui: &mut Ui, source: KeySource) {
    *ui = super::fixture(super::DesktopShortcuts::Registered);
    ui.platform = super::platform::PlatformState::preview(super::platform::Capabilities::WINDOWS);
    ui.host = super::platform::PlatformHost::Fixture(Vec::new());
    ui.settings.transcription = Default::default();
    ui.picker.installed = Some(vec![ui.settings.transcription.model]);
    ui.advanced = vec![super::Pane::General];
    ui.openai_key = KeyRow {
        source,
        ..KeyRow::default()
    };
}

/// The row, and a note under it when saving failed.
pub(super) fn rows(ui: &Ui, p: Paint) -> Vec<CardRow> {
    if !openai::ONLY {
        return Vec::new();
    }
    let state = &ui.openai_key;
    let title = tr("OpenAI API key");
    let row_el = match &state.source {
        KeySource::Variable(name) => icon_row(
            p,
            "openai-key",
            &LOCK,
            title,
            Some(tr_fill("Set by {} on this computer", name).as_str()),
            rune::el(),
        ),
        KeySource::Proxy(host) => icon_row(
            p,
            "openai-key",
            &LOCK,
            title,
            Some(tr_fill("Requests go through {}, which adds the key", host).as_str()),
            rune::el(),
        ),
        KeySource::Saved(masked) if !state.editing => icon_row(
            p,
            "openai-key",
            &LOCK,
            title,
            Some(tr("Saved on this computer only")),
            row()
                .key("openai-key-controls")
                .gap(8.0)
                .center_y()
                .child(text(masked.clone()).key("masked").color(p.muted()))
                .child(button(
                    p,
                    "openai-key-change",
                    tr("Change"),
                    ButtonKind::Secondary,
                    |ui: &mut Ui, _| ui.openai_key.editing = true,
                ))
                .child(button(
                    p,
                    "openai-key-remove",
                    tr("Remove"),
                    ButtonKind::Secondary,
                    |ui: &mut Ui, _| save(ui, None),
                )),
        ),
        KeySource::Saved(_) | KeySource::Missing => {
            let mut controls = row()
                .key("openai-key-controls")
                .gap(8.0)
                .center_y()
                .child(
                    TextInput::new(&state.field)
                        .secure()
                        .placeholder("sk-…")
                        .width(Some(FIELD_WIDTH))
                        .on_edit(edit_access(|ui: &mut Ui| &mut ui.openai_key.field))
                        .on_submit(|ui: &mut Ui, _| save_field(ui))
                        .build(p.k)
                        .key("openai-key-field"),
                )
                .child(button(
                    p,
                    "openai-key-save",
                    tr("Save"),
                    ButtonKind::Secondary,
                    |ui: &mut Ui, _| save_field(ui),
                ));
            if state.editing {
                controls = controls.child(button(
                    p,
                    "openai-key-cancel",
                    tr("Cancel"),
                    ButtonKind::Secondary,
                    |ui: &mut Ui, _| {
                        ui.openai_key.editing = false;
                        ui.openai_key.field = TextEdit::default();
                    },
                ));
            }
            icon_row(
                p,
                "openai-key",
                &LOCK,
                title,
                Some(tr("Dictation needs one; it stays on this computer")),
                controls,
            )
        }
    };
    let mut rows = vec![CardRow::from(row_el)];
    if let Some(error) = &state.error {
        rows.push(note("openai-key-error", error, p.danger()).into());
    }
    rows
}

fn save_field(ui: &mut Ui) {
    let key = ui.openai_key.field.text.trim().to_owned();
    if !key.is_empty() {
        save(ui, Some(&key));
    }
}

/// Saves `key` in `openai.json`, or removes it. A scene changes only what
/// it shows.
fn save(ui: &mut Ui, key: Option<&str>) {
    let fixture = ui.is_fixture();
    let state = &mut ui.openai_key;
    if fixture {
        state.source = key.map_or(KeySource::Missing, |key| {
            KeySource::Saved(openai::masked(key))
        });
    } else {
        match openai::save_api_key(key) {
            Ok(()) => state.source = openai::Config::load().key_source(),
            Err(error) => {
                state.error = Some(tr_fill("Couldn't save the key: {}", &format!("{error:#}")));
                return;
            }
        }
    }
    state.error = None;
    state.editing = false;
    state.field = TextEdit::default();
}
