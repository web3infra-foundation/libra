//! Generate mbox-formatted email patch files from commits.
//!
//! `libra format-patch` walks a revision range, produces one `.patch` file per
//! non-merge commit, and formats each as an mbox message with RFC 2822 headers,
//! a plain-text diffstat, and a unified diff.  The output is designed for
//! email-based patch review and is compatible with `git am`.

use std::{
    collections::HashSet,
    io::Write,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use clap::Parser;
use git_internal::{hash::ObjectHash, internal::object::commit::Commit};
use serde::Serialize;

use crate::{
    command::log,
    common_utils::parse_commit_msg,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::OutputConfig,
        util,
    },
};

// ---------------------------------------------------------------------------
// EXAMPLES constant for `--help` (required by compat_help_examples_banner)
// ---------------------------------------------------------------------------
pub const FORMAT_PATCH_EXAMPLES: &str = "\
Examples:
  # Generate patches for the last two commits
  libra format-patch HEAD~2..HEAD

  # Numbered output to a directory
  libra format-patch -n -o patches/ main..feature

  # With cover letter and threading
  libra format-patch --cover-letter --thread origin/main..

  # Version 2 of a series, replying to a previous thread
  libra format-patch -v 2 --in-reply-to '<msgid@example>' origin/main..

  # All output to stdout (suitable for piping to `git am`)
  libra format-patch --stdout origin/main..
";

// ---------------------------------------------------------------------------
// Args
// ---------------------------------------------------------------------------

/// Generate mbox-formatted email patch files from commits.
#[derive(Parser, Debug)]
#[command(after_help = FORMAT_PATCH_EXAMPLES)]
pub struct FormatPatchArgs {
    /// Revision range expression: a single commit, or a range like A..B.
    /// When a single commit is given, all commits reachable from HEAD but not
    /// from that commit are included (equivalent to <commit>..HEAD).
    #[arg(value_name = "revision-range")]
    pub revision_range: Option<String>,

    /// Write patch files into DIR instead of the current working directory.
    #[arg(short = 'o', long = "output-directory", value_name = "DIR")]
    pub output_directory: Option<String>,

    /// Print all patches to stdout instead of individual files.
    #[arg(long = "stdout")]
    pub stdout: bool,

    /// Name output files in [PATCH n/m] order with a leading sequence number
    /// (e.g. "0001-subject.patch").
    #[arg(short = 'n', long = "numbered")]
    pub numbered: bool,

    /// Start numbering patches at N instead of 1.
    #[arg(long = "start-number", value_name = "N", default_value = "1")]
    pub start_number: usize,

    /// Use PREFIX instead of "PATCH" in the Subject: line.
    #[arg(
        long = "subject-prefix",
        value_name = "PREFIX",
        default_value = "PATCH"
    )]
    pub subject_prefix: String,

    /// Generate a 0000-cover-letter.patch template before the actual patches.
    #[arg(long = "cover-letter")]
    pub cover_letter: bool,

    /// Add In-Reply-To and References headers so mailers thread the series
    /// (default on).
    #[arg(long = "thread", default_value_t = true)]
    pub thread: bool,

    /// Disable threading headers.
    #[arg(long = "no-thread", overrides_with = "thread")]
    pub no_thread: bool,

    /// Make the first mail appear as a reply to the given Message-ID.
    #[arg(long = "in-reply-to", value_name = "MESSAGE_ID")]
    pub in_reply_to: Option<String>,

    /// Mark the patch series as version N (changes "[PATCH]" to "[PATCH vN]").
    #[arg(short = 'v', long = "reroll-count", value_name = "N")]
    pub reroll_count: Option<usize>,

    /// Append a Signed-off-by trailer to each commit message.
    #[arg(short = 's', long = "signoff")]
    pub signoff: bool,

    /// Show full object IDs in diff index header lines.
    #[arg(long = "full-index")]
    pub full_index: bool,

    /// Do not show the diffstat summary before the diff.
    #[arg(long = "no-stat")]
    pub no_stat: bool,

    /// Keep the original [PATCH] prefix if present in the commit subject,
    /// instead of stripping it (default behaviour in Git).
    #[arg(long = "keep-subject")]
    pub keep_subject: bool,
}

