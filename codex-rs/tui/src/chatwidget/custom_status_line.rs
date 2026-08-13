//! User-command-backed status line support.

use super::status_surfaces::approval_mode_display;
use super::status_surfaces::five_hour_status_window;
use super::status_surfaces::permissions_display;
use super::status_surfaces::weekly_status_window;
use super::*;
use crate::workspace_command::WorkspaceCommand;
use serde::Serialize;

const CUSTOM_STATUS_LINE_REFRESH_INTERVAL: Duration = Duration::from_secs(/*secs*/ 1);
const CUSTOM_STATUS_LINE_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 2);
const CUSTOM_STATUS_LINE_OUTPUT_BYTES_CAP: usize = 16 * 1024;
const CUSTOM_STATUS_LINE_MAX_LINES: usize = 4;

#[derive(Default)]
pub(super) struct CustomStatusLineState {
    command: Option<String>,
    value: Option<Vec<Line<'static>>>,
    pending_request_id: Option<u64>,
    next_request_id: u64,
    last_requested_at: Option<Instant>,
}

impl CustomStatusLineState {
    fn sync_command(&mut self, command: &str) {
        if self.command.as_deref() == Some(command) {
            return;
        }
        self.command = Some(command.to_string());
        self.value = None;
        self.pending_request_id = None;
        self.last_requested_at = None;
    }

    fn clear(&mut self) {
        let next_request_id = self.next_request_id;
        *self = Self::default();
        self.next_request_id = next_request_id;
    }
}

#[derive(Serialize)]
struct CustomStatusLineInput {
    hook_event_name: &'static str,
    session_id: Option<String>,
    cwd: String,
    version: &'static str,
    model: CustomStatusLineModel,
    workspace: CustomStatusLineWorkspace,
    context_window: CustomStatusLineContextWindow,
    last_turn: CustomStatusLineTokenUsage,
    thinking: CustomStatusLineThinking,
    effort: CustomStatusLineEffort,
    rate_limits: CustomStatusLineRateLimits,
    agent_state: String,
    permissions: String,
    approval_mode: String,
}

#[derive(Serialize)]
struct CustomStatusLineModel {
    id: String,
    display_name: String,
}

#[derive(Serialize)]
struct CustomStatusLineWorkspace {
    current_dir: String,
}

#[derive(Serialize)]
struct CustomStatusLineContextWindow {
    used_percentage: i64,
    context_window_size: i64,
    total_input_tokens: i64,
    total_output_tokens: i64,
}

#[derive(Serialize)]
struct CustomStatusLineTokenUsage {
    input_tokens: i64,
    cached_input_tokens: i64,
    output_tokens: i64,
    reasoning_output_tokens: i64,
}

#[derive(Serialize)]
struct CustomStatusLineThinking {
    enabled: bool,
}

#[derive(Serialize)]
struct CustomStatusLineEffort {
    level: String,
}

#[derive(Serialize)]
struct CustomStatusLineRateLimits {
    five_hour: CustomStatusLineRateLimitWindow,
    seven_day: CustomStatusLineRateLimitWindow,
}

#[derive(Serialize)]
struct CustomStatusLineRateLimitWindow {
    used_percentage: f64,
    resets_at: Option<i64>,
}

impl ChatWidget {
    pub(super) fn custom_status_line_enabled(&self) -> bool {
        self.configured_custom_status_line_command().is_some()
    }

    fn configured_custom_status_line_command(&self) -> Option<&str> {
        self.config
            .tui_status_line_command
            .as_deref()
            .map(str::trim)
            .filter(|command| !command.is_empty())
    }

