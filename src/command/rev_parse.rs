//! Implements `rev-parse` to resolve revision names and print repository metadata.

use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

use clap::{ArgGroup, Parser};
use git_internal::hash::ObjectHash;
use serde::Serialize;

use crate::{
    command::merge_base::{self, MergeBaseError},
    internal::{
        branch::{Branch, BranchStoreError},
        config::ConfigKv,
        head::Head,
        tag,
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        text::SHORT_HASH_LEN,
        util::{self, CommitBaseError},
    },
};

/// `--help` examples shown in `libra rev-parse --help` output.
///
/// `rev-parse` is the canonical script bridge: resolve a revision spec
/// to a commit hash, a short hash, a branch name, or print the
/// repository top-level. The banner pins the four mutually-exclusive
/// modes plus a JSON variant for agents so users see all supported
/// forms without reading the design doc. Cross-cutting `--help`
/// EXAMPLES rollout per `docs/improvement/README.md` item B.
pub const REV_PARSE_EXAMPLES: &str = "\
EXAMPLES:
    libra rev-parse HEAD                Print the full 40-char hash for HEAD
    libra rev-parse main~3              Resolve any revision spec to a full hash
    libra rev-parse --short HEAD        Print a non-ambiguous short hash
    libra rev-parse --abbrev-ref HEAD   Print the branch name (or HEAD when detached)
    libra rev-parse --verify HEAD       Assert HEAD resolves to one object (exit 128 if not)
    libra rev-parse --show-toplevel     Print the absolute path of the repository root
    libra rev-parse --show-prefix       Print cwd relative to the repository root
    libra rev-parse main..HEAD          Print range endpoints for plumbing scripts
    libra rev-parse --sq HEAD main      Shell-quote resolved revisions
    libra rev-parse --json HEAD         Structured JSON output for agents";

#[derive(Parser, Debug)]
#[command(after_help = REV_PARSE_EXAMPLES)]
#[command(group(
    ArgGroup::new("metadata_mode")
        .multiple(false)
        .args([
            "show_toplevel",
            "git_dir",
            "show_prefix",
            "show_cdup",
            "is_inside_git_dir",
            "is_inside_work_tree",
            "is_bare_repository",
        ]),
))]
pub struct RevParseArgs {
    /// Show a non-ambiguous short object name.
    #[clap(long, conflicts_with_all = ["abbrev_ref", "metadata_mode"])]
    pub short: bool,

