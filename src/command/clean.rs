//! Implements `clean` to remove untracked files from the working tree.

use std::{
    fs,
    io::{BufRead, Write},
    path::{Path, PathBuf},
};

use clap::Parser;
use git_internal::internal::index::Index;
use serde::Serialize;

use crate::utils::{
    error::{CliError, CliResult, StableErrorCode},
    ignore::{self, IgnorePolicy},
    output::{OutputConfig, emit_json_data},
    path, util, worktree,
};

const CLEAN_EXAMPLES: &str = "\
EXAMPLES:
    libra clean -n                      Preview what would be removed (dry-run)
    libra clean -f                      Remove untracked files (files only)
    libra clean -fd                     Also remove untracked directories
    libra clean -fx                     Remove untracked files including ignored ones
    libra clean -fX                     Remove only ignored files
    libra clean -i                      Interactively choose which items to remove
    libra clean -n -ffd                 Preview removing nested repos too (-ff) before deleting
    libra clean -f --exclude '*.log'    Layer an additional exclusion on top of .libraignore
    libra clean -n --json               Structured JSON output for agents";

#[derive(Parser, Debug, Clone, Default)]
#[command(after_help = CLEAN_EXAMPLES)]
pub struct CleanArgs {
    /// Show what would be removed without actually removing
    #[clap(short = 'n', long)]
    pub dry_run: bool,
    /// Force removal of untracked files (repeat as `-ff` to also remove nested repositories)
    #[clap(short, long, action = clap::ArgAction::Count)]
    pub force: u8,
    /// Interactively choose which untracked items to remove (mutually exclusive with --json)
    #[clap(short = 'i', long)]
    pub interactive: bool,
    /// Remove untracked directories in addition to untracked files
    #[clap(short = 'd', long = "dir")]
    pub directories: bool,
    /// Remove all untracked files, including those in .gitignore/.libraignore
    #[clap(short = 'x')]
    pub ignored: bool,
    /// Remove only untracked files that are in .gitignore/.libraignore
    #[clap(short = 'X')]
    pub only_ignored: bool,
    /// Exclude files matching the given pattern (can be repeated)
    #[clap(short = 'e', long = "exclude", value_name = "pattern")]
    pub exclude: Vec<String>,
    /// Pathspec filter (not supported; Phase 1 / Phase 1 rejection)
    #[clap(value_name = "pathspec", trailing_var_arg = true, allow_hyphen_values = true)]
    pub pathspec: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct CleanOutput {
    dry_run: bool,
    removed: Vec<String>,
    /// Paths that could not be removed (tolerant cleanup). Back-compatible:
    /// omitted from the JSON envelope when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    failed: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
enum CleanError {
    #[error(
        "clean requires -f, -n, or -i (use -f to remove files, -n to dry-run, -i for interactive; or set clean.requireForce=false)"
    )]
    MissingMode,
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    #[error("failed to load index: {0}")]
    LoadIndex(String),
    #[error("{0}")]
    ScanUntracked(String),
    #[error("failed to resolve working directory: {0}")]
    ResolveWorkdir(String),
    #[error("failed to resolve path {path}: {detail}")]
    ResolvePath { path: String, detail: String },
    #[error("refusing to remove path outside workdir: {0}")]
    OutsideWorkdir(String),
    #[error("failed to remove {path}: {detail}")]
    RemoveFile { path: String, detail: String },
    #[error("interactive clean I/O error: {detail}")]
    Io { detail: String },
    #[error("libra: clean <pathspec> is not supported (see declined.md#D17); use -f or explicit file removal instead")]
    PathspecDeclined,
}

