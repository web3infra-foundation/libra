//! Integration tests for `libra metadata` (branch/repo metadata KV, lore.md §1.5).
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use super::*;

fn meta_repo() -> tempfile::TempDir {
    create_committed_repo_via_cli()
}

#[test]
fn metadata_branch_roundtrip_set_get_list_unset() {
    let repo = meta_repo();
    let p = repo.path();
    // set + protect notice on stderr
    let set = run_libra_command(
        &["metadata", "set", "protect", "true", "--branch", "main"],
        p,
    );
    assert_cli_success(&set, "set");
    assert!(
        String::from_utf8_lossy(&set.stderr).contains("not yet enforced"),
        "protect emits the not-enforced notice"
    );
    // get
    let get = run_libra_command(&["metadata", "get", "protect", "--branch", "main"], p);
    assert_cli_success(&get, "get");
    assert_eq!(String::from_utf8_lossy(&get.stdout).trim(), "true");
    // overwrite returns previous in JSON
    let over = run_libra_command(
        &[
            "--json", "metadata", "set", "protect", "false", "--branch", "main",
        ],
        p,
    );
    assert_cli_success(&over, "overwrite");
    let json = parse_json_stdout(&over);
    assert_eq!(json["data"]["previous"].as_str(), Some("true"));
    // empty-string value is legal and distinct from absent
    assert_cli_success(
        &run_libra_command(&["metadata", "set", "note", "", "--branch", "main"], p),
        "empty value",
    );
    let empty = run_libra_command(&["metadata", "get", "note", "--branch", "main"], p);
    assert_cli_success(&empty, "get empty (exit 0)");
    // list with prefix
    assert_cli_success(
        &run_libra_command(
            &[
                "metadata",
                "set",
                "lineage.parent",
                "dev",
                "--branch",
                "main",
            ],
            p,
        ),
        "lineage",
    );
    let listed = run_libra_command(
        &[
            "metadata", "list", "--branch", "main", "--prefix", "lineage.",
        ],
        p,
    );
    let out = String::from_utf8_lossy(&listed.stdout);
    assert!(
        out.contains("lineage.parent=dev") && !out.contains("protect"),
        "{out}"
    );
    // unset (and the clear alias)
    assert_cli_success(
        &run_libra_command(&["metadata", "unset", "protect", "--branch", "main"], p),
        "unset",
    );
    assert_cli_success(
        &run_libra_command(&["metadata", "clear", "note", "--branch", "main"], p),
        "clear alias",
    );
    // miss exits 1 (get and unset)
    let miss = run_libra_command(&["metadata", "get", "protect", "--branch", "main"], p);
    assert_eq!(miss.status.code(), Some(1), "get miss exits 1");
    let unmiss = run_libra_command(&["metadata", "unset", "protect", "--branch", "main"], p);
    assert_eq!(unmiss.status.code(), Some(1), "unset miss exits 1");
    // json miss shape
    let jmiss = run_libra_command(
        &["--json", "metadata", "get", "protect", "--branch", "main"],
        p,
    );
    assert_eq!(jmiss.status.code(), Some(1));
    let json = parse_json_stdout(&jmiss);
    assert!(
        json["data"]["value"].is_null(),
        "miss value is null: {json}"
    );
}

#[test]
fn metadata_repo_scope_is_config_kv_dual_surface() {
    let repo = meta_repo();
    let p = repo.path();
    assert_cli_success(
        &run_libra_command(&["metadata", "set", "owner", "platform", "--repo"], p),
        "repo set",
    );
    // Same key visible through the config surface.
    let via_config = run_libra_command(&["config", "--get", "metadata.owner"], p);
    assert_cli_success(&via_config, "config --get sees it");
    assert_eq!(
        String::from_utf8_lossy(&via_config.stdout).trim(),
        "platform"
    );
    // Multi-value via config --add → metadata set/unset refuse with a hint.
    assert_cli_success(
        &run_libra_command(&["config", "--add", "metadata.owner", "second"], p),
        "config --add",
    );
    let conflict = run_libra_command(&["metadata", "set", "owner", "x", "--repo"], p);
    assert_eq!(conflict.status.code(), Some(129), "multi-value set refused");
    assert!(
        String::from_utf8_lossy(&conflict.stderr).contains("unset-all"),
        "actionable hint: {}",
        String::from_utf8_lossy(&conflict.stderr)
    );
    // get remains last-one-wins (no error).
    let get = run_libra_command(&["metadata", "get", "owner", "--repo"], p);
    assert_cli_success(&get, "multi-value get is last-one-wins");
    assert_eq!(String::from_utf8_lossy(&get.stdout).trim(), "second");
}