    /// Show the branch name instead of the commit hash.
    #[clap(
        long = "abbrev-ref",
        conflicts_with_all = [
            "show_toplevel",
            "git_dir",
            "show_prefix",
            "show_cdup",
            "is_inside_git_dir",
            "is_inside_work_tree",
            "is_bare_repository",
            "short",
            "verify",
            "sq",
            "sq_quote",
            "symbolic",
            "symbolic_full_name",
        ]
    )]
    pub abbrev_ref: bool,

    /// Show the absolute path of the top-level working tree.
    #[clap(long = "show-toplevel", conflicts_with_all = ["specs", "verify"])]
    pub show_toplevel: bool,

    /// Show the Libra storage directory.
    #[clap(long = "git-dir", conflicts_with_all = ["specs", "verify"])]
    pub git_dir: bool,

    /// Show the current directory prefix relative to the worktree root.
    #[clap(long = "show-prefix", conflicts_with_all = ["specs", "verify"])]
    pub show_prefix: bool,

    /// Show a ../ path from the current directory back to the worktree root.
    #[clap(long = "show-cdup", conflicts_with_all = ["specs", "verify"])]
    pub show_cdup: bool,

    /// Print whether the current directory is inside the Libra storage directory.
    #[clap(long = "is-inside-git-dir", conflicts_with_all = ["specs", "verify"])]
    pub is_inside_git_dir: bool,

    /// Print whether the current directory is inside the worktree.
    #[clap(long = "is-inside-work-tree", conflicts_with_all = ["specs", "verify"])]
    pub is_inside_work_tree: bool,

    /// Print whether this repository is configured as bare.
    #[clap(long = "is-bare-repository", conflicts_with_all = ["specs", "verify"])]
    pub is_bare_repository: bool,

    /// Verify that the single argument resolves to one object and print it;
    /// otherwise fail (exit 128, or exit 1 under global `--quiet`). May be
    /// combined with `--short`.
    #[clap(
        long,
        conflicts_with_all = [
            "abbrev_ref",
            "metadata_mode",
            "sq",
            "sq_quote",
            "symbolic",
            "symbolic_full_name",
        ]
    )]
    pub verify: bool,

    /// Quiet verify mode (`git rev-parse --verify -q` exits 1 silently).
    #[clap(short = 'q', long = "quiet")]
    pub quiet: bool,

    /// Revision to use when no positional SPEC is given (Git's `--default`).
    #[clap(long, value_name = "SPEC")]
    pub default: Option<String>,

    /// Shell-quote resolved revision outputs on one line.
    #[clap(long, conflicts_with_all = ["sq_quote", "metadata_mode", "verify", "abbrev_ref"])]
    pub sq: bool,

    /// Shell-quote positional arguments literally without resolving revisions.
    #[clap(long = "sq-quote", conflicts_with_all = ["sq", "metadata_mode", "verify", "abbrev_ref"])]
    pub sq_quote: bool,

    /// Prefer symbolic names for plain ref inputs.
    #[clap(long, conflicts_with_all = ["symbolic_full_name", "metadata_mode", "verify", "abbrev_ref"])]
    pub symbolic: bool,

    /// Prefer full ref names for plain ref inputs.
    #[clap(
        long = "symbolic-full-name",
        conflicts_with_all = ["symbolic", "metadata_mode", "verify", "abbrev_ref"]
    )]
    pub symbolic_full_name: bool,

    /// Revisions or literal arguments. Defaults to HEAD when omitted except
    /// under `--verify` and `--sq-quote`.
    #[clap(value_name = "SPEC", allow_hyphen_values = true)]
    pub specs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct RevParseOutput {
    mode: &'static str,
    input: Option<String>,
    value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    values: Option<Vec<String>>,
}

pub async fn execute(args: RevParseArgs) -> Result<(), String> {
    execute_safe(args, &OutputConfig::default())
        .await
        .map_err(|err| err.render())
}

pub async fn execute_safe(args: RevParseArgs, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() && (args.sq || args.sq_quote) {
        return Err(CliError::command_usage(
            "rev-parse --sq/--sq-quote do not support --json or --machine output",
        ));
    }
    if !args.sq_quote {
        util::require_repo().map_err(|_| CliError::repo_not_found())?;
    }
    let result = resolve_rev_parse(&args, output.quiet || args.quiet).await?;

    if output.is_json() {
        emit_json_data("rev-parse", &result, output)
    } else if output.quiet {
        Ok(())
    } else {
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();
        write_rev_parse_output(&mut writer, &result.value)
    }
}

fn write_rev_parse_output<W: Write>(writer: &mut W, value: &str) -> CliResult<()> {
    match writeln!(writer, "{value}") {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(
            CliError::fatal(format!("failed to write rev-parse output: {error}"))
                .with_stable_code(StableErrorCode::IoWriteFailed),
        ),
    }
}

