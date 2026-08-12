use std::path::PathBuf;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc::unbounded_channel;

use super::ChatComposer;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::key_hint;
use crate::keymap::KeymapContext;
use crate::keymap::RuntimeKeymap;

fn new_composer() -> ChatComposer {
    let (tx, _rx) = unbounded_channel::<AppEvent>();
    ChatComposer::new(
        /*has_input_focus*/ true,
        AppEventSender::new(tx),
        /*enhanced_keys_supported*/ false,
        "Ask Codex to do anything".to_string(),
        /*disable_paste_burst*/ false,
    )
}

fn press_ctrl_s(composer: &mut ChatComposer) -> bool {
    composer
        .handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .1
}

#[test]
fn ctrl_s_stashes_and_restores_the_complete_draft() {
    let mut composer = new_composer();
    composer.set_text_content(
        "draft [Pasted Content 5 chars]".to_string(),
        Vec::new(),
        vec![PathBuf::from("/tmp/local.png")],
    );
    composer.set_remote_image_urls(vec!["https://example.com/remote.png".to_string()]);
    composer.set_pending_pastes(vec![(
        "[Pasted Content 5 chars]".to_string(),
        "paste".to_string(),
    )]);
    let original = composer.draft_snapshot();

    assert!(press_ctrl_s(&mut composer));
    assert!(composer.is_empty());
    assert!(composer.stashed_draft.is_some());

    assert!(press_ctrl_s(&mut composer));
    assert_eq!(composer.draft_snapshot(), original);
    assert!(composer.stashed_draft.is_none());
}

#[test]
fn ctrl_s_swaps_a_new_draft_with_the_stashed_draft() {
    let mut composer = new_composer();
    composer.set_text_content("first".to_string(), Vec::new(), Vec::new());
    assert!(press_ctrl_s(&mut composer));

    composer.set_text_content("second".to_string(), Vec::new(), Vec::new());
    assert!(press_ctrl_s(&mut composer));
    assert_eq!(composer.current_text(), "first");

    assert!(press_ctrl_s(&mut composer));
    assert_eq!(composer.current_text(), "second");
}

#[test]
fn ctrl_s_is_a_noop_without_a_current_or_stashed_draft() {
    let mut composer = new_composer();

    assert!(!press_ctrl_s(&mut composer));
    assert!(composer.is_empty());
    assert!(composer.stashed_draft.is_none());
}

#[test]
fn stash_uses_the_configured_history_forward_binding() {
    let mut composer = new_composer();
    let mut keymap = RuntimeKeymap::defaults();
    keymap.composer.history_search_next = vec![key_hint::plain(KeyCode::F(2))];
    composer.set_keymap_bindings(&keymap);
    composer.set_text_content("draft".to_string(), Vec::new(), Vec::new());

    press_ctrl_s(&mut composer);
    assert_eq!(composer.current_text(), "draft");
    assert!(composer.stashed_draft.is_none());
    let (_, redraw) = composer.handle_key_event(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
    assert!(redraw);
    assert!(composer.is_empty());
    assert_eq!(
        keymap
            .primary_hint(KeymapContext::Composer, "history_search_next")
            .map(crate::key_hint::ShortcutHint::display_label),
        Some("f2".to_string())
    );
}
