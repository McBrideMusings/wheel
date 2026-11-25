//! The client's one-line text fields: the browser's address, the space, window and session names,
//! the artifact palette's, command palette's, conversations' and settings' searches, the
//! processes filter and a setting's value. Each is a gpui-component `InputState`, as the composer
//! is, so each has a caret, a selection, ⌘C, ⌘X, ⌘V, ⌘A and undo. The client keeps each field's
//! text where it kept it before (`Rename.draft`, `CommandPalette.query`, …): an edit in the input
//! copies into it, and a text the client sets (a debug route, a reset) goes into the input on the
//! next frame. A field's own key handler keeps Return, Escape, Up, Down and Tab.
use super::*;
use crate::settings::{Field as SettingField, Topic};
use gpui_component::input::{Input, InputEvent, InputState};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FieldId {
    Address(String),
    SpaceName,
    WindowName,
    ArtifactSearch(String),
    CommandSearch,
    ConversationSearch,
    ProcessFilter,
    SettingsSearch(Topic),
    SettingValue(SettingField),
}

/// Whether `k` edits a field's text: typed text, Backspace and Delete, the caret's moves, and
/// ⌘A, ⌘C, ⌘V, ⌘X and ⌘Z. Return, Escape, Up, Down and Tab are the field's own.
pub fn edits_text(k: &gpui::Keystroke) -> bool {
    let m = &k.modifiers;
    if m.control || m.function && !matches!(k.key.as_str(), "left" | "right" | "home" | "end" | "delete") {
        return false;
    }
    if m.platform {
        return matches!(k.key.as_str(), "a" | "c" | "v" | "x" | "z" | "left" | "right" | "backspace");
    }
    !matches!(k.key.as_str(), "enter" | "escape" | "up" | "down" | "tab")
}

/// An open field's input, the placeholder it shows, and the last text the input and the client
/// agreed on.
pub(super) struct Field {
    pub(super) input: gpui::Entity<InputState>,
    placeholder: String,
    synced: String,
}

impl Client {
    /// Each field open now with the text the client holds for it and its placeholder; the first
    /// holds the keyboard.
    pub(super) fn open_fields(&self) -> Vec<(FieldId, String, String)> {
        let mut open = Vec::new();
        if let Some(edit) = &self.name_edit {
            let placeholder = match edit.target {
                super::NameTarget::Server { .. } => "Pairing code".to_owned(),
                _ => self.space().map(|s| s.name.clone()).unwrap_or_else(|| "Window name".into()),
            };
            open.push((FieldId::WindowName, edit.draft.clone(), placeholder));
        }
        if let Some(p) = &self.command_palette {
            open.push((FieldId::CommandSearch, p.query.clone(), p.placeholder().to_string()));
        }
        if self.settings_open {
            match &self.settings_edit {
                Some(SettingsEdit::Field(f)) => open.push((FieldId::SettingValue(f.clone()), self.settings.text(f), String::new())),
                Some(SettingsEdit::Search(t)) => open.push((FieldId::SettingsSearch(*t), self.settings.page(*t).query.clone(), "Search settings".into())),
                None => {}
            }
        }
        if self.procs.open.is_some() {
            open.push((FieldId::ProcessFilter, self.procs.filter.clone(), "Filter".into()));
        }
        if let Some(p) = &self.conversations.open {
            open.push((FieldId::ConversationSearch, p.query.clone(), "Search conversations".into()));
        }
        if let Some(r) = &self.rename {
            open.push((FieldId::SpaceName, r.draft.clone(), String::new()));
        }
        if let Some((pane, draft)) = &self.address_edit {
            open.push((FieldId::Address(pane.clone()), draft.clone(), crate::browser::PLACEHOLDER.into()));
        }
        if let Some(pane) = &self.focus.current
            && let Some(p) = self.palettes.get(pane).filter(|p| p.searchable())
        {
            open.push((FieldId::ArtifactSearch(pane.clone()), p.query.clone(), p.field_placeholder().to_string()));
        }
        open
    }

    /// The input `id` changed to `text`: the client's copy follows, with what typing did before.
    fn field_edited(&mut self, id: &FieldId, text: String) {
        match id {
            FieldId::Address(pane) => {
                if let Some((_, draft)) = self.address_edit.as_mut().filter(|(p, _)| p == pane) {
                    *draft = text.replace(['\n', '\r'], "");
                }
            }
            FieldId::SpaceName => {
                if let Some(r) = self.rename.as_mut() {
                    r.draft = text;
                }
            }
            FieldId::WindowName => {
                if let Some(e) = self.name_edit.as_mut() {
                    e.draft = text;
                }
            }
            FieldId::ArtifactSearch(pane) => {
                if let Some(p) = self.palettes.get_mut(pane).filter(|p| p.searchable()) {
                    p.type_text(&text);
                }
            }
            FieldId::CommandSearch => self.command_palette_typed(&text),
            FieldId::ConversationSearch => {
                let Some(p) = self.conversations.open.as_mut() else { return };
                p.query = text.replace(['\n', '\r'], " ");
                if !p.query.is_empty() {
                    self.more_conversations();
                }
            }
            FieldId::ProcessFilter => self.procs.filter = text,
            FieldId::SettingsSearch(topic) => self.settings.page_mut(*topic).query = text,
            FieldId::SettingValue(field) => {
                self.settings.type_text(field, &text);
                self.ask_homes(field);
            }
        }
    }

