//! Implements `ls-remote` to list refs advertised by a remote repository.

use std::{collections::BTreeMap, io::Write, path::Path};

use clap::Parser;
use git_internal::errors::GitError;
use regex::Regex;
use serde::Serialize;
use url::Url;

use crate::{
    command::fetch::{RemoteClient, redact_url_credentials},
    git_protocol::ServiceType::UploadPack,
    internal::{
        config::ConfigKv,
        protocol::{DiscRef, ssh_client::is_ssh_spec},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util,
    },
};

const LS_REMOTE_EXAMPLES: &str = "\
EXAMPLES:
    libra ls-remote origin                          List all refs on a configured remote
    libra ls-remote https://example.com/repo.git    List all refs on a remote URL (no remote setup)
    libra ls-remote --heads origin main             List only branch heads matching `main`
    libra ls-remote --get-url origin                Print the resolved remote URL (offline, no network)
    libra ls-remote --symref origin                 Show what HEAD points to (ref: refs/heads/main)
    libra ls-remote --sort=version:refname --tags origin   Sort tags by version
    libra ls-remote --exit-code --heads origin topic       Exit 2 when no branch matches `topic`
    libra --json ls-remote --tags origin            Structured JSON output for agents (tags only)";

#[derive(Parser, Debug)]
#[command(after_help = LS_REMOTE_EXAMPLES)]
pub struct LsRemoteArgs {
    /// Show only branch refs (refs/heads/)
    #[clap(long)]
    pub heads: bool,

    /// Show only tag refs (refs/tags/)
    #[clap(long, short = 't')]
    pub tags: bool,

    /// Do not show HEAD or peeled tag refs (refs ending in ^{})
    #[clap(long)]
    pub refs: bool,

    /// Show the underlying ref a symbolic ref points to (e.g. `ref: refs/heads/main\tHEAD`)
    #[clap(long)]
    pub symref: bool,

    /// Exit with status 2 when no matching refs are found (status 0 otherwise)
    #[clap(long = "exit-code")]
    pub exit_code: bool,

    /// Print only the resolved remote URL and exit, without contacting the remote
    #[clap(long = "get-url")]
    pub get_url: bool,

    /// Sort refs by key: `refname`, `-refname`, `version:refname` / `v:refname` (prefix `-` to reverse)
    #[clap(long, value_name = "KEY")]
    pub sort: Option<String>,

    /// Transmit the given option to the server (accepted for compatibility; not yet forwarded)
    #[clap(short = 'o', long = "server-option", value_name = "OPTION")]
    pub server_option: Vec<String>,

    /// Remote name, URL, or local repository path
    pub repository: String,

    /// Optional ref patterns. Plain names match full refs or path components.
    pub patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct LsRemoteEntry {
    hash: String,
    refname: String,
}

#[derive(Debug, Clone, Serialize)]
struct LsRemoteOutput {
    remote: String,
    url: String,
    heads_only: bool,
    tags_only: bool,
    refs_only: bool,
    patterns: Vec<String>,
    entries: Vec<LsRemoteEntry>,
    /// Symbolic refs advertised by the remote (`--symref`); `HEAD` → `refs/heads/main`.
    /// Omitted from JSON when empty so existing consumers are unaffected.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    symrefs: BTreeMap<String, String>,
}

/// Minimal `--get-url` payload (the JSON `data` carries only the resolved URL).
#[derive(Debug, Clone, Serialize)]
struct LsRemoteUrlOutput {
    url: String,
}

/// Supported `--sort` keys (a subset of git's `for-each-ref` keys — ls-remote
/// only has hash + refname, so object-metadata keys are unsupported).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortKey {
    RefName,
    RefNameDesc,
    Version,
    VersionDesc,
}

#[derive(thiserror::Error, Debug)]
enum LsRemoteError {
    #[error("failed to read remote configuration: {0}")]
    ConfigRead(String),
    #[error("invalid remote '{spec}': {reason}")]
    InvalidRemote { spec: String, reason: String },
    #[error("invalid ref pattern '{pattern}': {reason}")]
    InvalidPattern { pattern: String, reason: String },
    #[error("failed to discover references from '{remote}': {source}")]
    Discovery { remote: String, source: GitError },
    #[error("invalid sort key '{key}'")]
    InvalidSortKey { key: String },
    /// `--exit-code` with no matching refs: a status-2 signal, not an error.
    #[error("no matching refs")]
    NoMatchingRefs,
}

