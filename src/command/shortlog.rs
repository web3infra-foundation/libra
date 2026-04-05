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
use serde::Serialize;

use crate::{
    internal::log::date_parser::parse_date,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util::{self, CommitBaseError},
    },
};

const SHORTLOG_EXAMPLES: &str = "\
EXAMPLES:
  libra shortlog
  libra shortlog HEAD~5
  libra shortlog -n -s
  libra shortlog --json
";

#[derive(Parser, Debug)]
#[command(after_help = SHORTLOG_EXAMPLES)]
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

    /// Revision to summarize. Defaults to HEAD.
    pub revision: Option<String>,
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

#[derive(Debug, Clone, Serialize)]
struct ShortlogAuthor {
    name: String,
    email: Option<String>,
    count: usize,
    subjects: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ShortlogOutput {
    revision: String,
    numbered: bool,
    summary: bool,
    email: bool,
    total_authors: usize,
    total_commits: usize,
    authors: Vec<ShortlogAuthor>,
}

pub async fn execute_to(args: ShortlogArgs, writer: &mut impl Write) -> CliResult<()> {
    crate::utils::util::require_repo().map_err(|_| CliError::repo_not_found())?;
    let shortlog_output = run_shortlog(&args).await?;
    render_shortlog_output(&shortlog_output, writer)
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
pub async fn execute_safe(args: ShortlogArgs, output: &OutputConfig) -> CliResult<()> {
    let shortlog_output = run_shortlog(&args).await?;

    if output.is_json() {
        emit_json_data("shortlog", &shortlog_output, output)?;
    } else if !output.quiet {
        let mut stdout = std::io::stdout();
        render_shortlog_output(&shortlog_output, &mut stdout)?;
    }

    Ok(())
}

async fn run_shortlog(args: &ShortlogArgs) -> CliResult<ShortlogOutput> {
    let since_ts = parse_shortlog_date_arg(args.since.as_deref(), "--since")?;
    let until_ts = parse_shortlog_date_arg(args.until.as_deref(), "--until")?;
    let revision = args.revision.clone().unwrap_or_else(|| "HEAD".to_string());
    let commits = get_commits_for_shortlog(args.revision.as_deref(), since_ts, until_ts).await?;

    Ok(aggregate_shortlog(args, &revision, commits))
}

fn aggregate_shortlog(args: &ShortlogArgs, revision: &str, commits: Vec<Commit>) -> ShortlogOutput {
    let total_commits = commits.len();
    let mut author_map: HashMap<String, AuthorStats> = HashMap::new();

    for commit in commits {
        let author_name = commit.author.name.clone();
        let author_email = commit.author.email.clone();
        let key = if args.email {
            format!("{} <{}>", author_name, author_email)
        } else {
            author_name.clone()
        };

        let subject = commit.format_message();

        author_map
            .entry(key)
            .or_insert_with(|| AuthorStats::new(author_name.clone(), author_email.clone()))
            .add_commit(subject);
    }

    let mut authors: Vec<ShortlogAuthor> = author_map
        .into_values()
        .map(|stats| ShortlogAuthor {
            name: stats.name,
            email: args.email.then_some(stats.email),
            count: stats.count,
            subjects: if args.summary {
                Vec::new()
            } else {
                stats.subjects
            },
        })
        .collect();

    if args.numbered {
        authors.sort_by_key(|stats| (std::cmp::Reverse(stats.count), stats.name.to_lowercase()));
    } else {
        authors.sort_by_key(|stats| stats.name.to_lowercase());
    }

    ShortlogOutput {
        revision: revision.to_string(),
        numbered: args.numbered,
        summary: args.summary,
        email: args.email,
        total_authors: authors.len(),
        total_commits,
        authors,
    }
}

fn render_shortlog_output(output: &ShortlogOutput, writer: &mut impl Write) -> CliResult<()> {
    let max_count = output
        .authors
        .iter()
        .map(|stats| stats.count)
        .max()
        .unwrap_or(0);
    let width = std::cmp::max(4, max_count.to_string().len());

    for stats in &output.authors {
        if output.email {
            if !write_shortlog_line(
                writer,
                format_args!(
                    "{:>width$}  {} <{}>",
                    stats.count,
                    stats.name,
                    stats.email.as_deref().unwrap_or(""),
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

        if !output.summary {
            for subject in &stats.subjects {
                if !write_shortlog_line(writer, format_args!("      {}", subject))? {
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

async fn get_commits_for_shortlog(
    revision: Option<&str>,
    since_ts: Option<i64>,
    until_ts: Option<i64>,
) -> CliResult<Vec<Commit>> {
    use crate::command::log::get_reachable_commits;

    let revision = revision.unwrap_or("HEAD");
    let commit_hash = util::get_commit_base_typed(revision)
        .await
        .map_err(|error| shortlog_commit_base_error(revision, error))?
        .to_string();

    let mut commits: Vec<Commit> = get_reachable_commits(commit_hash, None)
        .await?
        .into_iter()
        .filter(|c| passes_filter(c, since_ts, until_ts))
        .collect();

    commits.sort_by_key(|b| std::cmp::Reverse(b.author.timestamp));

    Ok(commits)
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

fn parse_shortlog_date_arg(value: Option<&str>, flag: &str) -> CliResult<Option<i64>> {
    value.map(parse_date).transpose().map_err(|error| {
        CliError::fatal(format!("invalid {flag} date: {error}"))
            .with_stable_code(StableErrorCode::CliInvalidArguments)
            .with_hint(r#"supported formats: YYYY-MM-DD, "N days ago", unix timestamp"#)
    })
}

fn shortlog_commit_base_error(revision: &str, error: CommitBaseError) -> CliError {
    match error {
        CommitBaseError::HeadUnborn => CliError::fatal("HEAD does not point to a commit")
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint("create a commit before running 'libra shortlog'."),
        CommitBaseError::InvalidReference(message) => CliError::fatal(format!(
            "failed to resolve revision '{revision}': {message}"
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget),
        CommitBaseError::ReadFailure(message) => {
            CliError::fatal(message).with_stable_code(StableErrorCode::IoReadFailed)
        }
        CommitBaseError::CorruptReference(message) => {
            CliError::fatal(message).with_stable_code(StableErrorCode::RepoCorrupt)
        }
    }
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
