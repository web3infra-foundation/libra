//! `libra apply --check` — validate that a unified-diff patch applies cleanly,
//! without writing anything. A focused MVP of `git apply --check` built on the
//! same `diffy` patch engine used elsewhere.
//!
//! Only `--check` is supported in this version: the patch is parsed, every
//! target path is safety-checked, and each file hunk-set is test-applied
//! against the current working tree. Actually writing the result (atomic
//! temp-file + rename) is a documented future extension.

use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use clap::Parser;
use serde::Serialize;

use crate::utils::{
    error::{CliError, CliResult, StableErrorCode},
    output::{OutputConfig, emit_json_data},
    util,
};

/// Hard cap on a patch input, matching the grit-gap plan's default.
const MAX_PATCH_BYTES: usize = 64 * 1024 * 1024;

/// `--help` examples (cross-cutting EXAMPLES contract, `_general.md`).
pub const APPLY_EXAMPLES: &str = "\
EXAMPLES:
    libra apply --check fix.patch            Check whether a patch applies cleanly
    libra apply --check -p0 fix.patch        Do not strip a leading path component
    cat fix.patch | libra apply --check      Read the patch from stdin
    libra --json apply --check fix.patch     Structured { applies, files }";

/// Validate that a unified-diff patch applies (`--check` only in this version).
#[derive(Parser, Debug)]
#[command(after_help = APPLY_EXAMPLES)]
pub struct ApplyArgs {
    /// Check whether the patch applies, without writing. Required in this
    /// version (actually applying the patch is not yet supported).
    #[clap(long)]
    pub check: bool,

    /// Strip `<n>` leading path components from each patched path (like
    /// `git apply -p<n>`; default 1).
    #[clap(short = 'p', value_name = "N", default_value_t = 1)]
    pub strip: u32,

    /// Patch files to read; if none are given, the patch is read from stdin.
    #[clap(value_name = "PATCH")]
    pub patches: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ApplyOutput {
    /// Whether the whole patch applies cleanly.
    applies: bool,
    /// The target paths the patch touches.
    files: Vec<String>,
}

pub async fn execute(args: ApplyArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
        std::process::exit(err.exit_code());
    }
}

/// Safe entry point. Exit 0 when the patch applies, 1 when it does not, 128 on
/// errors (not a repo, unsupported mode, unreadable/oversized/malformed patch,
/// or an unsafe target path).
pub async fn execute_safe(args: ApplyArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;

    let error = |message: String| {
        CliError::fatal(message)
            .with_exit_code(128)
            .with_stable_code(StableErrorCode::CliInvalidArguments)
    };

    if !args.check {
        return Err(error(
            "this version of `apply` only supports --check; pass --check to validate a patch"
                .to_string(),
        ));
    }

    let patch_text = read_patch(&args.patches).map_err(error)?;
    let workdir = util::working_dir();

    let mut files = Vec::new();
    let mut applies = true;
    for section in split_file_patches(&patch_text) {
        let patch = diffy::Patch::from_str(&section)
            .map_err(|err| error(format!("malformed patch: {err}")))?;
        let target = patch_target(&patch, args.strip).map_err(error)?;
        let absolute = resolve_safe(&target, &workdir).map_err(error)?;
        files.push(target.clone());

        let is_new_file = patch.original() == Some("/dev/null");
        let is_deletion = patch.modified() == Some("/dev/null");
        let base = if is_new_file {
            String::new()
        } else {
            match fs::read_to_string(&absolute) {
                Ok(content) => content,
                // A missing or unreadable target means the patch does not apply.
                Err(_) => {
                    applies = false;
                    continue;
                }
            }
        };
        match diffy::apply(&base, &patch) {
            // A deletion patch must reduce the file to nothing; a non-empty
            // result means the file did not match the patch's full extent.
            Ok(result) if is_deletion && !result.is_empty() => applies = false,
            Ok(_) => {}
            Err(_) => applies = false,
        }
    }

    if output.is_json() {
        emit_json_data("apply", &ApplyOutput { applies, files }, output)?;
    }

    if applies {
        Ok(())
    } else {
        // `--check` reports "does not apply" with exit 1 (Git-compatible); the
        // working tree was never touched.
        Err(CliError::silent_exit(1))
    }
}

