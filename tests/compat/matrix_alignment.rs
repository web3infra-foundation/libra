//! `tests/compat/matrix_alignment.rs` — drift detection for public
//! compatibility/documentation matrices.
//!
//! The compatibility matrix promises to cover every top-level
//! `src/cli.rs::Commands` variant. These checks used to be delegated to shell
//! scripts under `scripts/`; they are now **self-contained Rust** so
//! `cargo test --all` enforces them directly with no external script
//! dependency. CI runs the same assertions via `cargo test --all`.

use std::path::PathBuf;

/// Convert a PascalCase clap variant identifier to the kebab-case command name
/// clap derives by default (e.g. `ShowRef` -> `show-ref`, `CatFile` ->
/// `cat-file`). Every `Commands` variant here relies on that default rename, so
/// this mirrors what the binary actually exposes.
fn to_kebab(ident: &str) -> String {
    let mut out = String::with_capacity(ident.len() + 4);
    for (i, ch) in ident.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i != 0 {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Extract the top-level command names from the `enum Commands { … }` block in
/// `src/cli.rs`.
///
/// Variant lines are 4-space-indented tuple variants (`    Init(InitArgs),`);
/// attribute lines (`#[command(...)]`), their continuations, comments, and the
/// `Stash`/`Bisect` sub-enums that follow the block are skipped.
fn cli_command_names(cli_rs: &str) -> Vec<String> {
    let start = cli_rs
        .find("enum Commands {")
        .expect("src/cli.rs must define `enum Commands`");
    let body = &cli_rs[start..];
    // The enum closes at the first line that is exactly `}` (column 0); the
    // sub-enums (`pub enum Stash`) only appear after that brace.
    let end = body.find("\n}").expect("enum Commands must close with `}`");
    let body = &body[..end];

    let mut names = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim_start();
        // Variants are indented exactly four spaces inside the enum body.
        if line.len() - trimmed.len() != 4 {
            continue;
        }
        if !trimmed.starts_with(|c: char| c.is_ascii_uppercase()) {
            continue;
        }
        let ident_end = trimmed
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(trimmed.len());
        // Only tuple variants (`Ident(Args)`) name a command.
        if trimmed[ident_end..].starts_with('(') {
            names.push(to_kebab(&trimmed[..ident_end]));
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Extract command names from the `## Top-level commands` table in
/// `COMPATIBILITY.md` (the first cell of each data row).
fn compat_matrix_names(compat_md: &str) -> Vec<String> {
    let section = compat_md
        .split("## Top-level commands")
        .nth(1)
        .expect("COMPATIBILITY.md must have a `## Top-level commands` section");
    // Stop at the next level-2 heading so the "intentionally absent" table is
    // not pulled in.
    let section = section.split("\n## ").next().unwrap_or(section);

    let mut names = Vec::new();
    for line in section.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        let first = line
            .trim_matches('|')
            .split('|')
            .next()
            .unwrap_or("")
            .trim();
        // Skip the header row and the `|---|` separator.
        if first.is_empty() || first == "Command" || first.starts_with("---") {
            continue;
        }
        names.push(first.to_string());
    }
    names.sort();
    names.dedup();
    names
}

/// The compatibility matrix promises to cover every top-level
/// `src/cli.rs::Commands` variant (including hidden ones like `index-pack` and
/// `hooks`). This catches command additions/removals that forget to update the
/// public matrix. Previously delegated to `scripts/check_compat_matrix.sh`.
#[test]
fn compatibility_matrix_matches_cli_commands() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cli_rs = std::fs::read_to_string(repo.join("src/cli.rs")).expect("read src/cli.rs");
    let compat_md =
        std::fs::read_to_string(repo.join("COMPATIBILITY.md")).expect("read COMPATIBILITY.md");

    let cli = cli_command_names(&cli_rs);
    let matrix = compat_matrix_names(&compat_md);

    assert!(
        !cli.is_empty(),
        "failed to extract any command names from src/cli.rs::Commands"
    );

    let missing: Vec<&String> = cli.iter().filter(|c| !matrix.contains(c)).collect();
    let extra: Vec<&String> = matrix.iter().filter(|c| !cli.contains(c)).collect();

    assert!(
        missing.is_empty() && extra.is_empty(),
        "COMPATIBILITY.md `## Top-level commands` table drifted from \
         src/cli.rs::Commands.\n  \
         Missing from COMPATIBILITY.md (add a row): {missing:?}\n  \
         Extra in COMPATIBILITY.md (remove or rename): {extra:?}"
    );
}

/// Every `/api/code/*` route in `code_router()` must be documented in
/// `docs/commands/code-control.md`. Previously the check confirmed
/// `scripts/check_docs_consistency.sh` listed each route; it now verifies the
/// canonical control doc itself covers them.
#[test]
fn docs_consistency_covers_code_ui_router_matrix() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let web_mod = std::fs::read_to_string(repo.join("src/internal/ai/web/mod.rs"))
        .expect("read src/internal/ai/web/mod.rs");
    let doc = std::fs::read_to_string(repo.join("docs/commands/code-control.md"))
        .expect("read docs/commands/code-control.md");

    let router_region = web_mod
        .split("fn code_router()")
        .nth(1)
        .expect("code_router function exists")
        .split("async fn static_handler")
        .next()
        .expect("static_handler follows code routers");

    let mut routes = router_region
        .lines()
        .filter_map(|line| {
            let start = line.find(".route(\"")? + ".route(\"".len();
            let rest = &line[start..];
            let end = rest.find('"')?;
            Some(format!("/api/code{}", &rest[..end]))
        })
        .collect::<Vec<_>>();
    routes.sort();
    routes.dedup();

    assert!(
        !routes.is_empty(),
        "expected to extract /api/code routes from src/internal/ai/web/mod.rs"
    );

    for route in routes {
        assert!(
            doc.contains(&route),
            "docs/commands/code-control.md must document the Code UI endpoint {route}"
        );
    }
}

/// The `compat-web-check` CI job must fail when the committed `web/out/` static
/// export drifts from a fresh `pnpm build`. The drift check is now inlined into
/// `.github/workflows/base.yml` (previously `scripts/check_web_out_clean.sh`),
/// so assert the workflow still runs the porcelain check after the build.
#[test]
fn web_build_job_inlines_static_export_drift_check() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workflow = std::fs::read_to_string(repo.join(".github/workflows/base.yml"))
        .expect("read .github/workflows/base.yml");
    assert!(
        workflow.contains("git status --porcelain -- web/out"),
        "compat-web-check job must inline `git status --porcelain -- web/out` to detect \
         modified and untracked static export files after `pnpm build`"
    );
}

