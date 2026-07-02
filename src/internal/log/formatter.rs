//! Formatting helpers for `libra log` output modes.
//!
//! Boundary: formatting consumes already-selected commits and decorations; revision
//! walking and filtering live elsewhere. Command log tests cover empty history,
//! decorate modes, date formats, and machine-readable output.

use colored::Colorize;
use git_internal::internal::object::commit::Commit;

use crate::common_utils::parse_commit_msg;

/// Named `--pretty=<preset>` formats distinct from the default (`Full`) and
/// `oneline`. `medium` is Git's default and maps to [`FormatType::Full`], so it
/// has no separate variant here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogPreset {
    /// `short`: commit + Author + indented subject (no Date, no body).
    Short,
    /// `full`: commit + Author + Commit + indented full message (no dates).
    Full,
    /// `fuller`: commit + Author/AuthorDate + Commit/CommitDate + full message.
    Fuller,
    /// `reference`: a one-line `<abbrev> (<subject>, <short-date>)`.
    Reference,
    /// `raw`: the commit object's headers (tree/parent/author/committer, raw
    /// timestamps, optional gpgsig) plus the indented message.
    Raw,
}

/// Supported log output formats.
#[derive(Clone)]
pub enum FormatType {
    Full,
    Oneline,
    Custom(String),
    Preset(LogPreset),
}

/// Extra context supplied by the log renderer (graph/decorations).
pub struct FormatContext<'a> {
    pub graph_prefix: &'a str,
    pub decoration: &'a str,
    pub abbrev_len: usize,
    /// Pre-formatted parent or child commit ids (already abbreviated and
    /// space-joined) appended after the commit hash for `--parents`/`--children`
    /// in the full and oneline formats. Empty when neither flag is set.
    pub extra_hashes: &'a str,
}

pub struct CommitFormatter {
    format: FormatType,
    /// `--date=<mode>` rendering mode for author/committer dates ("" = default).
    date_mode: String,
    /// `--only-trailers`: show only the trailer block (selected keys; empty = all).
    only_trailers: Option<Vec<String>>,
}

impl CommitFormatter {
    pub fn new(format: FormatType) -> Self {
        Self {
            format,
            date_mode: String::new(),
            only_trailers: None,
        }
    }

    /// Set the `--date=<mode>` rendering mode applied to author/committer dates.
    pub fn with_date_mode(mut self, date_mode: String) -> Self {
        self.date_mode = date_mode;
        self
    }

    pub fn format(&self, commit: &Commit, ctx: &FormatContext<'_>) -> String {
        match &self.format {
            FormatType::Full => self.format_full(commit, ctx),
            FormatType::Oneline => self.format_oneline(commit, ctx),
            FormatType::Custom(template) => self.format_custom(commit, ctx, template),
            FormatType::Preset(preset) => self.format_preset(commit, ctx, *preset),
        }
    }

    /// The `commit <hash>[ <extra>][ (<decoration>)]` header line shared by the
    /// full/short/full-preset/fuller formats. The hash honours `--abbrev`.
    fn format_header_line(&self, commit: &Commit, ctx: &FormatContext<'_>) -> String {
        let full_hash = commit.id.to_string();
        let display_hash = if ctx.abbrev_len < full_hash.len() {
            full_hash.chars().take(ctx.abbrev_len).collect::<String>()
        } else {
            full_hash
        };
        let mut header = format!(
            "{}{} {}",
            ctx.graph_prefix,
            "commit".yellow(),
            display_hash.yellow()
        );
        if !ctx.extra_hashes.is_empty() {
            header.push(' ');
            header.push_str(ctx.extra_hashes);
        }
        if !ctx.decoration.is_empty() {
            header.push_str(&format!(" ({})", ctx.decoration));
        }
        header
    }

