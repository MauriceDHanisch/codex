//! Non-command tool lifecycle rendering for `ChatWidget`.
//!
//! This module handles patch, MCP, web search, image, and collaborator tool
//! events as transcript cells.

use super::*;
use codex_utils_path_uri::LegacyAppPathString;

impl ChatWidget {
    pub(crate) fn session_file_changes(
        &self,
    ) -> (HashMap<PathBuf, Arc<SessionFileChange>>, AbsolutePathBuf) {
        (
            self.session_file_changes
                .iter()
                .filter_map(|(path, change)| {
                    let change = if change.baseline_is_inferred() {
                        change.resolve_inferred_baseline()
                    } else {
                        change.as_ref().clone()
                    };
                    change
                        .is_changed()
                        .then(|| (path.clone(), Arc::new(change)))
                })
                .collect(),
            self.config.cwd.clone(),
        )
    }

    pub(crate) fn clear_session_file_changes(&mut self) {
        self.session_file_changes.clear();
    }

    pub(super) fn on_patch_apply_begin(&mut self, changes: HashMap<PathBuf, FileChange>) {
        for (path, change) in &changes {
            let session_path = session_change_key(&self.session_file_changes, path);
            self.session_file_changes
                .entry(session_path)
                .or_insert_with(|| {
                    Arc::new(SessionFileChange::new(
                        read_session_file(&self.config.cwd, path),
                        change.clone(),
                    ))
                });
        }
        self.add_to_history(history_cell::new_patch_event(changes, &self.config.cwd));
    }

    pub(super) fn on_view_image_tool_call(&mut self, path: LegacyAppPathString) {
        self.flush_answer_stream_with_separator();
        self.add_to_history(history_cell::new_view_image_tool_call(
            path,
            &self.config.cwd,
        ));
        self.request_redraw();
    }

    pub(super) fn on_image_generation_begin(&mut self) {
        self.flush_answer_stream_with_separator();
        if self.bottom_pane.is_task_running() {
            self.bottom_pane.ensure_status_indicator();
        }
    }

    pub(super) fn on_image_generation_end(
        &mut self,
        call_id: String,
        status: String,
        revised_prompt: Option<String>,
        saved_path: Option<AbsolutePathBuf>,
    ) {
        self.flush_answer_stream_with_separator();
        self.add_to_history(history_cell::new_image_generation_call(
            call_id,
            &status,
            revised_prompt,
            saved_path,
        ));
        self.request_redraw();
    }

    pub(super) fn on_file_change_completed(&mut self, item: ThreadItem) {
        self.defer_or_handle(
            item,
            InterruptManager::push_item_completed,
            Self::handle_file_change_completed_now,
        );
    }

    pub(super) fn on_mcp_tool_call_started(&mut self, item: ThreadItem) {
        self.defer_or_handle(
            item,
            InterruptManager::push_item_started,
            Self::handle_mcp_tool_call_started_now,
        );
    }

    pub(super) fn on_mcp_tool_call_completed(&mut self, item: ThreadItem) {
        self.defer_or_handle(
            item,
            InterruptManager::push_item_completed,
            Self::handle_mcp_tool_call_completed_now,
        );
    }

    pub(super) fn on_web_search_begin(&mut self, call_id: String) {
        self.flush_answer_stream_with_separator();
        self.flush_active_cell();
        self.transcript.active_cell = Some(Box::new(history_cell::new_active_web_search_call(
            call_id,
            String::new(),
            self.config.animations,
        )));
        self.bump_active_cell_revision();
        self.request_redraw();
    }

    pub(super) fn on_web_search_end(
        &mut self,
        call_id: String,
        query: String,
        action: codex_app_server_protocol::WebSearchAction,
    ) {
        self.flush_answer_stream_with_separator();
        let mut handled = false;
        if let Some(cell) = self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<WebSearchCell>())
            && cell.call_id() == call_id
        {
            cell.update(action.clone(), query.clone());
            cell.complete();
            self.bump_active_cell_revision();
            self.flush_active_cell();
            handled = true;
        }