    fn custom_status_line_input(&self) -> CustomStatusLineInput {
        let cwd = self.status_line_cwd().display().to_string();
        let usage = self.status_line_total_usage();
        let last_turn = self
            .token_info
            .as_ref()
            .map(|usage| usage.last_token_usage.clone())
            .unwrap_or_default();
        let reasoning_effort = self
            .effective_reasoning_effort()
            .or_else(|| self.config.model_reasoning_effort.clone());
        let rate_limits = self.rate_limit_snapshots_by_limit_id.get("codex");
        let five_hour = rate_limits
            .and_then(five_hour_status_window)
            .map(|(window, _is_secondary)| window);
        let seven_day = rate_limits
            .and_then(weekly_status_window)
            .map(|(window, _is_secondary)| window);

        CustomStatusLineInput {
            hook_event_name: "Status",
            session_id: self.thread_id.map(|id| id.to_string()),
            cwd: cwd.clone(),
            version: CODEX_CLI_VERSION,
            model: CustomStatusLineModel {
                id: self.model_display_name().to_string(),
                display_name: self.model_display_name().to_string(),
            },
            workspace: CustomStatusLineWorkspace { current_dir: cwd },
            context_window: CustomStatusLineContextWindow {
                used_percentage: self.status_line_context_used_percent().unwrap_or(0),
                context_window_size: self.status_line_context_window_size().unwrap_or(0),
                total_input_tokens: usage.input_tokens,
                total_output_tokens: usage.output_tokens,
            },
            last_turn: CustomStatusLineTokenUsage {
                input_tokens: last_turn.input_tokens,
                cached_input_tokens: last_turn.cached_input_tokens,
                output_tokens: last_turn.output_tokens,
                reasoning_output_tokens: last_turn.reasoning_output_tokens,
            },
            thinking: CustomStatusLineThinking {
                enabled: reasoning_effort.is_some(),
            },
            effort: CustomStatusLineEffort {
                level: reasoning_effort
                    .as_ref()
                    .map(|effort| effort.as_str().to_string())
                    .unwrap_or_else(|| "none".to_string()),
            },
            rate_limits: CustomStatusLineRateLimits {
                five_hour: custom_status_line_rate_limit_window(five_hour),
                seven_day: custom_status_line_rate_limit_window(seven_day),
            },
            agent_state: self.run_state_status_text(),
            permissions: permissions_display(&self.config),
            approval_mode: approval_mode_display(&self.config),
        }
    }

    pub(super) fn refresh_custom_status_line_if_due(&mut self) {
        let Some(command) = self
            .configured_custom_status_line_command()
            .map(str::to_string)
        else {
            self.custom_status_line_state.clear();
            return;
        };
        self.custom_status_line_state.sync_command(&command);

        let Some(runner) = self.workspace_command_runner.clone() else {
            return;
        };
        if self.custom_status_line_state.pending_request_id.is_some() {
            return;
        }

        let input = match serde_json::to_string(&self.custom_status_line_input()) {
            Ok(input) => input,
            Err(error) => {
                tracing::debug!(%error, "failed to serialize custom status line input");
                return;
            }
        };
        let now = Instant::now();
        let refresh_due =
            self.custom_status_line_state
                .last_requested_at
                .is_none_or(|last_requested_at| {
                    now.saturating_duration_since(last_requested_at)
                        >= CUSTOM_STATUS_LINE_REFRESH_INTERVAL
                });
        if !refresh_due {
            self.frame_requester
                .schedule_frame_in(CUSTOM_STATUS_LINE_REFRESH_INTERVAL);
            return;
        }

        let request_id = self.custom_status_line_state.next_request_id;
        self.custom_status_line_state.next_request_id = self
            .custom_status_line_state
            .next_request_id
            .wrapping_add(/*rhs*/ 1);
        self.custom_status_line_state.pending_request_id = Some(request_id);
        self.custom_status_line_state.last_requested_at = Some(now);

        let cwd = self.status_line_cwd().to_path_buf();
        let tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let result = run_custom_status_line_command(runner.as_ref(), command, input, cwd).await;
            tx.send(AppEvent::CustomStatusLineUpdated { request_id, result });
        });
    }

    pub(crate) fn set_custom_status_line_output(
        &mut self,
        request_id: u64,
        result: Result<String, String>,
    ) -> bool {
        if self.custom_status_line_state.pending_request_id != Some(request_id) {
            return false;
        }
        self.custom_status_line_state.pending_request_id = None;
        match result {
            Ok(output) => {
                self.custom_status_line_state.value = parse_custom_status_line_output(&output);
            }
            Err(error) => {
                tracing::debug!(%error, "custom status line command failed");
            }
        }
        let value = self.custom_status_line_state.value.clone();
        self.set_status_line(value);
        self.frame_requester
            .schedule_frame_in(CUSTOM_STATUS_LINE_REFRESH_INTERVAL);
        true
    }

    pub(super) fn custom_status_line_value(&self) -> Option<Vec<Line<'static>>> {
        self.custom_status_line_state.value.clone()
    }
}