#[test]
fn metadata_repo_sensitive_and_encrypted_keys_are_refused() {
    let repo = meta_repo();
    let p = repo.path();
    // A sensitive-looking key must not be stored plaintext here.
    let sensitive = run_libra_command(&["metadata", "set", "apitoken", "s3cret", "--repo"], p);
    assert_eq!(sensitive.status.code(), Some(129), "sensitive key refused");
    assert!(
        String::from_utf8_lossy(&sensitive.stderr).contains("config"),
        "hint points at the config door: {}",
        String::from_utf8_lossy(&sensitive.stderr)
    );
    // An existing encrypted row must not be corrupted by a plaintext write.
    assert_cli_success(
        &run_libra_command(&["config", "set", "--encrypt", "metadata.deploy", "v1"], p),
        "config --encrypt",
    );
    let over = run_libra_command(&["metadata", "set", "deploy", "v2", "--repo"], p);
    assert_eq!(over.status.code(), Some(129), "encrypted row write refused");
    // get renders redacted, never the ciphertext.
    let get = run_libra_command(&["metadata", "get", "deploy", "--repo"], p);
    assert_cli_success(&get, "encrypted get");
    assert_eq!(String::from_utf8_lossy(&get.stdout).trim(), "<REDACTED>");
}

#[test]
fn metadata_survives_branch_self_copy() {
    let repo = meta_repo();
    let p = repo.path();
    assert_cli_success(&run_libra_command(&["branch", "feature"], p), "branch");
    assert_cli_success(
        &run_libra_command(
            &["metadata", "set", "protect", "true", "--branch", "feature"],
            p,
        ),
        "set",
    );
    // Forced self-copy must not wipe the branch's metadata — asserted
    // UNCONDITIONALLY: even if the copy command errors, the metadata must
    // still be present (the defect was data loss, not the exit code).
    let _selfcopy = run_libra_command(&["branch", "-C", "feature", "feature"], p);
    let get = run_libra_command(&["metadata", "get", "protect", "--branch", "feature"], p);
    assert_cli_success(&get, "metadata survives a self-copy");
}

#[test]
fn metadata_error_matrix() {
    let repo = meta_repo();
    let p = repo.path();
    // Missing scope → clap usage error (129).
    let noscope = run_libra_command(&["metadata", "set", "k", "v"], p);
    assert_eq!(noscope.status.code(), Some(129), "scope is required");
    // Both scopes → clap group conflict (129).
    let both = run_libra_command(
        &["metadata", "set", "k", "v", "--branch", "main", "--repo"],
        p,
    );
    assert_eq!(both.status.code(), Some(129), "scopes are exclusive");
    // Nonexistent branch → 129 LBR-CLI-003 (Libra CLI-error convention).
    let nobranch = run_libra_command(&["metadata", "get", "k", "--branch", "nope"], p);
    assert_eq!(nobranch.status.code(), Some(129));
    assert!(
        String::from_utf8_lossy(&nobranch.stderr).contains("branch 'nope' not found"),
        "{}",
        String::from_utf8_lossy(&nobranch.stderr)
    );
    // Remote-tracking spelling → hint.
    let remote = run_libra_command(&["metadata", "get", "k", "--branch", "origin/main"], p);
    assert_eq!(remote.status.code(), Some(129));
    assert!(
        String::from_utf8_lossy(&remote.stderr).contains("local branch"),
        "{}",
        String::from_utf8_lossy(&remote.stderr)
    );
    // Invalid key (whitespace) → usage error.
    let badkey = run_libra_command(&["metadata", "set", "bad key", "v", "--branch", "main"], p);
    assert_eq!(badkey.status.code(), Some(129), "invalid key");
}

#[test]
fn metadata_follows_branch_lifecycle() {
    let repo = meta_repo();
    let p = repo.path();
    assert_cli_success(&run_libra_command(&["branch", "feature"], p), "branch");
    assert_cli_success(
        &run_libra_command(
            &["metadata", "set", "protect", "true", "--branch", "feature"],
            p,
        ),
        "set",
    );
    // Copy replicates metadata.
    assert_cli_success(
        &run_libra_command(&["branch", "-c", "feature", "feature-copy"], p),
        "copy",
    );
    let copied = run_libra_command(
        &["metadata", "get", "protect", "--branch", "feature-copy"],
        p,
    );
    assert_cli_success(&copied, "copied metadata");
    // Rename moves metadata.
    assert_cli_success(
        &run_libra_command(&["branch", "-m", "feature", "feature-renamed"], p),
        "rename",
    );
    let moved = run_libra_command(
        &["metadata", "get", "protect", "--branch", "feature-renamed"],
        p,
    );
    assert_cli_success(&moved, "renamed branch keeps metadata");
    // Delete cascades: recreate a branch with the old copy's name and verify
    // it starts clean after the copy is force-deleted.
    assert_cli_success(
        &run_libra_command(&["branch", "-D", "feature-copy"], p),
        "delete",
    );
    assert_cli_success(
        &run_libra_command(&["branch", "feature-copy"], p),
        "recreate",
    );
    let clean = run_libra_command(
        &["metadata", "get", "protect", "--branch", "feature-copy"],
        p,
    );
    assert_eq!(
        clean.status.code(),
        Some(1),
        "recreated branch starts with no metadata (cascade ran)"
    );
}