        if !handled {
            self.add_to_history(history_cell::new_web_search_call(call_id, query, action));
        }
        self.transcript.had_work_activity = true;
    }

    pub(super) fn on_collab_event(&mut self, cell: PlainHistoryCell) {
        self.flush_answer_stream_with_separator();
        self.add_to_history(cell);
        self.request_redraw();
    }

    pub(super) fn on_collab_agent_tool_call(&mut self, item: ThreadItem) {
        let ThreadItem::CollabAgentToolCall {
            id, tool, status, ..
        } = &item
        else {
            return;
        };
        if matches!(tool, CollabAgentTool::SpawnAgent)
            && let Some(spawn_request) = multi_agents::spawn_request_summary(&item)
        {
            self.pending_collab_spawn_requests
                .insert(id.clone(), spawn_request);
        }

        let cached_spawn_request = if matches!(tool, CollabAgentTool::SpawnAgent)
            && !matches!(status, CollabAgentToolCallStatus::InProgress)
        {
            self.pending_collab_spawn_requests.remove(id)
        } else {
            None
        };

        if let Some(cell) = multi_agents::tool_call_history_cell(
            &item,
            cached_spawn_request.as_ref(),
            |thread_id| self.collab_agent_metadata(thread_id),
        ) {
            self.on_collab_event(cell);
        }
    }

    pub(super) fn on_sub_agent_activity(&mut self, item: ThreadItem) {
        if let Some(cell) = multi_agents::sub_agent_activity_history_cell(&item) {
            self.on_collab_event(cell);
        }
    }

    pub(crate) fn handle_file_change_completed_now(&mut self, item: ThreadItem) {
        let ThreadItem::FileChange {
            changes, status, ..
        } = item
        else {
            return;
        };
        // If the patch was successful, just let the "Edited" block stand.
        // Otherwise, add a failure block.
        if matches!(status, codex_app_server_protocol::PatchApplyStatus::Failed) {
            self.add_to_history(history_cell::new_patch_apply_failure(String::new()));
        }
        if !matches!(
            status,
            codex_app_server_protocol::PatchApplyStatus::Completed
        ) {
            for path in file_update_changes_to_display(changes.clone()).into_keys() {
                let session_path = session_change_key(&self.session_file_changes, &path);
                if self
                    .session_file_changes
                    .get(&session_path)
                    .is_some_and(|change| !change.is_completed())
                {
                    self.session_file_changes.remove(&session_path);
                }
            }
        }
        if matches!(
            status,
            codex_app_server_protocol::PatchApplyStatus::Completed
        ) {
            for (path, change) in file_update_changes_to_display(changes) {
                let session_path = session_change_key(&self.session_file_changes, &path);
                let current_path = match &change {
                    FileChange::Update {
                        move_path: Some(move_path),
                        ..
                    } => move_path,
                    _ => &path,
                };
                let current_content = read_session_file(&self.config.cwd, current_path);
                let session_change = self
                    .session_file_changes
                    .get(&session_path)
                    .map(|existing| existing.complete(current_content.clone(), change.clone()))
                    .unwrap_or_else(|| SessionFileChange::from_completed(current_content, change));
                if session_change.is_changed() {
                    self.session_file_changes
                        .insert(session_path, Arc::new(session_change));
                } else {
                    self.session_file_changes.remove(&session_path);
                }
            }
        }
        // Mark that actual work was done (patch applied)
        self.transcript.had_work_activity = true;
    }

    pub(crate) fn handle_mcp_tool_call_started_now(&mut self, item: ThreadItem) {
        let ThreadItem::McpToolCall {
            id,
            server,
            tool,
            arguments,
            ..
        } = item
        else {
            return;
        };
        self.flush_answer_stream_with_separator();
        self.flush_active_cell();
        self.transcript.active_cell = Some(Box::new(history_cell::new_active_mcp_tool_call(
            id,
            McpInvocation {
                server,
                tool,
                arguments: Some(arguments),
            },
            self.config.animations,
        )));
        self.bump_active_cell_revision();
        self.request_redraw();
    }

    pub(crate) fn handle_mcp_tool_call_completed_now(&mut self, item: ThreadItem) {
        self.flush_answer_stream_with_separator();

        let ThreadItem::McpToolCall {
            id,
            server,
            tool,
            status,
            arguments,
            result,
            error,
            duration_ms,
            ..
        } = item
        else {
            return;
        };
        let invocation = McpInvocation {
            server,
            tool,
            arguments: Some(arguments),
        };
        let duration = Duration::from_millis(duration_ms.unwrap_or_default().max(0) as u64);
        let result = match (result, error) {
            (_, Some(error)) => Err(error.message),
            (Some(result), None) => {
                let result = *result;
                Ok(codex_protocol::mcp::CallToolResult {
                    content: result.content,
                    structured_content: result.structured_content,
                    is_error: Some(status == codex_app_server_protocol::McpToolCallStatus::Failed),
                    meta: None,
                })
            }
            (None, None) => Err("MCP tool call completed without a result".to_string()),
        };

        let extra_cell = match self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<McpToolCallCell>())
        {
            Some(cell) if cell.call_id() == id => cell.complete(duration, result),
            _ => {
                self.flush_active_cell();
                let mut cell =
                    history_cell::new_active_mcp_tool_call(id, invocation, self.config.animations);
                let extra_cell = cell.complete(duration, result);
                self.transcript.active_cell = Some(Box::new(cell));
                extra_cell
            }
        };

        self.flush_active_cell();
        if let Some(extra) = extra_cell {
            self.add_boxed_history(extra);
        }
        // Mark that actual work was done (MCP tool call)
        self.transcript.had_work_activity = true;
    }

    pub(crate) fn handle_queued_item_started_now(&mut self, item: ThreadItem) {
        match item {
            item @ ThreadItem::CommandExecution { .. } => {
                self.handle_command_execution_started_now(item);
            }
            item @ ThreadItem::McpToolCall { .. } => {
                self.handle_mcp_tool_call_started_now(item);
            }
            _ => {}
        }
    }

    pub(crate) fn handle_queued_item_completed_now(&mut self, item: ThreadItem) {
        match item {
            item @ ThreadItem::CommandExecution { .. } => {
                self.handle_command_execution_completed_now(item);
            }
            item @ ThreadItem::FileChange { .. } => self.handle_file_change_completed_now(item),
            item @ ThreadItem::McpToolCall { .. } => self.handle_mcp_tool_call_completed_now(item),
            _ => {}
        }
    }
}

fn read_session_file(cwd: &AbsolutePathBuf, path: &Path) -> Option<String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.as_path().join(path)
    };
    std::fs::read_to_string(path).ok()
}

fn session_change_key(changes: &HashMap<PathBuf, Arc<SessionFileChange>>, path: &Path) -> PathBuf {
    if changes.contains_key(path) {
        return path.to_path_buf();
    }
    changes
        .iter()
        .find_map(|(session_path, change)| change.moved_to(path).then(|| session_path.clone()))
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn later_edit_uses_the_original_session_key_after_a_move() {
        let source = PathBuf::from("source.txt");
        let destination = PathBuf::from("destination.txt");
        let change = FileChange::Update {
            unified_diff: String::new(),
            move_path: Some(destination.clone()),
        };
        let session_change = SessionFileChange::new(Some("same\n".to_string()), change.clone())
            .complete(Some("same\n".to_string()), change);
        let changes = HashMap::from([(source.clone(), Arc::new(session_change))]);

        assert_eq!(session_change_key(&changes, &destination), source);
    }
}
