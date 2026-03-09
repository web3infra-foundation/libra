//! SSH protocol client that spawns an `ssh` subprocess for Git transport.
//!
//! Supports both `ssh://[user@]host[:port]/path` and `user@host:path` URL formats.
//! Uses the vault-generated SSH private key for authentication when available.

use std::io::Error as IoError;

use bytes::{Bytes, BytesMut};
use futures_util::stream::{self, StreamExt};
use git_internal::errors::GitError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    DiscoveryResult, FetchStream, generate_upload_pack_content, parse_discovered_references,
};
use crate::git_protocol::ServiceType;

const DEFAULT_SSH_PORT: u16 = 22;

pub struct SshClient {
    user: String,
    host: String,
    port: u16,
    repo_path: String,
    key_path: Option<String>,
    strict_host_key_checking: String,
}

impl SshClient {
    /// Parse an SSH URL in either `ssh://[user@]host[:port]/path` or `user@host:path` format.
    pub fn from_ssh_spec(spec: &str) -> Result<Self, String> {
        if spec.starts_with("ssh://") {
            Self::from_ssh_url(spec)
        } else {
            Self::from_scp_style(spec)
        }
    }

    /// Set the path to the SSH private key for authentication.
    pub fn with_key_path(mut self, key_path: String) -> Self {
        self.key_path = Some(key_path);
        self
    }

    /// Configure StrictHostKeyChecking mode.
    ///
    /// Supported values: `yes` (default), `accept-new` (explicit opt-in).
    pub fn with_strict_host_key_checking(mut self, mode: String) -> Result<Self, String> {
        let normalized = normalize_host_key_checking_mode(&mode).ok_or_else(|| {
            format!(
                "invalid ssh.strictHostKeyChecking value '{mode}', expected 'yes' or 'accept-new'"
            )
        })?;
        self.strict_host_key_checking = normalized.to_string();
        Ok(self)
    }

    fn from_ssh_url(spec: &str) -> Result<Self, String> {
        let url = url::Url::parse(spec).map_err(|e| format!("invalid SSH URL: {e}"))?;
        let user = if url.username().is_empty() {
            "git".to_string()
        } else {
            url.username().to_string()
        };
        let host = url.host_str().ok_or("missing host in SSH URL")?.to_string();
        let port = url.port().unwrap_or(DEFAULT_SSH_PORT);
        let mut repo_path = url.path().to_string();
        if repo_path.starts_with('/') {
            repo_path = repo_path[1..].to_string();
        }
        if repo_path.ends_with('/') && repo_path.len() > 1 {
            repo_path.pop();
        }
        Ok(Self {
            user,
            host,
            port,
            repo_path,
            key_path: None,
            strict_host_key_checking: "yes".to_string(),
        })
    }

    /// Parse SCP-style `user@host:path` format.
    fn from_scp_style(spec: &str) -> Result<Self, String> {
        let (user_host, path) = spec
            .split_once(':')
            .ok_or_else(|| format!("invalid SCP-style SSH spec: {spec}"))?;
        let (user, host) = if let Some((u, h)) = user_host.split_once('@') {
            (u.to_string(), h.to_string())
        } else {
            ("git".to_string(), user_host.to_string())
        };
        let repo_path = path.trim_end_matches('/').to_string();
        Ok(Self {
            user,
            host,
            port: DEFAULT_SSH_PORT,
            repo_path,
            key_path: None,
            strict_host_key_checking: "yes".to_string(),
        })
    }

    /// Spawn an SSH subprocess running the given Git service on the remote.
    async fn spawn_service(&self, service: ServiceType) -> Result<tokio::process::Child, IoError> {
        let service_cmd = match service {
            ServiceType::UploadPack => "git-upload-pack",
            ServiceType::ReceivePack => "git-receive-pack",
        };
        // Build: ssh [opts] user@host "git-upload-pack '/repo/path'"
        let ssh_bin = std::env::var("LIBRA_SSH_COMMAND").unwrap_or_else(|_| "ssh".to_string());
        let mut cmd = tokio::process::Command::new(ssh_bin);
        cmd.arg("-o").arg(format!(
            "StrictHostKeyChecking={}",
            self.strict_host_key_checking
        ));
        cmd.arg("-o").arg("BatchMode=yes");
        if let Some(ref key) = self.key_path {
            cmd.arg("-i").arg(key);
        }
        if self.port != DEFAULT_SSH_PORT {
            cmd.arg("-p").arg(self.port.to_string());
        }
        cmd.arg(format!("{}@{}", self.user, self.host));
        cmd.arg(format!(
            "{service_cmd} {}",
            shell_single_quote(&self.repo_path)
        ));
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
    }

