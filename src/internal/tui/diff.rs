//! Diff rendering for TUI display.
//!
//! This module is a self-contained renderer that converts a map of file
//! changes (each carrying a unified-diff string produced by
//! `diffy::create_patch`) into styled `ratatui::text::Line` rows ready for
//! `Paragraph::render`. The output is consumed by `DiffHistoryCell` and the
//! orchestrator's plan-summary cells.
//!
//! Layout per file:
//! ```text
//! ● Update(src/main.rs)
//! └ Added 3 lines, removed 2 lines
//!     12 -old line
//!     12 +new line
//!     14  context line
//! ```
//!
//! Multi-file summaries are indented and separated by blank lines so the
//! reader can visually scan file headers.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use ratatui::{
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
};

use super::theme;

/// File change type for diff display.
///
/// All variants store a unified diff string produced by `diffy::create_patch`.
/// `Update` additionally tracks the destination path for renames so the
/// header can render `Update(old -> new)`.
#[derive(Debug, Clone)]
pub enum FileChange {
    /// New file being added; the diff describes additions from an empty file.
    Add { unified_diff: String },
    /// File being deleted; the diff describes removals down to an empty file.
    Delete { unified_diff: String },
    /// File being modified, optionally renamed via `move_path`.
    Update {
        unified_diff: String,
        /// New path when the change is also a rename; `None` for in-place
        /// edits.
        move_path: Option<PathBuf>,
    },
}

/// Summary of file changes for display.
///
/// Bundles a map of changes with a cwd anchor so callers can build a
/// summary once and pass it around without losing the relative-path context.
#[derive(Debug, Clone)]
pub struct DiffSummary {
    /// Map of file paths to their changes.
    pub changes: HashMap<PathBuf, FileChange>,
    /// Current working directory used for relative path display.
    pub cwd: PathBuf,
}

impl DiffSummary {
    /// Create a new diff summary.
    pub fn new(changes: HashMap<PathBuf, FileChange>, cwd: PathBuf) -> Self {
        Self { changes, cwd }
    }
}

// Internal representation for diff line rendering: tags each rendered line
// with whether it's an addition, deletion, or unchanged context so the
// renderer can pick the right colour and sign character.
enum DiffLineType {
    Insert,
    Delete,
    Context,
}

/// Create styled lines for diff summary display.
///
/// Functional scope: the public entry point. Collects per-file metadata,
/// sorts deterministically by path, then renders headers + diff bodies.
///
/// Boundary conditions:
/// - `wrap_cols` is the *visible* terminal column count; the renderer
///   subtracts indents and gutters internally before splitting long lines.
/// - The output is `Vec<Line<'static>>` because callers cache it inside
///   history cells that outlive the originating diff buffers.
///
/// See: [`tests::test_create_diff_summary_single_file`],
/// [`tests::test_create_diff_summary_update`], [`tests::test_multiple_files`].
pub fn create_diff_summary(
    changes: &HashMap<PathBuf, FileChange>,
    cwd: &Path,
    wrap_cols: usize,
) -> Vec<Line<'static>> {
    let rows = collect_rows(changes);
    render_changes_block(rows, wrap_cols, cwd)
}

// Shared row for per-file presentation. `Row` aggregates the path, optional
// rename target, derived line counts, and the original FileChange so the
// renderer doesn't have to re-parse the diff for header information.
#[derive(Clone)]
struct Row {
    path: PathBuf,
    move_path: Option<PathBuf>,
    added: usize,
    removed: usize,
    change: FileChange,
}

/// Convert the raw `(path, change)` map into deterministically ordered Rows.
///
/// Sort is by path so output is reproducible across runs even though
/// `HashMap` iteration order is randomised.
fn collect_rows(changes: &HashMap<PathBuf, FileChange>) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for (path, change) in changes.iter() {
        let unified_diff = match change {
            FileChange::Add { unified_diff }
            | FileChange::Delete { unified_diff }
            | FileChange::Update { unified_diff, .. } => unified_diff,
        };
        let (added, removed) = calculate_add_remove_from_diff(unified_diff);
        let move_path = match change {
            FileChange::Update {
                move_path: Some(new),
                ..
            } => Some(new.clone()),
            _ => None,
        };
        rows.push(Row {
            path: path.clone(),
            move_path,
            added,
            removed,
            change: change.clone(),
        });
    }
    rows.sort_by_key(|r| r.path.clone());
    rows
}

