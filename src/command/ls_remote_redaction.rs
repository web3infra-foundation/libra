use git_internal::errors::GitError;
use regex::Regex;

use crate::command::fetch::redact_url_credentials;

pub(super) fn visible_remote_url(remote_url: &str) -> String {
    redact_remote_spec_for_diagnostics(remote_url)
}

pub(super) fn visible_remote_display(remote_display: &str, remote_name: Option<&str>) -> String {
    if remote_name.is_some() {
        remote_display.to_string()
    } else {
        redact_remote_spec_for_diagnostics(remote_display)
    }
}

pub(super) fn sanitize_remote_error_reason(reason: &str, remote_url: &str) -> String {
    let redacted_remote = redact_remote_spec_for_diagnostics(remote_url);
    let reason = reason.replace(remote_url, &redacted_remote);
    redact_embedded_remote_credentials(&reason)
}

pub(super) fn sanitize_discovery_error(source: GitError, remote_url: &str) -> GitError {
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