    /// Read pkt-line advertisement from the SSH child's stdout.
    async fn read_advertisement(
        stdout: &mut tokio::process::ChildStdout,
    ) -> Result<Bytes, IoError> {
        let mut buf = BytesMut::new();
        loop {
            let mut len_buf = [0u8; 4];
            stdout.read_exact(&mut len_buf).await?;
            let len_str = std::str::from_utf8(&len_buf)
                .map_err(|e| IoError::other(format!("invalid pkt-line length: {e}")))?;
            let len = usize::from_str_radix(len_str, 16)
                .map_err(|e| IoError::other(format!("invalid pkt-line length: {e}")))?;
            buf.extend_from_slice(&len_buf);
            if len == 0 {
                break;
            }
            let mut data = vec![0u8; len - 4];
            stdout.read_exact(&mut data).await?;
            buf.extend_from_slice(&data);
        }
        Ok(buf.freeze())
    }

    pub async fn discovery_reference(
        &self,
        service: ServiceType,
    ) -> Result<DiscoveryResult, GitError> {
        let mut child = self
            .spawn_service(service)
            .await
            .map_err(|e| GitError::NetworkError(format!("SSH spawn failed: {e}")))?;
        let response = {
            let stdout = child.stdout.as_mut().ok_or_else(|| {
                GitError::NetworkError("SSH child stdout not captured".to_string())
            })?;
            Self::read_advertisement(stdout).await
        };
        let response = match response {
            Ok(response) => response,
            Err(read_err) => {
                let output = child.wait_with_output().await.map_err(|wait_err| {
                    GitError::NetworkError(format!(
                        "SSH read failed: {read_err}; unable to collect process output: {wait_err}"
                    ))
                })?;
                return Err(GitError::NetworkError(format!(
                    "SSH read failed: {read_err}; {}",
                    describe_process_output(&output)
                )));
            }
        };
        // Discovery only needs the advertisement packet. Kill and reap the child
        // to avoid leaving an unreaped process around.
        let _ = child.kill().await;
        let output = child
            .wait_with_output()
            .await
            .map_err(|e| GitError::NetworkError(format!("SSH wait failed: {e}")))?;
        // If the process was not killed by signal and exited non-zero, surface diagnostics.
        if !output.status.success() && output.status.code().is_some() {
            return Err(GitError::NetworkError(format!(
                "SSH discovery command failed: {}",
                describe_process_output(&output)
            )));
        }
        parse_discovered_references(response, service)
    }

    pub async fn fetch_objects(
        &self,
        have: &[String],
        want: &[String],
        depth: Option<usize>,
    ) -> Result<FetchStream, IoError> {
        let mut child = self.spawn_service(ServiceType::UploadPack).await?;
        let advertisement = {
            let stdout = child
                .stdout
                .as_mut()
                .ok_or_else(|| IoError::other("SSH child stdout not captured"))?;
            Self::read_advertisement(stdout).await
        };
        if let Err(read_err) = advertisement {
            let output = child.wait_with_output().await.map_err(|wait_err| {
                IoError::other(format!(
                    "SSH advertisement read failed: {read_err}; unable to collect process output: {wait_err}"
                ))
            })?;
            return Err(IoError::other(format!(
                "SSH advertisement read failed: {read_err}; {}",
                describe_process_output(&output)
            )));
        }

        // Send the upload-pack request
        let body = generate_upload_pack_content(have, want, depth);
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| IoError::other("SSH child stdin not captured"))?;
        stdin.write_all(&body).await?;
        stdin.shutdown().await?;

        let output = child.wait_with_output().await?;
        if !output.status.success() {
            return Err(IoError::other(format!(
                "SSH upload-pack failed: {}",
                describe_process_output(&output)
            )));
        }
        Ok(stream::once(async move { Ok(Bytes::from(output.stdout)) }).boxed())
    }

    pub async fn send_pack(&self, data: Bytes) -> Result<Bytes, IoError> {
        let mut child = self.spawn_service(ServiceType::ReceivePack).await?;
        let advertisement = {
            let stdout = child
                .stdout
                .as_mut()
                .ok_or_else(|| IoError::other("SSH child stdout not captured"))?;
            Self::read_advertisement(stdout).await
        };
        if let Err(read_err) = advertisement {
            let output = child.wait_with_output().await.map_err(|wait_err| {
                IoError::other(format!(
                    "SSH advertisement read failed: {read_err}; unable to collect process output: {wait_err}"
                ))
            })?;
            return Err(IoError::other(format!(
                "SSH advertisement read failed: {read_err}; {}",
                describe_process_output(&output)
            )));
        }

        // Send the pack data
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| IoError::other("SSH child stdin not captured"))?;
        stdin.write_all(&data).await?;
        stdin.shutdown().await?;

        let output = child.wait_with_output().await?;
        if !output.status.success() {
            return Err(IoError::other(format!(
                "SSH receive-pack failed: {}",
                describe_process_output(&output)
            )));
        }
        Ok(Bytes::from(output.stdout))
    }
}