/// Build the human-readable "Added N lines, removed M lines" summary string.
///
/// Boundary conditions:
/// - Returns an empty string when both counts are zero, signalling the
///   caller to omit the summary line entirely.
/// - Pluralises `line` correctly for both fields independently.
fn render_line_count_summary_text(added: usize, removed: usize) -> String {
    let mut parts = Vec::new();
    if added > 0 {
        let noun = if added == 1 { "line" } else { "lines" };
        parts.push(format!("Added {added} {noun}"));
    }
    if removed > 0 {
        let noun = if removed == 1 { "line" } else { "lines" };
        parts.push(format!("removed {removed} {noun}"));
    }
    if parts.is_empty() {
        return String::new();
    }
    parts.join(", ")
}

/// Render every row's header, summary line, and diff body, joining them with
/// blank-line separators when there are multiple files.
///
/// Functional scope: the actual layout machine. Picks an indent ("" for
/// single-file output, "  " for multi-file) so multi-file diffs visually
/// nest under a shared root.
///
/// Boundary conditions: diff bodies are wrapped to `wrap_cols - indent` so
/// continuation lines align with the gutter rather than overflowing.
fn render_changes_block(rows: Vec<Row>, wrap_cols: usize, cwd: &Path) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let indent = if rows.len() > 1 { "  " } else { "" };
    let content_indent = format!("{indent}  ");

    for (idx, r) in rows.iter().enumerate() {
        if idx > 0 {
            out.push("".into());
        }

        // File header: ● Update(path) / ● Added(path) / ● Deleted(path)
        let (verb, bullet_color) = match &r.change {
            FileChange::Add { .. } => ("Added", theme::diff::added_header_color()),
            FileChange::Delete { .. } => ("Deleted", theme::diff::removed_header_color()),
            FileChange::Update { .. } => ("Update", theme::diff::updated_header_color()),
        };
        let path_display = display_path_for(&r.path, cwd);
        let move_suffix = r
            .move_path
            .as_ref()
            .map(|mp| format!(" -> {}", display_path_for(mp, cwd)))
            .unwrap_or_default();
        out.push(Line::from(vec![
            Span::raw(indent.to_string()),
            Span::styled("● ", Style::default().fg(bullet_color).bold()),
            Span::styled(
                format!("{verb}({path_display}{move_suffix})"),
                theme::text::primary().add_modifier(Modifier::BOLD),
            ),
        ]));

        // Summary: └ Added N lines, removed M lines
        let summary = render_line_count_summary_text(r.added, r.removed);
        if !summary.is_empty() {
            out.push(Line::from(vec![
                Span::styled(format!("{indent}\u{2514} "), theme::text::muted()),
                Span::styled(summary, theme::text::muted().add_modifier(Modifier::DIM)),
            ]));
        }

        // Diff content
        let mut lines = vec![];
        render_change(
            &r.change,
            &mut lines,
            wrap_cols.saturating_sub(content_indent.len()),
        );
        out.extend(prefix_lines(lines, &content_indent, &content_indent));
    }

    out
}