    /// Indent every line of `msg` by four spaces (blank lines become `"    "`),
    /// matching Git's commit-message rendering in the medium/full formats.
    fn indent_message(&self, msg: &str) -> String {
        let mut out = String::new();
        for line in msg.lines() {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    /// `log --only-trailers` (Libra extension, lore.md §1.9): replace the
    /// indented message with the commit's qualifying trailer block — unfolded
    /// `Key: value` lines (plus separator-less recognized lines like
    /// `(cherry picked from commit …)` verbatim), optionally key-filtered.
    /// Empty for commits without a qualifying block.
    pub fn with_only_trailers(mut self, selected_keys: Vec<String>) -> Self {
        self.only_trailers = Some(selected_keys);
        self
    }

    fn only_trailers_body(&self, commit: &Commit, selected_keys: &[String]) -> String {
        let trailers = crate::internal::log::trailer::parse_trailers(&commit.message);
        let mut lines: Vec<String> = Vec::new();
        if selected_keys.is_empty() {
            // All keys: render the raw qualifying block (keeps recognized
            // separator-less lines verbatim).
            if let Some(block) = crate::internal::log::trailer::trailer_block(&commit.message) {
                lines = block;
            }
        } else {
            for trailer in &trailers {
                if selected_keys.iter().any(|key| trailer.key_matches(key)) {
                    lines.push(format!("{}: {}", trailer.key, trailer.value));
                }
            }
        }
        lines.join("\n")
    }

    fn format_full(&self, commit: &Commit, ctx: &FormatContext<'_>) -> String {
        let mut out = self.format_header_line(commit, ctx);
        out.push('\n');

        out.push_str(&format!(
            "Author: {} <{}>\n",
            commit.author.name.trim(),
            commit.author.email.trim()
        ));
        out.push_str(&format!(
            "Date:   {}\n\n",
            format_timestamp_with(commit.committer.timestamp as i64, &self.date_mode)
        ));

        if let Some(selected_keys) = &self.only_trailers {
            let body = self.only_trailers_body(commit, selected_keys);
            out.push_str(&self.indent_message(&body));
            return out;
        }
        let (message, _) = parse_commit_msg(&commit.message);
        out.push_str(&self.indent_message(message));
        out
    }

    /// Render the `short` / `full` / `fuller` / `reference` / `raw` presets.
    fn format_preset(&self, commit: &Commit, ctx: &FormatContext<'_>, preset: LogPreset) -> String {
        match preset {
            LogPreset::Reference => self.format_reference(commit, ctx),
            LogPreset::Raw => self.format_raw(commit, ctx),
            LogPreset::Short | LogPreset::Full | LogPreset::Fuller => {
                let mut out = self.format_header_line(commit, ctx);
                out.push('\n');

                let author = format!(
                    "{} <{}>",
                    commit.author.name.trim(),
                    commit.author.email.trim()
                );
                let committer = format!(
                    "{} <{}>",
                    commit.committer.name.trim(),
                    commit.committer.email.trim()
                );
                match preset {
                    LogPreset::Short => out.push_str(&format!("Author: {author}\n\n")),
                    LogPreset::Full => {
                        out.push_str(&format!("Author: {author}\n"));
                        out.push_str(&format!("Commit: {committer}\n\n"));
                    }
                    LogPreset::Fuller => {
                        out.push_str(&format!("Author:     {author}\n"));
                        out.push_str(&format!(
                            "AuthorDate: {}\n",
                            format_timestamp_with(commit.author.timestamp as i64, &self.date_mode)
                        ));
                        out.push_str(&format!("Commit:     {committer}\n"));
                        out.push_str(&format!(
                            "CommitDate: {}\n\n",
                            format_timestamp_with(
                                commit.committer.timestamp as i64,
                                &self.date_mode
                            )
                        ));
                    }
                    _ => unreachable!("only short/full/fuller reach this arm"),
                }

                let (message, _) = parse_commit_msg(&commit.message);
                if matches!(preset, LogPreset::Short) {
                    // `short` shows only the subject — the first line of the
                    // message, matching Git's title for the common case.
                    let subject = message.lines().next().unwrap_or("");
                    out.push_str(&self.indent_message(subject));
                } else {
                    out.push_str(&self.indent_message(message));
                }
                out
            }
        }
    }

    /// `reference`: `<abbrev> (<subject>, <author-date-short>)`.
    fn format_reference(&self, commit: &Commit, ctx: &FormatContext<'_>) -> String {
        let short_hash: String = commit.id.to_string().chars().take(ctx.abbrev_len).collect();
        let (message, _) = parse_commit_msg(&commit.message);
        let subject = message.lines().next().unwrap_or("");
        let date = format_timestamp_with(commit.author.timestamp as i64, "short");
        format!(
            "{}{} ({}, {})",
            ctx.graph_prefix,
            short_hash.yellow(),
            subject,
            date
        )
    }

    /// `raw`: the commit object's header lines (full hashes, raw timestamps,
    /// optional gpgsig) followed by the indented message. Timestamps render in
    /// UTC (`+0000`), matching the rest of `libra log`.
    fn format_raw(&self, commit: &Commit, ctx: &FormatContext<'_>) -> String {
        let mut header = format!(
            "{}{} {}",
            ctx.graph_prefix,
            "commit".yellow(),
            commit.id.to_string().yellow()
        );
        if !ctx.decoration.is_empty() {
            header.push_str(&format!(" ({})", ctx.decoration));
        }
        let mut out = header;
        out.push('\n');

        out.push_str(&format!("tree {}\n", commit.tree_id));
        for parent in &commit.parent_commit_ids {
            out.push_str(&format!("parent {parent}\n"));
        }
        out.push_str(&format!(
            "author {} <{}> {}\n",
            commit.author.name.trim(),
            commit.author.email.trim(),
            format_timestamp_with(commit.author.timestamp as i64, "raw")
        ));
        out.push_str(&format!(
            "committer {} <{}> {}\n",
            commit.committer.name.trim(),
            commit.committer.email.trim(),
            format_timestamp_with(commit.committer.timestamp as i64, "raw")
        ));

        let (message, signature) = parse_commit_msg(&commit.message);
        if let Some(signature) = signature {
            // Emit the stored gpgsig header block VERBATIM rather than
            // reconstructing it: the original bytes already carry the correct
            // header name (`gpgsig` or `gpgsig-sha256`) and single-space
            // continuation lines, so re-prefixing would double the spacing and
            // re-labelling could mislabel SHA-256 signatures. `signature` is the
            // captured body (`-----BEGIN…-----END…`), a subslice of
            // `commit.message`; the block runs from the start of the message
            // through the end of that body.
            if let Some(pos) = commit.message.find(signature) {
                out.push_str(&commit.message[..pos + signature.len()]);
                out.push('\n');
            }
        }

        out.push('\n');
        out.push_str(&self.indent_message(message));
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

        // Parent/child ids (when `--parents`/`--children`) sit right after the
        // hash, before any ref decoration, matching Git.
        let hash_part = if ctx.extra_hashes.is_empty() {
            short_hash.yellow().to_string()
        } else {
            format!("{} {}", short_hash.yellow(), ctx.extra_hashes)
        };

        if ctx.decoration.is_empty() {
            format!("{}{} {}", ctx.graph_prefix, hash_part, first_line)
        } else {
            format!(
                "{}{} ({}) {}",
                ctx.graph_prefix, hash_part, ctx.decoration, first_line
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
        result = result.replace(
            "%ad",
            &format_timestamp_with(commit.author.timestamp as i64, &self.date_mode),
        );
        result = result.replace("%cn", commit.committer.name.trim());
        result = result.replace("%ce", commit.committer.email.trim());
        result = result.replace(
            "%cd",
            &format_timestamp_with(commit.committer.timestamp as i64, &self.date_mode),
        );
        result = result.replace("%d", &decoration);

        format!("{}{}", ctx.graph_prefix, result)
    }
}

pub fn format_timestamp(timestamp: i64) -> String {
    format_timestamp_with(timestamp, "")
}

/// Render a commit timestamp according to a `--date=<mode>` value. Supported
/// modes: `short`, `iso`/`iso8601`, `iso-strict`/`iso8601-strict`, `rfc`/`rfc2822`,
/// `unix`, `raw`; any other value (including "" and `default`) uses Git's default
/// `Day Mon DD HH:MM:SS YYYY +ZZZZ` form. Timestamps are rendered in UTC, so the
/// zone is always `+0000` (Libra stores a per-signature tz that this i64-only
/// entry point does not receive).
pub fn format_timestamp_with(timestamp: i64, mode: &str) -> String {
    use chrono::{DateTime, Utc};
    let dt = DateTime::<Utc>::from_timestamp(timestamp, 0).unwrap_or(chrono::DateTime::UNIX_EPOCH);
    match mode {
        "short" => dt.format("%Y-%m-%d").to_string(),
        "iso" | "iso8601" => dt.format("%Y-%m-%d %H:%M:%S %z").to_string(),
        "iso-strict" | "iso8601-strict" => dt.to_rfc3339(),
        "rfc" | "rfc2822" => dt.to_rfc2822(),
        "unix" => timestamp.to_string(),
        "raw" => format!("{timestamp} +0000"),
        _ => dt.format("%a %b %d %H:%M:%S %Y %z").to_string(),
    }
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
            extra_hashes: "",
        };
        let out = formatter.format(&commit, &ctx);
        assert!(out.contains(" - Test subject"));
        assert!(out.split_whitespace().next().unwrap().len() <= 8);
    }

    #[test]
    fn format_raw_emits_gpgsig_header_verbatim() {
        let ctx = FormatContext {
            graph_prefix: "",
            decoration: "",
            abbrev_len: 7,
            extra_hashes: "",
        };
        let raw = CommitFormatter::new(FormatType::Preset(LogPreset::Raw));

        // A PGP-signed commit: the stored gpgsig block has single-space
        // continuation lines. `raw` must emit it verbatim (no doubled spaces).
        let signed = build_commit(
            "gpgsig -----BEGIN PGP SIGNATURE-----\n \n iQsig0\n iQsig1\n -----END PGP SIGNATURE-----\n\nSubject line",
        );
        let out = raw.format(&signed, &ctx);
        assert!(
            out.contains(
                "gpgsig -----BEGIN PGP SIGNATURE-----\n \n iQsig0\n iQsig1\n -----END PGP SIGNATURE-----\n"
            ),
            "gpgsig block must be verbatim (single-space continuation): {out}"
        );
        assert!(
            !out.contains("  iQsig0"),
            "continuation lines must not be double-spaced: {out}"
        );
        assert!(out.contains("    Subject line"), "indented message: {out}");
        assert!(out.contains("\ntree "), "raw tree line: {out}");

        // A SHA-256 signature keeps its `gpgsig-sha256` header (not relabelled).
        let signed_256 = build_commit(
            "gpgsig-sha256 -----BEGIN PGP SIGNATURE-----\n iQ256\n -----END PGP SIGNATURE-----\n\nSubject",
        );
        let out_256 = raw.format(&signed_256, &ctx);
        assert!(
            out_256.contains("gpgsig-sha256 -----BEGIN PGP SIGNATURE-----"),
            "SHA-256 signature header must not be relabelled as gpgsig: {out_256}"
        );
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
            extra_hashes: "",
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

    #[test]
    fn format_timestamp_with_modes() {
        // 2020-09-13 12:26:40 UTC.
        let ts = 1_600_000_000;
        assert_eq!(format_timestamp_with(ts, "short"), "2020-09-13");
        assert_eq!(format_timestamp_with(ts, "unix"), "1600000000");
        assert_eq!(format_timestamp_with(ts, "raw"), "1600000000 +0000");
        assert_eq!(
            format_timestamp_with(ts, "iso"),
            "2020-09-13 12:26:40 +0000"
        );
        assert!(format_timestamp_with(ts, "iso-strict").starts_with("2020-09-13T12:26:40"));
        // Unknown / default fall back to the canonical form (same as the wrapper).
        assert_eq!(format_timestamp_with(ts, "bogus"), format_timestamp(ts));
        assert_eq!(format_timestamp_with(ts, ""), format_timestamp(ts));
    }
}
