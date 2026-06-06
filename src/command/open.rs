//! `libra open` command implementation for opening repository remotes in a browser.
//!
//! `libra open` 命令实现，用于在浏览器中打开存储库远程。
//!
//! Boundary: this command parses common Git remote URL forms, assembles a
//! browsable web URL (optionally a branch/commit/issue/PR deep link across
//! GitHub/GitLab/Gitea/Bitbucket), and delegates launching to the host OS. It
//! does not validate network reachability. Command tests cover HTTPS, SSH/SCP
//! URLs, deep-link targets, platform templates, missing remotes, and malformed
//! input. `libra open` is an intentional Libra extension (Git has no `git open`).

use std::process::Command;

use clap::Parser;
use lazy_static::lazy_static;
use regex::Regex;
use serde::Serialize;

use crate::{
    internal::{config::ConfigKv, db::get_db_conn_instance, head::Head},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        pager::LIBRA_TEST_ENV,
        util::require_repo,
    },
};

const OPEN_EXAMPLES: &str = "\
EXAMPLES:
    libra open                                            Open the auto-detected upstream in the browser
    libra open origin                                     Open a specific remote
    libra open https://github.com/web3infra-foundation/libra    Open a direct URL
    libra open --json                                     Structured JSON output for agents (no browser)
    libra open --print-only                               Print the resolved URL without opening the browser
    libra open origin --print-only                        Print a specific remote's URL without opening the browser";

#[derive(Parser, Debug, Clone, Default)]
    libra open -b main origin                            Open a branch page (/tree/main)
    libra open -c a1b2c3d origin                         Open a commit page
    libra open --issue=42 origin                         Open issue #42 (use --issue alone for the list)
    libra open --pr=7 origin                             Open pull request #7 (use --pr alone for the list)
    libra open --json                                    Structured JSON output for agents (no browser)";

#[derive(Parser, Debug, Default)]
#[command(after_help = OPEN_EXAMPLES)]
pub struct OpenArgs {
    /// Remote name (e.g. `origin`) or a direct URL. Omit to auto-detect from the current branch's upstream
    #[arg(value_name = "REMOTE_OR_URL")]
    pub remote: Option<String>,

    /// Only print the resolved URL; do not open the browser
    #[arg(long = "print-only")]
    pub print_only: bool,
    /// Open the page for a specific branch (`/tree/<name>`)
    #[arg(short = 'b', long, value_name = "NAME", conflicts_with_all = ["commit", "issue", "pr"])]
    pub branch: Option<String>,

    /// Open the page for a specific commit (full or short hash)
    #[arg(short = 'c', long, value_name = "HASH", conflicts_with_all = ["branch", "issue", "pr"])]
    pub commit: Option<String>,

    /// Open the issues page; pass `--issue=<ID>` to open a specific issue
    #[arg(
        short = 'i',
        long,
        value_name = "ID",
        num_args = 0..=1,
        require_equals = true,
        conflicts_with_all = ["branch", "commit", "pr"]
    )]
    pub issue: Option<Option<String>>,

    /// Open the pull-requests page; pass `--pr=<ID>` to open a specific PR/MR
    #[arg(
        short = 'p',
        long,
        value_name = "ID",
        num_args = 0..=1,
        require_equals = true,
        conflicts_with_all = ["branch", "commit", "issue"]
    )]
    pub pr: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize)]
struct OpenOutput {
    // NOTE: existing fields `remote`/`remote_url`/`web_url`/`launched` are a
    // published, frozen contract. New fields are *appended* (additive schema)
    // so existing JSON consumers that read known keys keep working.
    remote: Option<String>,
    remote_url: String,
    web_url: String,
    launched: bool,
    resolved_from_remote: bool,
    target_type: String,
    platform: String,
}

#[derive(Debug)]
struct OpenResolution {
    remote: Option<String>,
    remote_url: String,
    resolved_from_remote: bool,
}

/// Hosting platform whose web-URL path conventions differ. `libra open` is an
/// intentional Libra extension; platform identification is a domain heuristic
/// overridable by `open.platform`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    GitHub,
    GitLab,
    Gitea,
    Bitbucket,
    Custom,
}

impl Platform {
    fn as_str(self) -> &'static str {
        match self {
            Platform::GitHub => "github",
            Platform::GitLab => "gitlab",
            Platform::Gitea => "gitea",
            Platform::Bitbucket => "bitbucket",
            Platform::Custom => "custom",
        }
    }

    fn branch_path(self, branch: &str) -> String {
        match self {
            Platform::GitLab => format!("/-/tree/{branch}"),
            Platform::Bitbucket => format!("/src/{branch}"),
            Platform::Gitea => format!("/src/branch/{branch}"),
            // github + custom-fallback
            _ => format!("/tree/{branch}"),
        }
    }

    fn commit_path(self, commit: &str) -> String {
        match self {
            Platform::GitLab => format!("/-/commit/{commit}"),
            Platform::Bitbucket => format!("/commits/{commit}"),
            // github + gitea + custom-fallback
            _ => format!("/commit/{commit}"),
        }
    }

    fn issue_path(self, id: Option<&str>) -> String {
        let prefix = match self {
            Platform::GitLab => "/-/issues",
            _ => "/issues",
        };
        match id {
            Some(id) => format!("{prefix}/{id}"),
            None => prefix.to_string(),
        }
    }

    fn pr_path(self, id: Option<&str>) -> String {
        match self {
            Platform::GitLab => match id {
                Some(id) => format!("/-/merge_requests/{id}"),
                None => "/-/merge_requests".to_string(),
            },
            Platform::Bitbucket => match id {
                Some(id) => format!("/pull-requests/{id}"),
                None => "/pull-requests".to_string(),
            },
            Platform::Gitea => match id {
                Some(id) => format!("/pulls/{id}"),
                None => "/pulls".to_string(),
            },
            // github + custom-fallback: single PR is `/pull/<id>`, list is `/pulls`
            _ => match id {
                Some(id) => format!("/pull/{id}"),
                None => "/pulls".to_string(),
            },
        }
    }
}

