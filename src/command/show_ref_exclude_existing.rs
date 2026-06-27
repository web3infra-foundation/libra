use std::{
    collections::HashSet,
    io::{self, BufRead, Write},
};

use serde::Serialize;

use crate::{
    command::show_ref::collect_raw_show_ref_entries,
    utils::{
        error::{CliError, CliResult},
        output::{OutputConfig, emit_json_data},
        util::is_valid_refname,
    },
};

#[derive(Debug, Clone, Serialize)]
struct ExcludeExistingEntry {
    line: String,
    refname: String,
}

pub(crate) async fn execute(pattern: Option<&str>, output: &OutputConfig) -> CliResult<()> {
    let existing_refs = collect_raw_show_ref_entries(true, true, true, false)
        .await?
        .into_iter()
        .map(|entry| normalize_refname(&entry.refname))
        .collect::<HashSet<_>>();

    let entries = filter_stdin_refs(pattern, &existing_refs)?;
    if output.is_json() {
        return emit_json_data(
            "show-ref",
            &serde_json::json!({
                "exclude_existing": true,
                "pattern": pattern,
                "entries": entries,
            }),
            output,
        );
    }
    if output.quiet {
        return Ok(());
    }
    write_entries(&entries)
}

fn filter_stdin_refs(
    pattern: Option<&str>,
    existing_refs: &HashSet<String>,
) -> CliResult<Vec<ExcludeExistingEntry>> {
    let stdin = io::stdin();
    let mut entries = Vec::new();

    for line in stdin.lock().lines() {
        let line = line.map_err(|error| CliError::io(format!("failed to read stdin: {error}")))?;
        let Some(refname) = parse_candidate_refname(&line) else {
            continue;
        };

        if !is_valid_refname(refname) {
            eprintln!("warning: ref '{refname}' ignored");
            continue;
        }
        if pattern.is_some_and(|prefix| !refname.starts_with(prefix)) {
            continue;
        }
        if existing_refs.contains(refname) {
            continue;
        }

        let refname = refname.to_string();
        entries.push(ExcludeExistingEntry { line, refname });
    }

    Ok(entries)
}

fn parse_candidate_refname(line: &str) -> Option<&str> {
    line.split_whitespace()
        .next_back()
        .map(|candidate| candidate.strip_suffix("^{}").unwrap_or(candidate))
}

fn normalize_refname(refname: &str) -> String {
    refname.strip_suffix("^{}").unwrap_or(refname).to_string()
}

fn write_entries(entries: &[ExcludeExistingEntry]) -> CliResult<()> {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    for entry in entries {
        writeln!(writer, "{}", entry.line)
            .map_err(|error| CliError::io(format!("failed to write show-ref output: {error}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_candidate_refname;
    use crate::utils::util::is_valid_refname;

    #[test]
    fn parse_candidate_refname_uses_last_field_and_strips_peel_suffix() {
        assert_eq!(
            parse_candidate_refname("abcd refs/tags/v1^{}"),
            Some("refs/tags/v1")
        );
    }

    #[test]
    fn valid_refname_accepts_head_and_full_refs() {
        assert!(is_valid_refname("HEAD"));
        assert!(is_valid_refname("refs/heads/main"));
        assert!(!is_valid_refname("main"));
        assert!(!is_valid_refname("refs/heads/bad name"));
    }
}