pub async fn execute(args: CleanArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting.
///
/// # Side Effects
/// - Scans the working tree for untracked files.
/// - Removes matching files unless `--dry-run` is active.
/// - Renders removed or would-remove paths in human or JSON form.
///
/// # Errors
/// Returns [`CliError`] when the command is run outside a repository, candidate
/// paths cannot be resolved safely, a path escapes the worktree, or removal
/// fails.
pub async fn execute_safe(args: CleanArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;

    if !args.pathspec.is_empty() {
        return Err(clean_cli_error(CleanError::PathspecDeclined));
    }

    preflight(&args, output).await.map_err(clean_cli_error)?;

    let clean_output = run_clean(args, output.quiet).map_err(clean_cli_error)?;

    // Scheme A: emit the success listing first, then signal a non-zero exit when
    // some removals failed. Human and JSON modes diverge so neither double-renders.
    let failure_count = clean_output.failed.len();
    let first_failed = clean_output.failed.first().cloned();

    if output.is_json() {
        emit_json_data("clean", &clean_output, output)?;
        if failure_count > 0 {
            // The failure detail is carried by the success envelope's `failed`
            // field; exit 128 without a second (error) envelope.
            return Err(CliError::silent_exit(128));
        }
    } else {
        if !output.quiet {
            for path in &clean_output.removed {
                if clean_output.dry_run {
                    println!("Would remove {path}");
                } else {
                    println!("Removing {path}");
                }
            }
        }
        if failure_count > 0 {
            let path = first_failed.unwrap_or_default();
            let detail = if failure_count == 1 {
                "removal failed".to_string()
            } else {
                format!("removal failed ({failure_count} paths)")
            };
            return Err(clean_cli_error(CleanError::RemoveFile { path, detail }));
        }
    }

    Ok(())
}

/// Validate argument combinations and the `clean.requireForce` safety fuse
/// before any scan/removal runs. This is the single source of truth for the
/// "missing run mode" decision (`run_clean` no longer re-checks it).
async fn preflight(args: &CleanArgs, output: &OutputConfig) -> Result<(), CleanError> {
    use crate::internal::config::{
        LocalIdentityTarget, parse_config_bool, read_cascaded_config_value,
    };

    // Interactive mode is human-only and self-previews, so it cannot be combined
    // with machine output or a separate dry-run.
    if args.interactive && output.is_json() {
        return Err(CleanError::InvalidArgs(
            "cannot use --interactive and --json together".to_string(),
        ));
    }
    if args.interactive && args.dry_run {
        return Err(CleanError::InvalidArgs(
            "cannot use --interactive and --dry-run together".to_string(),
        ));
    }
    if args.ignored && args.only_ignored {
        return Err(CleanError::InvalidArgs(
            "cannot use -x and -X together".to_string(),
        ));
    }

    // `clean.requireForce` (documented spelling; the config store matches keys
    // exactly). Default `true`, matching Git; any read/parse failure falls back
    // to the safe default rather than panicking.
    let require_force =
        match read_cascaded_config_value(LocalIdentityTarget::CurrentRepo, "clean.requireForce")
            .await
        {
            Ok(Some(raw)) => parse_config_bool(&raw).unwrap_or(true),
            Ok(None) => true,
            Err(error) => {
                tracing::debug!(%error, "failed to read clean.requireForce; defaulting to true");
                true
            }
        };

    if require_force && args.force == 0 && !args.dry_run && !args.interactive {
        return Err(CleanError::MissingMode);
    }

    Ok(())
}

fn run_clean(args: CleanArgs, quiet: bool) -> Result<CleanOutput, CleanError> {
    // Mode validation (missing-mode / -x+-X / -i+--json / -i+-n) is performed once
    // in `preflight`; `run_clean` assumes the arguments have already been vetted.
    let candidates = collect_clean_candidates(&args, quiet)?;

    if args.interactive {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let mut reader = stdin.lock();
        let mut writer = stdout.lock();
        let selected = run_interactive_loop(&mut reader, &mut writer, &args, &candidates)?;
        return delete_clean_candidates(&selected, quiet);
    }

    if candidates.is_empty() {
        return Ok(CleanOutput {
            dry_run: args.dry_run,
            ..Default::default()
        });
    }

    if args.dry_run {
        return Ok(CleanOutput {
            dry_run: true,
            removed: candidates.iter().map(|p| p.display().to_string()).collect(),
            ..Default::default()
        });
    }

    delete_clean_candidates(&candidates, quiet)
}

/// Scan the working tree and return the final list of removal candidates
/// (relative paths) after the ignore policy, nested-repo pruning, `-d` directory
/// merge, and `--exclude` filters. Performs NO removal and NO dry-run shortcut,
/// so it can feed both the non-interactive delete path and the interactive loop.
fn collect_clean_candidates(args: &CleanArgs, quiet: bool) -> Result<Vec<PathBuf>, CleanError> {
    let index_path = path::index();
    let index = match Index::load(&index_path) {
        Ok(index) => index,
        Err(e) => {
            if !index_path.exists() {
                Index::new()
            } else {
                return Err(CleanError::LoadIndex(e.to_string()));
            }
        }
    };

    // Determine the ignore policy based on flags
    let policy = if args.only_ignored {
        IgnorePolicy::OnlyIgnored
    } else if args.ignored {
        IgnorePolicy::IncludeIgnored
    } else {
        IgnorePolicy::Respect
    };

    // Collect workdir files and apply ignore policy. The default path can prune ignored
    // directories; -x/-X must still inspect ignored files because those modes target them.
    let workdir_files = match policy {
        IgnorePolicy::Respect => util::list_workdir_files(),
        IgnorePolicy::IncludeIgnored | IgnorePolicy::OnlyIgnored => {
            util::list_workdir_files_unfiltered()
        }
    }
    .map_err(|e| CleanError::ScanUntracked(e.to_string()))?;
    let filtered_files = ignore::filter_workdir_paths(workdir_files, policy, &index);

    // Find untracked files
    let mut untracked: Vec<PathBuf> = Vec::new();
    for path in filtered_files {
        let path_str = path.to_str().ok_or_else(|| {
            CleanError::ScanUntracked(format!("path {:?} is not valid UTF-8", path))
        })?;
        if !worktree::index_has_any_stage(&index, path_str) {
            untracked.push(path);
        }
    }

    // Nested-repository protection: a directory whose direct children include
    // `.git` or `.libra` is an independent repository. Without a second `-f`
    // (`force >= 2`), it (and every file under it) is pruned so a stray `clean`
    // never wipes out an unrelated checkout. This guards BOTH the file-level
    // candidates above and the `-d` directory candidates below.
    let force_double = args.force >= 2;
    let nested_roots = find_nested_repo_roots()?;
    if !force_double && !nested_roots.is_empty() {
        untracked.retain(|p| !nested_roots.iter().any(|root| p.starts_with(root)));
        if !quiet {
            for root in &nested_roots {
                eprintln!("Skipping repository {}", root.display());
            }
        }
    }

    // If -d, also find untracked directories
    if args.directories {
        let untracked_dirs = find_untracked_dirs(&index, policy)?;
        for dir in untracked_dirs {
            // Skip the root directory (empty path)
            if dir.as_os_str().is_empty() {
                continue;
            }
            // Skip nested repositories unless `-ff` was given.
            if !force_double && nested_roots.iter().any(|root| dir.starts_with(root)) {
                continue;
            }
            // Remove any files that are inside this directory from the untracked list
            // since the directory itself will be removed
            untracked.retain(|p| !p.starts_with(&dir));
            // Add the directory if it's not already covered by a parent directory
            if !untracked.iter().any(|p| dir.starts_with(p)) {
                untracked.push(dir);
            }
        }
    }

    // Apply --exclude patterns
    if !args.exclude.is_empty() {
        untracked.retain(|path| {
            let path_str = path.display().to_string();
            !args
                .exclude
                .iter()
                .any(|pattern| matches_exclude_pattern(&path_str, pattern))
        });
    }

    Ok(untracked)
}

/// Physically remove the given candidate paths, tolerating per-path failures.
///
/// A failure to delete a single path (e.g. a read-only file) is warned about and
/// recorded in `CleanOutput.failed` while cleanup continues. The workdir-escape
/// check stays fatal — it indicates a symlink/traversal attack, not a hiccup.
fn delete_clean_candidates(candidates: &[PathBuf], quiet: bool) -> Result<CleanOutput, CleanError> {
    let workdir = fs::canonicalize(util::working_dir())
        .map_err(|e| CleanError::ResolveWorkdir(e.to_string()))?;
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    for path in candidates {
        let abs_path = util::workdir_to_absolute(path);
        if !abs_path.exists() {
            continue;
        }
        let resolved = fs::canonicalize(&abs_path).map_err(|e| CleanError::ResolvePath {
            path: abs_path.display().to_string(),
            detail: e.to_string(),
        })?;
        if !resolved.starts_with(&workdir) {
            return Err(CleanError::OutsideWorkdir(abs_path.display().to_string()));
        }
        let outcome = if abs_path.is_dir() {
            fs::remove_dir_all(&abs_path)
        } else {
            fs::remove_file(&abs_path)
        };
        match outcome {
            Ok(()) => removed.push(path.display().to_string()),
            Err(error) => {
                if !quiet {
                    eprintln!("warning: failed to remove {}: {error}", path.display());
                }
                failed.push(path.display().to_string());
            }
        }
    }
    Ok(CleanOutput {
        dry_run: false,
        removed,
        failed,
    })
}

/// Find nested repository roots inside the working tree: directories whose
/// direct children include a `.git` or `.libra` folder. Such a directory is an
/// independent repository and is pruned (not recursed) here. The working-tree
/// root itself is never reported (it owns the current repo's `.libra`).
fn find_nested_repo_roots() -> Result<Vec<PathBuf>, CleanError> {
    let workdir = util::working_dir();
    let mut roots = Vec::new();

    fn walk(dir: &Path, workdir: &Path, roots: &mut Vec<PathBuf>) -> Result<(), CleanError> {
        let entries = fs::read_dir(dir).map_err(|e| CleanError::ScanUntracked(e.to_string()))?;
        for entry in entries {
            let entry = entry.map_err(|e| CleanError::ScanUntracked(e.to_string()))?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = path.file_name().unwrap_or_default();
            if name == ".git" || name == util::ROOT_DIR {
                continue;
            }
            if path.join(".git").exists() || path.join(util::ROOT_DIR).exists() {
                if let Ok(relative) = path.strip_prefix(workdir) {
                    roots.push(relative.to_path_buf());
                }
                // Prune: do not descend into a nested repository.
                continue;
            }
            walk(&path, workdir, roots)?;
        }
        Ok(())
    }

    walk(&workdir, &workdir, &mut roots)?;
    Ok(roots)
}

/// The seven help lines rendered by the interactive `help` subcommand. Mirrors
/// the layout of `git clean -i`'s help so the experience is familiar.
const INTERACTIVE_HELP_LINES: [&str; 7] = [
    "clean               - start cleaning",
    "filter by pattern   - exclude items from deletion",
    "select by numbers   - select items to be deleted by numbers",
    "ask each            - confirm each deletion (like \"rm -i\")",
    "quit                - stop cleaning",
    "help                - this screen",
    "?                   - help for prompt selection",
];

/// Run the interactive `clean -i` selection loop against `reader`/`writer`.
///
/// This is a pure state machine: it NEVER touches the filesystem. It starts with
/// every candidate selected, lets the user refine the selection via the same six
/// subcommands Git offers (`clean`, `filter by pattern`, `select by numbers`,
/// `ask each`, `quit`, `help`), and returns the final list of paths to delete.
/// Taking generic `BufRead`/`Write` makes it unit-testable with `io::Cursor`.
///
/// EOF (a `read_line` of 0 bytes) is treated as `quit` so a closed/piped stdin
/// can never hang the loop. `quit` returns an empty selection.
fn run_interactive_loop<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    _args: &CleanArgs,
    candidates: &[PathBuf],
) -> Result<Vec<PathBuf>, CleanError> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // `selected[i]` tracks whether candidate `i` is still slated for removal.
    let mut selected = vec![true; candidates.len()];

    loop {
        render_candidate_list(writer, candidates, &selected)?;
        write!(
            writer,
            "*** Commands ***\n    1: clean                2: filter by pattern    3: select by numbers\n    4: ask each             5: quit                 6: help\nWhat now> "
        )
        .map_err(map_io_write)?;
        writer.flush().map_err(map_io_write)?;

        let Some(line) = read_line(reader)? else {
            // EOF: behave like `quit` to avoid hanging on a closed stdin.
            return Ok(Vec::new());
        };
        let choice = line.trim();
        match interactive_command(choice) {
            InteractiveCommand::Clean => {
                return Ok(collect_selected(candidates, &selected));
            }
            InteractiveCommand::FilterByPattern => {
                interactive_filter_by_pattern(reader, writer, candidates, &mut selected)?;
            }
            InteractiveCommand::SelectByNumbers => {
                interactive_select_by_numbers(reader, writer, candidates, &mut selected)?;
            }
            InteractiveCommand::AskEach => {
                interactive_ask_each(reader, writer, candidates, &mut selected)?;
            }
            InteractiveCommand::Quit => {
                return Ok(Vec::new());
            }
            InteractiveCommand::Help => {
                for help_line in INTERACTIVE_HELP_LINES {
                    writeln!(writer, "{help_line}").map_err(map_io_write)?;
                }
            }
            InteractiveCommand::Unknown => {
                writeln!(writer, "Huh ({choice})?").map_err(map_io_write)?;
            }
        }
    }
}

