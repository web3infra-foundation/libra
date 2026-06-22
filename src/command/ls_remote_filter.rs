use regex::Regex;

use super::{LsRemoteArgs, LsRemoteEntry, LsRemoteError};
use crate::{internal::protocol::DiscRef, utils::util::version_refname_cmp};

pub(super) struct CompiledPattern {
    raw: String,
    regex: Option<Regex>,
}

impl CompiledPattern {
    pub(super) fn new(pattern: &str) -> Result<Self, LsRemoteError> {
        let has_glob = pattern.chars().any(|c| matches!(c, '*' | '?' | '['));
        let regex = if has_glob {
            Some(glob_to_regex(pattern)?)
        } else {
            None
        };
        Ok(Self {
            raw: pattern.to_string(),
            regex,
        })
    }

    pub(super) fn matches(&self, refname: &str) -> bool {
        if let Some(regex) = &self.regex {
            return regex.is_match(refname);
        }

        refname == self.raw || refname.ends_with(&format!("/{}", self.raw))
    }
}

pub(super) fn compile_patterns(patterns: &[String]) -> Result<Vec<CompiledPattern>, LsRemoteError> {
    patterns
        .iter()
        .map(|pattern| CompiledPattern::new(pattern))
        .collect()
}

pub(super) fn include_reference(
    reference: &DiscRef,
    args: &LsRemoteArgs,
    patterns: &[CompiledPattern],
) -> bool {
    let refname = reference._ref.as_str();
    if args.refs && (refname == "HEAD" || refname.ends_with("^{}")) {
        return false;
    }
    if args.heads || args.tags {
        let matches_heads = args.heads && refname.starts_with("refs/heads/");
        let matches_tags = args.tags && refname.starts_with("refs/tags/");
        if !matches_heads && !matches_tags {
            return false;
        }
    }
    patterns.is_empty() || patterns.iter().any(|pattern| pattern.matches(refname))
}

pub(super) fn sort_entries(
    entries: &mut [LsRemoteEntry],
    sort: Option<&str>,
) -> Result<(), LsRemoteError> {
    match sort {
        None => Ok(()),
        Some("refname") => {
            entries.sort_by(|left, right| left.refname.cmp(&right.refname));
            Ok(())
        }
        Some("-refname") => {
            entries.sort_by(|left, right| right.refname.cmp(&left.refname));
            Ok(())
        }
        Some("version:refname" | "v:refname") => {
            entries.sort_by(|left, right| version_refname_cmp(&left.refname, &right.refname));
            Ok(())
        }
        Some("-version:refname" | "-v:refname") => {
            entries.sort_by(|left, right| version_refname_cmp(&right.refname, &left.refname));
            Ok(())
        }
        Some(key) => Err(LsRemoteError::UnsupportedSortKey(key.to_string())),
    }
}

fn glob_to_regex(pattern: &str) -> Result<Regex, LsRemoteError> {
    let mut regex = String::from("(^|.*/)");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            '[' => regex.push('['),
            ']' => regex.push(']'),
            '.' | '+' | '(' | ')' | '{' | '}' | '|' | '^' | '$' | '\\' => {
                regex.push('\\');
                regex.push(ch);
            }
            other => regex.push(other),
        }
    }
    regex.push('$');
    Regex::new(&regex).map_err(|error| LsRemoteError::InvalidPattern {
        pattern: pattern.to_string(),
        reason: error.to_string(),
    })
}

// `version_refname_cmp` now lives in `crate::utils::util` and is shared with
// `for-each-ref`'s `--sort=version:refname`.