// ---------------------------------------------------------------------------
// Error enum
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
enum FormatPatchError {
    #[error("not a libra repository (or any parent)")]
    NotInRepo,

    #[error("{0}")]
    InvalidTarget(String),

    #[error("failed to write patch file '{}': {detail}", .path.display())]
    OutputWrite { path: PathBuf, detail: String },

    #[error("failed to create output directory '{}': {detail}", .dir)]
    OutputDirCreate { dir: String, detail: String },
}

impl From<FormatPatchError> for CliError {
    fn from(err: FormatPatchError) -> Self {
        match &err {
            FormatPatchError::NotInRepo => CliError::repo_not_found(),
            FormatPatchError::InvalidTarget(_) => CliError::failure(err.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget),
            FormatPatchError::OutputWrite { .. } => {
                CliError::fatal(err.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            FormatPatchError::OutputDirCreate { .. } => {
                CliError::fatal(err.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Convenience wrapper for callers that do not need structured output.
pub async fn execute(args: FormatPatchArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// # Side Effects
/// - Reads commit, tree, and blob objects from the object store.
/// - When `--stdout` is not set, creates `.patch` files in the output
///   directory (or the current working directory).  The working tree is
///   **not** modified.
///
/// # Errors
/// - `CliInvalidTarget` when the revision range resolves to zero commits or
///   a specified reference does not exist.
/// - `IoWriteFailed` when a patch file cannot be written or the output
///   directory cannot be created.
/// - Errors from lower-level object loading are forwarded as `CliError`.
pub async fn execute_safe(args: FormatPatchArgs, output: &OutputConfig) -> CliResult<()> {
    // 1. Ensure we are in a repo
    if util::require_repo().is_err() {
        return Err(CliError::from(FormatPatchError::NotInRepo));
    }

    // 2. Resolve revision range
    let commits = resolve_range_commits(&args).await?;
    if commits.is_empty() {
        return Err(CliError::failure("no patches to generate")
            .with_stable_code(StableErrorCode::CliInvalidTarget));
    }

    // 3. Determine output directory
    let out_dir = resolve_output_dir(&args)?;

    // 4. Build thread Message-ID (used for --thread / --in-reply-to)
    let thread_id = build_thread_id(&args, &commits);

    // 5. Determine numbering
    let total = commits.len();
    let start_num = args.start_number;

    // 6. Generate cover letter if requested
    let mut records = Vec::new();
    if args.cover_letter {
        let cover_body = format_cover_letter(&args, &commits)?;
        if !output.quiet {
            let path = write_patch_file(&args, &out_dir, 0, total, start_num, "", &cover_body)?;
            eprintln!("{}", path.display());
        } else {
            write_patch_file(&args, &out_dir, 0, total, start_num, "", &cover_body)?;
        }
    }

    // 7. Iterate commits and generate patches
    for (idx, commit) in commits.iter().enumerate() {
        let patch_num = start_num + idx;
        let patch_body =
            format_patch_body(&args, commit, patch_num, total, start_num, &thread_id).await?;
        let slug = patch_slug(commit, &args);
        if !output.quiet {
            let path = write_patch_file(
                &args,
                &out_dir,
                patch_num,
                total,
                start_num,
                &slug,
                &patch_body,
            )?;
            eprintln!("{}", path.display());
            records.push(PatchRecord {
                number: patch_num,
                commit: commit.id.to_string(),
                subject: commit_subject_line(commit),
                path: path.display().to_string(),
            });
        } else {
            let path = write_patch_file(
                &args,
                &out_dir,
                patch_num,
                total,
                start_num,
                &slug,
                &patch_body,
            )?;
            records.push(PatchRecord {
                number: patch_num,
                commit: commit.id.to_string(),
                subject: commit_subject_line(commit),
                path: path.display().to_string(),
            });
        }
    }

    // 8. Emit JSON if requested
    if output.is_json() {
        let envelope = FormatPatchOutput { patches: records };
        crate::utils::output::emit_json_data("format-patch", &envelope, output)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Structured output types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct FormatPatchOutput {
    patches: Vec<PatchRecord>,
}

#[derive(Serialize)]
struct PatchRecord {
    number: usize,
    commit: String,
    subject: String,
    path: String,
}

// ---------------------------------------------------------------------------
// Revision range resolution
// ---------------------------------------------------------------------------

/// Resolve `A..B` or a single revision into the ordered list of non-merge
/// commits to export (oldest first).  The excluded side (`A`) is reachable
/// from the included side (`B`); commits equal to or reachable from `A` are
/// stripped.
async fn resolve_range_commits(args: &FormatPatchArgs) -> Result<Vec<Commit>, CliError> {
    let spec = args.revision_range.as_deref().unwrap_or("HEAD");

    // Parse A..B notation
    let (exclude_tip_opt, include_tip) = if let Some((left, right)) = spec.split_once("..") {
        // Both empty sides default to HEAD (Git-compatible: ..HEAD equals
        // HEAD..HEAD producing zero patches).
        let left_spec = if left.is_empty() { "HEAD" } else { left };
        let right_spec = if right.is_empty() { "HEAD" } else { right };
        (
            Some(resolve_single_rev(left_spec).await?),
            resolve_single_rev(right_spec).await?,
        )
    } else {
        // Single revision: range is <spec>..HEAD
        let head = resolve_current_head().await?;
        (Some(resolve_single_rev(spec).await?), head)
    };

    // Collect the set of excluded OIDs (the exclude tip and all its ancestors)
    let excluded: HashSet<ObjectHash> = if let Some(exclude_tip) = exclude_tip_opt {
        let mut s = HashSet::new();
        s.insert(exclude_tip);
        if let Ok(ancestors) = log::get_reachable_commits(exclude_tip.to_string(), None).await {
            // INVARIANT: get_reachable_commits returns in BFS order; we only
            // need the OIDs for set membership so iteration is fine.
            for c in &ancestors {
                s.insert(c.id);
            }
        }
        s
    } else {
        HashSet::new()
    };

    // Walk from the included tip.  get_reachable_commits performs a BFS
    // traversal, returning commits in newest-first order (tip, then
    // parents, then grandparents, …).
    let all_reachable = log::get_reachable_commits(include_tip.to_string(), None)
        .await
        .map_err(|e| {
            FormatPatchError::InvalidTarget(format!("failed to walk commits from '{spec}': {e}"))
        })?;

    // Filter, deduplicate, then reverse so patches are numbered oldest-first.
    let mut commits: Vec<Commit> = all_reachable
        .into_iter()
        .filter(|c| !excluded.contains(&c.id))
        .filter(|c| c.parent_commit_ids.len() <= 1) // skip merge commits
        .collect();

    // Deduplicate (reachable set may already include the tip).
    let mut seen = HashSet::new();
    commits.retain(|c| seen.insert(c.id));

    // BFS returns newest-first; reverse gives oldest-first for linear history.
    commits.reverse();

    Ok(commits)
}

/// Resolve a single revision expression to an [`ObjectHash`].
async fn resolve_single_rev(spec: &str) -> Result<ObjectHash, CliError> {
    util::get_commit_base_typed(spec).await.map_err(|e| {
        FormatPatchError::InvalidTarget(format!("cannot resolve '{spec}': {e}")).into()
    })
}

/// Return the OID of `HEAD`.
async fn resolve_current_head() -> Result<ObjectHash, CliError> {
    use crate::internal::head::Head;
    Head::current_commit().await.ok_or_else(|| {
        FormatPatchError::InvalidTarget(
            "HEAD does not point to a commit (unborn branch?)".to_string(),
        )
        .into()
    })
}

// ---------------------------------------------------------------------------
// Patch body formatting
// ---------------------------------------------------------------------------

/// Assemble the complete mbox body for a single patch.
async fn format_patch_body(
    args: &FormatPatchArgs,
    commit: &Commit,
    patch_num: usize,
    total: usize,
    start_num: usize,
    thread_id: &Option<String>,
) -> Result<String, CliError> {
    let mut out = String::new();

    // ---- "From " mbox envelope ----
    let ts = timestamp_from_commit(commit);
    out.push_str(&format!(
        "From {} {}\n",
        commit.id,
        ts.format("%a %b %e %H:%M:%S %Y")
    ));

    // ---- From: ----
    out.push_str(&format!(
        "From: {} <{}>\n",
        commit.author.name.trim(),
        commit.author.email.trim()
    ));

    // ---- Date: (RFC 2822) ----
    out.push_str(&format!("Date: {}\n", ts.to_rfc2822()));

    // ---- Subject: ----
    let (raw_msg, _sig) = parse_commit_msg(&commit.message);
    let subject = raw_msg.lines().next().unwrap_or("").to_string();
    let subject = clean_subject(&subject, args.keep_subject);

    let version = args
        .reroll_count
        .map(|v| format!(" v{v}"))
        .unwrap_or_default();
    let prefix = format!("{prefix}{version}", prefix = args.subject_prefix);

    let num_str = if args.numbered || args.cover_letter {
        let width = number_width(total);
        format!(" {:0width$}/{}", patch_num, total + start_num - 1)
    } else {
        String::new()
    };

    out.push_str(&format!("Subject: [{prefix}{num_str}] {subject}\n"));

    // ---- Threading headers ----
    push_threading_headers(&mut out, thread_id, patch_num, start_num, total);

    // ---- MIME (always plain text UTF-8) ----
    out.push_str("MIME-Version: 1.0\n");
    out.push_str("Content-Type: text/plain; charset=UTF-8\n");
    out.push_str("Content-Transfer-Encoding: 8bit\n");

    // ---- Blank line: headers -> body ----
    out.push('\n');

    // ---- Commit message body ----
    let body = raw_msg
        .lines()
        .skip(1) // skip the subject line
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    if !body.is_empty() {
        out.push_str(&body);
        out.push('\n');
    }

    // ---- Signed-off-by ----
    if args.signoff {
        let (name, email) = resolve_signoff_identity().await;
        out.push_str(&format!("Signed-off-by: {name} <{email}>\n"));
    }

    // ---- "---" separator ----
    out.push_str("---\n");

    // ---- Diffstat ----
    if !args.no_stat
        && let Ok(stats) = log::compute_commit_stat(commit, vec![]).await
    {
        let stat_text = format_diffstat_plain(&stats);
        if !stat_text.is_empty() {
            out.push_str(&stat_text);
            out.push('\n');
        }
    }

    // ---- Unified diff ----
    let diff = log::generate_diff(commit, vec![]).await?;
    out.push_str(&diff);

    // ---- Footer ----
    out.push_str("-- \n");
    out.push_str(&format!("{}\n", env!("CARGO_PKG_VERSION")));

    Ok(out)
}

// ---------------------------------------------------------------------------
// Threading helpers
// ---------------------------------------------------------------------------

/// Build the thread Message-ID.  When `--in-reply-to` is given it becomes
/// the root; otherwise a deterministic ID is derived from the first commit.
fn build_thread_id(args: &FormatPatchArgs, commits: &[Commit]) -> Option<String> {
    if !args.thread || args.no_thread {
        return None;
    }
    if let Some(ref reply_to) = args.in_reply_to {
        return Some(reply_to.clone());
    }
    commits.first().map(|c| {
        let ts = c.committer.timestamp;
        let short = &c.id.to_string()[..8.min(c.id.to_string().len())];
        format!("{short}-{ts}@libra")
    })
}

/// Push `Message-ID`, `In-Reply-To`, and `References` headers.
fn push_threading_headers(
    out: &mut String,
    thread_id: &Option<String>,
    patch_num: usize,
    start_num: usize,
    total: usize,
) {
    let Some(msg_id) = thread_id else {
        return;
    };
    let is_first = patch_num == start_num;
    let patch_msg_id = if total == 1 || is_first {
        format!("Message-ID: <{msg_id}>\n")
    } else {
        format!(
            "Message-ID: <{msg_id}-p{patch_num}>\n\
             In-Reply-To: <{msg_id}>\n\
             References: <{msg_id}>\n"
        )
    };
    out.push_str(&patch_msg_id);
}

// ---------------------------------------------------------------------------
// Cover letter
// ---------------------------------------------------------------------------

/// Generate a cover-letter template (0000-cover-letter.patch).
fn format_cover_letter(args: &FormatPatchArgs, commits: &[Commit]) -> Result<String, CliError> {
    let now = Utc::now();

    let mut out = String::new();

    // "From " envelope
    out.push_str(&format!(
        "From 0000000000000000000000000000000000000000 {}\n",
        now.format("%a %b %e %H:%M:%S %Y")
    ));

    let version = args
        .reroll_count
        .map(|v| format!(" v{v}"))
        .unwrap_or_default();
    let prefix = format!("{}{}", args.subject_prefix, version);

    out.push_str("From: \n");
    out.push_str(&format!("Date: {}\n", now.to_rfc2822()));
    out.push_str(&format!(
        "Subject: [{prefix} 0/{total}] *** SUBJECT HERE ***\n",
        total = commits.len()
    ));
    out.push_str("MIME-Version: 1.0\n");
    out.push_str("Content-Type: text/plain; charset=UTF-8\n");
    out.push_str("Content-Transfer-Encoding: 8bit\n");
    out.push('\n');

    out.push_str("*** SUBJECT HERE ***\n\n");
    out.push_str("*** BLURB HERE ***\n\n");

    // Shortlog of the patches
    for (i, commit) in commits.iter().enumerate() {
        let (msg, _) = parse_commit_msg(&commit.message);
        let _subject = msg.lines().next().unwrap_or("");
        out.push_str(&format!(
            "{:0width$}-{}.patch\n",
            i + 1,
            patch_slug(commit, args),
            width = number_width(commits.len())
        ));
    }
    out.push('\n');

    out.push_str("-- \n");
    out.push_str(&format!("{}\n", env!("CARGO_PKG_VERSION")));

    Ok(out)
}

// ---------------------------------------------------------------------------
// Diffstat (plain-text, no ANSI colours)
// ---------------------------------------------------------------------------

/// Render a plain-text diffstat summary from [`log::FileStat`] records.
fn format_diffstat_plain(stats: &[log::FileStat]) -> String {
    const MAX_BAR: usize = 40;

    if stats.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let total_ins: usize = stats.iter().map(|s| s.insertions).sum();
    let total_del: usize = stats.iter().map(|s| s.deletions).sum();
    let total_files = stats.len();

    for stat in stats {
        let changes = stat.insertions + stat.deletions;
        let bar = if changes > MAX_BAR { MAX_BAR } else { changes };
        let plus = (stat.insertions * bar).checked_div(changes).unwrap_or(0);
        let minus = bar.saturating_sub(plus);

        out.push_str(&format!(
            " {} | {:>3} {}{}\n",
            stat.path,
            changes,
            "+".repeat(plus),
            "-".repeat(minus),
        ));
    }

    out.push_str(&format!(
        " {} file{} changed, {} insertion{}(+), {} deletion{}(-)\n",
        total_files,
        if total_files == 1 { "" } else { "s" },
        total_ins,
        if total_ins == 1 { "" } else { "s" },
        total_del,
        if total_del == 1 { "" } else { "s" },
    ));

    out
}

// ---------------------------------------------------------------------------
// Subject cleaning
// ---------------------------------------------------------------------------

/// Extract the first line of the commit message (subject).
fn commit_subject_line(commit: &Commit) -> String {
    let (msg, _) = parse_commit_msg(&commit.message);
    msg.lines().next().unwrap_or("").to_string()
}

/// Clean a commit subject for use in the `Subject:` header.
///
/// Strips leading `[PATCH ...]` / `[RFC ...]` bracketed prefixes unless
/// `--keep-subject` is set, and trims whitespace.
fn clean_subject(subject: &str, keep: bool) -> String {
    let s = subject.trim();
    if keep {
        return s.to_string();
    }
    // Strip leading bracket group(s), e.g. "[PATCH v2 3/5]" or "[RFC]"
    let mut cleaned = s;
    while let Some(rest) = cleaned
        .strip_prefix('[')
        .and_then(|t| t.split_once(']'))
        .map(|(_, rest)| rest.trim_start())
    {
        cleaned = rest;
    }
    cleaned.to_string()
}

// ---------------------------------------------------------------------------
// File naming
// ---------------------------------------------------------------------------

/// Build a safe filename slug from the commit subject.
fn patch_slug(commit: &Commit, _args: &FormatPatchArgs) -> String {
    let slug = commit_subject_line(commit)
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();

    // Collapse consecutive dashes
    let slug = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    truncate_slug(&slug)
}

/// Truncate a slug to at most ~52 characters without splitting a word segment.
fn truncate_slug(slug: &str) -> String {
    const MAX: usize = 52;
    if slug.len() <= MAX {
        return slug.to_string();
    }
    let end = slug[..MAX].rfind('-').unwrap_or(MAX);
    slug[..end].to_string()
}

/// Build the output file path for a single patch.
/// When `slug` is empty the file is treated as the cover letter.
fn patch_filename(
    numbered: bool,
    patch_num: usize,
    total: usize,
    start_num: usize,
    slug: &str,
) -> String {
    if slug.is_empty() {
        // Cover letter (only the cover-letter code path passes an empty slug)
        "0000-cover-letter.patch".to_string()
    } else if numbered {
        let width = number_width(total + start_num - 1);
        format!("{:0width$}-{}.patch", patch_num, slug)
    } else {
        format!("{}.patch", slug)
    }
}

/// Number of decimal digits needed to represent `n`.
fn number_width(n: usize) -> usize {
    if n == 0 {
        4
    } else {
        (n as f64).log10().floor() as usize + 1
    }
    .max(4)
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// Resolve the output directory: `--output-directory` value, or CWD.
/// Creates the directory if it does not already exist.
fn resolve_output_dir(args: &FormatPatchArgs) -> Result<PathBuf, CliError> {
    if let Some(dir) = &args.output_directory {
        let path = PathBuf::from(dir);
        std::fs::create_dir_all(&path).map_err(|e| FormatPatchError::OutputDirCreate {
            dir: dir.clone(),
            detail: e.to_string(),
        })?;
        Ok(path)
    } else {
        std::env::current_dir().map_err(|e| {
            FormatPatchError::OutputDirCreate {
                dir: "<current directory>".to_string(),
                detail: e.to_string(),
            }
            .into()
        })
    }
}

/// Write `body` to a `.patch` file (or stdout when `--stdout` is set).
/// Returns the display path.
fn write_patch_file(
    args: &FormatPatchArgs,
    out_dir: &Path,
    patch_num: usize,
    total: usize,
    start_num: usize,
    slug: &str,
    body: &str,
) -> Result<PathBuf, CliError> {
    if args.stdout {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(body.as_bytes()).map_err(|e| {
            CliError::fatal(format!("failed to write patch to stdout: {e}"))
                .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;
        stdout.write_all(b"\n").ok();
        Ok(PathBuf::from("<stdout>"))
    } else {
        let filename = patch_filename(
            args.numbered || args.cover_letter,
            patch_num,
            total,
            start_num,
            slug,
        );
        let path = out_dir.join(&filename);
        std::fs::write(&path, body).map_err(|e| FormatPatchError::OutputWrite {
            path: path.clone(),
            detail: e.to_string(),
        })?;
        Ok(path)
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Convert a commit's committer timestamp to a `DateTime<Utc>`.
fn timestamp_from_commit(commit: &Commit) -> DateTime<Utc> {
    DateTime::from_timestamp(commit.committer.timestamp as i64, 0).unwrap_or(DateTime::UNIX_EPOCH)
}

/// Resolve the Signed-off-by identity from `user.name` / `user.email` config.
async fn resolve_signoff_identity() -> (String, String) {
    let (_, committer) = util::create_signatures().await;
    (committer.name, committer.email)
}
