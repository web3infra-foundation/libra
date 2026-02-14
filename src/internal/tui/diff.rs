//! Diff rendering for TUI display.
//!
//! This module provides functionality to render file changes (add/delete/update)
//! as styled terminal lines with colors, line numbers, and wrapping support.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use ratatui::{
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
};

/// File change type for diff display.
#[derive(Debug, Clone)]
pub enum FileChange {
    /// New file being added.
    Add { content: String },
    /// File being deleted.
    Delete { content: String },
    /// File being modified.
    Update {
        unified_diff: String,
        move_path: Option<PathBuf>,
    },
}

/// Summary of file changes for display.
#[derive(Debug, Clone)]
pub struct DiffSummary {
    /// Map of file paths to their changes.
    pub changes: HashMap<PathBuf, FileChange>,
    /// Current working directory for relative path display.
    pub cwd: PathBuf,
}

impl DiffSummary {
    /// Create a new diff summary.
    pub fn new(changes: HashMap<PathBuf, FileChange>, cwd: PathBuf) -> Self {
        Self { changes, cwd }
    }
}

// Internal representation for diff line rendering
enum DiffLineType {
    Insert,
    Delete,
    Context,
}

/// Create styled lines for diff summary display.
///
/// This is the main entry point for rendering file changes as TUI lines.
pub fn create_diff_summary(
    changes: &HashMap<PathBuf, FileChange>,
    cwd: &Path,
    wrap_cols: usize,
) -> Vec<Line<'static>> {
    let rows = collect_rows(changes);
    render_changes_block(rows, wrap_cols, cwd)
}

// Shared row for per-file presentation
#[derive(Clone)]
struct Row {
    #[allow(dead_code)]
    path: PathBuf,
    move_path: Option<PathBuf>,
    added: usize,
    removed: usize,
    change: FileChange,
}

fn collect_rows(changes: &HashMap<PathBuf, FileChange>) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for (path, change) in changes.iter() {
        let (added, removed) = match change {
            FileChange::Add { content } => (content.lines().count(), 0),
            FileChange::Delete { content } => (0, content.lines().count()),
            FileChange::Update { unified_diff, .. } => calculate_add_remove_from_diff(unified_diff),
        };
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

fn render_line_count_summary(added: usize, removed: usize) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    spans.push("(".into());
    spans.push(format!("+{added}").green());
    spans.push(" ".into());
    spans.push(format!("-{removed}").red());
    spans.push(")".into());
    spans
}

fn render_changes_block(rows: Vec<Row>, wrap_cols: usize, cwd: &Path) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();

    let render_path = |row: &Row| -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        spans.push(display_path_for(&row.path, cwd).into());
        if let Some(move_path) = &row.move_path {
            spans.push(format!(" -> {}", display_path_for(move_path, cwd)).into());
        }
        spans
    };

    // Header
    let total_added: usize = rows.iter().map(|r| r.added).sum();
    let total_removed: usize = rows.iter().map(|r| r.removed).sum();
    let file_count = rows.len();
    let noun = if file_count == 1 { "file" } else { "files" };
    let mut header_spans: Vec<Span<'static>> = vec!["* ".dim()];
    if let [row] = &rows[..] {
        let verb = match &row.change {
            FileChange::Add { .. } => "Added",
            FileChange::Delete { .. } => "Deleted",
            _ => "Edited",
        };
        header_spans.push(verb.bold());
        header_spans.push(" ".into());
        header_spans.extend(render_path(row));
        header_spans.push(" ".into());
        header_spans.extend(render_line_count_summary(row.added, row.removed));
    } else {
        header_spans.push("Edited".bold());
        header_spans.push(format!(" {file_count} {noun} ").into());
        header_spans.extend(render_line_count_summary(total_added, total_removed));
    }
    out.push(Line::from(header_spans));

    for (idx, r) in rows.into_iter().enumerate() {
        // Insert a blank separator between file chunks (except before the first)
        if idx > 0 {
            out.push("".into());
        }
        // File header line (skip when single-file header already shows the name)
        let skip_file_header = file_count == 1;
        if !skip_file_header {
            let mut header: Vec<Span<'static>> = Vec::new();
            header.push("  |- ".dim());
            header.extend(render_path(&r));
            header.push(" ".into());
            header.extend(render_line_count_summary(r.added, r.removed));
            out.push(Line::from(header));
        }

        let mut lines = vec![];
        render_change(&r.change, &mut lines, wrap_cols.saturating_sub(4));
        out.extend(prefix_lines(lines, "    ", "    "));
    }

    out
}