/// The six interactive subcommands plus an `Unknown` fallback.
enum InteractiveCommand {
    Clean,
    FilterByPattern,
    SelectByNumbers,
    AskEach,
    Quit,
    Help,
    Unknown,
}

/// Map a raw menu entry to a command. Accepts the leading number, the full word,
/// or a case-insensitive first-letter shortcut — matching `git clean -i`.
fn interactive_command(choice: &str) -> InteractiveCommand {
    let normalized = choice.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "" => InteractiveCommand::Unknown,
        "1" | "c" | "clean" => InteractiveCommand::Clean,
        "2" | "f" | "filter by pattern" | "filter" => InteractiveCommand::FilterByPattern,
        "3" | "s" | "select by numbers" | "select" => InteractiveCommand::SelectByNumbers,
        "4" | "a" | "ask each" | "ask" => InteractiveCommand::AskEach,
        "5" | "q" | "quit" => InteractiveCommand::Quit,
        "6" | "h" | "help" => InteractiveCommand::Help,
        "?" => InteractiveCommand::Help,
        _ => InteractiveCommand::Unknown,
    }
}

/// Render the numbered candidate list, marking still-selected entries with `*`.
fn render_candidate_list<W: Write>(
    writer: &mut W,
    candidates: &[PathBuf],
    selected: &[bool],
) -> Result<(), CleanError> {
    writeln!(writer, "Would remove the following items:").map_err(map_io_write)?;
    for (idx, path) in candidates.iter().enumerate() {
        let marker = if selected[idx] { '*' } else { ' ' };
        writeln!(writer, "  {marker} {:>3}: {}", idx + 1, path.display()).map_err(map_io_write)?;
    }
    Ok(())
}

