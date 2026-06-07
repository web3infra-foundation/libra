//! Formatting helpers for `libra log` output modes.
//!
//! Boundary: formatting consumes already-selected commits and decorations; revision
//! walking and filtering live elsewhere. Command log tests cover empty history,
//! decorate modes, date formats, and machine-readable output.

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
        // `format:<str>` and `tformat:<str>` select the template; `tformat`
        // additionally appends a trailing newline to each commit's output. A bare
        // template (no prefix) behaves like `format:`.
        let (template, trailing_newline) = if let Some(rest) = template.strip_prefix("tformat:") {
            (rest, true)
        } else if let Some(rest) = template.strip_prefix("format:") {
            (rest, false)
        } else {
            (template, false)
        };

        let body = self.expand_placeholders(commit, ctx, template);
        let mut out = format!("{}{}", ctx.graph_prefix, body);
        if trailing_newline {
            out.push('\n');
        }
        out
    }

    /// Expands the git `--pretty=format:` placeholders in `template` against
    /// `commit`. Unknown `%`-escapes are preserved literally. No expansion path
    /// panics: root commits (`%P` empty), empty bodies, and malformed `%x` byte
    /// escapes all degrade safely.
    fn expand_placeholders(
        &self,
        commit: &Commit,
        ctx: &FormatContext<'_>,
        template: &str,
    ) -> String {
        let commit_id = commit.id.to_string();
        let tree_id = commit.tree_id.to_string();
        let (raw_body, _) = parse_commit_msg(&commit.message);
        let subject_line = raw_body.lines().next().unwrap_or("");
        // %b: the message body — everything after the subject line, with the
        // blank separator line stripped.
        let body = raw_body
            .split_once('\n')
            .map(|(_, rest)| rest.trim_start_matches('\n'))
            .unwrap_or("");
        let abbrev = |hash: &str| hash.chars().take(ctx.abbrev_len).collect::<String>();
        let parents: Vec<String> = commit
            .parent_commit_ids
            .iter()
            .map(|p| p.to_string())
            .collect();
        let decoration = if ctx.decoration.is_empty() {
            String::new()
        } else {
            format!(" ({})", ctx.decoration)
        };

        let mut out = String::with_capacity(template.len());
        let mut chars = template.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.next() {
                None => out.push('%'),
                Some('H') => out.push_str(&commit_id),
                Some('h') => out.push_str(&abbrev(&commit_id)),
                Some('T') => out.push_str(&tree_id),
                Some('t') => out.push_str(&abbrev(&tree_id)),
                Some('P') => out.push_str(&parents.join(" ")),
                Some('p') => {
                    let joined = parents
                        .iter()
                        .map(|p| abbrev(p))
                        .collect::<Vec<_>>()
                        .join(" ");
                    out.push_str(&joined);
                }
                Some('s') => out.push_str(subject_line),
                Some('f') => out.push_str(&subject_line.replace(' ', "-")),
                Some('b') => out.push_str(body),
                Some('B') => out.push_str(raw_body),
                Some('d') => out.push_str(&decoration),
                Some('n') => out.push('\n'),
                // %an / %ae / %ad
                Some('a') => match chars.next() {
                    Some('n') => out.push_str(commit.author.name.trim()),
                    Some('e') => out.push_str(commit.author.email.trim()),
                    Some('d') => out.push_str(&format_timestamp(commit.author.timestamp as i64)),
                    Some(other) => {
                        out.push('%');
                        out.push('a');
                        out.push(other);
                    }
                    None => out.push_str("%a"),
                },
                // %cn / %ce / %cd
                Some('c') => match chars.next() {
                    Some('n') => out.push_str(commit.committer.name.trim()),
                    Some('e') => out.push_str(commit.committer.email.trim()),
                    Some('d') => out.push_str(&format_timestamp(commit.committer.timestamp as i64)),
                    Some(other) => {
                        out.push('%');
                        out.push('c');
                        out.push(other);
                    }
                    None => out.push_str("%c"),
                },
                // %x<HH>: literal byte from two hex digits.
                Some('x') => {
                    let h1 = chars.peek().copied().filter(|c| c.is_ascii_hexdigit());
                    if let Some(h1) = h1 {
                        chars.next();
                        let h2 = chars.peek().copied().filter(|c| c.is_ascii_hexdigit());
                        if let Some(h2) = h2 {
                            chars.next();
                            let byte = (h1.to_digit(16).unwrap_or(0) * 16
                                + h2.to_digit(16).unwrap_or(0))
                                as u8;
                            out.push(byte as char);
                        } else {
                            out.push('%');
                            out.push('x');
                            out.push(h1);
                        }
                    } else {
                        out.push('%');
                        out.push('x');
                    }
                }
                Some(other) => {
                    out.push('%');
                    out.push(other);
                }
            }
        }
        out
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
        commit.author.timestamp = 1_600_000_000;
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

    #[test]
    fn format_custom_all_placeholders() {
        let mut commit = build_commit("Fancy subject line");
        commit.author.name = "Author Name".into();
        commit.author.email = "author@test.com".into();
        commit.author.timestamp = 1_600_000_000;
        commit.committer.name = "Committer Name".into();
        commit.committer.email = "committer@test.com".into();
        commit.committer.timestamp = 1_700_000_000;

        let formatter = CommitFormatter::new(FormatType::Custom(
            "%H %h %s %f %an %ae %ad %cn %ce %cd %d".into(),
        ));
        let ctx = FormatContext {
            graph_prefix: "* ",
            decoration: "tag: v1.0",
            abbrev_len: 8,
        };

        let out = formatter.format(&commit, &ctx);
        let full_hash = commit.id.to_string();
        let short_hash = full_hash.chars().take(ctx.abbrev_len).collect::<String>();
        let author_date = format_timestamp(commit.author.timestamp as i64);
        let committer_date = format_timestamp(commit.committer.timestamp as i64);

        assert!(out.starts_with("* "));
        assert!(out.contains(&full_hash));
        assert!(out.contains(&short_hash));
        assert!(out.contains("Fancy subject line"));
        assert!(out.contains("Fancy-subject-line"));
        assert!(out.contains(commit.author.name.trim()));
        assert!(out.contains(commit.author.email.trim()));
        assert!(out.contains(&author_date));
        assert!(out.contains(commit.committer.name.trim()));
        assert!(out.contains(commit.committer.email.trim()));
        assert!(out.contains(&committer_date));
        assert!(out.contains(" (tag: v1.0)"));
        assert_ne!(author_date, committer_date);
    }

    fn ctx7() -> FormatContext<'static> {
        FormatContext {
            graph_prefix: "",
            decoration: "",
            abbrev_len: 7,
        }
    }

    fn render(template: &str, commit: &Commit) -> String {
        CommitFormatter::new(FormatType::Custom(template.into())).format(commit, &ctx7())
    }

    #[test]
    fn test_pretty_format_full_hash() {
        let commit = build_commit("subject");
        assert_eq!(render("%H", &commit), commit.id.to_string());
    }

    #[test]
    fn test_pretty_format_abbrev_hash() {
        let commit = build_commit("subject");
        let out = render("%h", &commit);
        assert_eq!(out.len(), 7);
        assert!(commit.id.to_string().starts_with(&out));
    }

    #[test]
    fn test_pretty_format_tree() {
        let commit = build_commit("subject");
        assert_eq!(render("%T", &commit), commit.tree_id.to_string());
        assert_eq!(
            render("%t", &commit),
            commit
                .tree_id
                .to_string()
                .chars()
                .take(7)
                .collect::<String>()
        );
    }

    #[test]
    fn test_pretty_format_parents_root_empty() {
        // Root commit: %P / %p expand to the empty string (no panic).
        let root = build_commit("root");
        assert_eq!(render("%P", &root), "");
        assert_eq!(render("%p", &root), "");

        // Merge commit with two parents: space-separated.
        let p1 = ObjectHash::new(&[2; 20]);
        let p2 = ObjectHash::new(&[3; 20]);
        let merge = Commit::from_tree_id(ObjectHash::new(&[1; 20]), vec![p1, p2], "merge");
        assert_eq!(render("%P", &merge), format!("{p1} {p2}"));
        let abbrev = render("%p", &merge);
        assert_eq!(
            abbrev,
            format!(
                "{} {}",
                p1.to_string().chars().take(7).collect::<String>(),
                p2.to_string().chars().take(7).collect::<String>()
            )
        );
    }

    #[test]
    fn test_pretty_format_body_subject() {
        let commit = build_commit("Subject line\n\nBody paragraph one.\nBody line two.");
        assert_eq!(render("%s", &commit), "Subject line");
        assert_eq!(render("%b", &commit), "Body paragraph one.\nBody line two.");
        assert_eq!(
            render("%B", &commit),
            "Subject line\n\nBody paragraph one.\nBody line two."
        );
    }

    #[test]
    fn test_pretty_format_newline_and_hexbyte() {
        let commit = build_commit("s");
        assert_eq!(render("a%nb", &commit), "a\nb");
        assert_eq!(render("a%x20b", &commit), "a b");
        assert_eq!(render("%x00", &commit), "\0");
        // Malformed %x (not two hex digits) is preserved literally.
        assert_eq!(render("%xZZ", &commit), "%xZZ");
    }

    #[test]
    fn test_pretty_tformat_trailing_newline() {
        let commit = build_commit("hello");
        assert_eq!(render("tformat:%s", &commit), "hello\n");
        assert_eq!(render("format:%s", &commit), "hello");
        assert_eq!(render("%s", &commit), "hello");
    }

    #[test]
    fn test_pretty_unknown_placeholder_literal() {
        let commit = build_commit("s");
        // Unknown placeholders are kept verbatim (no panic).
        assert_eq!(render("%Q", &commit), "%Q");
        assert_eq!(render("%aZ", &commit), "%aZ");
    }
}