fn render_change(change: &FileChange, out: &mut Vec<Line<'static>>, width: usize) {
    match change {
        FileChange::Add { content } => {
            let line_number_width = line_number_width(content.lines().count());
            for (i, raw) in content.lines().enumerate() {
                out.extend(push_wrapped_diff_line(
                    i + 1,
                    DiffLineType::Insert,
                    raw,
                    width,
                    line_number_width,
                ));
            }
        }
        FileChange::Delete { content } => {
            let line_number_width = line_number_width(content.lines().count());
            for (i, raw) in content.lines().enumerate() {
                out.extend(push_wrapped_diff_line(
                    i + 1,
                    DiffLineType::Delete,
                    raw,
                    width,
                    line_number_width,
                ));
            }
        }
        FileChange::Update { unified_diff, .. } => {
            if let Ok(patch) = diffy::Patch::from_str(unified_diff) {
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
                        let spacer = format!("{:width$} ", "", width = line_number_width.max(1));
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
        }
    }
}

/// Format a path for display relative to the current working directory.
///
/// Prefers relative paths when possible for cleaner display.
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

fn line_number_width(max_line_number: usize) -> usize {
    if max_line_number == 0 {
        1
    } else {
        max_line_number.to_string().len()
    }
}

fn style_gutter() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn style_context() -> Style {
    Style::default()
}

fn style_add() -> Style {
    Style::default().fg(Color::Green)
}

fn style_del() -> Style {
    Style::default().fg(Color::Red)
}

/// Add a prefix to each line in the vector.
fn prefix_lines(
    lines: Vec<Line<'static>>,
    first_prefix: &'static str,
    rest_prefix: &'static str,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let prefix = if i == 0 { first_prefix } else { rest_prefix };
            let mut spans = vec![Span::raw(prefix)];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_add_remove_from_diff() {
        let original = "line one\nline two\nline three\n";
        let modified = "line one\nline two changed\nline three\n";
        let patch = diffy::create_patch(original, modified).to_string();

        let (added, removed) = calculate_add_remove_from_diff(&patch);
        assert_eq!(added, 1);
        assert_eq!(removed, 1);
    }

    #[test]
    fn test_display_path_relative() {
        let cwd = std::path::PathBuf::from("/workspace/project");
        let path = std::path::PathBuf::from("/workspace/project/src/main.rs");

        let rendered = display_path_for(&path, &cwd);
        assert_eq!(rendered, "src/main.rs");
    }

    #[test]
    fn test_display_path_already_relative() {
        let cwd = std::path::PathBuf::from("/workspace/project");
        let path = std::path::PathBuf::from("src/main.rs");

        let rendered = display_path_for(&path, &cwd);
        assert_eq!(rendered, "src/main.rs");
    }

    #[test]
    fn test_create_diff_summary_single_file() {
        let mut changes: HashMap<PathBuf, FileChange> = HashMap::new();
        changes.insert(
            PathBuf::from("example.txt"),
            FileChange::Add {
                content: "hello\nworld\n".to_string(),
            },
        );

        let lines = create_diff_summary(&changes, &PathBuf::from("/"), 80);

        // Should have header + spacing + 2 content lines
        assert!(!lines.is_empty());
    }

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

    #[test]
    fn test_push_wrapped_diff_line_short() {
        let lines = push_wrapped_diff_line(1, DiffLineType::Insert, "short line", 80, 1);

        // Short line should not wrap
        assert_eq!(lines.len(), 1);
    }

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
        changes.insert(
            PathBuf::from("b.txt"),
            FileChange::Add {
                content: "new\n".to_string(),
            },
        );

        let lines = create_diff_summary(&changes, &PathBuf::from("/"), 80);

        // Should have header showing "2 files"
        let header_text: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect::<String>();
        assert!(header_text.contains("2 files"));
    }
}