impl From<LsRemoteError> for CliError {
    fn from(error: LsRemoteError) -> Self {
        match &error {
            LsRemoteError::ConfigRead(_) => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
            }
            LsRemoteError::InvalidRemote { .. } | LsRemoteError::InvalidPattern { .. } => {
                CliError::command_usage(error.to_string())
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("use 'libra remote -v' to inspect configured remotes")
            }
            LsRemoteError::Discovery { source, .. } => match source {
                GitError::UnAuthorized(_) => CliError::fatal(error.to_string())
                    .with_stable_code(StableErrorCode::AuthPermissionDenied)
                    .with_hint("check SSH key / HTTP credentials and repository access rights"),
                GitError::NetworkError(_) | GitError::IOError(_) => {
                    CliError::fatal(error.to_string())
                        .with_stable_code(StableErrorCode::NetworkUnavailable)
                        .with_hint("check the remote URL and network connectivity")
                }
                _ => CliError::fatal(error.to_string())
                    .with_stable_code(StableErrorCode::NetworkProtocol),
            },
            LsRemoteError::InvalidSortKey { .. } => CliError::command_usage(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("ls-remote only supports the refname / version:refname sort keys"),
            // `--exit-code` with no matches is a Git-specific status-2 signal:
            // stdout/stderr stay silent (see `git ls-remote --exit-code`).
            LsRemoteError::NoMatchingRefs => CliError::silent_exit(2),
        }
    }
}

pub async fn execute_safe(args: LsRemoteArgs, output: &OutputConfig) -> CliResult<()> {
    // `--get-url` is a purely offline lookup: resolve the URL via config/spec
    // and print it without ever constructing a client or contacting the remote.
    if args.get_url {
        let (_, remote_url, _) = resolve_remote(&args.repository)
            .await
            .map_err(CliError::from)?;
        let url = redact_remote_spec_for_diagnostics(&remote_url);
        if output.is_json() {
            return emit_json_data("ls-remote", &LsRemoteUrlOutput { url }, output);
        }
        if output.quiet {
            return Ok(());
        }
        println!("{url}");
        return Ok(());
    }

    let data = run_ls_remote(args).await.map_err(CliError::from)?;
    render_ls_remote_output(&data, output)
}

async fn run_ls_remote(args: LsRemoteArgs) -> Result<LsRemoteOutput, LsRemoteError> {
    let (remote_display, remote_url, remote_name) = resolve_remote(&args.repository).await?;
    let visible_remote = visible_remote_display(&remote_display, remote_name.as_deref());
    let client = RemoteClient::from_spec_with_remote(&remote_url, remote_name.as_deref()).map_err(
        |reason| LsRemoteError::InvalidRemote {
            spec: visible_remote.clone(),
            reason: sanitize_remote_error_reason(&reason, &remote_url),
        },
    )?;
    let discovery = client
        .discovery_reference(UploadPack)
        .await
        .map_err(|source| LsRemoteError::Discovery {
            remote: visible_remote.clone(),
            source: sanitize_discovery_error(source, &remote_url),
        })?;
    let patterns = compile_patterns(&args.patterns)?;
    let mut entries: Vec<LsRemoteEntry> = discovery
        .refs
        .iter()
        .filter(|reference| include_reference(reference, &args, &patterns))
        .map(|reference| LsRemoteEntry {
            hash: reference._hash.clone(),
            refname: reference._ref.clone(),
        })
        .collect();

    if let Some(key) = &args.sort {
        let sort_key = parse_sort_key(key)?;
        sort_entries(&mut entries, sort_key);
    }

    // Symref advertisements (`--symref`): a symref line is printed when its
    // *name* (e.g. `HEAD`) passes the same `--heads`/`--tags`/pattern filter.
    let symrefs = if args.symref {
        parse_symrefs(&discovery.capabilities)
            .into_iter()
            .filter(|(name, _)| symref_name_included(name, &args, &patterns))
            .collect()
    } else {
        BTreeMap::new()
    };

    // `--exit-code`: a successful handshake with no matching refs is a status-2
    // signal. Symref-only matches still count as a match (git behavior).
    if args.exit_code && entries.is_empty() && symrefs.is_empty() {
        return Err(LsRemoteError::NoMatchingRefs);
    }

    Ok(LsRemoteOutput {
        remote: visible_remote,
        url: visible_remote_url(&remote_url),
        heads_only: args.heads,
        tags_only: args.tags,
        refs_only: args.refs,
        patterns: args.patterns,
        entries,
        symrefs,
    })
}