/// Read the patch from the given files (concatenated) or from stdin, enforcing
/// the size cap.
fn read_patch(patches: &[String]) -> Result<String, String> {
    let mut buffer = Vec::new();
    if patches.is_empty() {
        io::stdin()
            .take(MAX_PATCH_BYTES as u64 + 1)
            .read_to_end(&mut buffer)
            .map_err(|err| format!("failed to read patch from stdin: {err}"))?;
    } else {
        for path in patches {
            let bytes =
                fs::read(path).map_err(|err| format!("cannot read patch '{path}': {err}"))?;
            buffer.extend_from_slice(&bytes);
            if buffer.len() > MAX_PATCH_BYTES {
                break;
            }
        }
    }
    if buffer.len() > MAX_PATCH_BYTES {
        return Err(format!(
            "patch exceeds the {} MiB limit",
            MAX_PATCH_BYTES / (1024 * 1024)
        ));
    }
    String::from_utf8(buffer).map_err(|_| "patch is not valid UTF-8".to_string())
}

/// Split a (possibly multi-file) unified diff into per-file sections. Git-style
/// diffs split on `diff --git `; plain diffs split on a `--- ` line immediately
/// followed by `+++ ` (so a `--- ...` content-removal line is not mistaken for a
/// file header).
fn split_file_patches(patch: &str) -> Vec<String> {
    let lines: Vec<&str> = patch.lines().collect();
    let git_style = lines.iter().any(|line| line.starts_with("diff --git "));

    let mut starts: Vec<usize> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let is_start = if git_style {
            line.starts_with("diff --git ")
        } else {
            line.starts_with("--- ")
                && lines
                    .get(index + 1)
                    .is_some_and(|next| next.starts_with("+++ "))
        };
        if is_start {
            starts.push(index);
        }
    }

    if starts.is_empty() {
        return vec![patch.to_string()];
    }

    let mut sections = Vec::new();
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(lines.len());
        sections.push(format!("{}\n", lines[start..end].join("\n")));
    }
    sections
}

/// Resolve the target path of a file patch (the modified side, or the original
/// side for a deletion), stripping `strip` leading components.
fn patch_target(patch: &diffy::Patch<'_, str>, strip: u32) -> Result<String, String> {
    let deleted = patch.modified() == Some("/dev/null");
    let raw = if deleted {
        patch.original()
    } else {
        patch.modified()
    };
    let raw = raw.ok_or_else(|| "patch is missing a target filename".to_string())?;
    // Validate the RAW path before `-p<n>` stripping: an absolute path must be
    // rejected even when stripping would turn it into a relative one (e.g.
    // `/abs/file` with `-p1` -> `abs/file`).
    if Path::new(raw).is_absolute() || raw.contains('\0') {
        return Err(format!("refusing absolute or NUL patch path '{raw}'"));
    }
    strip_path(raw, strip)
        .ok_or_else(|| format!("cannot strip {strip} path component(s) from '{raw}'"))
}

/// Strip `n` leading slash-separated components from a patch path.
fn strip_path(path: &str, n: u32) -> Option<String> {
    let components: Vec<&str> = path.split('/').collect();
    if components.len() <= n as usize {
        return None;
    }
    Some(components[n as usize..].join("/"))
}

/// Resolve a stripped patch path against the worktree and reject anything that
/// escapes it, contains a NUL, or points inside `.libra/`.
fn resolve_safe(target: &str, workdir: &Path) -> Result<PathBuf, String> {
    if target.is_empty() || target.contains('\0') {
        return Err(format!("invalid target path '{target}'"));
    }
    if Path::new(target).is_absolute() || target.split('/').any(|c| c == "..") {
        return Err(format!(
            "refusing to patch outside the working tree: '{target}'"
        ));
    }
    if target == util::ROOT_DIR || target.starts_with(&format!("{}/", util::ROOT_DIR)) {
        return Err(format!(
            "refusing to patch inside {}: '{target}'",
            util::ROOT_DIR
        ));
    }
    let absolute = workdir.join(target);
    if !util::is_sub_path(&absolute, workdir) {
        return Err(format!(
            "refusing to patch outside the working tree: '{target}'"
        ));
    }
    Ok(absolute)
}