/// `filter by pattern`: read space-separated globs and deselect any candidate
/// matched by one — directly or via one of its ancestors. A blank line returns.
fn interactive_filter_by_pattern<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    candidates: &[PathBuf],
    selected: &mut [bool],
) -> Result<(), CleanError> {
    loop {
        write!(writer, "Input ignore patterns>> ").map_err(map_io_write)?;
        writer.flush().map_err(map_io_write)?;
        let Some(line) = read_line(reader)? else {
            return Ok(());
        };
        let patterns: Vec<&str> = line.split_whitespace().collect();
        if patterns.is_empty() {
            return Ok(());
        }
        for (idx, path) in candidates.iter().enumerate() {
            if !selected[idx] {
                continue;
            }
            if patterns
                .iter()
                .any(|pattern| pattern_matches_with_ancestors(path, pattern))
            {
                selected[idx] = false;
            }
        }
        render_candidate_list(writer, candidates, selected)?;
    }
}

/// `select by numbers`: toggle selection by index. Accepts comma/space-separated
/// tokens: a bare number selects, `N-M` a closed range, `N-` an open range, `*`
/// all, and a leading `-` deselects (`-3`, `-2-5`). Out-of-range tokens are
/// ignored. After applying, the selection becomes ONLY the marked items.
fn interactive_select_by_numbers<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    candidates: &[PathBuf],
    selected: &mut [bool],
) -> Result<(), CleanError> {
    write!(writer, "Select items to delete>> ").map_err(map_io_write)?;
    writer.flush().map_err(map_io_write)?;
    let Some(line) = read_line(reader)? else {
        return Ok(());
    };
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    let tokens: Vec<&str> = trimmed
        .split([',', ' '])
        .filter(|t| !t.is_empty())
        .collect();
    let any_select = tokens.iter().any(|t| !t.starts_with('-'));

    if any_select {
        // At least one positive token: the selection is REPLACED by exactly the
        // marked items (positive tokens set, trailing `-` tokens then clear).
        let mut next = vec![false; candidates.len()];
        for token in &tokens {
            match token.strip_prefix('-') {
                Some(rest) => apply_select_token(rest, candidates.len(), true, &mut next),
                None => apply_select_token(token, candidates.len(), false, &mut next),
            }
        }
        selected.copy_from_slice(&next);
    } else {
        // Pure deselect (e.g. `-3`): refine the EXISTING selection in place.
        for token in &tokens {
            if let Some(rest) = token.strip_prefix('-') {
                apply_select_token(rest, candidates.len(), true, selected);
            }
        }
    }
    render_candidate_list(writer, candidates, selected)?;
    Ok(())
}

