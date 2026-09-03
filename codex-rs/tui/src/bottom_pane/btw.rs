//! Inline, non-persistent questions about the current conversation.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Instant;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::key_hint;
use crate::key_hint::has_ctrl_or_alt;
use crate::keymap::KeymapContextSet;
use crate::render::renderable::Renderable;
use crate::wrapping::word_wrap_lines;
use codex_protocol::ThreadId;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::StatefulWidgetRef;
use ratatui::widgets::Widget;
use uuid::Uuid;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use super::bottom_pane_view::ViewCompletion;
use super::paste_burst::PasteBurst;
use super::popup_consts::accept_cancel_hint_line;
use super::textarea::TextArea;
use super::textarea::TextAreaState;

const BTW_VIEW_ID: &str = "btw";
const MAX_EXCHANGES: usize = 20;
const MAX_RENDERED_HISTORY_LINES: usize = 14;

struct BtwExchange {
    question: String,
    answer: Result<String, String>,
}

struct PendingBtwRequest {
    request_id: Uuid,
    question: String,
}

/// A small inline chat surface that never changes the active thread.
pub(crate) struct BtwView {
    parent_thread_id: ThreadId,
    app_event_tx: AppEventSender,
    textarea: TextArea,
    textarea_state: RefCell<TextAreaState>,
    paste_burst: PasteBurst,
    exchanges: VecDeque<BtwExchange>,
    pending: Option<PendingBtwRequest>,
    completion: Option<ViewCompletion>,
}

impl BtwView {
    pub(crate) fn new(
        parent_thread_id: ThreadId,
        initial_text: String,
        app_event_tx: AppEventSender,
    ) -> Self {
        let mut textarea = TextArea::new();
        if !initial_text.is_empty() {
            textarea.set_text_clearing_elements(&initial_text);
            textarea.set_cursor(initial_text.len());
        }

        Self {
            parent_thread_id,
            app_event_tx,
            textarea,
            textarea_state: RefCell::new(TextAreaState::default()),
            paste_burst: PasteBurst::default(),
            exchanges: VecDeque::new(),
            pending: None,
            completion: None,
        }
    }

    pub(crate) fn set_keymap_bindings(&mut self, keymap: &crate::keymap::RuntimeKeymap) {
        self.textarea.set_keymap_bindings(keymap);
    }

    pub(crate) fn enable_vim_in_insert_mode(&mut self) {
        self.textarea.set_vim_enabled(/*enabled*/ true);
        self.textarea.enter_vim_insert_mode();
    }

    pub(crate) fn submit_initial_question(&mut self) {
        self.submit();
    }

    fn submit(&mut self) {
        if self.pending.is_some() {
            return;
        }

        let question = self.textarea.text().trim().to_string();
        if question.is_empty() {
            return;
        }

        let request_id = Uuid::new_v4();
        self.textarea.set_text_clearing_elements("");
        self.textarea.set_cursor(0);
        self.pending = Some(PendingBtwRequest {
            request_id,
            question: question.clone(),
        });
        self.app_event_tx.send(AppEvent::StartBtw {
            parent_thread_id: self.parent_thread_id,
            request_id,
            question,
        });
    }

    fn input_height(&self, width: u16) -> u16 {
        let usable_width = width.saturating_sub(2);
        self.textarea.desired_height(usable_width).clamp(1, 6) + 1
    }

    fn history_lines(&self, width: u16) -> Vec<Line<'static>> {
        let width = usize::from(width.max(1));
        let mut lines = Vec::new();
        for exchange in &self.exchanges {
            let question = format!("you: {}", exchange.question);
            lines.extend(word_wrap_lines([question], width));
            match &exchange.answer {
                Ok(answer) => {
                    let answer = format!("btw: {answer}");
                    lines.extend(word_wrap_lines([answer], width));
                }
                Err(error) => {
                    let error = format!("btw error: {error}");
                    lines.extend(word_wrap_lines([error], width));
                }
            }
        }
        if self.pending.is_some() {
            lines.push(Line::from("btw: thinking…".dim()));
        }
        if lines.len() > MAX_RENDERED_HISTORY_LINES {
            lines.drain(..lines.len() - MAX_RENDERED_HISTORY_LINES);
        }
        lines
    }
}