async fn resolve_rev_parse(args: &RevParseArgs, quiet: bool) -> CliResult<RevParseOutput> {
    if args.sq_quote {
        return Ok(RevParseOutput {
            mode: "sq_quote",
            input: None,
            value: shell_quote_literals(&args.specs, true),
            values: None,
        });
    }

    if args.show_toplevel {
        let workdir = resolve_show_toplevel_path().await?;
        return Ok(RevParseOutput {
            mode: "show_toplevel",
            input: None,
            value: util::path_to_string(&workdir),
            values: None,
        });
    }
    if args.git_dir {
        let storage = util::try_get_storage_path(None).map_err(map_repo_path_error)?;
        return Ok(RevParseOutput {
            mode: "git_dir",
            input: None,
            value: util::path_to_string(&storage),
            values: None,
        });
    }
    if args.show_prefix {
        return Ok(RevParseOutput {
            mode: "show_prefix",
            input: None,
            value: resolve_show_prefix()?,
            values: None,
        });
    }
    if args.show_cdup {
        return Ok(RevParseOutput {
            mode: "show_cdup",
            input: None,
            value: resolve_show_cdup()?,
            values: None,
        });
    }
    if args.is_inside_git_dir {
        return Ok(RevParseOutput {
            mode: "is_inside_git_dir",
            input: None,
            value: bool_output(is_current_dir_inside_storage()?),
            values: None,
        });
    }
    if args.is_inside_work_tree {
        return Ok(RevParseOutput {
            mode: "is_inside_work_tree",
            input: None,
            value: bool_output(!is_current_dir_inside_storage()?),
            values: None,
        });
    }
    if args.is_bare_repository {
        return Ok(RevParseOutput {
            mode: "is_bare_repository",
            input: None,
            value: bool_output(is_bare_repository().await?),
            values: None,
        });
    }

    // `--default <SPEC>` supplies the revision when no positional spec is given
    // (Git semantics). Without either, revision modes fall back to HEAD.
    let specs = effective_specs(args, !args.verify);

    // `--verify`: the argument must resolve to exactly one object, otherwise
    // fail with Git's plumbing exit codes — 128 normally, or 1 under `--quiet`
    // (silent). Unlike the default path it never falls back to HEAD.
    if args.verify {
        if specs.len() != 1 {
            return Err(verify_failure(quiet));
        }
        let spec = specs[0].clone();
        let commit = util::get_commit_base_typed(&spec)
            .await
            .map_err(|_| verify_failure(quiet))?;
        let value = if args.short {
            resolve_short_commit(&commit).await?
        } else {
            commit.to_string()
        };
        return Ok(RevParseOutput {
            mode: "verify",
            input: Some(spec),
            value,
            values: None,
        });
    }

    if args.abbrev_ref {
        let spec = single_spec(&specs)?;
        let value = resolve_abbrev_ref(&spec).await?;
        return Ok(RevParseOutput {
            mode: "abbrev_ref",
            input: Some(spec),
            value,
            values: None,
        });
    }

    let mut outputs = Vec::new();
    for spec in &specs {
        outputs.extend(resolve_spec_outputs(spec, args).await?);
    }
    let value = if args.sq {
        shell_quote_literals(&outputs, false)
    } else {
        outputs.join("\n")
    };

    Ok(RevParseOutput {
        mode: rev_parse_mode(args),
        input: Some(specs.join(" ")),
        value,
        values: rev_parse_values(args, &outputs),
    })
}

fn effective_specs(args: &RevParseArgs, default_head: bool) -> Vec<String> {
    if !args.specs.is_empty() {
        return args.specs.clone();
    }
    if let Some(default) = &args.default {
        return vec![default.clone()];
    }
    if default_head {
        return vec!["HEAD".to_string()];
    }
    Vec::new()
}

fn single_spec(specs: &[String]) -> CliResult<String> {
    if specs.len() == 1 {
        Ok(specs[0].clone())
    } else {
        Err(CliError::command_usage(
            "rev-parse mode requires exactly one revision",
        ))
    }
}

fn rev_parse_mode(args: &RevParseArgs) -> &'static str {
    if args.sq {
        "sq"
    } else if args.specs.iter().any(|spec| spec_is_range_query(spec)) {
        "range"
    } else if args.symbolic {
        "symbolic"
    } else if args.symbolic_full_name {
        "symbolic_full_name"
    } else if args.short {
        "short"
    } else {
        "resolve"
    }
}