/// Apply one selection token (`*`, `N`, `N-M`, `N-`) to `target`. `deselect`
/// inverts the operation. Indices are 1-based on input; out-of-range is ignored.
fn apply_select_token(body: &str, len: usize, deselect: bool, target: &mut [bool]) {
    let value = !deselect;
    if body == "*" {
        target.iter_mut().for_each(|slot| *slot = value);
        return;
    }
    if let Some((start, end)) = body.split_once('-') {
        let start_idx = start.trim().parse::<usize>().ok();
        let end_idx = if end.trim().is_empty() {
            Some(len)
        } else {
            end.trim().parse::<usize>().ok()
        };
        if let (Some(start_idx), Some(end_idx)) = (start_idx, end_idx)
            && start_idx >= 1
            && start_idx <= end_idx
        {
            for one_based in start_idx..=end_idx.min(len) {
                target[one_based - 1] = value;
            }
        }
        return;
    }
    if let Ok(one_based) = body.trim().parse::<usize>()
        && one_based >= 1
        && one_based <= len
    {
        target[one_based - 1] = value;
    }
}

/// `ask each`: walk the selected candidates, prompt `Remove <path>? [y/N]`, and
/// keep only the ones answered yes. This NEVER deletes — it just refines the set.
fn interactive_ask_each<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    candidates: &[PathBuf],
    selected: &mut [bool],
) -> Result<(), CleanError> {
    for (idx, path) in candidates.iter().enumerate() {
        if !selected[idx] {
            continue;
        }
        write!(writer, "Remove {}? [y/N] ", path.display()).map_err(map_io_write)?;
        writer.flush().map_err(map_io_write)?;
        let Some(line) = read_line(reader)? else {
            // EOF: leave the remaining answers as the default (No) and stop.
            selected[idx] = false;
            for slot in selected.iter_mut().skip(idx + 1) {
                *slot = false;
            }
            return Ok(());
        };
        let answer = line.trim().to_ascii_lowercase();
        selected[idx] = matches!(answer.as_str(), "y" | "yes");
    }
    Ok(())
}

/// Collect the still-selected candidates into a fresh `Vec`.
fn collect_selected(candidates: &[PathBuf], selected: &[bool]) -> Vec<PathBuf> {
    candidates
        .iter()
        .zip(selected)
        .filter(|&(_, keep)| *keep)
        .map(|(path, _)| path.clone())
        .collect()
}

/// Does `pattern` match `path` directly, or any of its ancestor directories?
///
/// Ancestor inheritance means filtering `build` also removes `build/out/app.js`,
/// matching how `git clean -i`'s pattern filter prunes whole subtrees.
fn pattern_matches_with_ancestors(path: &Path, pattern: &str) -> bool {
    let path_str = path.display().to_string();
    if matches_exclude_pattern(&path_str, pattern) {
        return true;
    }
    let mut current = path;
    while let Some(parent) = current.parent() {
        if parent.as_os_str().is_empty() {
            break;
        }
        let parent_str = parent.display().to_string();
        if matches_exclude_pattern(&parent_str, pattern) {
            return true;
        }
        current = parent;
    }
    false
}

