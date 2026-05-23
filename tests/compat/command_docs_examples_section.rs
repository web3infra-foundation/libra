//! `tests/compat/command_docs_examples_section.rs` — surface contract
//! ensuring every per-command doc in `docs/commands/<name>.md` carries
//! an Examples / Common Commands section.
//!
//! Companion to `compat_help_examples_banner` (the binary-level guard)
//! and `cli::tests::root_after_help_lists_every_visible_command` (the
//! root-help grouping guard). This file guarantees that the canonical
//! doc users land on when reading `docs/commands/` *also* shows them
//! invocation examples, so we never have a command where the runtime
//! `--help` and the doc disagree about which invocations are canonical
//! (or worse, where the doc skips invocation examples entirely).
//!
//! The contract accepts either of two heading shapes:
//!
//! - `## Examples` / `### Examples` — the canonical form used by
//!   newer docs (db.md, automation.md, usage.md, sandbox.md, agent.md,
//!   publish.md, ls-remote.md, code-control.md, hooks.md, fsck.md,
//!   index-pack.md, cat-file.md, hash-object.md, verify-pack.md,
//!   describe.md, show.md, show-ref.md, symbolic-ref.md).
//! - `## Common Commands` — the older form used by docs migrated
//!   from the pre-improvement layout (add.md, branch.md, blame.md,
//!   bisect.md, clean.md, cherry-pick.md, code.md, cloud.md,
//!   commit.md, diff.md, fetch.md, log.md, etc.).
//!
//! Both shapes serve the same purpose; a future doc that ships without
//! either fails this guard.

use std::{fs, path::PathBuf};

fn commands_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("docs")
        .join("commands")
}

#[test]
fn every_command_doc_has_examples_or_common_commands_section() {
    let dir = commands_dir();
    let entries = fs::read_dir(&dir).unwrap_or_else(|err| {
        panic!(
            "failed to read docs/commands/ directory at {}: {err}",
            dir.display()
        )
    });

    let mut missing: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Skip the index/README and anything that is not a markdown file.
        if file_name == "README.md" || !file_name.ends_with(".md") {
            continue;
        }
        let body = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read command doc {}: {err}", path.display()));
        let has_examples = body.lines().any(|line| {
            line == "## Examples"
                || line == "### Examples"
                || line == "## Example"
                || line == "## Common Commands"
        });
        if !has_examples {
            missing.push(file_name.to_string());
        }
    }

    assert!(
        missing.is_empty(),
        "Every docs/commands/<name>.md page must include either an \
         '## Examples' (or '### Examples'/'## Example') section or a \
         '## Common Commands' section so users land on canonical \
         invocations alongside the synopsis/options. Missing: {missing:?}. \
         The companion `compat_help_examples_banner` guard checks the \
         runtime `--help` surface; this guard checks the doc surface."
    );
}
