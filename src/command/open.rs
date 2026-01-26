use std::process::Command;

use clap::Parser;
use regex::Regex;

use crate::internal::config::Config;

#[derive(Parser, Debug)]
pub struct OpenArgs {
    /// The remote to open
    pub remote: Option<String>,
}

pub async fn open(args: OpenArgs) {
    let remote_url = if let Some(remote_name) = args.remote {
        let remotes = Config::all_remote_configs().await;
        if remotes.iter().any(|r| r.name == remote_name) {
            Config::get_remote_url(&remote_name).await
        } else {
            eprintln!("fatal: Remote '{}' not found", remote_name);
            return;
        }
    } else {
        match Config::get_current_remote_url().await {
            Some(url) => url,
            None => {
                // Fallback, try origin
                let remotes = Config::all_remote_configs().await;
                if let Some(origin) = remotes.iter().find(|r| r.name == "origin") {
                    origin.url.clone()
                } else if let Some(first) = remotes.first() {
                    first.url.clone() // Fallback to first available remote
                } else {
                    eprintln!("fatal: No remote configured");
                    return;
                }
            }
        }
    };

    let url = transform_url(&remote_url);
    println!("Opening {}", url);

    #[cfg(target_os = "windows")]
    {
        Command::new("cmd").args(["/C", "start", &url]).spawn().ok();
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(&url).spawn().ok();
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        Command::new("xdg-open").arg(&url).spawn().ok();
    }
}

fn transform_url(remote: &str) -> String {
    if remote.starts_with("http://") || remote.starts_with("https://") {
        return remote.trim_end_matches(".git").to_string();
    }

    // Handle SCP-like syntax: git@github.com:user/repo.git
    let scp_re = Regex::new(r"^git@([^:]+):(.+?)(\.git)?$").unwrap();
    if let Some(caps) = scp_re.captures(remote) {
        let host = &caps[1];
        let path = &caps[2];
        return format!("https://{}/{}", host, path);
    }

    // Handle ssh:// syntax
    // ssh://[user@]host.xz[:port]/path/to/repo.git/
    if remote.starts_with("ssh://") {
        let ssh_re = Regex::new(r"^ssh://(?:[^@]+@)?([^:/]+)(?::\d+)?/(.+?)(\.git)?$").unwrap();
        if let Some(caps) = ssh_re.captures(remote) {
            let host = &caps[1];
            let path = &caps[2];
            return format!("https://{}/{}", host, path);
        }
    }

    // Fallback: return as is, maybe it is already workable or user has weird config
    remote.to_string()
}

// Uint test
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transform_url() {
        assert_eq!(
            transform_url("git@github.com:rust-lang/rust.git"),
            "https://github.com/rust-lang/rust"
        );
        assert_eq!(
            transform_url("git@gitlab.com:group/project.git"),
            "https://gitlab.com/group/project"
        );
        assert_eq!(
            transform_url("https://github.com/rust-lang/rust.git"),
            "https://github.com/rust-lang/rust"
        );
        assert_eq!(
            transform_url("ssh://git@github.com/rust-lang/rust.git"),
            "https://github.com/rust-lang/rust"
        );
        assert_eq!(
            transform_url("ssh://user@host.com:2222/repo.git"),
            "https://host.com/repo"
        );
    }
}
