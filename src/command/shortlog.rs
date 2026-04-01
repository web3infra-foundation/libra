//! Shortlog command for summarizing commit history by author.
//!
//! This module implements a `git shortlog`-style report used primarily for
//! release announcements and contributor overviews. It is structured as a
//! standard CLI command module, following the conventions used by other
//! commands in this crate:
//!
//! - **Argument parsing** is handled by [`ShortlogArgs`], which defines the
//!   supported flags and options using `clap::Parser`. The key flags are:
//!   - `numbered` (`-n` / `--numbered`): sort authors by descending commit
//!     count rather than by name.
//!   - `summary` (`-s` / `--summary`): emit only per-author commit counts,
//!     suppressing individual commit subjects.
//!   - `email` (`-e` / `--email`): include the author email address in the
//!     report header.
//!   - `since` / `until`: restrict the set of commits by committer timestamp,
//!     using the repository-wide date parser in [`parse_date`].
//!
//! - **Execution entrypoints**:
//!   - [`execute`] is the user-facing async entrypoint used by the CLI
//!     dispatcher. It writes human-readable output to `stdout`.
//!   - [`execute_to`] contains the core logic and is parameterized over an
//!     arbitrary `Write` implementor, which makes it easier to test and to
//!     reuse from other tooling without being tied to a specific output
//!     stream.
//!
//! - **Commit collection and filtering**:
//!   - [`get_commits_for_shortlog`] resolves the current [`Head`] and
//!     obtains the relevant list of [`Commit`] objects to be included in the
//!     report. The exact traversal strategy is delegated to the internal git
//!     engine.
//!   - [`passes_filter`] applies `since`/`until` constraints to each
//!     commit, converting user-supplied date strings via [`parse_date`] and
//!     comparing them against the commit committer timestamp (to match `git log`).
//!
//! - **Aggregation and formatting**:
//!   - Commits are grouped by author identity in an in-memory
//!     `HashMap<String, AuthorStats>`, where [`AuthorStats`] tracks the
//!     author name, optional email address, total commit count, and a list
//!     of commit subjects.
//!   - If `-e` is provided, grouping is by `name <email>`. Otherwise, it is
//!     by `name` only (merging multiple emails for the same author).
//!   - After aggregation, the authors are converted to a vector, optionally
//!     sorted by commit count (`numbered`) or left in deterministic order,
//!     and finally rendered to the provided writer in either detailed or
//!     summary form depending on the `summary` flag.
//!
//! The implementation is intentionally streaming-friendly at the output
//! layer (it writes directly to the provided `Write`), while still
//! aggregating per-author statistics in memory for predictable formatting.

use std::{
    collections::HashMap,
    fmt,
    io::{self, Write},
};

use clap::Parser;
use git_internal::internal::object::commit::Commit;

use crate::{
    internal::{branch::BranchStoreError, head::Head, log::date_parser::parse_date},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::OutputConfig,
    },
};

#[derive(Parser, Debug)]
pub struct ShortlogArgs {
    /// Sort output according to the number of commits per author
    #[clap(short = 'n', long = "numbered")]
    pub numbered: bool,

    /// Suppress commit description and provide a commit count summary only
    #[clap(short = 's', long = "summary")]
    pub summary: bool,

    /// Show the email address of each author
    #[clap(short = 'e', long = "email")]
    pub email: bool,

    /// Show commits more recent than a specific date
    #[clap(long = "since")]
    pub since: Option<String>,

    /// Show commits older than a specific date
    #[clap(long = "until")]
    pub until: Option<String>,
}

struct AuthorStats {
    name: String,
    email: String,
    count: usize,
    subjects: Vec<String>,
}

impl AuthorStats {
    fn new(name: String, email: String) -> Self {
        Self {
            name,
            email,
            count: 0,
            subjects: Vec::new(),
        }
    }

    fn add_commit(&mut self, subject: String) {
        self.count += 1;
        self.subjects.push(subject);
    }
}

