//! Integration tests for `repack` and the hidden `pack-objects` plumbing.
//!
//! **Layer:** L1 — deterministic, no external dependencies.
//!
//! The central correctness check is that a pack produced by the shared writer
//! round-trips through `index-pack` (i.e. it is a well-formed pack whose trailer
//! checksum matches its bytes) and that objects remain readable after `-d`
//! removes the loose copies. The pre-refactor maintenance writer produced packs
//! that failed exactly this check.

use std::{
    fs,
    io::Write,
    process::{Output, Stdio},
};

use super::*;

/// Run the Libra binary feeding `stdin_bytes` on stdin (for `pack-objects`).
fn run_libra_with_stdin(args: &[&str], cwd: &std::path::Path, stdin_bytes: &[u8]) -> Output {
    let mut command = base_libra_command(args, cwd);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("failed to spawn libra");
    child
        .stdin
        .take()
        .expect("stdin pipe")
        .write_all(stdin_bytes)
        .expect("failed to write stdin");
    child.wait_with_output().expect("failed to wait for libra")
}

/// Every loose object id currently on disk under `.libra/objects/`.
fn loose_object_ids(repo: &std::path::Path) -> Vec<String> {
    let objects = repo.join(".libra").join("objects");
    let mut ids = Vec::new();
    let Ok(dirs) = fs::read_dir(&objects) else {
        return ids;
    };
    for dir in dirs.flatten() {
        let name = dir.file_name().to_string_lossy().into_owned();
        // Object shards are the 2-hex-char directories; skip `pack`, `info`, …
        if name.len() != 2 || !name.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let Ok(files) = fs::read_dir(dir.path()) else {
            continue;
        };
        for file in files.flatten() {
            ids.push(format!("{name}{}", file.file_name().to_string_lossy()));
        }
    }
    ids
}

/// Pack files (`*.pack`) currently in the pack directory.
fn pack_files(repo: &std::path::Path) -> Vec<std::path::PathBuf> {
    let pack_dir = repo.join(".libra").join("objects").join("pack");
    let Ok(entries) = fs::read_dir(&pack_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "pack"))
        .collect()
}

/// Build a repo with a few commits so several loose objects exist.
fn repo_with_history() -> tempfile::TempDir {
    let repo = create_committed_repo_via_cli();
    for i in 0..3 {
        fs::write(
            repo.path().join(format!("file{i}.txt")),
            format!("contents number {i}\n"),
        )
        .expect("write file");
        let output = run_libra_command(&["add", &format!("file{i}.txt")], repo.path());
        assert_cli_success(&output, "add file");
        let output = run_libra_command(
            &["commit", "-m", &format!("commit {i}"), "--no-verify"],
            repo.path(),
        );
        assert_cli_success(&output, "commit file");
    }
    repo
}

#[test]
fn repack_all_delete_produces_valid_pack_and_prunes_loose() {
    let repo = repo_with_history();
    assert!(
        !loose_object_ids(repo.path()).is_empty(),
        "precondition: repo should have loose objects"
    );

    let output = run_libra_command(&["repack", "-a", "-d"], repo.path());
    assert_cli_success(&output, "repack -a -d");

    // A pack + its index now exist.
    let packs = pack_files(repo.path());
    assert_eq!(packs.len(), 1, "exactly one pack should be written");
    let pack = &packs[0];
    assert!(
        pack.with_extension("idx").exists(),
        "the pack's .idx must be written alongside it"
    );

    // The pack round-trips through index-pack — i.e. it is well-formed and its
    // trailer checksum matches its bytes. The old hand-rolled writer failed here.
    let output = run_libra_command(&["index-pack", pack.to_str().unwrap()], repo.path());
    assert_cli_success(&output, "index-pack must validate the repacked pack");

    // Loose objects were pruned...
    assert!(
        loose_object_ids(repo.path()).is_empty(),
        "-d should remove the loose objects now stored in the pack"
    );
    // ...yet history is still fully readable from the pack.
    let output = run_libra_command(&["log", "--oneline"], repo.path());
    assert_cli_success(&output, "log after repack -d");
    let output = run_libra_command(&["cat-file", "-p", "HEAD"], repo.path());
    assert_cli_success(&output, "cat-file HEAD after repack -d");
}

#[test]
fn repack_is_idempotent_when_nothing_is_loose() {
    let repo = repo_with_history();
    let output = run_libra_command(&["repack", "-a", "-d"], repo.path());
    assert_cli_success(&output, "first repack");

    // With every reachable object already packed and no loose objects left, a
    // default repack has nothing to do.
    let output = run_libra_command(&["repack"], repo.path());
    assert_cli_success(&output, "second repack");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Nothing new to pack"),
        "expected a no-op message, got: {stdout}"
    );
}

#[test]
fn repack_json_reports_pack_and_counts() {
    let repo = repo_with_history();
    let output = run_libra_command(&["--json", "repack", "-a", "-d"], repo.path());
    assert_cli_success(&output, "repack --json");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_str(&stdout).expect("repack --json must emit valid JSON");
    let data = &value["data"];
    assert!(
        data["pack"]
            .as_str()
            .unwrap_or_default()
            .starts_with("pack-"),
        "json should name the new pack, got: {stdout}"
    );
    assert!(
        data["objects_packed"].as_u64().unwrap_or(0) > 0,
        "json should report a positive object count, got: {stdout}"
    );
}

#[test]
fn repack_outside_repository_fails() {
    let dir = tempdir().expect("tempdir");
    let output = run_libra_command(&["repack"], dir.path());
    assert!(
        !output.status.success(),
        "repack outside a repository must fail"
    );
}

#[test]
fn pack_objects_reads_ids_from_stdin_and_writes_valid_pack() {
    let repo = repo_with_history();
    let ids = loose_object_ids(repo.path());
    assert!(!ids.is_empty(), "need loose objects to pack");
    let stdin = ids.join("\n");

    let output = run_libra_with_stdin(&["pack-objects"], repo.path(), stdin.as_bytes());
    assert!(
        output.status.success(),
        "pack-objects should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().starts_with("pack-"),
        "pack-objects should print the new pack's name, got: {stdout}"
    );

    // The written pack validates through index-pack.
    let packs = pack_files(repo.path());
    assert!(!packs.is_empty(), "pack-objects should write a pack");
    let output = run_libra_command(&["index-pack", packs[0].to_str().unwrap()], repo.path());
    assert_cli_success(&output, "index-pack must validate the pack-objects output");
}

#[test]
fn pack_objects_empty_stdin_fails() {
    let repo = repo_with_history();
    let output = run_libra_with_stdin(&["pack-objects"], repo.path(), b"   \n");
    assert!(
        !output.status.success(),
        "pack-objects with no ids on stdin must fail"
    );
}