/// The kind of page to open. Serialised as the `target_type` JSON field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    Repo,
    Branch,
    Commit,
    Issue,
    PullRequest,
}

impl TargetKind {
    fn as_str(self) -> &'static str {
        match self {
            TargetKind::Repo => "repo",
            TargetKind::Branch => "branch",
            TargetKind::Commit => "commit",
            TargetKind::Issue => "issue",
            TargetKind::PullRequest => "pull_request",
        }
    }
}

/// Resolved deep-link target: its kind plus the sanitised value (branch name,
/// commit hash, issue/PR id). `value` is `None` for the repo root and for
/// issue/PR list pages.
#[derive(Debug, Clone)]
struct OpenTarget {
    kind: TargetKind,
    value: Option<String>,
}

/// Custom-platform URL templates read from `open.template.<kind>`.
#[derive(Debug, Default)]
struct TemplateSet {
    branch: Option<String>,
    commit: Option<String>,
    issue: Option<String>,
    pull_request: Option<String>,
}

/// Which whitelist a CLI-supplied reference component must satisfy.
#[derive(Debug, Clone, Copy)]
enum ComponentKind {
    Branch,
    Commit,
}

/// Maximum accepted length for any single CLI-supplied reference component.
const MAX_REF_LEN: usize = 256;

/// Browser-launch failure classification used to decide between a graceful
/// "open this URL manually" fallback (`NotFound`) and a hard error (`Io`).
#[derive(Debug, PartialEq, Eq)]
enum LaunchError {
    /// The launcher binary itself was missing (e.g. no `xdg-open`).
    NotFound,
    /// Any other spawn IO failure.
    Io(String),
}

#[derive(Debug, thiserror::Error)]
enum OpenError {
    #[error("not a libra repository (or any of the parent directories): .libra")]
    NotInRepo,
    #[error("failed to read remote configuration: {0}")]
    ConfigRead(String),
    #[error("no remote configured")]
    NoRemoteConfigured,
    #[error("remote '{0}' is configured but has no URL")]
    RemoteMissingUrl(String),
    #[error("calculated URL '{0}' is unsafe or invalid. Only http/https are supported.")]
    UnsafeUrl(String),
    #[error("failed to open browser: {0}")]
    BrowserLaunch(String),
}

lazy_static! {
    static ref SCP_RE: Regex = {
        // INVARIANT: this regex is a static literal validated in tests and code review.
        Regex::new(r"^git@([^:]+):(.+?)(\.git)?$").expect("static SCP regex must compile")
    };
    static ref SSH_RE: Regex = {
        // INVARIANT: this regex is a static literal validated in tests and code review.
        Regex::new(r"^ssh://(?:[^@]+@)?([^:/]+)(?::\d+)?/(.+?)(\.git)?$")
            .expect("static SSH regex must compile")
    };
}