/// Read one line, returning `None` on EOF (a 0-byte read). The trailing newline
/// is left intact for callers that `trim()`; `read_line`-style semantics.
fn read_line<R: BufRead>(reader: &mut R) -> Result<Option<String>, CleanError> {
    let mut buffer = String::new();
    let bytes = reader.read_line(&mut buffer).map_err(|e| CleanError::Io {
        detail: e.to_string(),
    })?;
    if bytes == 0 {
        Ok(None)
    } else {
        Ok(Some(buffer))
    }
}

/// Map a write failure on the interactive stream to a `CleanError`.
fn map_io_write(error: std::io::Error) -> CleanError {
    CleanError::Io {
        detail: error.to_string(),
    }
}

/// Find untracked directories based on the ignore policy.
/// A directory is considered untracked if it does not contain any tracked files.
fn find_untracked_dirs(index: &Index, policy: IgnorePolicy) -> Result<Vec<PathBuf>, CleanError> {
    let workdir = util::working_dir();
    let mut untracked_dirs = Vec::new();

    fn scan_dir(
        dir: &Path,
        workdir: &Path,
        index: &Index,
        policy: IgnorePolicy,
        untracked_dirs: &mut Vec<PathBuf>,
    ) -> Result<(), CleanError> {
        let entries = fs::read_dir(dir).map_err(|e| CleanError::ScanUntracked(e.to_string()))?;
        let mut has_tracked = false;
        let mut subdirs = Vec::new();

        for entry in entries {
            let entry = entry.map_err(|e| CleanError::ScanUntracked(e.to_string()))?;
            let path = entry.path();
            let relative = path
                .strip_prefix(workdir)
                .map_err(|e| CleanError::ScanUntracked(e.to_string()))?;

            if path.is_dir() {
                let name = path.file_name().unwrap_or_default();
                if name == ".git" || name == util::ROOT_DIR {
                    continue;
                }
                if policy == IgnorePolicy::Respect
                    && ignore::should_ignore(relative, IgnorePolicy::Respect, index)
                {
                    continue;
                }
                subdirs.push(path.clone());
            } else if let Some(path_str) = relative.to_str() {
                // Check if this file is tracked
                if index.tracked(path_str, 0) {
                    has_tracked = true;
                }
            }
        }

        if !has_tracked {
            // Check if this directory should be ignored
            let relative = dir
                .strip_prefix(workdir)
                .map_err(|e| CleanError::ScanUntracked(e.to_string()))?;
            let should_include = match policy {
                IgnorePolicy::Respect => {
                    // Only include if not ignored
                    !ignore::should_ignore(relative, policy, index)
                }
                IgnorePolicy::IncludeIgnored => true,
                IgnorePolicy::OnlyIgnored => {
                    // Only include if ignored
                    ignore::should_ignore(relative, IgnorePolicy::Respect, index)
                }
            };
            if should_include {
                untracked_dirs.push(relative.to_path_buf());
            }
        }

        // Recurse into subdirs
        for subdir in subdirs {
            scan_dir(&subdir, workdir, index, policy, untracked_dirs)?;
        }

        Ok(())
    }

    scan_dir(&workdir, &workdir, index, policy, &mut untracked_dirs)?;
    Ok(untracked_dirs)
}

/// Check if a path matches an exclude pattern using glob-style matching.
/// Supports * (match any characters) and ? (match single character).
fn matches_exclude_pattern(path: &str, pattern: &str) -> bool {
    // Escape special regex characters, then convert glob patterns
    let mut regex_pattern = String::new();
    regex_pattern.push('^');
    let chars = pattern.chars();
    for c in chars {
        match c {
            '*' => regex_pattern.push_str(".*"),
            '?' => regex_pattern.push('.'),
            '.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' | '\\' => {
                regex_pattern.push('\\');
                regex_pattern.push(c);
            }
            _ => regex_pattern.push(c),
        }
    }
    regex_pattern.push('$');

    if let Ok(re) = regex::Regex::new(&regex_pattern) {
        re.is_match(path)
    } else {
        // Fallback to simple string matching
        path.contains(pattern)
    }
}

