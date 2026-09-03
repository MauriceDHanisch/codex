use super::*;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crossterm::event::KeyCode;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc::unbounded_channel;

fn test_view(initial_text: &str) -> (BtwView, tokio::sync::mpsc::UnboundedReceiver<AppEvent>) {
    let (tx, rx) = unbounded_channel();
    let parent_thread_id = ThreadId::new();
    (
        BtwView::new(
            parent_thread_id,
            initial_text.to_string(),
            AppEventSender::new(tx),
        ),
        rx,
    )
}

#[test]
fn submitting_a_question_emits_a_btw_request_and_waits_for_response() {
    let (mut view, mut rx) = test_view("what changed?");

    view.handle_key_event(KeyEvent::from(KeyCode::Enter));

    let event = rx.try_recv().expect("expected Btw request");
    let (parent_thread_id, request_id, question) = match event {
        AppEvent::StartBtw {
            parent_thread_id,
            request_id,
            question,
        } => (parent_thread_id, request_id, question),
        other => panic!("expected StartBtw, got {other:?}"),
    };
    assert_eq!(question, "what changed?");
    assert!(!parent_thread_id.to_string().is_empty());
    assert!(!request_id.is_nil());
    assert_eq!(view.textarea.text(), "");
    assert!(view.pending.is_some());
}

#[test]
fn response_is_rendered_without_completing_the_inline_panel() {
    let (mut view, mut rx) = test_view("what changed?");
    view.handle_key_event(KeyEvent::from(KeyCode::Enter));
    let request_id = match rx.try_recv().expect("expected Btw request") {
        AppEvent::StartBtw { request_id, .. } => request_id,
        other => panic!("expected StartBtw, got {other:?}"),
    };

    assert!(view.apply_btw_response(request_id, Ok("Two files changed.".to_string())));
    assert!(!view.is_complete());
    assert!(view.pending.is_none());

    let area = Rect::new(0, 0, 60, view.desired_height(/*width*/ 60));
    let mut buffer = Buffer::empty(area);
    view.render(area, &mut buffer);
    insta::assert_snapshot!(format!("{buffer:?}"));
}

#[test]
fn stale_response_does_not_change_the_panel() {
    let (mut view, _rx) = test_view("");

    assert!(!view.apply_btw_response(Uuid::new_v4(), Err("stale".to_string())));
    assert!(view.exchanges.is_empty());
}