fn rev_parse_values(args: &RevParseArgs, outputs: &[String]) -> Option<Vec<String>> {
    if args.sq || outputs.len() <= 1 {
        None
    } else {
        Some(outputs.to_vec())
    }
}

fn spec_is_range_query(spec: &str) -> bool {
    spec.contains("..") || spec.starts_with('^')
}

async fn resolve_spec_outputs(spec: &str, args: &RevParseArgs) -> CliResult<Vec<String>> {
    if let Some((lhs, rhs)) = spec.split_once("...") {
        let left = resolve_range_endpoint(lhs).await?;
        let right = resolve_range_endpoint(rhs).await?;
        let mut out = vec![left.to_string(), right.to_string()];
        for base in merge_base::find_best_merge_bases(left, right).map_err(map_merge_base_error)? {
            out.push(format!("^{base}"));
        }
        return Ok(out);
    }
    if let Some((lhs, rhs)) = spec.split_once("..") {
        let left = resolve_range_endpoint(lhs).await?;
        let right = resolve_range_endpoint(rhs).await?;
        return Ok(vec![right.to_string(), format!("^{left}")]);
    }
    if let Some(rest) = spec.strip_prefix('^') {
        let commit = resolve_range_endpoint(rest).await?;
        return Ok(vec![format!("^{commit}")]);
    }
    if args.symbolic {
        return resolve_symbolic_name(spec).await;
    }
    if args.symbolic_full_name {
        return resolve_symbolic_full_name(spec)
            .await
            .map(|name| vec![name]);
    }
    let commit = util::get_commit_base_typed(spec)
        .await
        .map_err(|err| rev_parse_target_error(spec, err))?;
    let value = if args.short {
        resolve_short_commit(&commit).await?
    } else {
        commit.to_string()
    };
    Ok(vec![value])
}

async fn resolve_range_endpoint(spec: &str) -> CliResult<ObjectHash> {
    let endpoint = if spec.is_empty() { "HEAD" } else { spec };
    util::get_commit_base_typed(endpoint)
        .await
        .map_err(|err| rev_parse_target_error(endpoint, err))
}

fn map_merge_base_error(error: MergeBaseError) -> CliError {
    CliError::fatal(format!(
        "failed to compute merge base for rev-parse range: {error}"
    ))
    .with_stable_code(StableErrorCode::RepoCorrupt)
}

async fn resolve_symbolic_name(spec: &str) -> CliResult<Vec<String>> {
    Ok(vec![spec.to_string()])
}

async fn resolve_symbolic_full_name(spec: &str) -> CliResult<String> {
    if spec == "HEAD" {
        return match Head::current_result().await {
            Ok(Head::Branch(name)) => Ok(format!("refs/heads/{name}")),
            Ok(Head::Detached(_)) => Ok("HEAD".to_string()),
            Err(error) => Err(map_head_resolution_error(error)),
        };
    }
    if spec.starts_with("refs/heads/")
        && Branch::find_branch_result(spec.trim_start_matches("refs/heads/"), None)
            .await
            .map_err(|error| map_symbolic_ref_resolution_error(spec, error))?
            .is_some()
    {
        return Ok(spec.to_string());
    }
    if spec.starts_with("refs/remotes/") && resolve_remote_tracking_ref(spec, spec).await? {
        return Ok(spec.to_string());
    }
    if let Some(tag_name) = spec.strip_prefix("refs/tags/") {
        return resolve_tag_full_name(tag_name)
            .await
            .map(|()| spec.to_string());
    }
    if let Some(branch) = Branch::find_branch_result(spec, None)
        .await
        .map_err(|error| map_symbolic_ref_resolution_error(spec, error))?
    {
        return Ok(format!("refs/heads/{}", branch.name));
    }
    for (remote, branch_name) in util::remote_tracking_candidates(spec) {
        let full_ref = format!("refs/remotes/{remote}/{branch_name}");
        if Branch::find_branch_result(&full_ref, Some(remote))
            .await
            .map_err(|error| map_symbolic_ref_resolution_error(spec, error))?
            .is_some()
            || Branch::find_branch_result(branch_name, Some(remote))
                .await
                .map_err(|error| map_symbolic_ref_resolution_error(spec, error))?
                .is_some()
        {
            return Ok(full_ref);
        }
    }
    resolve_tag_full_name(spec)
        .await
        .map(|()| format!("refs/tags/{spec}"))
        .map_err(|_| {
            CliError::failure(format!("not a symbolic ref: '{spec}'"))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
        })
}