fn describe_process_output(output: &std::process::Output) -> String {
    let status = output.status.code().map_or_else(
        || "terminated by signal".to_string(),
        |code| code.to_string(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("exit status {status}")
    } else {
        format!("exit status {status}, stderr: {stderr}")
    }
}

fn normalize_host_key_checking_mode(mode: &str) -> Option<&'static str> {
    if mode.eq_ignore_ascii_case("yes") {
        Some("yes")
    } else if mode.eq_ignore_ascii_case("accept-new") {
        Some("accept-new")
    } else {
        None
    }
}

fn shell_single_quote(value: &str) -> String {
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

/// Check if a remote spec looks like an SSH URL.
pub fn is_ssh_spec(spec: &str) -> bool {
    if spec.starts_with("ssh://") {
        return true;
    }

    // SCP-style: [user@]host:path
    if spec.contains("://")
        || spec.starts_with('/')
        || spec.starts_with("./")
        || spec.starts_with("../")
    {
        return false;
    }

    let Some((user_host, path)) = spec.split_once(':') else {
        return false;
    };
    if user_host.is_empty() || path.is_empty() {
        return false;
    }

    // Avoid mistaking Windows local paths (e.g. C:\repo) for SSH remotes.
    if user_host.len() == 1
        && user_host
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic())
    {
        return false;
    }

    if user_host.contains('/') || user_host.contains('\\') {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ssh_spec() {
        assert!(is_ssh_spec("git@github.com:user/repo.git"));
        assert!(is_ssh_spec("github.com:user/repo.git"));
        assert!(is_ssh_spec("ssh://git@github.com/user/repo.git"));
        assert!(is_ssh_spec("ssh://github.com/user/repo.git"));
        assert!(!is_ssh_spec("https://github.com/user/repo.git"));
        assert!(!is_ssh_spec("git://github.com/user/repo.git"));
        assert!(!is_ssh_spec("/local/path/to/repo"));
        assert!(!is_ssh_spec("C:\\repo\\path"));
        assert!(!is_ssh_spec("foo/bar:baz"));
    }

    #[test]
    fn test_parse_scp_style() {
        let client = SshClient::from_scp_style("git@github.com:user/repo.git").unwrap();
        assert_eq!(client.user, "git");
        assert_eq!(client.host, "github.com");
        assert_eq!(client.repo_path, "user/repo.git");
        assert_eq!(client.port, 22);
    }

    #[test]
    fn test_parse_ssh_url() {
        let client = SshClient::from_ssh_url("ssh://git@github.com:2222/user/repo.git").unwrap();
        assert_eq!(client.user, "git");
        assert_eq!(client.host, "github.com");
        assert_eq!(client.repo_path, "user/repo.git");
        assert_eq!(client.port, 2222);
    }

    #[test]
    fn test_parse_ssh_url_default_user() {
        let client = SshClient::from_ssh_url("ssh://github.com/user/repo.git").unwrap();
        assert_eq!(client.user, "git");
        assert_eq!(client.host, "github.com");
    }

    #[test]
    fn test_shell_single_quote() {
        assert_eq!(shell_single_quote("user/repo.git"), "'user/repo.git'");
        assert_eq!(
            shell_single_quote("user/repo'weird.git"),
            "'user/repo'\"'\"'weird.git'"
        );
    }

    #[test]
    fn test_with_strict_host_key_checking_accept_new() {
        let client = SshClient::from_scp_style("git@github.com:user/repo.git")
            .unwrap()
            .with_strict_host_key_checking("accept-new".to_string())
            .unwrap();
        assert_eq!(client.strict_host_key_checking, "accept-new");
    }

    #[test]
    fn test_with_strict_host_key_checking_invalid_value() {
        let result = SshClient::from_scp_style("git@github.com:user/repo.git")
            .unwrap()
            .with_strict_host_key_checking("no".to_string());
        assert!(result.is_err(), "invalid mode should be rejected");
        let err = result.err().unwrap();
        assert!(err.contains("expected 'yes' or 'accept-new'"));
    }
}
