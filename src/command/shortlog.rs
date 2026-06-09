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

mod format;
mod mailmap;
#[cfg(test)]
mod mailmap_tests;
mod range;
mod render;
#[cfg(test)]
mod tests;
mod wrap;

use std::{collections::HashMap, io::Write};

use clap::Parser;
use git_internal::internal::object::commit::Commit;
use serde::Serialize;

use self::{mailmap::Mailmap, wrap::WrapOptions};
use crate::utils::{
    error::{CliError, CliResult, emit_warning},
    output::{OutputConfig, emit_json_data},
    util::{self, require_repo},
};

const SHORTLOG_EXAMPLES: &str = "\
EXAMPLES:
    libra shortlog                  Summarize commits reachable from HEAD by author
    libra shortlog HEAD~5           Summarize a subset of history starting from a revision
    libra shortlog -n -s            Sort by commit count, suppress subjects (count only)
    libra shortlog -c -s            Summarize by committer instead of author
    libra shortlog --no-merges      Exclude merge commits from the summary
    libra shortlog -w=72            Wrap subject lines at 72 columns
    libra shortlog --format '%h %s' Render per-commit descriptions with a template
    libra shortlog --since 24h      Restrict to commits in the last 24 hours
    libra shortlog --json           Structured JSON output for agents";

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

    /// Group commits by committer identity instead of author.
    #[clap(short = 'c', long = "committer")]
    pub committer: bool,

    /// Do not include merge commits (commits with more than one parent).
    #[clap(long = "no-merges")]
    pub no_merges: bool,

    /// Wrap subject lines; use -w for Git defaults or -w=<width>,<indent1>,<indent2>.
    #[clap(
        short = 'w',
        num_args = 0..=1,
        default_missing_value = "76,6,9",
        require_equals = true,
        value_name = "WIDTH"
    )]
    pub width: Option<Option<String>>,

    /// Render each commit description using a limited pretty-format template.
    #[clap(long = "format", value_name = "FORMAT")]
    pub format: Option<String>,

    /// Show commits more recent than DATE (RFC3339, `YYYY-MM-DD`, or relative like `24h` / `7d`)
    #[clap(long = "since", value_name = "DATE")]
    pub since: Option<String>,

    /// Show commits older than DATE (RFC3339, `YYYY-MM-DD`, or relative like `1h`)
    #[clap(long = "until", value_name = "DATE")]
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
    #[serde(skip)]
    wrap: Option<WrapOptions>,
}

/// Runs shortlog and writes **human-readable** output to the given writer.
///
/// This function always produces the human-formatted report regardless of
/// `OutputConfig` or `--json`. It is used by tests and callers that need
/// direct writer control. For the full CLI entry point that honours JSON /
/// quiet modes, use [`execute_safe`].
pub async fn execute_to(args: ShortlogArgs, writer: &mut impl Write) -> CliResult<()> {
    require_repo().map_err(|_| CliError::repo_not_found())?;
    let run = run_shortlog(&args).await?;
    render::render_shortlog_output(&run.output, writer)?;
    emit_shortlog_warnings(run.warnings);
    Ok(())
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
    require_repo().map_err(|_| CliError::repo_not_found())?;
    let run = run_shortlog(&args).await?;

    if output.is_json() {
        emit_json_data("shortlog", &run.output, output)?;
    } else if !output.quiet {
        let mut stdout = std::io::stdout();
        render::render_shortlog_output(&run.output, &mut stdout)?;
    }

    emit_shortlog_warnings(run.warnings);
    Ok(())
}

struct ShortlogRun {
    output: ShortlogOutput,
    warnings: Vec<String>,
}

async fn run_shortlog(args: &ShortlogArgs) -> CliResult<ShortlogRun> {
    let since_ts = range::parse_shortlog_date_arg(args.since.as_deref(), "--since")?;
    let until_ts = range::parse_shortlog_date_arg(args.until.as_deref(), "--until")?;
    let wrap = wrap::parse_width_arg(&args.width)?;
    let revision = args.revision.clone().unwrap_or_else(|| "HEAD".to_string());
    let mut commits =
        range::get_commits_for_shortlog(args.revision.as_deref(), since_ts, until_ts).await?;

    if args.no_merges {
        commits.retain(|commit| commit.parent_commit_ids.len() <= 1);
    }

    let workdir = util::try_working_dir().map_err(|_| CliError::repo_not_found())?;
    let mailmap_load = mailmap::load_mailmap(&workdir);
    let output = aggregate_shortlog(args, &revision, commits, &mailmap_load.mailmap, wrap)?;

    Ok(ShortlogRun {
        output,
        warnings: mailmap_load.warnings,
    })
}

fn aggregate_shortlog(
    args: &ShortlogArgs,
    revision: &str,
    commits: Vec<Commit>,
    mailmap: &Mailmap,
    wrap: Option<WrapOptions>,
) -> CliResult<ShortlogOutput> {
    let total_commits = commits.len();
    let mut author_map: HashMap<String, AuthorStats> = HashMap::new();

    for commit in commits {
        let signature = if args.committer {
            &commit.committer
        } else {
            &commit.author
        };
        let (ident_name, ident_email) = mailmap.resolve(&signature.name, &signature.email);
        let key = if args.email {
            format!("{} <{}>", ident_name, ident_email)
        } else {
            ident_name.clone()
        };

        let subject = format::format_subject(&commit, args.format.as_deref())?;

        author_map
            .entry(key)
            .or_insert_with(|| AuthorStats::new(ident_name.clone(), ident_email.clone()))
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

    Ok(ShortlogOutput {
        revision: revision.to_string(),
        numbered: args.numbered,
        summary: args.summary,
        email: args.email,
        total_authors: authors.len(),
        total_commits,
        authors,
        wrap,
    })
}

fn emit_shortlog_warnings(warnings: Vec<String>) {
    for warning in warnings {
        emit_warning(warning);
    }
}