async fn resolve_tag_full_name(spec: &str) -> CliResult<()> {
    match tag::find_tag_ref(spec).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(CliError::failure(format!("not a symbolic ref: '{spec}'"))
            .with_stable_code(StableErrorCode::CliInvalidTarget)),
        Err(error) => Err(CliError::fatal(format!(
            "failed to resolve symbolic tag '{spec}': {error}"
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)),
    }
}

fn shell_quote_literals(values: &[String], leading_space: bool) -> String {
    let mut out = String::new();
    if leading_space && !values.is_empty() {
        out.push(' ');
    }
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(&shell_quote(value));
    }
    out
}

fn shell_quote(value: &str) -> String {
    let mut out = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Build the error for a failed `--verify`: silent exit 1 under global
/// `--quiet` (Git's `rev-parse --verify -q` behavior), otherwise a fatal
/// `Needed a single revision` (exit 128).
fn verify_failure(quiet: bool) -> CliError {
    if quiet {
        CliError::silent_exit(1)
    } else {
        CliError::fatal("Needed a single revision")
            .with_stable_code(StableErrorCode::RepoStateInvalid)
    }
}

async fn resolve_abbrev_ref(spec: &str) -> CliResult<String> {
    if spec == "HEAD" {
        return match Head::current_result().await {
            Ok(Head::Branch(name)) => Ok(name),
            Ok(Head::Detached(_)) => Ok("HEAD".to_string()),
            Err(error) => Err(map_head_resolution_error(error)),
        };
    }

    if let Some(branch_name) = spec.strip_prefix("refs/heads/")
        && let Some(branch) = Branch::find_branch_result(branch_name, None)
            .await
            .map_err(|error| map_symbolic_ref_resolution_error(spec, error))?
    {
        return Ok(branch.name);
    }

    if let Some(short_name) = spec.strip_prefix("refs/remotes/")
        && resolve_remote_tracking_ref(spec, short_name).await?
    {
        return Ok(short_name.to_string());
    }

    if let Some(branch) = Branch::find_branch_result(spec, None)
        .await
        .map_err(|error| map_symbolic_ref_resolution_error(spec, error))?
    {
        return Ok(branch.name);
    }

    if resolve_remote_tracking_ref(spec, spec).await? {
        return Ok(spec.to_string());
    }

    Err(CliError::failure(format!("not a symbolic ref: '{spec}'"))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
        .with_hint("use 'libra rev-parse <rev>' to resolve it to a commit hash."))
}

async fn resolve_remote_tracking_ref(spec: &str, short_name: &str) -> CliResult<bool> {
    for (remote, branch_name) in util::remote_tracking_candidates(short_name) {
        let full_ref = format!("refs/remotes/{remote}/{branch_name}");

        if Branch::find_branch_result(&full_ref, Some(remote))
            .await
            .map_err(|error| map_symbolic_ref_resolution_error(spec, error))?
            .is_some()
        {
            return Ok(true);
        }

        if Branch::find_branch_result(branch_name, Some(remote))
            .await
            .map_err(|error| map_symbolic_ref_resolution_error(spec, error))?
            .is_some()
        {
            return Ok(true);
        }
    }

    Ok(false)
}

async fn resolve_short_commit(commit: &ObjectHash) -> CliResult<String> {
    let full = commit.to_string();
    let storage = util::objects_storage();

    for len in SHORT_HASH_LEN..=full.len() {
        let prefix = &full[..len];
        let matches = storage.search_result(prefix).await.map_err(|error| {
            CliError::fatal(format!(
                "failed to search objects while abbreviating '{full}': {error}"
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })?;

        if matches.len() == 1 && matches[0] == *commit {
            return Ok(prefix.to_string());
        }
    }

    Ok(full)
}

async fn is_bare_repository() -> CliResult<bool> {
    fn parse_git_bool(value: &str) -> Option<bool> {
        match value.trim() {
            v if v.eq_ignore_ascii_case("true")
                || v.eq_ignore_ascii_case("yes")
                || v.eq_ignore_ascii_case("on")
                || v == "1" =>
            {
                Some(true)
            }
            v if v.eq_ignore_ascii_case("false")
                || v.eq_ignore_ascii_case("no")
                || v.eq_ignore_ascii_case("off")
                || v == "0" =>
            {
                Some(false)
            }
            _ => None,
        }
    }

    match ConfigKv::get("core.bare").await {
        Ok(Some(entry)) => parse_git_bool(&entry.value).ok_or_else(|| {
            CliError::fatal(format!(
                "Invalid core.bare value: '{}'. Expected true/false/yes/no/on/off/1/0",
                entry.value
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt)
        }),
        Ok(None) => Ok(false),
        Err(error) => Err(
            CliError::fatal(format!("Failed to read core.bare config: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed),
        ),
    }
}

async fn resolve_show_toplevel_path() -> CliResult<PathBuf> {
    let workdir = util::try_working_dir().map_err(map_repo_path_error)?;
    let storage = util::try_get_storage_path(None).map_err(map_repo_path_error)?;
    if workdir == storage {
        if is_bare_repository().await? {
            return Err(CliError::fatal("this operation must be run in a work tree")
                .with_stable_code(StableErrorCode::RepoStateInvalid));
        }

        let storage = fs::canonicalize(&storage).map_err(|error| {
            CliError::io(format!(
                "failed to resolve repository storage path '{}': {error}",
                storage.display()
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })?;

        return storage
            .parent()
            .map(PathBuf::from)
            .ok_or_else(CliError::repo_not_found);
    }
    Ok(workdir)
}

fn resolve_show_prefix() -> CliResult<String> {
    let (current, workdir, storage) = canonical_repo_paths()?;
    if current.starts_with(&storage) {
        return Ok(String::new());
    }
    let Ok(relative) = current.strip_prefix(&workdir) else {
        return Ok(String::new());
    };
    slash_path(relative, true)
}

fn resolve_show_cdup() -> CliResult<String> {
    let prefix = resolve_show_prefix()?;
    if prefix.is_empty() {
        return Ok(String::new());
    }
    let depth = prefix.trim_end_matches('/').split('/').count();
    Ok("../".repeat(depth))
}

fn is_current_dir_inside_storage() -> CliResult<bool> {
    let (current, _, storage) = canonical_repo_paths()?;
    Ok(current.starts_with(storage))
}

fn canonical_repo_paths() -> CliResult<(PathBuf, PathBuf, PathBuf)> {
    let current = std::env::current_dir().map_err(map_repo_path_error)?;
    let workdir = util::try_working_dir().map_err(map_repo_path_error)?;
    let storage = util::try_get_storage_path(None).map_err(map_repo_path_error)?;
    Ok((
        canonicalize_path(&current)?,
        canonicalize_path(&workdir)?,
        canonicalize_path(&storage)?,
    ))
}

fn canonicalize_path(path: &Path) -> CliResult<PathBuf> {
    fs::canonicalize(path).map_err(|error| {
        CliError::io(format!(
            "failed to resolve repository path '{}': {error}",
            path.display()
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })
}

fn slash_path(path: &Path, trailing_slash: bool) -> CliResult<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let value = value.to_str().ok_or_else(|| {
                    CliError::fatal(format!(
                        "repository path contains non-UTF-8 component: {}",
                        path.display()
                    ))
                    .with_stable_code(StableErrorCode::IoReadFailed)
                })?;
                parts.push(value.to_string());
            }
            Component::CurDir => {}
            Component::ParentDir => parts.push("..".to_string()),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    let mut out = parts.join("/");
    if trailing_slash && !out.is_empty() {
        out.push('/');
    }
    Ok(out)
}

fn bool_output(value: bool) -> String {
    if value {
        "true".to_string()
    } else {
        "false".to_string()
    }
}

fn map_repo_path_error(err: std::io::Error) -> CliError {
    match err.kind() {
        std::io::ErrorKind::NotFound => CliError::repo_not_found(),
        _ => CliError::io(format!("failed to determine repository root: {err}"))
            .with_stable_code(StableErrorCode::IoReadFailed),
    }
}

fn map_head_resolution_error(error: BranchStoreError) -> CliError {
    map_symbolic_ref_resolution_error("HEAD", error)
}

fn map_symbolic_ref_resolution_error(spec: &str, error: BranchStoreError) -> CliError {
    match error {
        BranchStoreError::Corrupt { detail, .. } => {
            CliError::fatal(format!("failed to resolve symbolic ref '{spec}': {detail}"))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        }
        BranchStoreError::Query(detail)
        | BranchStoreError::NotFound(detail)
        | BranchStoreError::Delete { detail, .. } => {
            CliError::fatal(format!("failed to resolve symbolic ref '{spec}': {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
    }
}

fn rev_parse_target_error(spec: &str, error: CommitBaseError) -> CliError {
    match error {
        CommitBaseError::HeadUnborn => CliError::failure(format!(
            "not a valid object name: '{spec}' (HEAD does not point to a commit)"
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
        .with_hint("create a commit before resolving HEAD."),
        CommitBaseError::InvalidReference(detail) => {
            CliError::failure(format!("not a valid object name: '{spec}' ({detail})"))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
        }
        CommitBaseError::ReadFailure(detail) => {
            CliError::fatal(format!("failed to resolve '{spec}': {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        CommitBaseError::CorruptReference(detail) => {
            CliError::fatal(format!("failed to resolve '{spec}': {detail}"))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use clap::Parser;

    use super::{RevParseArgs, shell_quote, shell_quote_literals, write_rev_parse_output};
    use crate::utils::error::StableErrorCode;

    struct FailingWriter {
        kind: io::ErrorKind,
    }

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(self.kind, "test write failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn test_rev_parse_args_default() {
        let args = RevParseArgs::try_parse_from(["rev-parse"]).unwrap();
        assert!(!args.short);
        assert!(!args.abbrev_ref);
        assert!(!args.show_toplevel);
        assert!(args.specs.is_empty());
    }

    #[test]
    fn test_rev_parse_args_short_head() {
        let args = RevParseArgs::try_parse_from(["rev-parse", "--short", "HEAD"]).unwrap();
        assert!(args.short);
        assert_eq!(args.specs, vec!["HEAD".to_string()]);
    }

    #[test]
    fn test_rev_parse_args_abbrev_ref() {
        let args = RevParseArgs::try_parse_from(["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert!(args.abbrev_ref);
        assert_eq!(args.specs, vec!["HEAD".to_string()]);
    }

    #[test]
    fn test_rev_parse_args_show_toplevel() {
        let args = RevParseArgs::try_parse_from(["rev-parse", "--show-toplevel"]).unwrap();
        assert!(args.show_toplevel);
    }

    #[test]
    fn test_rev_parse_args_verify_and_default() {
        let args = RevParseArgs::try_parse_from(["rev-parse", "--verify", "HEAD"]).unwrap();
        assert!(args.verify);
        assert_eq!(args.specs, vec!["HEAD".to_string()]);

        let args =
            RevParseArgs::try_parse_from(["rev-parse", "--verify", "--default", "HEAD"]).unwrap();
        assert!(args.verify);
        assert!(args.specs.is_empty());
        assert_eq!(args.default.as_deref(), Some("HEAD"));
    }

    #[test]
    fn test_rev_parse_args_sq_quote_hyphen_literal() {
        let args = RevParseArgs::try_parse_from(["rev-parse", "--sq-quote", "-x", "a b"])
            .expect("hyphen literal should be accepted");
        assert!(args.sq_quote);
        assert_eq!(args.specs, vec!["-x".to_string(), "a b".to_string()]);
    }

    #[test]
    fn test_shell_quote_escapes_single_quote() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(
            shell_quote_literals(&["foo".to_string(), "a b".to_string()], true),
            " 'foo' 'a b'"
        );
    }

    #[test]
    fn test_rev_parse_args_verify_allows_short() {
        let args =
            RevParseArgs::try_parse_from(["rev-parse", "--verify", "--short", "HEAD"]).unwrap();
        assert!(args.verify);
        assert!(args.short);
    }

    #[test]
    fn test_rev_parse_args_verify_conflicts_with_abbrev_ref() {
        let err = RevParseArgs::try_parse_from(["rev-parse", "--verify", "--abbrev-ref", "HEAD"])
            .expect_err("--verify should reject --abbrev-ref");
        assert!(
            err.to_string().contains("cannot be used with"),
            "unexpected clap error: {err}"
        );
    }

    #[test]
    fn test_verify_failure_quiet_is_silent_exit_1() {
        let err = super::verify_failure(true);
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn test_verify_failure_loud_is_fatal_128() {
        let err = super::verify_failure(false);
        assert_eq!(err.stable_code(), StableErrorCode::RepoStateInvalid);
        assert_eq!(err.exit_code(), 128);
    }

    #[test]
    fn test_rev_parse_args_show_toplevel_conflicts_with_spec() {
        let err = RevParseArgs::try_parse_from(["rev-parse", "--show-toplevel", "HEAD"])
            .expect_err("--show-toplevel should reject SPEC");
        let rendered = err.to_string();
        assert!(
            rendered.contains("cannot be used with") || rendered.contains("unexpected argument"),
            "unexpected clap error: {rendered}"
        );
    }

    #[test]
    fn test_rev_parse_args_abbrev_ref_conflicts_with_short() {
        let err = RevParseArgs::try_parse_from(["rev-parse", "--abbrev-ref", "--short", "HEAD"])
            .expect_err("--abbrev-ref should reject --short");
        let rendered = err.to_string();
        assert!(
            rendered.contains("cannot be used with"),
            "unexpected clap error: {rendered}"
        );
    }

    #[test]
    fn test_rev_parse_args_show_toplevel_conflicts_with_short() {
        let err = RevParseArgs::try_parse_from(["rev-parse", "--show-toplevel", "--short"])
            .expect_err("--show-toplevel should reject --short");
        let rendered = err.to_string();
        assert!(
            rendered.contains("cannot be used with"),
            "unexpected clap error: {rendered}"
        );
    }

    #[test]
    fn test_write_rev_parse_output_maps_write_failure_to_write_code() {
        let mut writer = FailingWriter {
            kind: io::ErrorKind::PermissionDenied,
        };

        let error = write_rev_parse_output(&mut writer, "abc123").expect_err("write should fail");

        assert_eq!(error.stable_code(), StableErrorCode::IoWriteFailed);
    }

    #[test]
    fn test_write_rev_parse_output_ignores_broken_pipe() {
        let mut writer = FailingWriter {
            kind: io::ErrorKind::BrokenPipe,
        };

        write_rev_parse_output(&mut writer, "abc123").expect("broken pipe should be ignored");
    }
}
