use std::io::Write;

use crate::{
    command::show_ref::ShowRefEntry,
    utils::{
        error::{CliError, CliResult},
        output::{OutputConfig, emit_json_data},
    },
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ShowRefRenderOptions {
    pub(crate) hash_only: bool,
    pub(crate) abbrev: Option<usize>,
}

impl ShowRefRenderOptions {
    pub(crate) const fn from_args(hash: Option<usize>, abbrev: Option<usize>) -> Self {
        let hash_only = hash.is_some();
        let abbrev = match (hash, abbrev) {
            (Some(width), _) if width > 0 => Some(width),
            (Some(_), Some(width)) => Some(width),
            (Some(_), None) => None,
            (None, Some(width)) => Some(width),
            (None, None) => None,
        };
        Self { hash_only, abbrev }
    }
}

pub(crate) fn render_show_ref_entries(
    entries: &[ShowRefEntry],
    options: ShowRefRenderOptions,
    output: &OutputConfig,
) -> CliResult<()> {
    let entries = entries
        .iter()
        .map(|entry| ShowRefEntry {
            hash: format_hash(&entry.hash, options.abbrev),
            refname: entry.refname.clone(),
        })
        .collect::<Vec<_>>();

    if output.is_json() {
        emit_json_data(
            "show-ref",
            &serde_json::json!({
                "hash_only": options.hash_only,
                "abbrev": options.abbrev,
                "entries": entries,
            }),
            output,
        )
    } else if output.quiet {
        Ok(())
    } else {
        write_human_entries(&entries, options.hash_only)
    }
}

fn write_human_entries(entries: &[ShowRefEntry], hash_only: bool) -> CliResult<()> {
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    for entry in entries {
        if hash_only {
            writeln!(writer, "{}", entry.hash)
        } else {
            writeln!(writer, "{} {}", entry.hash, entry.refname)
        }
        .map_err(|error| CliError::io(format!("failed to write show-ref output: {error}")))?;
    }
    Ok(())
}

fn format_hash(hash: &str, abbrev: Option<usize>) -> String {
    match abbrev {
        Some(width) if width > 0 && width < hash.len() => hash.chars().take(width).collect(),
        Some(_) | None => hash.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{ShowRefRenderOptions, format_hash};

    #[test]
    fn hash_width_overrides_abbrev_when_explicit() {
        let options = ShowRefRenderOptions::from_args(Some(12), Some(7));
        assert!(options.hash_only);
        assert_eq!(options.abbrev, Some(12));
    }

    #[test]
    fn hash_without_width_uses_abbrev_width() {
        let options = ShowRefRenderOptions::from_args(Some(0), Some(10));
        assert!(options.hash_only);
        assert_eq!(options.abbrev, Some(10));
    }

    #[test]
    fn zero_width_keeps_full_hash() {
        assert_eq!(format_hash("1234567890", Some(0)), "1234567890");
    }
}
