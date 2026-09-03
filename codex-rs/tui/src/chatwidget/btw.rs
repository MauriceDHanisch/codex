use super::*;

impl ChatWidget {
    pub(crate) fn open_btw_prompt(&mut self, parent_thread_id: ThreadId, initial_text: String) {
        let submit_initial_question = !initial_text.trim().is_empty();
        let mut view = BtwView::new(parent_thread_id, initial_text, self.app_event_tx.clone());
        if submit_initial_question {
            view.submit_initial_question();
        }
        self.bottom_pane.show_btw_view(view);
    }

    pub(crate) fn apply_btw_response(
        &mut self,
        request_id: uuid::Uuid,
        result: Result<String, String>,
    ) {
        if self.bottom_pane.apply_btw_response(request_id, result) {
            self.request_redraw();
        }
    }
}