fn custom_status_line_rate_limit_window(
    window: Option<&RateLimitWindowDisplay>,
) -> CustomStatusLineRateLimitWindow {
    CustomStatusLineRateLimitWindow {
        used_percentage: window.map_or(0.0, |window| window.used_percent),
        resets_at: window.and_then(|window| window.resets_at_epoch),
    }
}

fn parse_custom_status_line_output(output: &str) -> Option<Vec<Line<'static>>> {
    let output = output.trim_end_matches(['\r', '\n']);
    if output.is_empty() {
        return None;
    }
    let mut lines = codex_ansi_escape::ansi_escape(output).lines;
    lines.truncate(CUSTOM_STATUS_LINE_MAX_LINES);
    (!lines.is_empty()).then_some(lines)
}

async fn run_custom_status_line_command(
    runner: &dyn crate::workspace_command::WorkspaceCommandExecutor,
    command: String,
    input: String,
    cwd: PathBuf,
) -> Result<String, String> {
    let output = runner
        .run(custom_status_line_workspace_command(command, input, cwd))
        .await
        .map_err(|error| error.to_string())?;
    if output.success() {
        return Ok(output.stdout);
    }

    let stderr = output.stderr.trim();
    if stderr.is_empty() {
        Err(format!(
            "custom status line command exited with status {}",
            output.exit_code
        ))
    } else {
        Err(format!(
            "custom status line command exited with status {}: {stderr}",
            output.exit_code
        ))
    }
}

#[cfg(not(target_os = "windows"))]
fn custom_status_line_workspace_command(
    command: String,
    input: String,
    cwd: PathBuf,
) -> WorkspaceCommand {
    let mut request = WorkspaceCommand::new([
        "sh",
        "-c",
        "printf '%s' \"$CODEX_STATUS_INPUT\" | sh -lc \"$CODEX_STATUS_COMMAND\"",
    ])
    .cwd(cwd)
    .env("CODEX_STATUS_COMMAND", command)
    .env("CODEX_STATUS_INPUT", input)
    .timeout(CUSTOM_STATUS_LINE_TIMEOUT);
    request.output_bytes_cap = CUSTOM_STATUS_LINE_OUTPUT_BYTES_CAP;
    request
}

#[cfg(target_os = "windows")]
fn custom_status_line_workspace_command(
    command: String,
    input: String,
    cwd: PathBuf,
) -> WorkspaceCommand {
    let mut request = WorkspaceCommand::new([
        "powershell.exe",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "$env:CODEX_STATUS_INPUT | Invoke-Expression $env:CODEX_STATUS_COMMAND",
    ])
    .cwd(cwd)
    .env("CODEX_STATUS_COMMAND", command)
    .env("CODEX_STATUS_INPUT", input)
    .timeout(CUSTOM_STATUS_LINE_TIMEOUT);
    request.output_bytes_cap = CUSTOM_STATUS_LINE_OUTPUT_BYTES_CAP;
    request
}

#[cfg(test)]
#[path = "custom_status_line_tests.rs"]
mod tests;
