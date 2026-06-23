use std::io::Write;

use serde::Serialize;

use super::rev_list_spec::RevListSide;
use crate::utils::{
    error::{CliError, CliResult, StableErrorCode},
    output::OutputConfig,
};

/// `--help` examples shown in `libra rev-list --help` output.
///
/// `rev-list` walks the commit graph from the given spec (default
/// `HEAD`) and prints each reachable commit hash on its own line. The
/// banner pins the default `HEAD` walk, an explicit branch walk, a
/// quiet form, and a JSON variant for agents so users see all
/// supported forms without reading the design doc. Cross-cutting
/// `--help` EXAMPLES rollout per `docs/development/commands/_general.md` item B.
pub const REV_LIST_EXAMPLES: &str = "\
EXAMPLES:
    libra rev-list                  Walk ancestry from HEAD (one hash per line)
    libra rev-list --count HEAD     Count reachable commits after filters
    libra rev-list -n 5 HEAD        Limit output to the first five commits
    libra rev-list --reverse HEAD   Print reachable commits oldest first
    libra rev-list --all            Walk every branch, remote-tracking, and tag ref
    libra rev-list main feature     Walk from multiple revisions, de-duplicated
    libra rev-list ^main feature    Exclude commits reachable from main
    libra rev-list main..feature    Walk commits reachable from feature, not main
    libra rev-list main...feature   Walk the symmetric difference between two refs
    libra rev-list --merges HEAD    Print only merge commits
    libra rev-list --max-parents 0 HEAD
                                    Print only root commits
    libra rev-list --no-min-parents --no-max-parents HEAD
                                    Clear parent-count bounds
    libra rev-list --first-parent HEAD
                                    Follow only the first parent of merge commits
    libra rev-list --author alice HEAD
                                    Filter commits by author name or email
    libra rev-list --committer alice HEAD
                                    Filter commits by committer name or email
    libra rev-list --grep 'fix' HEAD
                                    Filter commits by message regex
    libra rev-list HEAD -- src/     Limit commits to changes under src/
    libra rev-list --left-right main...feature
                                    Mark symmetric-difference sides
    libra rev-list --cherry-pick main...feature
                                    Omit patch-equivalent commits across sides
    libra rev-list --cherry main...feature
                                    Show right side and mark equivalent commits
    libra rev-list --parents HEAD   Include parent commit IDs on each line
    libra rev-list --children HEAD  Include child commit IDs on each line
    libra rev-list --timestamp HEAD Prefix each line with the committer timestamp
    libra rev-list main             Walk ancestry from refs/heads/main
    libra rev-list HEAD~5           Walk ancestry from a relative ref
    libra rev-list --json HEAD      Structured JSON output (input + commits[] + total)
    libra rev-list --quiet HEAD     Suppress stdout (use exit code as truthy probe)";

#[derive(Debug, Clone, Serialize)]
pub(super) struct RevListEntry {
    pub(super) commit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) side: Option<RevListSide>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) cherry_equivalent: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) parents: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) children: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) timestamp: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct RevListOutput {
    pub(super) input: String,
    pub(super) inputs: Vec<String>,
    pub(super) commits: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) entries: Option<Vec<RevListEntry>>,
    pub(super) total: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) count_fields: Vec<usize>,
    pub(super) count_only: bool,
    pub(super) parents: bool,
    pub(super) children: bool,
    pub(super) timestamp: bool,
    pub(super) first_parent: bool,
    pub(super) author: Option<String>,
    pub(super) committer: Option<String>,
    pub(super) grep: Vec<String>,
    pub(super) pathspecs: Vec<String>,
    pub(super) left_right: bool,
    pub(super) left_only: bool,
    pub(super) right_only: bool,
    pub(super) cherry_pick: bool,
    pub(super) cherry_mark: bool,
    pub(super) cherry: bool,
    pub(super) since: Option<String>,
    pub(super) until: Option<String>,
    pub(super) merges: bool,
    pub(super) no_merges: bool,
    pub(super) min_parents: Option<usize>,
    pub(super) max_parents: Option<usize>,
    pub(super) no_min_parents: bool,
    pub(super) no_max_parents: bool,
    pub(super) max_count: Option<usize>,
    pub(super) skip: usize,
}

impl RevListOutput {
    pub(super) fn human_lines(&self) -> Vec<String> {
        if let Some(entries) = &self.entries {
            return entries
                .iter()
                .map(|entry| {
                    format_rev_list_entry(
                        entry,
                        self.parents,
                        self.children,
                        self.timestamp,
                        self.left_right,
                        self.cherry_mark,
                        self.cherry,
                    )
                })
                .collect();
        }

        self.commits.clone()
    }
}

pub(super) fn emit_human_rev_list(output: &OutputConfig, result: &RevListOutput) -> CliResult<()> {
    if output.quiet {
        Ok(())
    } else if result.count_only {
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();
        write_rev_list_count_fields(&mut writer, &result.count_fields)
    } else {
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();
        write_rev_list_output(&mut writer, &result.human_lines())
    }
}

#[cfg(test)]
pub(super) fn write_rev_list_count<W: Write>(writer: &mut W, total: usize) -> CliResult<()> {
    write_rev_list_count_fields(writer, &[total])
}

pub(super) fn write_rev_list_count_fields<W: Write>(
    writer: &mut W,
    fields: &[usize],
) -> CliResult<()> {
    let count = fields
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\t");
    match writeln!(writer, "{count}") {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(
            CliError::fatal(format!("failed to write rev-list output: {error}"))
                .with_stable_code(StableErrorCode::IoWriteFailed),
        ),
    }
}

pub(super) fn write_rev_list_output<W: Write>(writer: &mut W, commits: &[String]) -> CliResult<()> {
    for commit in commits {
        match writeln!(writer, "{commit}") {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => return Ok(()),
            Err(error) => {
                return Err(
                    CliError::fatal(format!("failed to write rev-list output: {error}"))
                        .with_stable_code(StableErrorCode::IoWriteFailed),
                );
            }
        }
    }
    Ok(())
}

pub(super) fn format_rev_list_entry(
    entry: &RevListEntry,
    show_parents: bool,
    show_children: bool,
    show_timestamp: bool,
    show_left_right: bool,
    show_cherry_mark: bool,
    show_cherry: bool,
) -> String {
    let mut fields = Vec::new();
    if show_timestamp && let Some(timestamp) = entry.timestamp {
        fields.push(timestamp.to_string());
    }
    fields.push(format_entry_commit(
        entry,
        show_left_right,
        show_cherry_mark,
        show_cherry,
    ));
    if show_parents {
        fields.extend(entry.parents.iter().cloned());
    }
    if show_children {
        fields.extend(entry.children.iter().cloned());
    }
    fields.join(" ")
}

fn format_entry_commit(
    entry: &RevListEntry,
    show_left_right: bool,
    show_cherry_mark: bool,
    show_cherry: bool,
) -> String {
    let marker = if show_cherry_mark {
        if entry.cherry_equivalent.unwrap_or(false) {
            "="
        } else {
            "+"
        }
    } else if show_cherry {
        if entry.cherry_equivalent.unwrap_or(false) {
            "="
        } else if show_left_right {
            match entry.side {
                Some(RevListSide::Left) => "<",
                Some(RevListSide::Right) => ">",
                None => "",
            }
        } else {
            "+"
        }
    } else if show_left_right {
        match entry.side {
            Some(RevListSide::Left) => "<",
            Some(RevListSide::Right) => ">",
            None => "",
        }
    } else {
        ""
    };
    format!("{marker}{}", entry.commit)
}
