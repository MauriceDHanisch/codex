//! One-slot prompt stash for temporarily swapping composer drafts.

use crossterm::event::KeyEvent;
use ratatui::text::Line;

use super::ActivePopup;
use super::ChatComposer;
use super::ComposerDraft;
use super::InputResult;
use super::reset_mode_after_activity;
use crate::app_event::AppEvent;
use crate::key_hint::KeyBinding;
use crate::key_hint::KeyBindingListExt;
use crate::keymap::user_bindings;
use crate::style::accent_style;

impl ChatComposer {
    pub(super) fn is_prompt_stash_key(&self, key_event: &KeyEvent) -> bool {
        self.history_search_next_keys.is_pressed(*key_event)
    }

    pub(super) fn toggle_prompt_stash(&mut self) -> (InputResult, bool) {
        if let Some(pasted) = self.draft.paste_burst.flush_before_modified_input() {
            self.handle_paste(pasted);
        }
        self.draft.paste_burst.clear_window_after_non_char();

        if self.stashed_draft.is_none() && self.is_empty() {
            return (InputResult::None, false);
        }

        if self.popups.current_file_query.is_some() {
            self.app_event_tx
                .send(AppEvent::StartFileSearch(String::new()));
            self.popups.current_file_query = None;
        }
        self.popups.active = ActivePopup::None;
        self.attachments.clear_remote_image_selection();
        self.history.reset_navigation();
        self.footer.mode = reset_mode_after_activity(self.footer.mode);

        let current_draft = self.snapshot_draft();
        if let Some(stashed_draft) = self.stashed_draft.take() {
            if !self.is_empty() {
                self.stashed_draft = Some(current_draft);
            }
            self.restore_draft(stashed_draft);
        } else {
            self.stashed_draft = Some(current_draft);
            self.restore_draft(empty_draft());
        }

        (InputResult::None, true)
    }

    pub(super) fn prompt_stash_placeholder(&self) -> Option<String> {
        self.stashed_draft.as_ref()?;
        let shortcut = user_bindings(&self.history_search_next_keys)
            .first()
            .map_or_else(
                || "the stash shortcut".to_string(),
                KeyBinding::display_label,
            );
        Some(format!("Prompt stashed. Press {shortcut} to restore."))
    }

    pub(super) fn prompt_stash_indicator_line(&self) -> Option<Line<'static>> {
        self.stashed_draft.as_ref()?;
        Some(Line::styled("● 1 prompt stashed", accent_style()))
    }
}

fn empty_draft() -> ComposerDraft {
    ComposerDraft {
        text: String::new(),
        text_elements: Vec::new(),
        local_image_paths: Vec::new(),
        remote_image_urls: Vec::new(),
        mention_bindings: Vec::new(),
        pending_pastes: Vec::new(),
        cursor: 0,
    }
}