    /// Make an input for each field that opened, drop those of fields that closed, and give each
    /// the client's text when the client changed it (a debug route, a reset): its text differs
    /// from the last text the input and the client agreed on. Typing never makes them differ, so
    /// the input's own text is never overwritten by a copy its change has not reached yet. Runs
    /// each frame, before anything draws a field.
    pub(super) fn sync_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.open_fields();
        self.fields.retain(|id, _| open.iter().any(|(o, _, _)| o == id));
        for (id, text, placeholder) in open {
            let (input, shown, synced) = match self.fields.get(&id) {
                Some(f) => (f.input.clone(), f.placeholder.clone(), f.synced.clone()),
                None => {
                    let input = cx.new(|cx| InputState::new(window, cx));
                    let field = id.clone();
                    cx.subscribe_in(&input, window, move |this: &mut Self, input, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            let value = input.read(cx).value().to_string();
                            if let Some(f) = this.fields.get_mut(&field) {
                                f.synced = value.clone();
                            }
                            this.field_edited(&field, value);
                            cx.notify();
                        }
                    })
                    .detach();
                    (input, String::new(), String::new())
                }
            };
            if shown != placeholder {
                input.update(cx, |state, cx| state.set_placeholder(placeholder.clone(), window, cx));
            }
            let pushed = text != synced;
            self.fields.insert(id, Field { input: input.clone(), placeholder, synced: text.clone() });
            if pushed && input.read(cx).value() != text.as_str() {
                // Text goes in by `insert`, which leaves the caret after it.
                input.update(cx, |state, cx| {
                    state.set_value("", window, cx);
                    state.insert(text, window, cx);
                });
            }
        }
    }

    /// Each open field and the text the client holds for it, keyboard holder first, for `/state`.
    pub fn fields_json(&self) -> serde_json::Value {
        serde_json::Value::Array(self.open_fields().into_iter().map(|(id, text, _)| serde_json::json!({"field": format!("{id:?}"), "text": text})).collect())
    }

    /// The input of the field that holds the keyboard, when a field is open.
    pub fn keyboard_field(&self) -> Option<gpui::Entity<InputState>> {
        self.open_fields().first().and_then(|(id, _, _)| self.fields.get(id)).map(|f| f.input.clone())
    }

    /// Whether a field is open: its input takes the keys that edit text (`edits_text`).
    pub fn typing_in_field(&self) -> bool {
        !self.open_fields().is_empty()
    }

    /// Copy `input`'s text into the client before a key is handled, when its change event has not
    /// arrived yet: keys sent in a row reach the client ahead of the events, so Return would
    /// otherwise commit the text from before the letters typed just ahead of it.
    pub fn catch_up_field(&mut self, input: &gpui::Entity<InputState>, cx: &App) {
        let value = input.read(cx).value().to_string();
        let Some((id, f)) = self.fields.iter_mut().find(|(_, f)| &f.input == input) else { return };
        if f.synced == value {
            return;
        }
        f.synced = value.clone();
        let id = id.clone();
        self.field_edited(&id, value);
    }

    /// Whether `input` is one of the fields' inputs.
    pub fn is_field_input(&self, input: &gpui::Entity<InputState>) -> bool {
        self.fields.values().any(|f| &f.input == input)
    }

    /// Field `id` drawn as its input; `text` alone on the frame before `sync_fields` made it.
    pub(super) fn field_element(&self, id: &FieldId, text: &str) -> gpui::AnyElement {
        match self.fields.get(id) {
            Some(f) => Input::new(&f.input).appearance(false).p_0().into_any_element(),
            None => div().child(text.to_owned()).into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> gpui::Keystroke {
        gpui::Keystroke::parse(s).unwrap()
    }

    #[test]
    fn the_input_takes_editing_keys_and_the_field_keeps_its_own() {
        for k in ["a", "shift-a", "space", "backspace", "delete", "left", "right", "alt-left", "cmd-v", "cmd-c", "cmd-x", "cmd-a", "cmd-z", "cmd-backspace"] {
            assert!(edits_text(&key(k)), "{k} edits text");
        }
        for k in ["enter", "escape", "up", "down", "tab", "cmd-w", "cmd-k", "ctrl-c", "cmd-enter"] {
            assert!(!edits_text(&key(k)), "{k} is the field's or the window's");
        }
    }
}