#[test]
fn lfs_compatibility_docs_use_current_attributes_filename() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for path in [
        "COMPATIBILITY.md",
        "docs/improvement/compatibility/declined.md",
        "docs/improvement/compatibility/governance.md",
    ] {
        let body = std::fs::read_to_string(repo.join(path)).unwrap_or_else(|error| {
            panic!("read {path}: {error}");
        });
        assert!(
            body.contains(".libra_attributes"),
            "{path} must mention the current Libra attributes filename"
        );
        assert!(
            !body.contains(".libraattributes"),
            "{path} must not mention the retired .libraattributes spelling"
        );
    }
}

#[test]
fn compatibility_governance_roadmap_marks_landed_c7_c9_surfaces() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let governance =
        std::fs::read_to_string(repo.join("docs/improvement/compatibility/governance.md"))
            .expect("read docs/improvement/compatibility/governance.md");

    for row in [
        "| merge | partial | C7 ✅ | partial | fast-forward and single-head three-way merge supported; octopus/custom strategies/squash deferred |",
        "| pull | partial | C7 ✅ | partial | fetch + fast-forward/three-way merge supported; advanced strategy flags still partial |",
        "| push | partial | C8 ✅ | partial | branch/tag update, multi-refspec, delete, `--tags`, and `--mirror` supported; local file remote rejected intentionally |",
        "| checkout | partial | C9 ✅ | partial | visible branch compatibility surface plus explicit `checkout -- <path>` restoration alias; prefer `switch` / `restore` |",
    ] {
        assert!(
            governance.contains(row),
            "compatibility governance roadmap must retain completed row: {row}"
        );
    }

    assert!(
        governance.contains(
            "| checkout | partial | visible branch compatibility surface plus explicit `checkout -- <path>` restoration alias; prefer `switch` / `restore` |"
        ),
        "governance matrix skeleton must match the implemented checkout path-mode compatibility note"
    );
    assert!(
        !governance.contains("C7-C9 后续补录"),
        "governance roadmap heading must not describe completed C7-C9 work as a future supplement"
    );
}