/// Render the body of a single `FileChange` (hunks, gutter, sign column).
///
/// Functional scope: parses the unified diff with `diffy`, computes the
/// widest line number so the gutter stays aligned, then walks each hunk and
/// emits per-line rows. Hunks are visually separated with a `...` spacer to
/// indicate skipped content.
///
/// Boundary conditions:
/// - Bails silently when the diff string fails to parse — the parent
///   summary header still renders so the user is not left wondering why a
///   row appeared without details.
/// - `width` is the available content width *after* outer indent; the
///   per-line renderer subtracts the gutter and sign column further.
fn render_change(change: &FileChange, out: &mut Vec<Line<'static>>, width: usize) {
    let unified_diff = match change {
        FileChange::Add { unified_diff }
        | FileChange::Delete { unified_diff }
        | FileChange::Update { unified_diff, .. } => unified_diff,
    };

    let Ok(patch) = diffy::Patch::from_str(unified_diff) else {
        return;
    };

    let mut max_line_number = 0;
    for h in patch.hunks() {
        let mut old_ln = h.old_range().start();
        let mut new_ln = h.new_range().start();
        for l in h.lines() {
            match l {
                diffy::Line::Insert(_) => {
                    max_line_number = max_line_number.max(new_ln);
                    new_ln += 1;
                }
                diffy::Line::Delete(_) => {
                    max_line_number = max_line_number.max(old_ln);
                    old_ln += 1;
                }
                diffy::Line::Context(_) => {
                    max_line_number = max_line_number.max(new_ln);
                    old_ln += 1;
                    new_ln += 1;
                }
            }
        }
    }
    let line_number_width = line_number_width(max_line_number);
    let mut is_first_hunk = true;
    for h in patch.hunks() {
        if !is_first_hunk {
            let spacer = format!("{:w$} ", "", w = line_number_width.max(1));
            let spacer_span = Span::styled(spacer, style_gutter());
            out.push(Line::from(vec![spacer_span, "...".dim()]));
        }
        is_first_hunk = false;

        let mut old_ln = h.old_range().start();
        let mut new_ln = h.new_range().start();
        for l in h.lines() {
            match l {
                diffy::Line::Insert(text) => {
                    let s = text.trim_end_matches('\n');
                    out.extend(push_wrapped_diff_line(
                        new_ln,
                        DiffLineType::Insert,
                        s,
                        width,
                        line_number_width,
                    ));
                    new_ln += 1;
                }
                diffy::Line::Delete(text) => {
                    let s = text.trim_end_matches('\n');
                    out.extend(push_wrapped_diff_line(
                        old_ln,
                        DiffLineType::Delete,
                        s,
                        width,
                        line_number_width,
                    ));
                    old_ln += 1;
                }
                diffy::Line::Context(text) => {
                    let s = text.trim_end_matches('\n');
                    out.extend(push_wrapped_diff_line(
                        new_ln,
                        DiffLineType::Context,
                        s,
                        width,
                        line_number_width,
                    ));
                    old_ln += 1;
                    new_ln += 1;
                }
            }
        }
    }
}

/// Format a path for display relative to the current working directory.
///
/// Functional scope: tries (in order) the literal relative path,
/// `strip_prefix(cwd)`, `pathdiff::diff_paths`, and finally a `~/` home-
/// directory shortening before falling back to the full absolute path.
///
/// Boundary conditions:
/// - Already-relative paths are returned verbatim.
/// - Paths outside `cwd` and not under the home directory render absolute,
///   ensuring no path ambiguity in the transcript.
///
/// See: [`tests::test_display_path_relative`],
/// [`tests::test_display_path_already_relative`].
pub fn display_path_for(path: &Path, cwd: &Path) -> String {
    if path.is_relative() {
        return path.display().to_string();
    }

    if let Ok(stripped) = path.strip_prefix(cwd) {
        return stripped.display().to_string();
    }

    // Try to relativize using pathdiff
    if let Some(relative) = pathdiff::diff_paths(path, cwd) {
        return relative.display().to_string();
    }

    // Try to shorten by using ~ for home directory
    if let Some(home) = dirs::home_dir()
        && let Ok(stripped) = path.strip_prefix(&home)
    {
        return format!("~/{}", stripped.display());
    }

    // Fallback to full path
    path.display().to_string()
}

/// Calculate the number of added and removed lines from a unified diff.
///
/// Functional scope: parses with `diffy::Patch::from_str` and counts insert
/// / delete hunks across all files.
///
/// Boundary conditions: returns `(0, 0)` when the diff cannot be parsed so
/// callers can render summaries safely without panicking on malformed
/// input.
///
/// See: [`tests::test_calculate_add_remove_from_diff`].
pub fn calculate_add_remove_from_diff(diff: &str) -> (usize, usize) {
    if let Ok(patch) = diffy::Patch::from_str(diff) {
        patch
            .hunks()
            .iter()
            .flat_map(diffy::Hunk::lines)
            .fold((0, 0), |(a, d), l| match l {
                diffy::Line::Insert(_) => (a + 1, d),
                diffy::Line::Delete(_) => (a, d + 1),
                diffy::Line::Context(_) => (a, d),
            })
    } else {
        // For unparsable diffs, return 0 for both counts.
        (0, 0)
    }
}

