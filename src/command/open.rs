use std::process::Command;

use clap::Parser;
use lazy_static::lazy_static;
use regex::Regex;

use crate::{
    internal::config::Config,
    utils::error::{CliError, CliResult},
};

#[derive(Parser, Debug)]
pub struct OpenArgs {
    /// The remote to open
    pub remote: Option<String>,
}

lazy_static! {
    static ref SCP_RE: Regex = Regex::new(r"^git@([^:]+):(.+?)(\.git)?$").expect("Invalid Regex");
    static ref SSH_RE: Regex =
        Regex::new(r"^ssh://(?:[^@]+@)?([^:/]+)(?::\d+)?/(.+?)(\.git)?$").expect("Invalid Regex");
}

pub async fn execute(args: OpenArgs) {
    if let Err(e) = execute_safe(args).await {
        eprintln!("{}", e.render());
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Resolves the remote URL and opens it in the default
/// browser.
pub async fn execute_safe(args: OpenArgs) -> CliResult<()> {
    let in_repo = crate::utils::util::require_repo().is_ok();

    let remote_url = if let Some(input) = args.remote {
        if in_repo {
            let remotes = Config::all_remote_configs().await;
            if remotes.iter().any(|r| r.name == input) {
                Config::get_remote_url(&input).await
            } else {
                input
            }
        } else {
            input
        }
    } else {
        if !in_repo {
            return Err(CliError::fatal(
                "not a libra repository (or any of the parent directories): .libra",
            ));
        }
        match Config::get_current_remote_url().await {
            Some(url) => url,
            None => {
                let remotes = Config::all_remote_configs().await;
                if let Some(origin) = remotes.iter().find(|r| r.name == "origin") {
                    origin.url.clone()
                } else if let Some(first) = remotes.first() {
                    first.url.clone()
                } else {
                    return Err(CliError::fatal("no remote configured"));
                }
            }
        }
    };

    let url = transform_url(&remote_url);

    if !is_safe_url(&url) {
        return Err(CliError::fatal(format!(
            "calculated URL '{}' is unsafe or invalid. Only http/https are supported.",
            url
        )));
    }

    println!("Opening {}", url);

    open_browser(&url).map_err(|e| CliError::fatal(format!("failed to open browser: {e}")))?;
    Ok(())
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
