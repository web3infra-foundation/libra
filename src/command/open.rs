use std::process::Command;

use clap::Parser;
use lazy_static::lazy_static;
use regex::Regex;
use serde::Serialize;

use crate::{
    internal::config::ConfigKv,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
    },
};

const OPEN_EXAMPLES: &str = "\
EXAMPLES:
  libra open
  libra open origin
  libra open https://github.com/web3infra-foundation/libra
  libra open --json
";

#[derive(Parser, Debug)]
#[command(after_help = OPEN_EXAMPLES)]
pub struct OpenArgs {
    /// The remote to open
    pub remote: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct OpenOutput {
    remote: Option<String>,
    remote_url: String,
    web_url: String,
    launched: bool,
}

#[derive(Debug)]
struct OpenResolution {
    remote: Option<String>,
    remote_url: String,
}

#[derive(Debug, thiserror::Error)]
enum OpenError {
    #[error("not a libra repository (or any of the parent directories): .libra")]
    NotInRepo,
    #[error("failed to read remote configuration: {0}")]
    ConfigRead(String),
    #[error("no remote configured")]
    NoRemoteConfigured,
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
/// errors and exiting. Resolves the remote URL and opens it in the default
/// browser.
pub async fn execute_safe(args: OpenArgs, output: &OutputConfig) -> CliResult<()> {
    let in_repo = crate::utils::util::require_repo().is_ok();
    let resolution = resolve_open_target(args, in_repo)
        .await
        .map_err(open_cli_error)?;
    let web_url = transform_url(&resolution.remote_url);

    if !is_safe_url(&web_url) {
        return Err(open_cli_error(OpenError::UnsafeUrl(web_url)));
    }

    open_browser(&web_url).map_err(|e| open_cli_error(OpenError::BrowserLaunch(e.to_string())))?;

    let open_output = OpenOutput {
        remote: resolution.remote,
        remote_url: resolution.remote_url,
        web_url: web_url.clone(),
        launched: true,
    };

    if output.is_json() {
        emit_json_data("open", &open_output, output)?;
    } else if !output.quiet {
        println!("Opening {}", web_url);
    }

    Ok(())
}

async fn resolve_open_target(args: OpenArgs, in_repo: bool) -> Result<OpenResolution, OpenError> {
    if let Some(input) = args.remote {
        if in_repo {
            let remotes = ConfigKv::all_remote_configs()
                .await
                .map_err(|error| OpenError::ConfigRead(error.to_string()))?;
            if remotes.iter().any(|remote| remote.name == input) {
                let remote_url = load_remote_url(&input).await?;
                return Ok(OpenResolution {
                    remote: Some(input),
                    remote_url,
                });
            }
        }

        return Ok(OpenResolution {
            remote: None,
            remote_url: input,
        });
    }

    if !in_repo {
        return Err(OpenError::NotInRepo);
    }

    if let Some(current_remote) = ConfigKv::get_current_remote()
        .await
        .map_err(|error| OpenError::ConfigRead(error.to_string()))?
    {
        let remote_url = load_remote_url(&current_remote).await?;
        return Ok(OpenResolution {
            remote: Some(current_remote),
            remote_url,
        });
    }

    let remotes = ConfigKv::all_remote_configs()
        .await
        .map_err(|error| OpenError::ConfigRead(error.to_string()))?;
    if let Some(origin) = remotes.iter().find(|remote| remote.name == "origin") {
        return Ok(OpenResolution {
            remote: Some("origin".to_string()),
            remote_url: origin.url.clone(),
        });
    }
    if let Some(first) = remotes.first() {
        return Ok(OpenResolution {
            remote: Some(first.name.clone()),
            remote_url: first.url.clone(),
        });
    }

    Err(OpenError::NoRemoteConfigured)
}

async fn load_remote_url(remote: &str) -> Result<String, OpenError> {
    ConfigKv::get_remote_url(remote).await.map_err(|error| {
        let message = error.to_string();
        if message.contains("No URL configured for remote") {
            OpenError::NoRemoteConfigured
        } else {
            OpenError::ConfigRead(message)
        }
    })
}

fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd").args(["/C", "start", "", url]).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).spawn()?;
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        Command::new("xdg-open").arg(url).spawn()?;
    }
    Ok(())
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
}