/// Render a single diff line with line-number gutter, sign column, and
/// soft-wrap continuation rows when the content overruns `width`.
///
/// Functional scope: produces one or more `Line<'static>` rows depending on
/// width; the first row carries the line number and sign character, while
/// continuation rows pad those columns with spaces so wrapped content stays
/// visually aligned with the original gutter.
///
/// Boundary conditions:
/// - Splits at UTF-8 character boundaries rather than byte counts to avoid
///   producing invalid strings on multi-byte characters.
/// - Always emits at least one row so the caller never has to special-case
///   short text.
fn push_wrapped_diff_line(
    line_number: usize,
    kind: DiffLineType,
    text: &str,
    width: usize,
    line_number_width: usize,
) -> Vec<Line<'static>> {
    let ln_str = line_number.to_string();
    let mut remaining_text: &str = text;

    // Reserve a fixed number of spaces (equal to the widest line number plus a
    // trailing spacer) so the sign column stays aligned across the diff block.
    let gutter_width = line_number_width.max(1);
    let prefix_cols = gutter_width + 1;

    let mut first = true;
    let (sign_char, line_style) = match kind {
        DiffLineType::Insert => ('+', style_add()),
        DiffLineType::Delete => ('-', style_del()),
        DiffLineType::Context => (' ', style_context()),
    };
    let mut lines: Vec<Line<'static>> = Vec::new();

    loop {
        // Fit the content for the current terminal row:
        // compute how many columns are available after the prefix, then split
        // at a UTF-8 character boundary so this row's chunk fits exactly.
        let available_content_cols = width.saturating_sub(prefix_cols + 1).max(1);
        let split_at_byte_index = remaining_text
            .char_indices()
            .nth(available_content_cols)
            .map(|(i, _)| i)
            .unwrap_or_else(|| remaining_text.len());
        let (chunk, rest) = remaining_text.split_at(split_at_byte_index);
        remaining_text = rest;

        if first {
            // Build gutter (right-aligned line number plus spacer) as a dimmed span
            let gutter = format!("{ln_str:>gutter_width$} ");
            // Content with a sign ('+'/'-'/' ') styled per diff kind
            let content = format!("{sign_char}{chunk}");
            lines.push(Line::from(vec![
                Span::styled(gutter, style_gutter()),
                Span::styled(content, line_style),
            ]));
            first = false;
        } else {
            // Continuation lines keep a space for the sign column so content aligns
            let gutter = format!("{:gutter_width$}  ", "");
            lines.push(Line::from(vec![
                Span::styled(gutter, style_gutter()),
                Span::styled(chunk.to_string(), line_style),
            ]));
        }
        if remaining_text.is_empty() {
            break;
        }
    }
    lines
}

/// Width in characters of the largest line number; minimum 1 so single-digit
/// diffs still get a coherent gutter.
fn line_number_width(max_line_number: usize) -> usize {
    if max_line_number == 0 {
        1
    } else {
        max_line_number.to_string().len()
    }
}

/// Style for the line-number gutter.
fn style_gutter() -> Style {
    theme::diff::gutter()
}

/// Style for unchanged context lines.
fn style_context() -> Style {
    theme::diff::context()
}

/// Style for `+` insertion lines.
fn style_add() -> Style {
    theme::diff::added_line()
}

/// Style for `-` deletion lines.
fn style_del() -> Style {
    theme::diff::removed_line()
}

