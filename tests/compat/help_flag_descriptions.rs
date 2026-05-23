//! `tests/compat/help_flag_descriptions.rs` — surface contract that every
//! visible flag rendered by `libra <cmd> --help` carries a non-empty
//! description line.
//!
//! Background: clap renders each option as
//!
//!     -f, --flag <VALUE>
//!         <description line indented underneath>
//!
//! When the originating `pub flag: ...` field has no `///` doc comment,
//! the description line is missing and the help output reads like a
//! flag with no documentation at all (e.g. `--bare` with nothing under
//! it). v0.17.886/v0.17.887 landed several such repairs (tag `--force`,
//! push `--set-upstream`, all of `init`'s flags); this guard prevents
//! the regression from re-appearing on any command's visible flags.
//!
//! Approach: scan the `<cmd> --help` output of every visible command
//! for "Options:" / "Arguments:" sections, then walk each flag line and
//! the line below it. If the next non-empty line is another flag (or
//! the end-of-section) instead of an indented description, fail with
//! a list of the empty flags.

use std::process::Command;

fn libra_bin() -> &'static str {
    env!("CARGO_BIN_EXE_libra")
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(libra_bin())
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", "/tmp")
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .output()
        .expect("failed to spawn libra binary")
}

/// Commands we cover. Mirrors `VISIBLE_COMMANDS` in
/// `help_examples_banner.rs` minus subcommand-style families whose
/// `--help` lists sub-commands rather than flags (those are covered by
/// their own per-subcommand tests).
const COMMANDS: &[&str] = &[
    "init",
    "clone",
    "status",
    "add",
    "rm",
    "mv",
    "restore",
    "clean",
    "log",
    "shortlog",
    "show",
    "show-ref",
    "ls-remote",
    "symbolic-ref",
    "rev-parse",
    "rev-list",
    "diff",
    "grep",
    "blame",
    "describe",
    "cat-file",
    "hash-object",
    "verify-pack",
    "commit",
    "branch",
    "switch",
    "checkout",
    "tag",
    "merge",
    "rebase",
    "reset",
    "cherry-pick",
    "push",
    "fetch",
    "pull",
    "fsck",
    "revert",
    "reflog",
    "open",
    "graph",
    "sandbox",
    "db",
    "usage",
];

fn extract_options_section(help: &str) -> Option<&str> {
    let start = help.find("Options:")?;
    let after = &help[start..];
    // Section ends at a blank-blank break before a new heading
    // (EXAMPLES:, NOTES:, Compatibility Notes:, Command Groups:, Help
    // Topics:, etc.) — clap prints all such headings at column 0.
    let mut end = after.len();
    for (idx, line) in after.lines().enumerate() {
        if idx == 0 {
            continue;
        }
        let trimmed = line.trim_end();
        let is_heading = !trimmed.is_empty()
            && !trimmed.starts_with(' ')
            && !trimmed.starts_with('\t')
            && trimmed != "Options:";
        if is_heading {
            // Locate the offset of this line within `after`.
            let pos: usize = after.lines().take(idx).map(|l| l.len() + 1).sum();
            end = pos;
            break;
        }
    }
    Some(&after[..end])
}

/// Returns true if `line` looks like a clap flag/option line at the
/// canonical two-space indent. Examples:
///   `  -h, --help`
///   `  -J, --json[=<FORMAT>]    Emit machine-readable JSON…`
///   `      --bare`
fn is_option_line(line: &str) -> bool {
    if !line.starts_with("  ") || line.starts_with("    ") {
        // Real flag lines start at column 2 or 6 (long-only) but never
        // deeper — descriptions live at column 8+.
        if !line.starts_with("      -") && !line.starts_with("      --") {
            return false;
        }
    }
    let trimmed = line.trim_start();
    trimmed.starts_with('-')
}

#[test]
fn every_visible_flag_has_a_description() {
    let mut empty: Vec<(String, String)> = Vec::new();

    for cmd in COMMANDS {
        let output = run(&[cmd, "--help"]);
        assert!(
            output.status.success(),
            "`libra {cmd} --help` should succeed; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let Some(opts) = extract_options_section(&stdout) else {
            continue;
        };

        let lines: Vec<&str> = opts.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !is_option_line(line) {
                continue;
            }

            // Same-line description? Clap puts at least two spaces
            // between the flag spec and the description.
            let trimmed = line.trim_end();
            let after_flag = trimmed.trim_start_matches([' ']);
            // Find the value-name end (last `>` or last whitespace
            // gap of two-or-more spaces).
            let two_space = after_flag.find("  ");
            let has_inline_desc = match two_space {
                Some(pos) => after_flag[pos..].trim().chars().any(|c| !c.is_whitespace()),
                None => false,
            };
            if has_inline_desc {
                continue;
            }

            // Otherwise, the next non-empty line should be an indented
            // description (column 8 or deeper).
            let mut j = i + 1;
            let mut found_desc = false;
            while j < lines.len() {
                let next = lines[j];
                if next.trim().is_empty() {
                    j += 1;
                    continue;
                }
                // Another flag at the same column means we never saw
                // a description for this flag.
                if is_option_line(next) {
                    break;
                }
                // Description lines indent further than 6 spaces.
                if next.starts_with("        ") || next.starts_with("    ") {
                    found_desc = true;
                }
                break;
            }

            if !found_desc {
                // Extract just the flag name(s) for the failure list.
                let flag_part = trimmed.trim();
                empty.push(((*cmd).to_string(), flag_part.to_string()));
            }
        }
    }

    assert!(
        empty.is_empty(),
        "The following flags are visible in `libra <cmd> --help` but have \
         no description line (clap renders a blank line under the flag). \
         Add a `///` doc comment on the corresponding `pub <field>: ...` \
         in `src/command/<cmd>.rs` describing what the flag does.\n\
         Found: {empty:#?}"
    );
}
