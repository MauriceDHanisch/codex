//! Minimal file-change model used by TUI diff rendering and approval previews.

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;
use similar::TextDiff;

const SESSION_DIFF_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum FileChange {
    Add {
        content: String,
    },
    Delete {
        content: String,
    },
    Update {
        unified_diff: String,
        move_path: Option<PathBuf>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct SessionFileChange {
    original_content: Option<String>,
    current_content: Option<String>,
    fallback: FileChange,
    move_path: Option<PathBuf>,
    baseline_inferred: bool,
    inferred_changes: Vec<FileChange>,
    completed: bool,
    reconstruction_unavailable: bool,
}

impl SessionFileChange {
    pub(crate) fn new(original_content: Option<String>, fallback: FileChange) -> Self {
        let move_path = match &fallback {
            FileChange::Update { move_path, .. } => move_path.clone(),
            _ => None,
        };
        Self {
            current_content: original_content.clone(),
            original_content,
            fallback,
            move_path,
            baseline_inferred: false,
            inferred_changes: Vec::new(),
            completed: false,
            reconstruction_unavailable: false,
        }
    }

    pub(crate) fn from_completed(current_content: Option<String>, fallback: FileChange) -> Self {
        let inferred_changes = vec![fallback.clone()];
        let mut change = Self::new(None, fallback.clone());
        let current_content_missing = current_content.is_none();
        change.current_content =
            current_content.or_else(|| match content_after_change(None, &fallback) {
                ContentAfterChange::Applied(content) => content,
                ContentAfterChange::Unavailable => None,
            });
        change.baseline_inferred = true;
        change.inferred_changes = inferred_changes;
        change.completed = true;
        change.reconstruction_unavailable =
            current_content_missing && matches!(fallback, FileChange::Update { .. });
        change
    }

    pub(crate) fn complete(&self, observed_content: Option<String>, fallback: FileChange) -> Self {
        let move_path = match &fallback {
            FileChange::Update {
                move_path: Some(move_path),
                ..
            } => Some(move_path.clone()),
            FileChange::Update {
                move_path: None, ..
            } => self.move_path.clone(),
            FileChange::Add { .. } => None,
            FileChange::Delete { .. } => self.move_path.clone(),
        };
        let mut inferred_changes = self.inferred_changes.clone();
        if self.baseline_inferred {
            inferred_changes.push(fallback.clone());
        }
        let applied_content = content_after_change(self.current_content.as_deref(), &fallback);
        let current_content = if self.baseline_inferred {
            observed_content.or_else(|| match &applied_content {
                ContentAfterChange::Applied(content) => content.clone(),
                ContentAfterChange::Unavailable => None,
            })
        } else {
            match &applied_content {
                ContentAfterChange::Applied(content) => content.clone(),
                ContentAfterChange::Unavailable => observed_content,
            }
        };
        let reconstruction_unavailable = self.reconstruction_unavailable
            || (matches!(applied_content, ContentAfterChange::Unavailable)
                && current_content.is_none());
        Self {
            original_content: self.original_content.clone(),
            current_content,
            fallback,
            move_path,
            baseline_inferred: self.baseline_inferred,
            inferred_changes,
            completed: true,
            reconstruction_unavailable,
        }
    }

    pub(crate) fn resolve_inferred_baseline(&self) -> Self {
        if !self.baseline_inferred {
            return self.clone();
        }
        let mut resolved = self.clone();
        let original_content = content_before_changes(
            resolved.current_content.as_deref(),
            &resolved.inferred_changes,
        );
        resolved.reconstruction_unavailable = original_content.is_err();
        if let Ok(original_content) = original_content {
            resolved.original_content = original_content;
        }
        resolved.baseline_inferred = false;
        resolved.inferred_changes.clear();
        resolved
    }

    pub(crate) fn baseline_is_inferred(&self) -> bool {
        self.baseline_inferred
    }

    pub(crate) fn is_completed(&self) -> bool {
        self.completed
    }

    pub(crate) fn moved_to(&self, path: &std::path::Path) -> bool {
        self.move_path.as_deref() == Some(path)
    }

    pub(crate) fn reconstruction_unavailable(&self) -> bool {
        self.reconstruction_unavailable
    }

    pub(crate) fn is_changed(&self) -> bool {
        self.completed
            && (self.original_content != self.current_content
                || self.move_path.is_some()
                || self.reconstruction_unavailable)
    }

    pub(crate) fn display_change(&self, context: usize) -> FileChange {
        if self.reconstruction_unavailable {
            return self.fallback.clone();
        }
        match (&self.original_content, &self.current_content) {
            (None, Some(content)) => FileChange::Add {
                content: content.clone(),
            },
            (Some(content), None) => FileChange::Delete {
                content: content.clone(),
            },
            (Some(original), Some(current)) => {
                let mut config = TextDiff::configure();
                config.timeout(SESSION_DIFF_TIMEOUT);
                FileChange::Update {
                    unified_diff: config
                        .diff_lines(original, current)
                        .unified_diff()
                        .context_radius(context)
                        .to_string(),
                    move_path: self.move_path.clone(),
                }
            }
            (None, None) => self.fallback.clone(),
        }
    }

    pub(crate) fn adaptive_display_change(
        &self,
        max_context: usize,
        mut fits: impl FnMut(&FileChange) -> bool,
    ) -> FileChange {
        let (Some(original), Some(current)) = (&self.original_content, &self.current_content)
        else {
            return self.display_change(/*context*/ 0);
        };
        let mut config = TextDiff::configure();
        config.timeout(SESSION_DIFF_TIMEOUT);
        let diff = config.diff_lines(original, current);
        let make_change = |context| FileChange::Update {
            unified_diff: diff.unified_diff().context_radius(context).to_string(),
            move_path: self.move_path.clone(),
        };
        let zero_context = make_change(0);
        if max_context == 0 || !fits(&zero_context) {
            return zero_context;
        }
        let mut low = 0;
        let mut high = original
            .lines()
            .count()
            .max(current.lines().count())
            .min(max_context);
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            if fits(&make_change(middle)) {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        make_change(low)
    }
}

fn content_before_change(current: Option<&str>, change: &FileChange) -> Result<Option<String>, ()> {
    match change {
        FileChange::Add { .. } => Ok(None),
        FileChange::Delete { content } => Ok(Some(content.clone())),
        FileChange::Update { unified_diff, .. } => {
            let Some(current) = current else {
                return Err(());
            };
            if unified_diff.is_empty() {
                return Ok(Some(current.to_string()));
            }
            let patch = diffy::Patch::from_str(unified_diff).map_err(|_| ())?;
            diffy::apply(current, &patch.reverse())
                .map(Some)
                .map_err(|_| ())
        }
    }
}

fn content_before_changes(
    current: Option<&str>,
    changes: &[FileChange],
) -> Result<Option<String>, ()> {
    let mut content = current.map(str::to_string);
    for change in changes.iter().rev() {
        content = content_before_change(content.as_deref(), change)?;
    }
    Ok(content)
}

enum ContentAfterChange {
    Applied(Option<String>),
    Unavailable,
}

fn content_after_change(current: Option<&str>, change: &FileChange) -> ContentAfterChange {
    match change {
        FileChange::Add { content } => ContentAfterChange::Applied(Some(content.clone())),
        FileChange::Delete { .. } => ContentAfterChange::Applied(None),
        FileChange::Update { unified_diff, .. } => {
            let Some(current) = current else {
                return ContentAfterChange::Unavailable;
            };
            if unified_diff.is_empty() {
                return ContentAfterChange::Applied(Some(current.to_string()));
            }
            let Ok(patch) = diffy::Patch::from_str(unified_diff) else {
                return ContentAfterChange::Unavailable;
            };
            diffy::apply(current, &patch)
                .map(|content| ContentAfterChange::Applied(Some(content)))
                .unwrap_or(ContentAfterChange::Unavailable)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_change_keeps_the_original_across_multiple_updates() {
        let original = "one\ntwo\nthree\nfour\n";
        let first = "one\nTWO\nthree\nfour\n";
        let second = "one\nTWO\nthree\nFOUR\n";
        let first_update = FileChange::Update {
            unified_diff: TextDiff::from_lines(original, first)
                .unified_diff()
                .context_radius(3)
                .to_string(),
            move_path: None,
        };
        let second_update = FileChange::Update {
            unified_diff: TextDiff::from_lines(first, second)
                .unified_diff()
                .context_radius(3)
                .to_string(),
            move_path: None,
        };
        let change = SessionFileChange::new(Some(original.to_string()), first_update.clone())
            .complete(Some(first.to_string()), first_update)
            .complete(Some(second.to_string()), second_update);

        let FileChange::Update { unified_diff, .. } = change.display_change(4) else {
            panic!("expected an update");
        };
        assert!(unified_diff.contains("-two"));
        assert!(unified_diff.contains("+TWO"));
        assert!(unified_diff.contains("-four"));
        assert!(unified_diff.contains("+FOUR"));
    }

    #[test]
    fn adaptive_context_does_not_render_candidates_larger_than_the_viewport() {
        let original = (1..=6_000)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let current = original.replace("line 3000\n", "changed 3000\n");
        let fallback = FileChange::Update {
            unified_diff: TextDiff::from_lines(&original, &current)
                .unified_diff()
                .context_radius(3)
                .to_string(),
            move_path: None,
        };
        let change = SessionFileChange::new(Some(original), fallback.clone())
            .complete(Some(current), fallback);
        let mut largest_candidate = 0;

        change.adaptive_display_change(20, |candidate| {
            let FileChange::Update { unified_diff, .. } = candidate else {
                return false;
            };
            largest_candidate = largest_candidate.max(unified_diff.lines().count());
            unified_diff.lines().count() <= 15
        });

        assert!(largest_candidate <= 43);
    }

    #[test]
    fn pending_session_change_is_not_visible() {
        let change = SessionFileChange::new(
            None,
            FileChange::Add {
                content: "pending\n".to_string(),
            },
        );

        assert!(!change.is_changed());
    }

    #[test]
    fn added_then_deleted_file_is_not_visible() {
        let add = FileChange::Add {
            content: "temporary\n".to_string(),
        };
        let delete = FileChange::Delete {
            content: "temporary\n".to_string(),
        };
        let change = SessionFileChange::new(None, add)
            .complete(Some("temporary\n".to_string()), delete.clone())
            .complete(None, delete);

        assert!(!change.is_changed());
    }

    #[test]
    fn pure_move_is_visible() {
        let move_change = FileChange::Update {
            unified_diff: String::new(),
            move_path: Some(PathBuf::from("new.txt")),
        };
        let change = SessionFileChange::new(Some("same\n".to_string()), move_change.clone())
            .complete(Some("same\n".to_string()), move_change);

        assert!(change.is_changed());
    }

    #[test]
    fn missing_update_result_remains_visible_as_unavailable() {
        let update = FileChange::Update {
            unified_diff: TextDiff::from_lines("old\n", "new\n")
                .unified_diff()
                .context_radius(3)
                .to_string(),
            move_path: None,
        };
        let change = SessionFileChange::new(None, update.clone()).complete(None, update);

        assert!(change.reconstruction_unavailable());
        assert!(change.is_changed());
    }

    #[test]
    fn replayed_missing_update_remains_visible_as_unavailable() {
        let update = FileChange::Update {
            unified_diff: TextDiff::from_lines("old\n", "new\n")
                .unified_diff()
                .context_radius(3)
                .to_string(),
            move_path: None,
        };
        let change = SessionFileChange::from_completed(None, update);

        assert!(change.reconstruction_unavailable());
        assert!(change.is_changed());
    }

    #[test]
    fn replayed_add_then_update_reconstructs_an_absent_baseline() {
        let add = FileChange::Add {
            content: "initial\n".to_string(),
        };
        let update = FileChange::Update {
            unified_diff: TextDiff::from_lines("initial\n", "updated\n")
                .unified_diff()
                .context_radius(3)
                .to_string(),
            move_path: None,
        };
        let change = SessionFileChange::from_completed(Some("initial\n".to_string()), add)
            .complete(Some("updated\n".to_string()), update)
            .resolve_inferred_baseline();

        assert!(!change.reconstruction_unavailable());
        assert!(matches!(
            change.display_change(/*context*/ 3),
            FileChange::Add { .. }
        ));
    }

    #[test]
    fn replayed_add_then_delete_collapses_to_no_change() {
        let add = FileChange::Add {
            content: "temporary\n".to_string(),
        };
        let delete = FileChange::Delete {
            content: "temporary\n".to_string(),
        };
        let change = SessionFileChange::from_completed(None, add)
            .complete(None, delete)
            .resolve_inferred_baseline();

        assert!(!change.reconstruction_unavailable());
        assert!(!change.is_changed());
    }

    #[test]
    fn replayed_pure_move_reconstructs_without_a_patch_body() {
        let move_change = FileChange::Update {
            unified_diff: String::new(),
            move_path: Some(PathBuf::from("destination.txt")),
        };
        let change = SessionFileChange::from_completed(Some("same\n".to_string()), move_change)
            .resolve_inferred_baseline();

        assert!(!change.reconstruction_unavailable());
        assert!(change.moved_to(PathBuf::from("destination.txt").as_path()));
    }

    #[test]
    fn later_update_retains_the_move_destination() {
        let original = "one\ntwo\n";
        let moved = FileChange::Update {
            unified_diff: String::new(),
            move_path: Some(PathBuf::from("destination.txt")),
        };
        let updated = FileChange::Update {
            unified_diff: TextDiff::from_lines(original, "one\nTWO\n")
                .unified_diff()
                .context_radius(3)
                .to_string(),
            move_path: None,
        };
        let change = SessionFileChange::new(Some(original.to_string()), moved.clone())
            .complete(Some(original.to_string()), moved)
            .complete(Some("one\nTWO\n".to_string()), updated);

        assert!(change.moved_to(PathBuf::from("destination.txt").as_path()));
    }

    #[test]
    fn replayed_session_change_reverses_all_updates_from_the_current_file() {
        let original = (1..=20)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let first = original.replace("line 4\n", "first change\n");
        let current = first.replace("line 17\n", "second change\n");
        let update = |before: &str, after: &str| FileChange::Update {
            unified_diff: TextDiff::from_lines(before, after)
                .unified_diff()
                .context_radius(3)
                .to_string(),
            move_path: None,
        };

        let second_update = update(&first, &current);
        let change =
            SessionFileChange::from_completed(Some(current.clone()), update(&original, &first))
                .complete(Some(current), second_update)
                .resolve_inferred_baseline();

        let FileChange::Update { unified_diff, .. } = change.display_change(3) else {
            panic!("expected an update");
        };
        assert!(unified_diff.contains("-line 4"));
        assert!(unified_diff.contains("+first change"));
        assert!(unified_diff.contains("-line 17"));
        assert!(unified_diff.contains("+second change"));
    }

    #[test]
    fn replayed_session_change_reverses_overlapping_updates_in_reverse_order() {
        let original = "one\ntwo\nthree\n";
        let first = "one\nTWO\nthree\n";
        let current = "one\nSECOND\nthree\n";
        let update = |before: &str, after: &str| FileChange::Update {
            unified_diff: TextDiff::from_lines(before, after)
                .unified_diff()
                .context_radius(3)
                .to_string(),
            move_path: None,
        };
        let second_update = update(first, current);

        let change =
            SessionFileChange::from_completed(Some(current.to_string()), update(original, first))
                .complete(Some(current.to_string()), second_update)
                .resolve_inferred_baseline();

        let FileChange::Update { unified_diff, .. } = change.display_change(3) else {
            panic!("expected an update");
        };
        assert!(unified_diff.contains("-two"));
        assert!(unified_diff.contains("+SECOND"));
    }
}
