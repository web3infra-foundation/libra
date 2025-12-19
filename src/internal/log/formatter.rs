use colored::Colorize;
use git_internal::internal::object::commit::Commit;

use crate::common_utils::parse_commit_msg;

/// Supported log output formats.
#[derive(Clone)]
pub enum FormatType {
    Full,
    Oneline,
    Custom(String),
}

/// Extra context supplied by the log renderer (graph/decorations).
pub struct FormatContext<'a> {
    pub graph_prefix: &'a str,
    pub decoration: &'a str,
    pub abbrev_len: usize,
}

pub struct CommitFormatter {
    format: FormatType,
}

impl CommitFormatter {
    pub fn new(format: FormatType) -> Self {
        Self { format }
    }

    pub fn format(&self, commit: &Commit, ctx: &FormatContext<'_>) -> String {
        match &self.format {
            FormatType::Full => self.format_full(commit, ctx),
            FormatType::Oneline => self.format_oneline(commit, ctx),
            FormatType::Custom(template) => self.format_custom(commit, ctx, template),
        }
    }

    fn format_full(&self, commit: &Commit, ctx: &FormatContext<'_>) -> String {
        let full_hash = commit.id.to_string();
        let display_hash = if ctx.abbrev_len < full_hash.len() {
            full_hash.chars().take(ctx.abbrev_len).collect::<String>()
        } else {
            full_hash.clone()
        };
        let mut out = String::new();
        let mut header = format!(
            "{}{} {}",
            ctx.graph_prefix,
            "commit".yellow(),
            display_hash.yellow()
        );
        if !ctx.decoration.is_empty() {
            header.push_str(&format!(" ({})", ctx.decoration));
        }
        out.push_str(&header);
        out.push('\n');

        out.push_str(&format!(
            "Author: {} <{}>\n",
            commit.author.name.trim(),
            commit.author.email.trim()
        ));
        out.push_str(&format!(
            "Date:   {}\n\n",
            format_timestamp(commit.committer.timestamp as i64)
        ));

        let (subject, _) = parse_commit_msg(&commit.message);
        for line in subject.lines() {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }

        out
    }

    fn format_oneline(&self, commit: &Commit, ctx: &FormatContext<'_>) -> String {
        let short_hash = commit
            .id
            .to_string()
            .chars()
            .take(ctx.abbrev_len)
            .collect::<String>();
        let (subject, _) = parse_commit_msg(&commit.message);
        let first_line = subject.lines().next().unwrap_or("");

        if ctx.decoration.is_empty() {
            format!("{}{} {}", ctx.graph_prefix, short_hash.yellow(), first_line)
        } else {
            format!(
                "{}{} ({}) {}",
                ctx.graph_prefix,
                short_hash.yellow(),
                ctx.decoration,
                first_line
            )
        }
    }

    fn format_custom(&self, commit: &Commit, ctx: &FormatContext<'_>, template: &str) -> String {
        let mut result = template.to_string();
        let commit_id = commit.id.to_string();
        let short_hash = commit_id.chars().take(ctx.abbrev_len).collect::<String>();
        let (subject, _) = parse_commit_msg(&commit.message);
        let subject_line = subject.lines().next().unwrap_or("");
        let decoration = if ctx.decoration.is_empty() {
            String::new()
        } else {
            format!(" ({})", ctx.decoration)
        };

        result = result.replace("%H", &commit_id);
        result = result.replace("%h", &short_hash);
        result = result.replace("%s", subject_line);
        result = result.replace("%f", &subject_line.replace(' ', "-"));
        result = result.replace("%an", commit.author.name.trim());
        result = result.replace("%ae", commit.author.email.trim());
        result = result.replace("%ad", &format_timestamp(commit.committer.timestamp as i64));
        result = result.replace("%cn", commit.committer.name.trim());
        result = result.replace("%ce", commit.committer.email.trim());
        result = result.replace("%cd", &format_timestamp(commit.committer.timestamp as i64));
        result = result.replace("%d", &decoration);

        format!("{}{}", ctx.graph_prefix, result)
    }
}

pub fn format_timestamp(timestamp: i64) -> String {
    use chrono::{DateTime, Utc};
    let dt = DateTime::<Utc>::from_timestamp(timestamp, 0).unwrap_or(chrono::DateTime::UNIX_EPOCH);
    dt.format("%a %b %d %H:%M:%S %Y %z").to_string()
}

#[cfg(test)]
mod tests {
    use git_internal::hash::ObjectHash;

    use super::*;

    fn build_commit(message: &str) -> Commit {
        let mut commit = Commit::from_tree_id(ObjectHash::new(&[1; 20]), vec![], message);
        commit.author.name = "Alice".into();
        commit.author.email = "alice@test.com".into();
        commit.committer.name = "Alice".into();
        commit.committer.email = "alice@test.com".into();
        commit.committer.timestamp = 1_700_000_000;
        commit
    }

    #[test]
    fn format_custom_short_hash() {
        let commit = build_commit("Test subject");
        let formatter = CommitFormatter::new(FormatType::Custom("%h - %s".into()));
        let ctx = FormatContext {
            graph_prefix: "",
            decoration: "",
            abbrev_len: 7,
        };
        let out = formatter.format(&commit, &ctx);
        assert!(out.contains(" - Test subject"));
        assert!(out.split_whitespace().next().unwrap().len() <= 8);
    }
}
