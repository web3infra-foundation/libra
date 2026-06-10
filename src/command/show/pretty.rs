use colored::Colorize;
use git_internal::internal::object::commit::Commit;

use crate::{
    command::show::ShowArgs,
    common_utils::parse_commit_msg,
    utils::error::{CliError, CliResult, StableErrorCode},
};

pub(super) fn render_commit_header(commit: &Commit, args: &ShowArgs) -> CliResult<String> {
    if let Some(template) = args.format.as_deref() {
        return render_custom_format(commit, template);
    }

    if let Some(pretty) = args.pretty.as_deref() {
        return render_pretty(commit, pretty);
    }

    if args.oneline {
        return render_pretty(commit, "oneline");
    }

    render_pretty(commit, "medium")
}

fn render_pretty(commit: &Commit, pretty: &str) -> CliResult<String> {
    if let Some(template) = pretty.strip_prefix("format:") {
        return render_custom_format(commit, template);
    }

    match pretty {
        "oneline" => Ok(format!("{} {}\n", short_hash(commit), subject(commit))),
        "short" => Ok(format!(
            "{} {}\nAuthor: {} <{}>\n\n    {}\n",
            "commit".yellow(),
            commit.id.to_string().yellow(),
            commit.author.name.trim(),
            commit.author.email.trim(),
            subject(commit)
        )),
        "medium" => Ok(format!(
            "{} {}\nAuthor: {} <{}>\nDate:   {}\n\n{}\n",
            "commit".yellow(),
            commit.id.to_string().yellow(),
            commit.author.name.trim(),
            commit.author.email.trim(),
            display_date(commit.committer.timestamp),
            indented_message(commit)
        )),
        "full" => Ok(format!(
            "{} {}\nAuthor: {} <{}>\nCommit: {} <{}>\n\n{}\n",
            "commit".yellow(),
            commit.id.to_string().yellow(),
            commit.author.name.trim(),
            commit.author.email.trim(),
            commit.committer.name.trim(),
            commit.committer.email.trim(),
            indented_message(commit)
        )),
        "fuller" => Ok(format!(
            "{} {}\nAuthor:     {} <{}>\nAuthorDate: {}\nCommit:     {} <{}>\nCommitDate: {}\n\n{}\n",
            "commit".yellow(),
            commit.id.to_string().yellow(),
            commit.author.name.trim(),
            commit.author.email.trim(),
            display_date(commit.author.timestamp),
            commit.committer.name.trim(),
            commit.committer.email.trim(),
            display_date(commit.committer.timestamp),
            indented_message(commit)
        )),
        other => Err(invalid_pretty(other)),
    }
}

fn render_custom_format(commit: &Commit, template: &str) -> CliResult<String> {
    let template = template.strip_prefix("format:").unwrap_or(template);

    let mut out = String::with_capacity(template.len() + 1);
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
            'h' => out.push_str(&short_hash(commit)),
            's' => out.push_str(&subject(commit)),
            'n' => out.push('\n'),
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
    out.push('\n');
    Ok(out)
}

fn subject(commit: &Commit) -> String {
    parse_commit_msg(&commit.message)
        .0
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

fn indented_message(commit: &Commit) -> String {
    let (message, _) = parse_commit_msg(&commit.message);
    let mut out = String::new();
    for line in message.lines() {
        out.push_str("    ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn short_hash(commit: &Commit) -> String {
    commit.id.to_string().chars().take(7).collect()
}

fn display_date(timestamp: usize) -> String {
    chrono::DateTime::from_timestamp(timestamp as i64, 0)
        .unwrap_or(chrono::DateTime::UNIX_EPOCH)
        .to_rfc2822()
}

fn invalid_pretty(value: &str) -> CliError {
    CliError::fatal(format!("unsupported show pretty format '{value}'"))
        .with_stable_code(StableErrorCode::CliInvalidArguments)
        .with_hint(
            "supported pretty formats: oneline, short, medium, full, fuller, format:<template>",
        )
}

fn unsupported_placeholder(placeholder: &str) -> CliError {
    CliError::fatal(format!(
        "unsupported show --format placeholder '{placeholder}'"
    ))
    .with_stable_code(StableErrorCode::CliInvalidArguments)
    .with_hint("supported placeholders: %s, %h, %H, %an, %ae, %cn, %ce, %%")
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use git_internal::{hash::ObjectHash, internal::object::commit::Commit};

    use super::*;

    fn test_commit() -> Commit {
        let mut commit = Commit::from_tree_id(ObjectHash::new(&[1; 20]), vec![], "\nsubject\n");
        commit.author.name = "Alice".into();
        commit.author.email = "alice@example.com".into();
        commit.author.timestamp = 1_600_000_000;
        commit.committer.name = "Bob".into();
        commit.committer.email = "bob@example.com".into();
        commit.committer.timestamp = 1_600_000_001;
        commit
    }

    #[test]
    fn render_format_uses_supported_placeholders() {
        let args = ShowArgs::parse_from(["show", "--format", "%h %an %s"]);
        let rendered = render_commit_header(&test_commit(), &args).unwrap();
        assert!(rendered.contains("Alice subject"));
    }

    #[test]
    fn render_pretty_rejects_unknown_placeholder() {
        let args = ShowArgs::parse_from(["show", "--format", "%b"]);
        let error = render_commit_header(&test_commit(), &args).unwrap_err();
        assert_eq!(error.stable_code(), StableErrorCode::CliInvalidArguments);
    }
}