async fn resolve_remote(
    repository: &str,
) -> Result<(String, String, Option<String>), LsRemoteError> {
    if is_unambiguous_direct_remote_spec(repository) {
        return Ok((repository.to_string(), repository.to_string(), None));
    }

    if util::try_get_storage_path(None).is_ok() {
        let configured = ConfigKv::remote_config(repository)
            .await
            .map_err(|error| LsRemoteError::ConfigRead(error.to_string()))?;
        if let Some(remote) = configured {
            return Ok((remote.name.clone(), remote.url, Some(remote.name)));
        }
    }

    Ok((repository.to_string(), repository.to_string(), None))
}

fn is_unambiguous_direct_remote_spec(repository: &str) -> bool {
    if is_ssh_spec(repository) || Url::parse(repository).is_ok() {
        return true;
    }

    let path = Path::new(repository);
    path.is_absolute()
        || repository.starts_with("./")
        || repository.starts_with("../")
        || repository.starts_with(".\\")
        || repository.starts_with("..\\")
}

fn compile_patterns(patterns: &[String]) -> Result<Vec<CompiledPattern>, LsRemoteError> {
    patterns
        .iter()
        .map(|pattern| CompiledPattern::new(pattern))
        .collect()
}

fn visible_remote_url(remote_url: &str) -> String {
    redact_remote_spec_for_diagnostics(remote_url)
}

fn visible_remote_display(remote_display: &str, remote_name: Option<&str>) -> String {
    if remote_name.is_some() {
        remote_display.to_string()
    } else {
        redact_remote_spec_for_diagnostics(remote_display)
    }
}

fn sanitize_remote_error_reason(reason: &str, remote_url: &str) -> String {
    let redacted_remote = redact_remote_spec_for_diagnostics(remote_url);
    let reason = reason.replace(remote_url, &redacted_remote);
    redact_embedded_remote_credentials(&reason)
}

fn sanitize_discovery_error(source: GitError, remote_url: &str) -> GitError {
    match source {
        GitError::NetworkError(message) => {
            GitError::NetworkError(sanitize_remote_error_reason(&message, remote_url))
        }
        GitError::UnAuthorized(message) => {
            GitError::UnAuthorized(sanitize_remote_error_reason(&message, remote_url))
        }
        GitError::CustomError(message) => {
            GitError::CustomError(sanitize_remote_error_reason(&message, remote_url))
        }
        GitError::IOError(error) => GitError::IOError(std::io::Error::new(
            error.kind(),
            sanitize_remote_error_reason(&error.to_string(), remote_url),
        )),
        other => other,
    }
}

fn redact_remote_spec_for_diagnostics(remote_url: &str) -> String {
    let redacted = redact_url_credentials(remote_url);
    if redacted != remote_url {
        return redacted;
    }
    redact_embedded_remote_credentials(remote_url)
}

fn redact_embedded_remote_credentials(input: &str) -> String {
    let mut redacted = input.to_string();

    if let Ok(url_like_userinfo) = Regex::new(r"(?i)([a-z][a-z0-9+.-]*://)([^\s/@]+@)") {
        redacted = url_like_userinfo
            .replace_all(&redacted, "${1}[REDACTED]@")
            .into_owned();
    }

    if let Ok(scp_like_userinfo) = Regex::new(r#"(^|[\s'`"])([^\s'`"/@]+:[^\s'`"/@]+@)"#) {
        redacted = scp_like_userinfo
            .replace_all(&redacted, "${1}[REDACTED]@")
            .into_owned();
    }

    redacted
}

