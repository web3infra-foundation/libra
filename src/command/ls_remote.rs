//! Implements `ls-remote` to list refs advertised by a remote repository.

use std::{io::Write, path::Path};

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
  libra ls-remote origin
  libra ls-remote https://example.com/repo.git
  libra ls-remote --heads origin main
  libra --json ls-remote --tags origin";

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
        }
    }
}

pub async fn execute_safe(args: LsRemoteArgs, output: &OutputConfig) -> CliResult<()> {
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
    let entries = discovery
        .refs
        .iter()
        .filter(|reference| include_reference(reference, &args, &patterns))
        .map(|reference| LsRemoteEntry {
            hash: reference._hash.clone(),
            refname: reference._ref.clone(),
        })
        .collect();

    Ok(LsRemoteOutput {
        remote: visible_remote,
        url: visible_remote_url(&remote_url),
        heads_only: args.heads,
        tags_only: args.tags,
        refs_only: args.refs,
        patterns: args.patterns,
        entries,
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
        for entry in &data.entries {
            writeln!(writer, "{}\t{}", entry.hash, entry.refname).map_err(|error| {
                CliError::io(format!("failed to write ls-remote output: {error}"))
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use git_internal::errors::GitError;
    use serial_test::serial;
    use tempfile::tempdir;

    use super::{
        CompiledPattern, LsRemoteArgs, include_reference, resolve_remote, sanitize_discovery_error,
        sanitize_remote_error_reason, visible_remote_display, visible_remote_url,
    };
    use crate::{
        internal::protocol::DiscRef,
        utils::{test::ChangeDirGuard, util},
    };

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
}
