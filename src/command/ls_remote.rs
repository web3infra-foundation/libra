//! Implements `ls-remote` to list refs advertised by a remote repository.

use std::io::Write;

use clap::Parser;
use git_internal::errors::GitError;
use regex::Regex;
use serde::Serialize;

use crate::{
    command::fetch::RemoteClient,
    git_protocol::ServiceType::UploadPack,
    internal::{config::ConfigKv, protocol::DiscRef},
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
    let client = RemoteClient::from_spec_with_remote(&remote_url, remote_name.as_deref()).map_err(
        |reason| LsRemoteError::InvalidRemote {
            spec: args.repository.clone(),
            reason,
        },
    )?;
    let discovery = client
        .discovery_reference(UploadPack)
        .await
        .map_err(|source| LsRemoteError::Discovery {
            remote: remote_display.clone(),
            source,
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
        remote: remote_display,
        url: remote_url,
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

fn compile_patterns(patterns: &[String]) -> Result<Vec<CompiledPattern>, LsRemoteError> {
    patterns
        .iter()
        .map(|pattern| CompiledPattern::new(pattern))
        .collect()
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
            '*' => regex.push_str("[^/]*"),
            '?' => regex.push_str("[^/]"),
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
    if args.heads && !refname.starts_with("refs/heads/") {
        return false;
    }
    if args.tags && !refname.starts_with("refs/tags/") {
        return false;
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
    use super::{CompiledPattern, LsRemoteArgs, include_reference};
    use crate::internal::protocol::DiscRef;

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
}