/// Add a prefix to each line in the vector.
///
/// Functional scope: takes a different prefix for the first line vs rest so
/// callers can emit nested bullet structures without writing it themselves.
/// Used to indent multi-file diff bodies under their per-file headers.
fn prefix_lines(
    lines: Vec<Line<'static>>,
    first_prefix: &str,
    rest_prefix: &str,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let prefix = if i == 0 { first_prefix } else { rest_prefix };
            let mut spans = vec![Span::raw(prefix.to_string())];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::tui::theme;

    /// Scenario: a one-line edit produces exactly one insert and one delete.
    /// Pin the count parser so changes to `diffy` semantics surface here.
    #[test]
    fn test_calculate_add_remove_from_diff() {
        let original = "line one\nline two\nline three\n";
        let modified = "line one\nline two changed\nline three\n";
        let patch = diffy::create_patch(original, modified).to_string();

        let (added, removed) = calculate_add_remove_from_diff(&patch);
        assert_eq!(added, 1);
        assert_eq!(removed, 1);
    }

    /// Scenario: an absolute path inside cwd is rendered relative for clarity.
    #[test]
    fn test_display_path_relative() {
        let cwd = std::path::PathBuf::from("/workspace/project");
        let path = std::path::PathBuf::from("/workspace/project/src/main.rs");

        let rendered = display_path_for(&path, &cwd);
        assert_eq!(rendered, "src/main.rs");
    }

    /// Scenario: an already-relative path passes through unchanged so the
    /// renderer doesn't accidentally absolutise paths it was given relative.
    #[test]
    fn test_display_path_already_relative() {
        let cwd = std::path::PathBuf::from("/workspace/project");
        let path = std::path::PathBuf::from("src/main.rs");

        let rendered = display_path_for(&path, &cwd);
        assert_eq!(rendered, "src/main.rs");
    }

    /// Scenario: a single-file Add change should produce at least a header
    /// and a body. We don't pin the exact line layout to avoid brittleness
    /// when styling changes; we just guarantee non-emptiness.
    #[test]
    fn test_create_diff_summary_single_file() {
        let mut changes: HashMap<PathBuf, FileChange> = HashMap::new();
        let patch = diffy::create_patch("", "hello\nworld\n").to_string();
        changes.insert(
            PathBuf::from("example.txt"),
            FileChange::Add {
                unified_diff: patch,
            },
        );

        let lines = create_diff_summary(&changes, &PathBuf::from("/"), 80);

        // Should have header + summary + content lines
        assert!(!lines.is_empty());
    }

    /// Scenario: an Update change produces output. Mirrors
    /// `test_create_diff_summary_single_file` for the rename-capable variant.
    #[test]
    fn test_create_diff_summary_update() {
        let mut changes: HashMap<PathBuf, FileChange> = HashMap::new();
        let original = "line one\nline two\nline three\n";
        let modified = "line one\nline two changed\nline three\n";
        let patch = diffy::create_patch(original, modified).to_string();

        changes.insert(
            PathBuf::from("example.txt"),
            FileChange::Update {
                unified_diff: patch,
                move_path: None,
            },
        );

        let lines = create_diff_summary(&changes, &PathBuf::from("/"), 80);

        // Should have header + context lines + changed lines
        assert!(!lines.is_empty());
    }

    /// Scenario: a short line fits in one row regardless of width policy,
    /// so the wrapper must not produce continuation rows for it.
    #[test]
    fn test_push_wrapped_diff_line_short() {
        let lines = push_wrapped_diff_line(1, DiffLineType::Insert, "short line", 80, 1);

        // Short line should not wrap
        assert_eq!(lines.len(), 1);
    }

    /// Scenario: an over-long line wraps into multiple rows. The first row
    /// keeps the line-number gutter and `+` sign; continuation rows drop
    /// them so users can tell at a glance which row owns the line number.
    #[test]
    fn test_push_wrapped_diff_line_long() {
        let long_line = "this is a very long line that should wrap across multiple terminal columns and continue";
        let lines = push_wrapped_diff_line(1, DiffLineType::Insert, long_line, 40, 1);

        // Long line should wrap into multiple lines
        assert!(lines.len() > 1);

        // First line should have the line number
        let first_line = &lines[0];
        assert!(first_line.spans[0].content.contains('1'));

        // Second line should not have the + sign
        let second_line = &lines[1];
        assert!(!second_line.spans[1].content.starts_with('+'));
    }

    /// Scenario: every diff row must use the shared theme so palette tweaks
    /// propagate. Pin gutter + add + delete + context to `theme::diff::*`.
    #[test]
    fn diff_line_styles_follow_theme() {
        let insert = push_wrapped_diff_line(1, DiffLineType::Insert, "added", 40, 1);
        let delete = push_wrapped_diff_line(1, DiffLineType::Delete, "removed", 40, 1);
        let context = push_wrapped_diff_line(1, DiffLineType::Context, "same", 40, 1);

        assert_eq!(insert[0].spans[0].style, theme::diff::gutter());
        assert_eq!(insert[0].spans[1].style, theme::diff::added_line());
        assert_eq!(delete[0].spans[1].style, theme::diff::removed_line());
        assert_eq!(context[0].spans[1].style, theme::diff::context());
    }

    #[test]
    fn test_multiple_files() {
        let mut changes: HashMap<PathBuf, FileChange> = HashMap::new();

        // File a.txt: update
        let patch_a = diffy::create_patch("one\n", "one changed\n").to_string();
        changes.insert(
            PathBuf::from("a.txt"),
            FileChange::Update {
                unified_diff: patch_a,
                move_path: None,
            },
        );

        // File b.txt: add
        let patch_b = diffy::create_patch("", "new\n").to_string();
        changes.insert(
            PathBuf::from("b.txt"),
            FileChange::Add {
                unified_diff: patch_b,
            },
        );

        let lines = create_diff_summary(&changes, &PathBuf::from("/"), 80);

        // Should have separate per-file headers
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.to_string())
            .collect::<String>();
        assert!(all_text.contains("a.txt"));
        assert!(all_text.contains("b.txt"));
    }
}
