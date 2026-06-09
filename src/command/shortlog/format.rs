use git_internal::internal::object::commit::Commit;

use crate::utils::error::{CliError, CliResult, StableErrorCode};

pub(super) fn format_subject(commit: &Commit, template: Option<&str>) -> CliResult<String> {
    let Some(template) = template else {
        return Ok(commit.format_message());
    };

    let template = template
        .strip_prefix("format:")
        .or_else(|| template.strip_prefix("tformat:"))
        .unwrap_or(template);
    expand_template(commit, template)
}

fn expand_template(commit: &Commit, template: &str) -> CliResult<String> {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }

        let Some(code) = chars.next() else {
            return Err(unsupported_placeholder("%"));
        };

        match code {
            '%' => out.push('%'),
            'H' => out.push_str(&commit.id.to_string()),
            'h' => out.push_str(&abbreviate(&commit.id.to_string())),
            's' => out.push_str(&commit.format_message()),
            'a' => match chars.next() {
                Some('n') => out.push_str(commit.author.name.trim()),
                Some('e') => out.push_str(commit.author.email.trim()),
                Some(other) => return Err(unsupported_placeholder(&format!("%a{other}"))),
                None => return Err(unsupported_placeholder("%a")),
            },
            'c' => match chars.next() {
                Some('n') => out.push_str(commit.committer.name.trim()),
                Some('e') => out.push_str(commit.committer.email.trim()),
                Some(other) => return Err(unsupported_placeholder(&format!("%c{other}"))),
                None => return Err(unsupported_placeholder("%c")),
            },
            other => return Err(unsupported_placeholder(&format!("%{other}"))),
        }
    }

    Ok(out)
}

fn abbreviate(hash: &str) -> String {
    hash.chars().take(7).collect()
}

fn unsupported_placeholder(placeholder: &str) -> CliError {
    CliError::fatal(format!(
        "unsupported shortlog --format placeholder '{placeholder}'"
    ))
    .with_stable_code(StableErrorCode::CliInvalidArguments)
    .with_hint("supported placeholders: %s, %h, %H, %an, %ae, %cn, %ce, %%")
}

#[cfg(test)]
mod tests {
    use git_internal::{hash::ObjectHash, internal::object::commit::Commit};

    use super::*;

    fn test_commit() -> Commit {
        let mut commit = Commit::from_tree_id(ObjectHash::new(&[1; 20]), vec![], "\nsubject\n");
        commit.author.name = "Alice".into();
        commit.author.email = "alice@example.com".into();
        commit.committer.name = "Bob".into();
        commit.committer.email = "bob@example.com".into();
        commit
    }

    #[test]
    fn format_subject_defaults_to_commit_subject() {
        let commit = test_commit();
        assert_eq!(format_subject(&commit, None).unwrap(), "subject");
    }

    #[test]
    fn format_subject_expands_supported_placeholders() {
        let commit = test_commit();
        let rendered = format_subject(&commit, Some("%h %an <%ae> via %cn <%ce>: %s")).unwrap();
        assert!(rendered.contains("Alice <alice@example.com>"));
        assert!(rendered.contains("via Bob <bob@example.com>: subject"));
    }

    #[test]
    fn format_subject_rejects_unsupported_placeholders() {
        let commit = test_commit();
        let error = format_subject(&commit, Some("%b")).unwrap_err();
        assert_eq!(error.stable_code(), StableErrorCode::CliInvalidArguments);
    }
}