impl BottomPaneView for BtwView {
    fn view_id(&self) -> Option<&'static str> {
        Some(BTW_VIEW_ID)
    }

    fn keymap_contexts(&self) -> KeymapContextSet {
        KeymapContextSet::new(self.textarea.keymap_context())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                if self.textarea.should_handle_vim_insert_escape(key_event) {
                    self.textarea.input(key_event);
                    self.paste_burst.clear_after_explicit_paste();
                } else {
                    self.on_ctrl_c();
                }
            }
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => self.submit(),
            KeyEvent {
                code: KeyCode::Char(_),
                modifiers,
                ..
            } if !has_ctrl_or_alt(modifiers) && self.textarea.allows_paste_burst() => {
                let now = Instant::now();
                let paste_like_burst = self.paste_burst.on_plain_char_no_hold(now).is_some();
                self.textarea.input(key_event);
                if paste_like_burst {
                    self.paste_burst.extend_window(now);
                }
            }
            KeyEvent {
                code: KeyCode::Tab,
                modifiers,
                ..
            } if !has_ctrl_or_alt(modifiers) && self.textarea.allows_paste_burst() => {
                let now = Instant::now();
                let in_paste_burst = self.paste_burst.direct_insert_newline_should_insert(now);
                self.textarea.input(key_event);
                if in_paste_burst {
                    self.paste_burst.extend_window(now);
                }
            }
            other => {
                self.textarea.input(other);
                self.paste_burst.clear_after_explicit_paste();
            }
        }
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.completion = Some(ViewCompletion::Cancelled);
        CancellationEvent::Handled
    }

    fn prefer_esc_to_handle_key_event(&self) -> bool {
        self.textarea.uses_vim_insert_cursor() || self.textarea.is_vim_operator_pending()
    }

    fn is_complete(&self) -> bool {
        self.completion.is_some()
    }

    fn completion(&self) -> Option<ViewCompletion> {
        self.completion
    }

    fn handle_paste(&mut self, pasted: String) -> bool {
        if pasted.is_empty() {
            return false;
        }
        self.textarea.insert_str(&pasted);
        self.paste_burst.clear_after_explicit_paste();
        true
    }

    fn apply_btw_response(&mut self, request_id: Uuid, result: Result<String, String>) -> bool {
        let Some(pending) = self
            .pending
            .take_if(|pending| pending.request_id == request_id)
        else {
            return false;
        };

        self.exchanges.push_back(BtwExchange {
            question: pending.question,
            answer: result,
        });
        while self.exchanges.len() > MAX_EXCHANGES {
            self.exchanges.pop_front();
        }
        true
    }
}

impl Renderable for BtwView {
    fn desired_height(&self, width: u16) -> u16 {
        let history_height = self.history_lines(width).len() as u16;
        1 + history_height + self.input_height(width) + 2
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        if area.height < 2 || area.width <= 2 {
            return None;
        }
        let history_height = self.history_lines(area.width).len() as u16;
        let input_height = self.input_height(area.width);
        let text_area_height = input_height.saturating_sub(1);
        if text_area_height == 0 {
            return None;
        }
        let textarea_rect = Rect {
            x: area.x.saturating_add(2),
            y: area
                .y
                .saturating_add(1)
                .saturating_add(history_height)
                .saturating_add(1),
            width: area.width.saturating_sub(2),
            height: text_area_height,
        };
        let state = *self.textarea_state.borrow();
        self.textarea.cursor_pos_with_state(textarea_rect, state)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let title = Line::from(vec!["▌ ".cyan(), "btw".bold()]);
        Paragraph::new(title).render(
            Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: 1,
            },
            buf,
        );

        let history = self.history_lines(area.width);
        for (offset, line) in history.iter().enumerate() {
            Paragraph::new(line.clone()).render(
                Rect {
                    x: area.x.saturating_add(2),
                    y: area.y.saturating_add(1 + offset as u16),
                    width: area.width.saturating_sub(2),
                    height: 1,
                },
                buf,
            );
        }

        let input_height = self.input_height(area.width);
        let input_y = area
            .y
            .saturating_add(1)
            .saturating_add(history.len() as u16);
        let input_area = Rect {
            x: area.x,
            y: input_y,
            width: area.width,
            height: input_height,
        };
        if input_area.width >= 2 {
            for row in 0..input_area.height {
                Paragraph::new(Line::from(vec!["▌ ".cyan()])).render(
                    Rect {
                        x: input_area.x,
                        y: input_area.y.saturating_add(row),
                        width: 2,
                        height: 1,
                    },
                    buf,
                );
            }
            let text_area_height = input_area.height.saturating_sub(1);
            if text_area_height > 0 {
                let textarea_rect = Rect {
                    x: input_area.x.saturating_add(2),
                    y: input_area.y.saturating_add(1),
                    width: input_area.width.saturating_sub(2),
                    height: text_area_height,
                };
                Clear.render(textarea_rect, buf);
                let mut state = self.textarea_state.borrow_mut();
                StatefulWidgetRef::render_ref(&(&self.textarea), textarea_rect, buf, &mut state);
                if self.textarea.text().is_empty() {
                    Paragraph::new(Line::from("Ask about the current conversation".dim()))
                        .render(textarea_rect, buf);
                }
            }
        }

        let hint_y = input_y.saturating_add(input_height).saturating_add(1);
        if hint_y < area.y.saturating_add(area.height) {
            Paragraph::new(accept_cancel_hint_line(
                Some(key_hint::plain(KeyCode::Enter).into()),
                "to ask",
                Some(key_hint::plain(KeyCode::Esc).into()),
                "to close",
            ))
            .render(
                Rect {
                    x: area.x,
                    y: hint_y,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
        }
    }
}

#[cfg(test)]
#[path = "btw_tests.rs"]
mod tests;