struct CompiledPattern {
    raw: String,
    regex: Option<Regex>,
}

impl CompiledPattern {
    fn new(pattern: &str) -> Result<Self, LsRemoteError> {
        let has_glob = pattern.chars().any(|c| matches!(c, '*' | '?' | '['));
        let regex = if has_glob {
            Some(glob_to_regex(pattern)?)
        } else {
            None
        };
        Ok(Self {
            raw: pattern.to_string(),
            regex,
        })
    }

    fn matches(&self, refname: &str) -> bool {
        if let Some(regex) = &self.regex {
            return regex.is_match(refname);
        }

        refname == self.raw || refname.ends_with(&format!("/{}", self.raw))
    }
}

fn glob_to_regex(pattern: &str) -> Result<Regex, LsRemoteError> {
    let mut regex = String::from("(^|.*/)");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            '[' => regex.push('['),
            ']' => regex.push(']'),
            '.' | '+' | '(' | ')' | '{' | '}' | '|' | '^' | '$' | '\\' => {
                regex.push('\\');
                regex.push(ch);
            }
            other => regex.push(other),
        }
    }
    regex.push('$');
    Regex::new(&regex).map_err(|error| LsRemoteError::InvalidPattern {
        pattern: pattern.to_string(),
        reason: error.to_string(),
    })
}

fn include_reference(
    reference: &DiscRef,
    args: &LsRemoteArgs,
    patterns: &[CompiledPattern],
) -> bool {
    let refname = reference._ref.as_str();
    if args.refs && (refname == "HEAD" || refname.ends_with("^{}")) {
        return false;
    }
    if args.heads || args.tags {
        let matches_heads = args.heads && refname.starts_with("refs/heads/");
        let matches_tags = args.tags && refname.starts_with("refs/tags/");
        if !matches_heads && !matches_tags {
            return false;
        }
    }
    patterns.is_empty() || patterns.iter().any(|pattern| pattern.matches(refname))
}

fn render_ls_remote_output(data: &LsRemoteOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        emit_json_data("ls-remote", data, output)
    } else if output.quiet {
        Ok(())
    } else {
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();
        write_ls_remote_text(&mut writer, data)
    }
}

fn write_ls_remote_text<W: Write>(writer: &mut W, data: &LsRemoteOutput) -> CliResult<()> {
    // Symref lines (`ref: <target>\t<name>`) print before the SHA rows.
    for (name, target) in &data.symrefs {
        writeln!(writer, "ref: {target}\t{name}")
            .map_err(|error| CliError::io(format!("failed to write ls-remote output: {error}")))?;
    }
    for entry in &data.entries {
        writeln!(writer, "{}\t{}", entry.hash, entry.refname)
            .map_err(|error| CliError::io(format!("failed to write ls-remote output: {error}")))?;
    }
    Ok(())
}

/// Extracts `symref=<name>:<target>` advertisements from the discovered protocol
/// capabilities. Each symref is already a separate space-split capability token;
/// malformed entries (no `:`) are logged and skipped. A `BTreeMap` gives a
/// deterministic order for output and tests.
fn parse_symrefs(capabilities: &[String]) -> BTreeMap<String, String> {
    let mut symrefs = BTreeMap::new();
    for cap in capabilities {
        let Some(spec) = cap.strip_prefix("symref=") else {
            continue;
        };
        match spec.split_once(':') {
            Some((name, target)) if !name.is_empty() && !target.is_empty() => {
                symrefs.insert(name.to_string(), target.to_string());
            }
            _ => tracing::debug!(capability = %cap, "skipping malformed symref capability"),
        }
    }
    symrefs
}

/// Whether a symref *name* (e.g. `HEAD`) passes the active `--heads`/`--tags`/
/// pattern filters, deciding if its `ref:` line is printed (git semantics).
fn symref_name_included(name: &str, args: &LsRemoteArgs, patterns: &[CompiledPattern]) -> bool {
    let probe = DiscRef {
        _hash: String::new(),
        _ref: name.to_string(),
    };
    include_reference(&probe, args, patterns)
}