fn clean_cli_error(error: CleanError) -> CliError {
    match error {
        CleanError::MissingMode => CliError::fatal(error.to_string())
            .with_stable_code(StableErrorCode::CliInvalidArguments)
            .with_hint("use 'libra clean -n' to preview removals.")
            .with_hint("use 'libra clean -f' to remove untracked files.")
            .with_hint("use 'libra clean -i' to choose interactively.")
            .with_hint("set 'clean.requireForce=false' to allow clean without a mode flag."),
        CleanError::InvalidArgs(message) => {
            CliError::fatal(format!("invalid arguments: {message}"))
                .with_stable_code(StableErrorCode::CliInvalidArguments)
        }
        CleanError::LoadIndex(message) => {
            CliError::fatal(format!("failed to load index: {message}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        CleanError::ScanUntracked(message) => {
            CliError::fatal(message).with_stable_code(StableErrorCode::IoReadFailed)
        }
        CleanError::ResolveWorkdir(message) => {
            CliError::fatal(format!("failed to resolve working directory: {message}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        CleanError::ResolvePath { path, detail } => {
            CliError::fatal(format!("failed to resolve path {path}: {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        CleanError::OutsideWorkdir(path) => {
            CliError::fatal(format!("refusing to remove path outside workdir: {path}"))
                .with_stable_code(StableErrorCode::ConflictOperationBlocked)
        }
        CleanError::RemoveFile { path, detail } => {
            CliError::fatal(format!("failed to remove {path}: {detail}"))
                .with_stable_code(StableErrorCode::IoWriteFailed)
        }
        CleanError::Io { detail } => {
            CliError::fatal(format!("interactive clean I/O error: {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        CleanError::PathspecDeclined => {
            CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::Unsupported)
                .with_hint("to remove files, use: libra clean -f")
                .with_hint("to remove interactively, use: libra clean -i")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CleanError, clean_cli_error};
    use crate::utils::error::StableErrorCode;

    #[test]
    fn resolve_workdir_cli_error_keeps_context() {
        let error = clean_cli_error(CleanError::ResolveWorkdir("permission denied".to_string()));

        assert_eq!(error.stable_code(), StableErrorCode::IoReadFailed);
        assert!(
            error
                .message()
                .contains("failed to resolve working directory"),
            "unexpected error message: {}",
            error.message()
        );
    }

    /// Pin the `Display` format for every variant of [`CleanError`].
    /// These strings are used as the CliError message via
    /// `clean_cli_error` and surface in both human and `--json`
    /// envelopes for the `clean` subcommand.
    #[test]
    fn clean_error_display_pins_each_variant() {
        assert_eq!(
            CleanError::MissingMode.to_string(),
            "clean requires -f, -n, or -i (use -f to remove files, -n to dry-run, -i for interactive; or set clean.requireForce=false)",
        );
        assert_eq!(
            CleanError::InvalidArgs("--fff is not a valid flag".to_string()).to_string(),
            "invalid arguments: --fff is not a valid flag",
        );
        assert_eq!(
            CleanError::LoadIndex("index file corrupt".to_string()).to_string(),
            "failed to load index: index file corrupt",
        );
        // ScanUntracked echoes the inner string verbatim.
        assert_eq!(
            CleanError::ScanUntracked("walk failed at /tmp".to_string()).to_string(),
            "walk failed at /tmp",
        );
        assert_eq!(
            CleanError::ResolveWorkdir("permission denied".to_string()).to_string(),
            "failed to resolve working directory: permission denied",
        );
        assert_eq!(
            CleanError::ResolvePath {
                path: "src/foo.rs".to_string(),
                detail: "no such file".to_string(),
            }
            .to_string(),
            "failed to resolve path src/foo.rs: no such file",
        );
        assert_eq!(
            CleanError::OutsideWorkdir("/tmp/elsewhere".to_string()).to_string(),
            "refusing to remove path outside workdir: /tmp/elsewhere",
        );
        assert_eq!(
            CleanError::RemoveFile {
                path: "build/artifact.o".to_string(),
                detail: "permission denied".to_string(),
            }
            .to_string(),
            "failed to remove build/artifact.o: permission denied",
        );
        assert_eq!(
            CleanError::Io {
                detail: "broken pipe".to_string(),
            }
            .to_string(),
            "interactive clean I/O error: broken pipe",
        );
    }

    /// The `Io` variant maps to the read-failed stable code (a broken
    /// interactive stream is surfaced as an I/O problem, not an arg error).
    #[test]
    fn clean_io_cli_error_maps_to_io_read() {
        let error = clean_cli_error(CleanError::Io {
            detail: "stream closed".to_string(),
        });
        assert_eq!(error.stable_code(), StableErrorCode::IoReadFailed);
        assert!(error.message().contains("interactive clean I/O error"));
    }
}

#[cfg(test)]
mod interactive_tests {
    use std::{io::Cursor, path::PathBuf};

    use super::{CleanArgs, run_interactive_loop};

    /// Build candidate paths from string slices.
    fn candidates(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    /// Drive the loop with `input` as piped stdin; returns the selected paths
    /// as display strings plus the full rendered transcript.
    fn run(input: &str, items: &[&str]) -> (Vec<String>, String) {
        let args = CleanArgs::default();
        let cands = candidates(items);
        let mut reader = Cursor::new(input.as_bytes().to_vec());
        let mut writer: Vec<u8> = Vec::new();
        let selected = run_interactive_loop(&mut reader, &mut writer, &args, &cands)
            .expect("interactive loop should not error on in-memory streams");
        let chosen = selected
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>();
        (chosen, String::from_utf8(writer).expect("utf8 transcript"))
    }

    #[test]
    fn empty_candidates_returns_immediately() {
        let args = CleanArgs::default();
        let mut reader = Cursor::new(Vec::new());
        let mut writer: Vec<u8> = Vec::new();
        let selected =
            run_interactive_loop(&mut reader, &mut writer, &args, &[]).expect("ok with no items");
        assert!(selected.is_empty());
        // No prompt should be rendered for an empty candidate list.
        assert!(writer.is_empty());
    }

    #[test]
    fn quit_returns_empty_selection() {
        let (chosen, _transcript) = run("quit\n", &["a.txt", "b.txt"]);
        assert!(chosen.is_empty());
    }

    #[test]
    fn quit_shortcut_q_returns_empty() {
        let (chosen, _transcript) = run("q\n", &["a.txt", "b.txt"]);
        assert!(chosen.is_empty());
    }

    #[test]
    fn eof_behaves_like_quit() {
        // No newline, immediate EOF on the first prompt.
        let (chosen, _transcript) = run("", &["a.txt"]);
        assert!(chosen.is_empty());
    }

    #[test]
    fn clean_command_keeps_all_by_default() {
        // `clean` with the initial all-selected state removes everything.
        let (chosen, _transcript) = run("clean\n", &["a.txt", "b.txt", "c.txt"]);
        assert_eq!(chosen, vec!["a.txt", "b.txt", "c.txt"]);
    }

    #[test]
    fn help_renders_seven_lines_then_quits() {
        let (_chosen, transcript) = run("help\nquit\n", &["a.txt"]);
        assert!(transcript.contains("clean               - start cleaning"));
        assert!(transcript.contains("filter by pattern   - exclude items from deletion"));
        assert!(transcript.contains("select by numbers   - select items to be deleted by numbers"));
        assert!(
            transcript.contains("ask each            - confirm each deletion (like \"rm -i\")")
        );
        assert!(transcript.contains("quit                - stop cleaning"));
        assert!(transcript.contains("help                - this screen"));
        assert!(transcript.contains("?                   - help for prompt selection"));
    }

    #[test]
    fn select_by_numbers_range_replaces_selection() {
        // Selecting `2-3` of four items, then clean, keeps only items 2 and 3.
        let (chosen, _transcript) = run("3\n2-3\nclean\n", &["a", "b", "c", "d"]);
        assert_eq!(chosen, vec!["b", "c"]);
    }

    #[test]
    fn select_by_numbers_open_range_to_end() {
        let (chosen, _transcript) = run("s\n2-\nc\n", &["a", "b", "c", "d"]);
        assert_eq!(chosen, vec!["b", "c", "d"]);
    }

    #[test]
    fn select_by_numbers_star_selects_all() {
        let (chosen, _transcript) = run("3\n*\nclean\n", &["a", "b"]);
        assert_eq!(chosen, vec!["a", "b"]);
    }

    #[test]
    fn select_by_numbers_deselect_refines_existing() {
        // Pure deselect `-2` removes item 2 from the initial all-selected set.
        let (chosen, _transcript) = run("3\n-2\nclean\n", &["a", "b", "c"]);
        assert_eq!(chosen, vec!["a", "c"]);
    }

    #[test]
    fn select_by_numbers_out_of_range_token_ignored() {
        // `9` is out of range for two items: it selects nothing, so the
        // replaced selection is empty and `clean` removes nothing.
        let (chosen, _transcript) = run("3\n9\nclean\n", &["a", "b"]);
        assert!(chosen.is_empty());
        // Index 0 is invalid (1-based input); it must not panic or select.
        let (chosen_zero, _t) = run("3\n0\nclean\n", &["a", "b"]);
        assert!(chosen_zero.is_empty());
    }

    #[test]
    fn filter_by_pattern_ancestor_inheritance() {
        // Filtering `build` must also drop nested `build/out/app.js`.
        let (chosen, _transcript) = run(
            "filter\nbuild\n\nclean\n",
            &["build/out/app.js", "build/cache", "src/main.rs"],
        );
        assert_eq!(chosen, vec!["src/main.rs"]);
    }

    #[test]
    fn filter_by_pattern_blank_returns_to_menu() {
        // An immediate blank line in the filter sub-prompt returns unchanged.
        let (chosen, _transcript) = run("filter\n\nclean\n", &["a", "b"]);
        assert_eq!(chosen, vec!["a", "b"]);
    }

    #[test]
    fn ask_each_collects_yes_only() {
        // y/N: only the first and third are confirmed, then `clean` removes them.
        let (chosen, _transcript) = run("ask\ny\nn\ny\nclean\n", &["a", "b", "c"]);
        assert_eq!(chosen, vec!["a", "c"]);
    }

    #[test]
    fn ask_each_then_clean_uses_refined_set() {
        // After `ask each` refines to {a}, the next `clean` removes only `a`.
        let (chosen, _transcript) = run("a\ny\nn\nclean\n", &["a", "b"]);
        assert_eq!(chosen, vec!["a"]);
    }

    #[test]
    fn unknown_command_reprompts() {
        let (chosen, transcript) = run("zzz\nquit\n", &["a"]);
        assert!(chosen.is_empty());
        assert!(transcript.contains("Huh (zzz)?"));
    }
}