pub async fn execute(args: OpenArgs) {
    if let Err(e) = execute_safe(args, &OutputConfig::default()).await {
        e.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Resolves the remote URL, assembles the (optionally
/// deep-linked) web URL, and opens it in the default browser.
pub async fn execute_safe(args: OpenArgs, output: &OutputConfig) -> CliResult<()> {
    let in_repo = require_repo().is_ok();
    let resolution = resolve_open_target(args.clone(), in_repo)

    // Parse and whitelist-sanitise the deep-link target up front so malicious
    // ref components are rejected before any URL assembly.
    let target = parse_target(&args).map_err(open_cli_error)?;

    let resolution = resolve_open_target(&args, in_repo)
        .await
        .map_err(open_cli_error)?;

    // Base browsable URL (SCP/SSH -> https, `.git` stripped).
    let base_url = transform_url(&resolution.remote_url);

    // Platform + templates: config is only read inside a repository (a direct
    // URL run outside a repo has no local `.libra/libra.db`).
    let (configured_platform, templates) =
        load_open_config(in_repo).await.map_err(open_cli_error)?;
    let host = url_host(&base_url);
    let platform = resolve_platform(configured_platform.as_deref(), &host);

    let web_url = build_target_url(&base_url, platform, &target, &templates);

    if !is_safe_url(&web_url) {
        return Err(open_cli_error(OpenError::UnsafeUrl(web_url)));
    }

    let launched = if args.print_only || output.is_json() {
        false
    } else {
        match open_browser(&web_url) {
            Ok(launched) => launched,
            Err(LaunchError::NotFound) => {
                // Treat a missing launcher as a tolerable condition so headless
                // / CI environments do not report a hard failure: print the URL
                // for the user to copy and exit successfully.
                if !output.quiet {
                    eprintln!("Could not launch a browser. Open this URL manually: {web_url}");
                }
                false
            }
            Err(LaunchError::Io(message)) => {
                return Err(open_cli_error(OpenError::BrowserLaunch(message)));
            }
        }
    };

    let open_output = OpenOutput {
        remote: resolution.remote,
        remote_url: resolution.remote_url,
        web_url: web_url.clone(),
        launched,
        resolved_from_remote: resolution.resolved_from_remote,
        target_type: target.kind.as_str().to_string(),
        platform: platform.as_str().to_string(),
    };

    if output.is_json() {
        emit_json_data("open", &open_output, output)?;
    } else if args.print_only {
        println!("{}", web_url);
    } else if !output.quiet {
        println!("Opening {web_url}");
    }

    Ok(())
}

async fn resolve_open_target(args: &OpenArgs, in_repo: bool) -> Result<OpenResolution, OpenError> {
    if let Some(input) = args.remote.clone() {
        if in_repo {
            let remotes = ConfigKv::all_remote_configs()
                .await
                .map_err(|error| OpenError::ConfigRead(error.to_string()))?;
            if remotes.iter().any(|remote| remote.name == input) {
                let remote_url = load_remote_url(&input).await?;
                return Ok(OpenResolution {
                    remote: Some(input),
                    remote_url,
                    resolved_from_remote: true,
                });
            }
        }

        return Ok(OpenResolution {
            remote: None,
            remote_url: input,
            resolved_from_remote: false,
        });
    }

    if !in_repo {
        return Err(OpenError::NotInRepo);
    }

    let current_remote = match Head::current_result().await {
        Ok(Head::Branch(branch_name)) => ConfigKv::get_remote(&branch_name)
            .await
            .map_err(|error| OpenError::ConfigRead(error.to_string()))?,
        Ok(Head::Detached(_)) => None,
        Err(error) => return Err(OpenError::ConfigRead(error.to_string())),
    };

    if let Some(current_remote) = current_remote {
        // If the branch's configured remote has a valid URL, use it.
        // Otherwise fall through to the origin / first-remote fallback so
        // that stale branch.<name>.remote config doesn't block `libra open`.
        match load_remote_url(&current_remote).await {
            Ok(remote_url) => {
                return Ok(OpenResolution {
                    remote: Some(current_remote),
                    remote_url,
                    resolved_from_remote: true,
                });
            }
            Err(_) => {
                tracing::debug!(
                    "current remote '{}' has no usable URL, falling back",
                    current_remote
                );
            }
        }
    }

    let remotes = ConfigKv::all_remote_configs()
        .await
        .map_err(|error| OpenError::ConfigRead(error.to_string()))?;
    if let Some(origin) = remotes
        .iter()
        .find(|remote| remote.name == "origin" && !remote.url.trim().is_empty())
    {
        return Ok(OpenResolution {
            remote: Some("origin".to_string()),
            remote_url: origin.url.clone(),
            resolved_from_remote: true,
        });
    }
    if let Some(first) = remotes.iter().find(|remote| !remote.url.trim().is_empty()) {
        return Ok(OpenResolution {
            remote: Some(first.name.clone()),
            remote_url: first.url.clone(),
            resolved_from_remote: true,
        });
    }
    if let Some(first) = remotes.first() {
        return Err(OpenError::RemoteMissingUrl(first.name.clone()));
    }

    Err(OpenError::NoRemoteConfigured)
}

async fn load_remote_url(remote: &str) -> Result<String, OpenError> {
    let configured_remote = ConfigKv::remote_config(remote)
        .await
        .map_err(|error| OpenError::ConfigRead(error.to_string()))?
        .ok_or_else(|| OpenError::RemoteMissingUrl(remote.to_string()))?;
    if configured_remote.url.trim().is_empty() {
        return Err(OpenError::RemoteMissingUrl(remote.to_string()));
    }
    Ok(configured_remote.url)
}

/// Derive the deep-link target from the CLI flags, applying the whitelist
/// sanitiser to each component. The first set flag wins; clap enforces
/// mutual exclusion at parse time.
fn parse_target(args: &OpenArgs) -> Result<OpenTarget, OpenError> {
    if let Some(branch) = &args.branch {
        let value = sanitize_ref_component(branch, ComponentKind::Branch)?;
        return Ok(OpenTarget {
            kind: TargetKind::Branch,
            value: Some(value.to_string()),
        });
    }
    if let Some(commit) = &args.commit {
        let value = sanitize_ref_component(commit, ComponentKind::Commit)?;
        return Ok(OpenTarget {
            kind: TargetKind::Commit,
            value: Some(value.to_string()),
        });
    }
    if let Some(issue) = &args.issue {
        let value = match issue {
            Some(raw) => Some(sanitize_numeric_id(raw)?),
            None => None,
        };
        return Ok(OpenTarget {
            kind: TargetKind::Issue,
            value,
        });
    }
    if let Some(pr) = &args.pr {
        let value = match pr {
            Some(raw) => Some(sanitize_numeric_id(raw)?),
            None => None,
        };
        return Ok(OpenTarget {
            kind: TargetKind::PullRequest,
            value,
        });
    }
    Ok(OpenTarget {
        kind: TargetKind::Repo,
        value: None,
    })
}

/// Whitelist-validate a branch name or commit hash. Returns the original slice
/// on success or [`OpenError::UnsafeUrl`] (mapped to `CliInvalidTarget`, exit
/// 129) when it contains anything outside the allowed set. This is the security
/// boundary — `is_safe_url` only checks the scheme and cannot catch shell
/// metacharacters embedded in an otherwise-valid http path.
fn sanitize_ref_component(value: &str, kind: ComponentKind) -> Result<&str, OpenError> {
    if value.is_empty() || value.len() > MAX_REF_LEN {
        return Err(OpenError::UnsafeUrl(value.to_string()));
    }
    match kind {
        ComponentKind::Branch => {
            let charset_ok = value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'));
            if !charset_ok {
                return Err(OpenError::UnsafeUrl(value.to_string()));
            }
            // The charset alone still permits path-traversal shapes; reject them.
            if value.starts_with('/') || value.ends_with('/') || value.contains("//") {
                return Err(OpenError::UnsafeUrl(value.to_string()));
            }
            if value
                .split('/')
                .any(|segment| segment == "." || segment == "..")
            {
                return Err(OpenError::UnsafeUrl(value.to_string()));
            }
        }
        ComponentKind::Commit => {
            if value.len() < 4 || value.len() > 64 || !value.chars().all(|c| c.is_ascii_hexdigit())
            {
                return Err(OpenError::UnsafeUrl(value.to_string()));
            }
        }
    }
    Ok(value)
}

/// Validate an issue / PR id: strip a single leading `#` then require `[0-9]+`.
fn sanitize_numeric_id(value: &str) -> Result<String, OpenError> {
    let trimmed = value.strip_prefix('#').unwrap_or(value);
    if trimmed.is_empty()
        || trimmed.len() > MAX_REF_LEN
        || !trimmed.chars().all(|c| c.is_ascii_digit())
    {
        return Err(OpenError::UnsafeUrl(value.to_string()));
    }
    Ok(trimmed.to_string())
}

/// Domain heuristic for platform identification. Unknown private domains fall
/// back to GitHub-style assembly.
fn detect_platform(host: &str) -> Platform {
    let host = host.to_ascii_lowercase();
    if host.contains("github") {
        Platform::GitHub
    } else if host.contains("gitlab") {
        Platform::GitLab
    } else if host.contains("bitbucket") {
        Platform::Bitbucket
    } else if host.contains("gitea") || host.contains("gogs") {
        Platform::Gitea
    } else {
        tracing::debug!("detect_platform: unknown host '{host}', defaulting to GitHub style");
        Platform::GitHub
    }
}

/// Resolve the effective platform: an explicit, recognised `open.platform`
/// value wins; an unrecognised value warns and falls back to host detection.
fn resolve_platform(configured: Option<&str>, host: &str) -> Platform {
    if let Some(raw) = configured {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "github" => return Platform::GitHub,
            "gitlab" => return Platform::GitLab,
            "gitea" | "gogs" => return Platform::Gitea,
            "bitbucket" => return Platform::Bitbucket,
            "custom" => return Platform::Custom,
            "" => {}
            other => {
                tracing::warn!(
                    "open.platform='{other}' is not recognized; falling back to host detection"
                );
            }
        }
    }
    detect_platform(host)
}

/// Replace every `(placeholder, value)` pair in `template`.
fn apply_template(template: &str, placeholders: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (placeholder, value) in placeholders {
        out = out.replace(placeholder, value);
    }
    out
}

/// Try to render a custom-platform URL from the configured template. Returns
/// `None` (so the caller falls back to GitHub-style assembly) when there is no
/// template for the kind, the value placeholder is missing, the value is empty,
/// or a known placeholder remains unsubstituted.
fn build_custom_url(base: &str, target: &OpenTarget, templates: &TemplateSet) -> Option<String> {
    let (template, placeholder) = match target.kind {
        TargetKind::Repo => return Some(base.to_string()),
        TargetKind::Branch => (templates.branch.as_deref()?, "{branch}"),
        TargetKind::Commit => (templates.commit.as_deref()?, "{commit}"),
        TargetKind::Issue => (templates.issue.as_deref()?, "{issue}"),
        TargetKind::PullRequest => (templates.pull_request.as_deref()?, "{pr}"),
    };

    // Issue/PR list pages (no id) cannot be expressed by an id-bearing
    // template; fall back to the default list path.
    let value = target.value.as_deref()?;
    if value.is_empty() || !template.contains(placeholder) {
        return None;
    }

    let url = apply_template(template, &[("{base_url}", base), (placeholder, value)]);
    if url.contains('{') {
        // An unknown placeholder remained; the template is malformed.
        return None;
    }
    Some(url)
}

/// Assemble the final web URL for `target` on `platform`, starting from the
/// normalised `base` repo URL.
fn build_target_url(
    base: &str,
    platform: Platform,
    target: &OpenTarget,
    templates: &TemplateSet,
) -> String {
    let base = base.trim_end_matches('/');

    if platform == Platform::Custom
        && let Some(url) = build_custom_url(base, target, templates)
    {
        return url;
    }

    // Custom without a usable template falls back to GitHub-style paths.
    let effective = if platform == Platform::Custom {
        Platform::GitHub
    } else {
        platform
    };

    match target.kind {
        TargetKind::Repo => base.to_string(),
        TargetKind::Branch => {
            format!(
                "{base}{}",
                effective.branch_path(target.value.as_deref().unwrap_or_default())
            )
        }
        TargetKind::Commit => {
            format!(
                "{base}{}",
                effective.commit_path(target.value.as_deref().unwrap_or_default())
            )
        }
        TargetKind::Issue => format!("{base}{}", effective.issue_path(target.value.as_deref())),
        TargetKind::PullRequest => {
            format!("{base}{}", effective.pr_path(target.value.as_deref()))
        }
    }
}

/// Extract the host from a (possibly already transformed) URL. Empty string on
/// parse failure (which `detect_platform` treats as the GitHub default).
fn url_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_default()
}

/// Pool-acquiring wrapper around [`load_open_config_with_conn`]. Reads nothing
/// outside a repository (a direct URL run has no local config DB).
async fn load_open_config(in_repo: bool) -> Result<(Option<String>, TemplateSet), OpenError> {
    if !in_repo {
        return Ok((None, TemplateSet::default()));
    }
    let db = get_db_conn_instance().await;
    load_open_config_with_conn(&db).await
}

/// Read `open.platform` and, when it is `custom`, the `open.template.<kind>`
/// keys from the current repository's local config (no global cascade).
async fn load_open_config_with_conn<C: sea_orm::ConnectionTrait>(
    db: &C,
) -> Result<(Option<String>, TemplateSet), OpenError> {
    let platform = read_config_value(db, "open.platform").await?;

    let mut templates = TemplateSet::default();
    let is_custom = platform
        .as_deref()
        .map(|value| value.trim().eq_ignore_ascii_case("custom"))
        .unwrap_or(false);
    if is_custom {
        templates.branch = read_config_value(db, "open.template.branch").await?;
        templates.commit = read_config_value(db, "open.template.commit").await?;
        templates.issue = read_config_value(db, "open.template.issue").await?;
        templates.pull_request = read_config_value(db, "open.template.pull_request").await?;
    }

    Ok((platform, templates))
}

async fn read_config_value<C: sea_orm::ConnectionTrait>(
    db: &C,
    key: &str,
) -> Result<Option<String>, OpenError> {
    Ok(ConfigKv::get_with_conn(db, key)
        .await
        .map_err(|error| OpenError::ConfigRead(error.to_string()))?
        .map(|entry| entry.value))
}

/// Build the `(program, args)` browser-launch command for this platform. Pure
/// and side-effect free so it can be unit-tested without spawning. On Windows
/// the argv is the fixed literal table `["/C", "start", "", <quoted_url>]` with
/// the URL as the only variable, double-quote wrapped (`url::Url::parse`
/// already rejects embedded quotes); Unix execs the launcher directly with a
/// single URL arg and never goes through a shell.
fn build_launch_command(url: &str) -> (&'static str, Vec<String>) {
    #[cfg(target_os = "windows")]
    let command = (
        "cmd",
        vec![
            "/C".to_string(),
            "start".to_string(),
            String::new(),
            quote_windows_cmd_arg(url),
        ],
    );
    #[cfg(target_os = "macos")]
    let command = ("open", vec![url.to_string()]);
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let command = ("xdg-open", vec![url.to_string()]);
    command
}

/// Classify a spawn result into a launch outcome: success, a tolerable missing
/// launcher, or a hard IO error.
fn classify_spawn_result(result: std::io::Result<()>) -> Result<bool, LaunchError> {
    match result {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(LaunchError::NotFound),
        Err(error) => Err(LaunchError::Io(error.to_string())),
    }
}

fn open_browser(url: &str) -> Result<bool, LaunchError> {
    if std::env::var_os(LIBRA_TEST_ENV).is_some() {
        // Keep integration tests side-effect free across all platforms.
        return Ok(false);
    }

    let (program, args) = build_launch_command(url);
    classify_spawn_result(Command::new(program).args(&args).spawn().map(|_child| ()))
}

#[cfg(any(target_os = "windows", test))]
fn quote_windows_cmd_arg(url: &str) -> String {
    // `is_safe_url()` relies on `url::Url::parse`, which rejects embedded
    // double quotes. That makes wrapping sufficient for the current validation.
    format!("\"{url}\"")
}

fn is_safe_url(url: &str) -> bool {
    // Validates that the URL uses http or https scheme.
    // This blocks local file access, javascript:, or other potential injection vectors
    match url::Url::parse(url) {
        Ok(parsed) => parsed.scheme() == "http" || parsed.scheme() == "https",
        Err(_) => false,
    }
}

fn transform_url(remote: &str) -> String {
    if remote.starts_with("http://") || remote.starts_with("https://") {
        return remote.trim_end_matches(".git").to_string();
    }

    // Handle SCP-like syntax: git@github.com:user/repo.git
    if let Some(caps) = SCP_RE.captures(remote) {
        let host = &caps[1];
        let path = &caps[2];
        return format!("https://{}/{}", host, path);
    }

    // Handle ssh:// syntax
    // ssh://[user@]host.xz[:port]/path/to/repo.git/
    if let Some(caps) = SSH_RE.captures(remote) {
        let host = &caps[1];
        let path = &caps[2];
        return format!("https://{}/{}", host, path);
    }

    // Fallback: return as is, maybe it is already workable or user has weird config
    tracing::debug!(
        "transform_url: no pattern matched for '{}', returning as-is",
        remote
    );
    remote.to_string()
}

fn open_cli_error(error: OpenError) -> CliError {
    match error {
        OpenError::NotInRepo => CliError::repo_not_found(),
        OpenError::ConfigRead(message) => {
            CliError::fatal(format!("failed to read remote configuration: {message}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        OpenError::NoRemoteConfigured => CliError::fatal("no remote configured")
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint("add a remote first, for example: 'libra remote add origin <url>'."),
        OpenError::RemoteMissingUrl(name) => {
            CliError::fatal(format!("remote '{name}' is configured but has no URL"))
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint(format!(
                    "configure the URL: 'libra config set remote.{name}.url <url>'."
                ))
        }
        OpenError::UnsafeUrl(url) => CliError::fatal(format!(
            "calculated URL '{url}' is unsafe or invalid. Only http/https are supported."
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
        .with_hint("pass an explicit https:// URL or configure a supported remote URL."),
        OpenError::BrowserLaunch(message) => {
            CliError::fatal(format!("failed to open browser: {message}"))
                .with_stable_code(StableErrorCode::IoWriteFailed)
        }
    }
}

// Unit test
#[cfg(test)]
mod tests {
    use super::*;

    fn target(kind: TargetKind, value: Option<&str>) -> OpenTarget {
        OpenTarget {
            kind,
            value: value.map(str::to_string),
        }
    }

    const NO_TEMPLATES: TemplateSet = TemplateSet {
        branch: None,
        commit: None,
        issue: None,
        pull_request: None,
    };

    #[test]
    fn test_transform_url() {
        assert_eq!(
            transform_url("git@github.com:web3infra-foundation/libra.git"),
            "https://github.com/web3infra-foundation/libra"
        );
        assert_eq!(
            transform_url("git@gitlab.com:group/project.git"),
            "https://gitlab.com/group/project"
        );
        assert_eq!(
            transform_url("https://github.com/web3infra-foundation/libra.git"),
            "https://github.com/web3infra-foundation/libra"
        );
        assert_eq!(
            transform_url("ssh://git@github.com/web3infra-foundation/libra.git"),
            "https://github.com/web3infra-foundation/libra"
        );
        assert_eq!(
            transform_url("ssh://user@host.com:2222/repo.git"),
            "https://host.com/repo"
        );
    }

    #[test]
    fn test_is_safe_url() {
        assert!(is_safe_url("https://github.com/rust-lang/rust"));
        assert!(is_safe_url("http://github.com/rust-lang/rust"));
        assert!(!is_safe_url("file:///etc/passwd"));
        assert!(!is_safe_url("javascript:alert(1)"));
        assert!(!is_safe_url("ftp://github.com/rust-lang/rust"));
    }

    #[test]
    fn test_quote_windows_cmd_arg_wraps_url() {
        assert_eq!(
            quote_windows_cmd_arg("https://evil.example/repo&calc.exe"),
            "\"https://evil.example/repo&calc.exe\""
        );
    }

    // ── URL assembly engine (Batch 0) ────────────────────────────────────

    #[test]
    fn open_target_branch_github() {
        assert_eq!(
            build_target_url(
                "https://github.com/user/repo",
                Platform::GitHub,
                &target(TargetKind::Branch, Some("main")),
                &NO_TEMPLATES
            ),
            "https://github.com/user/repo/tree/main"
        );
    }

    #[test]
    fn open_target_commit_github() {
        assert_eq!(
            build_target_url(
                "https://github.com/user/repo",
                Platform::GitHub,
                &target(TargetKind::Commit, Some("a1b2c3d")),
                &NO_TEMPLATES
            ),
            "https://github.com/user/repo/commit/a1b2c3d"
        );
    }

    #[test]
    fn open_target_branch_gitlab() {
        assert_eq!(
            build_target_url(
                "https://gitlab.com/user/repo",
                Platform::GitLab,
                &target(TargetKind::Branch, Some("main")),
                &NO_TEMPLATES
            ),
            "https://gitlab.com/user/repo/-/tree/main"
        );
    }

    #[test]
    fn open_target_commit_gitlab() {
        assert_eq!(
            build_target_url(
                "https://gitlab.com/user/repo",
                Platform::GitLab,
                &target(TargetKind::Commit, Some("a1b2c3d")),
                &NO_TEMPLATES
            ),
            "https://gitlab.com/user/repo/-/commit/a1b2c3d"
        );
    }

    #[test]
    fn open_target_issue_list() {
        assert_eq!(
            build_target_url(
                "https://github.com/user/repo",
                Platform::GitHub,
                &target(TargetKind::Issue, None),
                &NO_TEMPLATES
            ),
            "https://github.com/user/repo/issues"
        );
    }

    #[test]
    fn open_target_issue_id() {
        // CLI layer strips a leading `#`; the assembly function receives `12`.
        assert_eq!(sanitize_numeric_id("#12").unwrap(), "12");
        assert_eq!(
            build_target_url(
                "https://github.com/user/repo",
                Platform::GitHub,
                &target(TargetKind::Issue, Some("12")),
                &NO_TEMPLATES
            ),
            "https://github.com/user/repo/issues/12"
        );
    }

    #[test]
    fn open_target_pr_github_vs_gitlab() {
        assert_eq!(
            build_target_url(
                "https://github.com/user/repo",
                Platform::GitHub,
                &target(TargetKind::PullRequest, None),
                &NO_TEMPLATES
            ),
            "https://github.com/user/repo/pulls"
        );
        assert_eq!(
            build_target_url(
                "https://gitlab.com/user/repo",
                Platform::GitLab,
                &target(TargetKind::PullRequest, None),
                &NO_TEMPLATES
            ),
            "https://gitlab.com/user/repo/-/merge_requests"
        );
    }

    #[test]
    fn open_direct_url_with_branch() {
        assert_eq!(
            build_target_url(
                "https://github.com/foo/bar",
                Platform::GitHub,
                &target(TargetKind::Branch, Some("dev")),
                &NO_TEMPLATES
            ),
            "https://github.com/foo/bar/tree/dev"
        );
    }

    #[test]
    fn open_target_offline() {
        // Pure string assembly: no network, no reqwest, deterministic.
        let url = build_target_url(
            "https://github.com/user/repo",
            Platform::GitHub,
            &target(TargetKind::Branch, Some("topic")),
            &NO_TEMPLATES,
        );
        assert_eq!(url, "https://github.com/user/repo/tree/topic");
    }

    // ── Platform detection & templates (Batch 1) ─────────────────────────

    #[test]
    fn platform_detect_github() {
        assert_eq!(detect_platform("github.com"), Platform::GitHub);
    }

    #[test]
    fn platform_detect_self_hosted_gitlab() {
        assert_eq!(detect_platform("gitlab.company.com"), Platform::GitLab);
        // Unrecognized private host falls back to GitHub style.
        assert_eq!(detect_platform("vcs.internal.example"), Platform::GitHub);
    }

    #[test]
    fn platform_case_insensitive() {
        assert_eq!(
            resolve_platform(Some("GitLab"), "github.com"),
            Platform::GitLab
        );
        assert_eq!(
            resolve_platform(Some("gitlab"), "github.com"),
            Platform::GitLab
        );
    }

    #[test]
    fn platform_invalid_config_falls_back_to_host() {
        // Unrecognized config value warns and falls back to host detection.
        assert_eq!(
            resolve_platform(Some("nonsense"), "github.com"),
            Platform::GitHub
        );
    }

    #[test]
    fn platform_bitbucket_rules() {
        assert_eq!(
            build_target_url(
                "https://bitbucket.org/user/repo",
                Platform::Bitbucket,
                &target(TargetKind::Commit, Some("abc1234")),
                &NO_TEMPLATES
            ),
            "https://bitbucket.org/user/repo/commits/abc1234"
        );
        assert_eq!(
            build_target_url(
                "https://bitbucket.org/user/repo",
                Platform::Bitbucket,
                &target(TargetKind::PullRequest, None),
                &NO_TEMPLATES
            ),
            "https://bitbucket.org/user/repo/pull-requests"
        );
    }

    #[test]
    fn platform_gitea_rules() {
        assert_eq!(
            build_target_url(
                "https://gitea.com/user/repo",
                Platform::Gitea,
                &target(TargetKind::Commit, Some("abc1234")),
                &NO_TEMPLATES
            ),
            "https://gitea.com/user/repo/commit/abc1234"
        );
        assert_eq!(
            build_target_url(
                "https://gitea.com/user/repo",
                Platform::Gitea,
                &target(TargetKind::Issue, None),
                &NO_TEMPLATES
            ),
            "https://gitea.com/user/repo/issues"
        );
        assert_eq!(
            build_target_url(
                "https://gitea.com/user/repo",
                Platform::Gitea,
                &target(TargetKind::PullRequest, None),
                &NO_TEMPLATES
            ),
            "https://gitea.com/user/repo/pulls"
        );
    }

    #[test]
    fn template_custom_commit() {
        let templates = TemplateSet {
            commit: Some("{base_url}/commit-detail/{commit}".to_string()),
            ..TemplateSet::default()
        };
        assert_eq!(
            build_target_url(
                "https://example.com/u/r",
                Platform::Custom,
                &target(TargetKind::Commit, Some("deadbeef")),
                &templates
            ),
            "https://example.com/u/r/commit-detail/deadbeef"
        );
    }

    #[test]
    fn template_missing_placeholder_falls_back() {
        // Template lacks `{commit}` -> fall back to GitHub-style `/commit/`.
        let templates = TemplateSet {
            commit: Some("{base_url}/no-placeholder-here".to_string()),
            ..TemplateSet::default()
        };
        assert_eq!(
            build_target_url(
                "https://example.com/u/r",
                Platform::Custom,
                &target(TargetKind::Commit, Some("deadbeef")),
                &templates
            ),
            "https://example.com/u/r/commit/deadbeef"
        );
    }

    #[test]
    fn template_replacement_never_empty() {
        // The repo root never grows a trailing `/commit/` with an empty value.
        assert_eq!(
            build_target_url(
                "https://example.com/u/r",
                Platform::Custom,
                &target(TargetKind::Repo, None),
                &NO_TEMPLATES
            ),
            "https://example.com/u/r"
        );
        // apply_template substitutes only provided placeholders.
        assert_eq!(
            apply_template(
                "{base_url}/x/{commit}",
                &[("{base_url}", "B"), ("{commit}", "C")]
            ),
            "B/x/C"
        );
    }

    #[test]
    fn composed_url_passes_is_safe_url() {
        for platform in [
            Platform::GitHub,
            Platform::GitLab,
            Platform::Gitea,
            Platform::Bitbucket,
        ] {
            for tgt in [
                target(TargetKind::Repo, None),
                target(TargetKind::Branch, Some("main")),
                target(TargetKind::Commit, Some("abcdef12")),
                target(TargetKind::Issue, Some("3")),
                target(TargetKind::PullRequest, None),
            ] {
                let url =
                    build_target_url("https://host.example/u/r", platform, &tgt, &NO_TEMPLATES);
                assert!(is_safe_url(&url), "composed url not safe: {url}");
            }
        }
    }

    // ── Security: ref-component whitelist (Batch 2) ──────────────────────

    #[test]
    fn reject_branch_with_shell_metachars() {
        assert!(sanitize_ref_component("main; rm -rf /", ComponentKind::Branch).is_err());
        assert!(sanitize_ref_component("main&calc.exe", ComponentKind::Branch).is_err());
        assert!(sanitize_ref_component("a|b", ComponentKind::Branch).is_err());
        assert!(sanitize_ref_component("a$b", ComponentKind::Branch).is_err());
    }

    #[test]
    fn reject_branch_with_newline() {
        assert!(sanitize_ref_component("main\nrm", ComponentKind::Branch).is_err());
        assert!(sanitize_ref_component("a\tb", ComponentKind::Branch).is_err());
    }

    #[test]
    fn reject_commit_with_backtick() {
        assert!(sanitize_ref_component("abc`whoami`", ComponentKind::Commit).is_err());
        // Non-hex and out-of-range lengths are also rejected.
        assert!(sanitize_ref_component("zzzz", ComponentKind::Commit).is_err());
        assert!(sanitize_ref_component("abc", ComponentKind::Commit).is_err());
        assert!(sanitize_ref_component("a1b2c3d4", ComponentKind::Commit).is_ok());
    }

    #[test]
    fn reject_branch_path_traversal() {
        for bad in [
            "../../etc",
            "/abs",
            "a/../b",
            "a//b",
            "trailing/",
            "/lead",
            ".",
            "..",
        ] {
            assert!(
                sanitize_ref_component(bad, ComponentKind::Branch).is_err(),
                "expected `{bad}` to be rejected"
            );
        }
        // Legitimate slashed branch names are still accepted.
        assert!(sanitize_ref_component("feature/login", ComponentKind::Branch).is_ok());
    }

    #[test]
    fn sanitize_numeric_id_rules() {
        assert_eq!(sanitize_numeric_id("42").unwrap(), "42");
        assert_eq!(sanitize_numeric_id("#7").unwrap(), "7");
        assert!(sanitize_numeric_id("12a").is_err());
        assert!(sanitize_numeric_id("").is_err());
        assert!(sanitize_numeric_id("#").is_err());
    }

    // ── Browser launch (Batch 2) ─────────────────────────────────────────

    #[test]
    fn browser_not_found_exits_zero() {
        let not_found = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert_eq!(
            classify_spawn_result(Err(not_found)),
            Err(LaunchError::NotFound)
        );
        assert_eq!(classify_spawn_result(Ok(())), Ok(true));
        let other = std::io::Error::other("boom");
        assert!(matches!(
            classify_spawn_result(Err(other)),
            Err(LaunchError::Io(_))
        ));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn unix_launch_single_arg() {
        let (program, args) = build_launch_command("https://github.com/u/r");
        assert!(program == "open" || program == "xdg-open");
        assert_eq!(args, vec!["https://github.com/u/r".to_string()]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_launch_fixed_argv() {
        let (program, args) = build_launch_command("https://github.com/u/r");
        assert_eq!(program, "cmd");
        assert_eq!(
            args,
            vec![
                "/C".to_string(),
                "start".to_string(),
                String::new(),
                "\"https://github.com/u/r\"".to_string(),
            ]
        );
    }

    #[test]
    fn open_error_display_pins_each_variant() {
        assert_eq!(
            OpenError::NotInRepo.to_string(),
            "not a libra repository (or any of the parent directories): .libra",
        );
        assert_eq!(
            OpenError::ConfigRead("database is locked".to_string()).to_string(),
            "failed to read remote configuration: database is locked",
        );
        assert_eq!(
            OpenError::NoRemoteConfigured.to_string(),
            "no remote configured",
        );
        assert_eq!(
            OpenError::RemoteMissingUrl("origin".to_string()).to_string(),
            "remote 'origin' is configured but has no URL",
        );
        assert_eq!(
            OpenError::UnsafeUrl("file:///etc/passwd".to_string()).to_string(),
            "calculated URL 'file:///etc/passwd' is unsafe or invalid. Only http/https are supported.",
        );
        assert_eq!(
            OpenError::BrowserLaunch("xdg-open not found".to_string()).to_string(),
            "failed to open browser: xdg-open not found",
        );
    }
}
