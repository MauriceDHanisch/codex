use super::*;
use ratatui::style::Color;

#[test]
fn parses_ansi_and_preserves_multiple_lines() {
    let lines = parse_custom_status_line_output("\u{1b}[31mfirst\u{1b}[0m\nsecond\n")
        .expect("status line output");

    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].to_string(), "first");
    assert_eq!(lines[0].spans[0].style.fg, Some(Color::Red));
    assert_eq!(lines[1].to_string(), "second");
}

#[test]
fn preserves_unicode_in_ansi_styled_output() {
    let lines = parse_custom_status_line_output("\u{1b}[36m◇ codex 🧠\u{1b}[0m")
        .expect("status line output");

    assert_eq!(lines[0].to_string(), "◇ codex 🧠");
}

#[test]
fn limits_output_to_four_lines() {
    let lines =
        parse_custom_status_line_output("one\ntwo\nthree\nfour\nfive").expect("status line output");

    assert_eq!(
        lines.iter().map(ToString::to_string).collect::<Vec<_>>(),
        vec!["one", "two", "three", "four"]
    );
}

#[test]
fn treats_empty_output_as_absent() {
    assert!(parse_custom_status_line_output("\n\r\n").is_none());
}

#[cfg(not(target_os = "windows"))]
#[test]
fn command_is_bounded_and_receives_json_through_environment() {
    let command = custom_status_line_workspace_command(
        "bash ~/.codex/statusline.sh".to_string(),
        "{\"model\":{}}".to_string(),
        PathBuf::from("/workspace"),
    );

    assert_eq!(command.timeout, CUSTOM_STATUS_LINE_TIMEOUT);
    assert_eq!(
        command.output_bytes_cap,
        CUSTOM_STATUS_LINE_OUTPUT_BYTES_CAP
    );
    assert_eq!(command.cwd, Some(PathBuf::from("/workspace")));
    assert_eq!(
        command.env.get("CODEX_STATUS_COMMAND"),
        Some(&Some("bash ~/.codex/statusline.sh".to_string()))
    );
    assert_eq!(
        command.env.get("CODEX_STATUS_INPUT"),
        Some(&Some("{\"model\":{}}".to_string()))
    );
}