/// Parses a `--sort` key from the supported whitelist; anything else (including
/// git keys ls-remote cannot honor, like `objectname`) is rejected with 129.
fn parse_sort_key(key: &str) -> Result<SortKey, LsRemoteError> {
    match key {
        "refname" => Ok(SortKey::RefName),
        "-refname" => Ok(SortKey::RefNameDesc),
        "version:refname" | "v:refname" => Ok(SortKey::Version),
        "-version:refname" | "-v:refname" => Ok(SortKey::VersionDesc),
        other => Err(LsRemoteError::InvalidSortKey {
            key: other.to_string(),
        }),
    }
}

/// Splits a refname into numeric and non-numeric segments for natural version
/// ordering (digit runs compare numerically; everything else lexically).
fn version_segments(refname: &str) -> Vec<(bool, String)> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut current_is_digit = false;
    for ch in refname.chars() {
        let is_digit = ch.is_ascii_digit();
        if !current.is_empty() && is_digit != current_is_digit {
            segments.push((current_is_digit, std::mem::take(&mut current)));
        }
        current_is_digit = is_digit;
        current.push(ch);
    }
    if !current.is_empty() {
        segments.push((current_is_digit, current));
    }
    segments
}

fn version_segments_cmp(
    sa: &[(bool, String)],
    sb: &[(bool, String)],
    a: &str,
    b: &str,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    for (seg_a, seg_b) in sa.iter().zip(sb.iter()) {
        let ord = match (seg_a.0, seg_b.0) {
            // Two numeric runs compare by value (without overflow via u128;
            // fall back to string order for absurdly long digit runs).
            (true, true) => match (seg_a.1.parse::<u128>(), seg_b.1.parse::<u128>()) {
                (Ok(na), Ok(nb)) => na.cmp(&nb),
                _ => seg_a.1.cmp(&seg_b.1),
            },
            (false, false) => seg_a.1.cmp(&seg_b.1),
            // Numeric segments sort before alphabetic ones (e.g. `v1` < `vbeta`).
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    sa.len().cmp(&sb.len()).then_with(|| a.cmp(b))
}

fn sort_entries_by_version(entries: &mut [LsRemoteEntry], reverse: bool) {
    let mut keyed: Vec<(Vec<(bool, String)>, LsRemoteEntry)> = entries
        .iter()
        .cloned()
        .map(|entry| (version_segments(&entry.refname), entry))
        .collect();
    keyed.sort_by(|(segments_a, entry_a), (segments_b, entry_b)| {
        let ord = version_segments_cmp(segments_a, segments_b, &entry_a.refname, &entry_b.refname);
        if reverse { ord.reverse() } else { ord }
    });
    for (slot, (_, entry)) in entries.iter_mut().zip(keyed) {
        *slot = entry;
    }
}

fn sort_entries(entries: &mut [LsRemoteEntry], key: SortKey) {
    match key {
        SortKey::RefName => entries.sort_by(|a, b| a.refname.cmp(&b.refname)),
        SortKey::RefNameDesc => entries.sort_by(|a, b| b.refname.cmp(&a.refname)),
        SortKey::Version => sort_entries_by_version(entries, false),
        SortKey::VersionDesc => sort_entries_by_version(entries, true),
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs};

    use clap::Parser as _;
    use git_internal::errors::GitError;
    use serial_test::serial;
    use tempfile::tempdir;

    use super::{
        CompiledPattern, LsRemoteArgs, LsRemoteEntry, LsRemoteError, LsRemoteOutput, SortKey,
        include_reference, parse_sort_key, parse_symrefs, resolve_remote, sanitize_discovery_error,
        sanitize_remote_error_reason, sort_entries, visible_remote_display, visible_remote_url,
        write_ls_remote_text,
    };
    use crate::{
        internal::protocol::DiscRef,
        utils::{test::ChangeDirGuard, util},
    };

    /// Pin the `Display` format for the owned variants of [`LsRemoteError`].
    /// `ConfigRead`, `InvalidRemote`, and `InvalidPattern` are fully owned
    /// by this enum's `#[error(...)]` attributes; `Discovery` forwards
    /// `{source}` to `GitError` and is intentionally skipped (the wrapped
    /// type's Display contract lives in `git_internal`).
    #[test]
    fn ls_remote_error_display_pins_each_owned_variant() {
        assert_eq!(
            LsRemoteError::ConfigRead("db locked".to_string()).to_string(),
            "failed to read remote configuration: db locked",
        );
        assert_eq!(
            LsRemoteError::InvalidRemote {
                spec: "ftp://example.com/repo".to_string(),
                reason: "unsupported scheme".to_string(),
            }
            .to_string(),
            "invalid remote 'ftp://example.com/repo': unsupported scheme",
        );
        assert_eq!(
            LsRemoteError::InvalidPattern {
                pattern: "**".to_string(),
                reason: "empty alternation".to_string(),
            }
            .to_string(),
            "invalid ref pattern '**': empty alternation",
        );
    }

    fn disc_ref(refname: &str) -> DiscRef {
        DiscRef {
            _hash: "1111111111111111111111111111111111111111".to_string(),
            _ref: refname.to_string(),
        }
    }

    #[test]
    fn plain_pattern_matches_ref_tail() {
        let pattern = CompiledPattern::new("main").unwrap();
        assert!(pattern.matches("refs/heads/main"));
        assert!(!pattern.matches("refs/heads/feature"));
    }

    #[test]
    fn glob_pattern_matches_nested_refs_across_slashes() {
        let full_ref = CompiledPattern::new("refs/heads/*").unwrap();
        assert!(full_ref.matches("refs/heads/feature/foo"));
        assert!(!full_ref.matches("refs/tags/feature/foo"));

        let tail_ref = CompiledPattern::new("feature*").unwrap();
        assert!(tail_ref.matches("refs/heads/feature/foo"));

        let question_ref = CompiledPattern::new("a?b").unwrap();
        assert!(question_ref.matches("refs/heads/a/b"));
    }

    #[test]
    fn refs_flag_excludes_head_and_peeled_tags() {
        let args = LsRemoteArgs {
            heads: false,
            tags: false,
            refs: true,
            symref: false,
            exit_code: false,
            get_url: false,
            sort: None,
            server_option: vec![],
            repository: "origin".to_string(),
            patterns: vec![],
        };
        assert!(!include_reference(&disc_ref("HEAD"), &args, &[]));
        assert!(!include_reference(
            &disc_ref("refs/tags/v1.0^{}"),
            &args,
            &[]
        ));
        assert!(include_reference(&disc_ref("refs/tags/v1.0"), &args, &[]));
    }

    #[test]
    fn heads_and_tags_filters_use_union() {
        let args = LsRemoteArgs {
            heads: true,
            tags: true,
            refs: false,
            symref: false,
            exit_code: false,
            get_url: false,
            sort: None,
            server_option: vec![],
            repository: "origin".to_string(),
            patterns: vec![],
        };
        assert!(include_reference(&disc_ref("refs/heads/main"), &args, &[]));
        assert!(include_reference(&disc_ref("refs/tags/v1.0"), &args, &[]));
        assert!(!include_reference(&disc_ref("HEAD"), &args, &[]));
    }

    #[test]
    fn visible_remote_url_redacts_http_credentials() {
        assert_eq!(
            visible_remote_url("https://token@example.com/repo.git"),
            "https://example.com/repo.git"
        );
        assert_eq!(
            visible_remote_url("https://user:secret@example.com/repo.git"),
            "https://example.com/repo.git"
        );
    }

    #[test]
    fn visible_remote_url_redacts_scp_password() {
        assert_eq!(
            visible_remote_url("user:secret@example.com:repo.git"),
            "[REDACTED]@example.com:repo.git"
        );
    }

    #[tokio::test]
    #[serial]
    async fn resolve_direct_url_skips_broken_current_repo_config() {
        let repo = tempdir().unwrap();
        let storage = repo.path().join(util::ROOT_DIR);
        fs::create_dir_all(&storage).unwrap();
        fs::write(storage.join(util::DATABASE), b"not sqlite").unwrap();
        let _guard = ChangeDirGuard::new(repo.path());

        let resolved = resolve_remote("https://example.com/repo.git")
            .await
            .unwrap();

        assert_eq!(
            resolved,
            (
                "https://example.com/repo.git".to_string(),
                "https://example.com/repo.git".to_string(),
                None
            )
        );
    }

    #[test]
    fn visible_remote_display_redacts_direct_url_but_preserves_remote_name() {
        assert_eq!(
            visible_remote_display("https://token@example.com/repo.git", None),
            "https://example.com/repo.git"
        );
        assert_eq!(visible_remote_display("origin", Some("origin")), "origin");
    }

    #[test]
    fn visible_remote_display_redacts_direct_scp_password() {
        assert_eq!(
            visible_remote_display("user:secret@example.com:repo.git", None),
            "[REDACTED]@example.com:repo.git"
        );
        assert_eq!(
            visible_remote_display("user:secret@example.com:repo.git", Some("origin")),
            "user:secret@example.com:repo.git"
        );
    }

    #[test]
    fn invalid_remote_reason_redacts_valid_url_credentials() {
        let remote = "file://user:secret@example.com/repo.git";
        let reason = format!("invalid file url: {remote}");

        let sanitized = sanitize_remote_error_reason(&reason, remote);

        assert!(!sanitized.contains("user"));
        assert!(!sanitized.contains("secret"));
        assert!(sanitized.contains("file://example.com/repo.git"));
    }

    #[test]
    fn invalid_remote_reason_redacts_malformed_url_like_credentials() {
        let remote = "https://user:secret@";
        let reason = format!("invalid local repository '{remote}': not found");

        let sanitized = sanitize_remote_error_reason(&reason, remote);

        assert!(!sanitized.contains("user"));
        assert!(!sanitized.contains("secret"));
        assert!(sanitized.contains("https://[REDACTED]@"));
    }

    #[test]
    fn invalid_remote_reason_redacts_scp_like_password_credentials() {
        let remote = "user:secret@example.com:repo.git";
        let reason = format!("invalid local repository '{remote}': not found");

        let sanitized = sanitize_remote_error_reason(&reason, remote);

        assert!(!sanitized.contains("user:secret"));
        assert!(sanitized.contains("[REDACTED]@example.com:repo.git"));
    }

    #[test]
    fn discovery_error_redacts_url_credentials_in_source() {
        let remote = "https://user:secret@example.invalid/repo.git";
        let source = GitError::NetworkError(format!(
            "Failed to send request: error sending request for url ({remote}/info/refs?service=git-upload-pack): dns error"
        ));

        let sanitized = sanitize_discovery_error(source, remote).to_string();

        assert!(!sanitized.contains("user"));
        assert!(!sanitized.contains("secret"));
        assert!(sanitized.contains("https://example.invalid/repo.git"));
    }

    // ── symref / sort flag parsing + pure helpers ──

    fn entry(refname: &str) -> LsRemoteEntry {
        LsRemoteEntry {
            hash: "1111111111111111111111111111111111111111".to_string(),
            refname: refname.to_string(),
        }
    }

    #[test]
    fn parse_symrefs_single() {
        let caps = vec!["symref=HEAD:refs/heads/main".to_string()];
        let map = parse_symrefs(&caps);
        assert_eq!(map.get("HEAD").map(String::as_str), Some("refs/heads/main"));
    }

    #[test]
    fn parse_symrefs_multiple() {
        let caps = vec![
            "symref=HEAD:refs/heads/main".to_string(),
            "symref=refs/heads/alias:refs/heads/real".to_string(),
        ];
        let map = parse_symrefs(&caps);
        assert_eq!(map.len(), 2);
        assert_eq!(map["HEAD"], "refs/heads/main");
        assert_eq!(map["refs/heads/alias"], "refs/heads/real");
    }

    #[test]
    fn parse_symrefs_skips_malformed() {
        // No colon → malformed, skipped without panic.
        assert!(parse_symrefs(&["symref=HEAD".to_string()]).is_empty());
    }

    #[test]
    fn parse_symrefs_ignores_non_symref_caps() {
        let caps = vec![
            "object-format=sha1".to_string(),
            "agent=git/2.0".to_string(),
        ];
        assert!(parse_symrefs(&caps).is_empty());
    }

    #[test]
    fn parse_symrefs_empty_for_local() {
        assert!(parse_symrefs(&[]).is_empty());
    }

    #[test]
    fn text_output_writes_symrefs_before_entries() {
        let data = LsRemoteOutput {
            remote: "origin".to_string(),
            url: "https://example.com/repo.git".to_string(),
            heads_only: false,
            tags_only: false,
            refs_only: false,
            patterns: Vec::new(),
            entries: vec![entry("HEAD"), entry("refs/heads/main")],
            symrefs: BTreeMap::from([("HEAD".to_string(), "refs/heads/main".to_string())]),
        };
        let mut buffer = Vec::new();

        write_ls_remote_text(&mut buffer, &data).unwrap();

        let text = String::from_utf8(buffer).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines[0], "ref: refs/heads/main\tHEAD");
        assert_eq!(lines[1], "1111111111111111111111111111111111111111\tHEAD");
        assert_eq!(
            lines[2],
            "1111111111111111111111111111111111111111\trefs/heads/main"
        );
    }

    #[test]
    fn json_output_includes_symrefs_when_present() {
        let data = LsRemoteOutput {
            remote: "origin".to_string(),
            url: "https://example.com/repo.git".to_string(),
            heads_only: false,
            tags_only: false,
            refs_only: false,
            patterns: Vec::new(),
            entries: vec![entry("HEAD")],
            symrefs: BTreeMap::from([("HEAD".to_string(), "refs/heads/main".to_string())]),
        };

        let json = serde_json::to_value(&data).unwrap();

        assert_eq!(json["symrefs"]["HEAD"], "refs/heads/main");
        assert_eq!(json["entries"][0]["refname"], "HEAD");
    }

    #[test]
    fn parse_sort_key_whitelist() {
        assert_eq!(parse_sort_key("refname").unwrap(), SortKey::RefName);
        assert_eq!(parse_sort_key("-refname").unwrap(), SortKey::RefNameDesc);
        assert_eq!(parse_sort_key("version:refname").unwrap(), SortKey::Version);
        assert_eq!(parse_sort_key("v:refname").unwrap(), SortKey::Version);
        assert_eq!(parse_sort_key("-v:refname").unwrap(), SortKey::VersionDesc);
        assert!(matches!(
            parse_sort_key("objectname"),
            Err(LsRemoteError::InvalidSortKey { .. })
        ));
    }

    #[test]
    fn sort_entries_refname() {
        let mut entries = vec![
            entry("refs/tags/b"),
            entry("refs/tags/a"),
            entry("refs/tags/c"),
        ];
        sort_entries(&mut entries, SortKey::RefName);
        let names: Vec<_> = entries.iter().map(|e| e.refname.as_str()).collect();
        assert_eq!(names, ["refs/tags/a", "refs/tags/b", "refs/tags/c"]);

        sort_entries(&mut entries, SortKey::RefNameDesc);
        let names: Vec<_> = entries.iter().map(|e| e.refname.as_str()).collect();
        assert_eq!(names, ["refs/tags/c", "refs/tags/b", "refs/tags/a"]);
    }

    #[test]
    fn sort_entries_version_is_natural() {
        let mut entries = vec![
            entry("refs/tags/v1.10.0"),
            entry("refs/tags/v1.2.0"),
            entry("refs/tags/v1.9.0"),
        ];
        sort_entries(&mut entries, SortKey::Version);
        let names: Vec<_> = entries.iter().map(|e| e.refname.as_str()).collect();
        // 1.2 < 1.9 < 1.10 numerically (lexical order would wrongly put 1.10 first).
        assert_eq!(
            names,
            ["refs/tags/v1.2.0", "refs/tags/v1.9.0", "refs/tags/v1.10.0"]
        );
    }

    #[test]
    fn ls_remote_new_flags_parse() {
        let args = LsRemoteArgs::try_parse_from([
            "ls-remote",
            "--symref",
            "--exit-code",
            "--sort=refname",
            "-o",
            "opt1",
            "--server-option",
            "opt2",
            "origin",
        ])
        .unwrap();
        assert!(args.symref);
        assert!(args.exit_code);
        assert_eq!(args.sort.as_deref(), Some("refname"));
        assert_eq!(args.server_option, vec!["opt1", "opt2"]);
        assert_eq!(args.repository, "origin");
    }
}