pub async fn execute_to(args: ShortlogArgs, writer: &mut impl Write) -> CliResult<()> {
    crate::utils::util::require_repo().map_err(|_| CliError::repo_not_found())?;

    // Validate date arguments before processing
    let since_ts = if let Some(ref since_str) = args.since {
        match parse_date(since_str) {
            Ok(ts) => Some(ts),
            Err(e) => return Err(CliError::fatal(e.to_string())),
        }
    } else {
        None
    };

    let until_ts = if let Some(ref until_str) = args.until {
        match parse_date(until_str) {
            Ok(ts) => Some(ts),
            Err(e) => return Err(CliError::fatal(e.to_string())),
        }
    } else {
        None
    };

    let commits = get_commits_for_shortlog(&args, since_ts, until_ts)
        .await
        .map_err(|e| CliError::fatal(e.message().to_string()))?;

    let mut author_map: HashMap<String, AuthorStats> = HashMap::new();

    for commit in commits {
        let author_name = commit.author.name.clone();
        let author_email = commit.author.email.clone();

        // If email is not requested, group by name only.
        // If email is requested, group by name + email.
        let key = if args.email {
            format!("{} <{}>", author_name, author_email)
        } else {
            author_name.clone()
        };

        let subject = commit
            .message
            .trim()
            .lines()
            .next()
            .unwrap_or("")
            .to_string();

        author_map
            .entry(key)
            .or_insert_with(|| AuthorStats::new(author_name.clone(), author_email.clone()))
            .add_commit(subject);
    }

    let mut authors: Vec<(&String, &AuthorStats)> = author_map.iter().collect();

    if args.numbered {
        // Sort by commit count (descending) and then by author name (ascending) to ensure deterministic output
        authors.sort_by_key(|a| (std::cmp::Reverse(a.1.count), a.1.name.to_lowercase()));
    } else {
        authors.sort_by_key(|a| a.1.name.to_lowercase());
    }

    // Determine the width needed for the commit count column.
    // Use at least 4 characters to preserve the existing layout for small repositories.
    let max_count = authors
        .iter()
        .map(|(_, stats)| stats.count)
        .max()
        .unwrap_or(0);
    let width = std::cmp::max(4, max_count.to_string().len());

    for (_key, stats) in authors {
        if args.email {
            if !write_shortlog_line(
                writer,
                format_args!(
                    "{:>width$}  {} <{}>",
                    stats.count,
                    stats.name,
                    stats.email,
                    width = width
                ),
            )? {
                return Ok(());
            }
        } else if !write_shortlog_line(
            writer,
            format_args!("{:>width$}  {}", stats.count, stats.name, width = width),
        )? {
            return Ok(());
        }
        if !args.summary {
            for subject in &stats.subjects {
                if !write_shortlog_line(writer, format_args!("      {}", subject))? {
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

fn write_shortlog_line(writer: &mut impl Write, args: fmt::Arguments<'_>) -> CliResult<bool> {
    match writer.write_fmt(args) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => return Ok(false),
        Err(err) => return Err(shortlog_output_error(err)),
    }

    match writer.write_all(b"\n") {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(err) => Err(shortlog_output_error(err)),
    }
}

fn shortlog_output_error(err: io::Error) -> CliError {
    CliError::fatal(format!("shortlog output error: {err}"))
        .with_stable_code(StableErrorCode::IoWriteFailed)
}

pub async fn execute(args: ShortlogArgs) {
    if let Err(e) = execute_safe(args, &OutputConfig::default()).await {
        e.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Summarises commit history by author, delegating to
/// [`execute_to`] for formatted output.
pub async fn execute_safe(args: ShortlogArgs, _output: &OutputConfig) -> CliResult<()> {
    execute_to(args, &mut std::io::stdout()).await
}

async fn get_commits_for_shortlog(
    _args: &ShortlogArgs,
    since_ts: Option<i64>,
    until_ts: Option<i64>,
) -> CliResult<Vec<Commit>> {
    use crate::command::log::get_reachable_commits;

    let head = Head::current().await;
    let commit_hash = match head {
        Head::Branch(name) => {
            let branch = crate::internal::branch::Branch::find_branch_result(&name, None)
                .await
                .map_err(shortlog_branch_store_error)?
                .map(|b| b.commit.to_string());
            match branch {
                Some(h) => h,
                None => {
                    return Err(CliError::fatal("current branch has no commits"));
                }
            }
        }
        Head::Detached(hash) => hash.to_string(),
    };

    let mut commits: Vec<Commit> = get_reachable_commits(commit_hash, None)
        .await?
        .into_iter()
        .filter(|c| passes_filter(c, since_ts, until_ts))
        .collect();

    commits.sort_by_key(|b| std::cmp::Reverse(b.author.timestamp));

    Ok(commits)
}

fn shortlog_branch_store_error(error: BranchStoreError) -> CliError {
    match error {
        BranchStoreError::Query(detail) => {
            CliError::fatal(format!("failed to read branch storage: {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        other => CliError::fatal(format!("failed to resolve current branch: {other}"))
            .with_stable_code(StableErrorCode::RepoCorrupt),
    }
}

fn passes_filter(commit: &Commit, since_ts: Option<i64>, until_ts: Option<i64>) -> bool {
    let commit_ts = commit.committer.timestamp as i64;

    if let Some(since) = since_ts
        && commit_ts < since
    {
        return false;
    }

    if let Some(until) = until_ts
        && commit_ts > until
    {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;
    use crate::utils::error::StableErrorCode;

    #[test]
    fn test_parse_args() {
        let args = ShortlogArgs::parse_from(["shortlog"]);
        assert!(!args.numbered);
        assert!(!args.summary);
        assert!(!args.email);

        let args = ShortlogArgs::parse_from(["shortlog", "-n", "-s", "-e"]);
        assert!(args.numbered);
        assert!(args.summary);
        assert!(args.email);

        let args = ShortlogArgs::parse_from(["shortlog", "--since", "2024-01-01"]);
        assert!(args.since.is_some());
    }

    #[test]
    fn broken_pipe_writer_is_ignored() {
        struct BrokenPipeWriter;

        impl Write for BrokenPipeWriter {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut writer = BrokenPipeWriter;
        assert!(
            !write_shortlog_line(&mut writer, format_args!("alice")).unwrap(),
            "BrokenPipe should terminate output quietly"
        );
    }

    #[test]
    fn non_broken_pipe_writer_error_is_structured() {
        struct PermissionDeniedWriter;

        impl Write for PermissionDeniedWriter {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut writer = PermissionDeniedWriter;
        let err = write_shortlog_line(&mut writer, format_args!("alice")).unwrap_err();
        assert_eq!(err.stable_code(), StableErrorCode::IoWriteFailed);
        assert!(err.message().contains("shortlog output error"));
    }
}
