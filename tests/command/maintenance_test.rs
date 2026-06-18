//! Integration tests for the `maintenance` command.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    fs,
    time::{Duration, SystemTime},
};

use tempfile::tempdir;
use walkdir::WalkDir;

use super::*;

// ---------------------------------------------------------------------------
// Basic Functionality Tests (≥ 4 required)
// ---------------------------------------------------------------------------

#[test]

/// Tests `maintenance run` on a healthy repository passes successfully.
/// Verifies the basic happy path for running all maintenance tasks.
fn test_maintenance_run_all_tasks_passes() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["maintenance", "run"], repo.path());
    assert!(
        output.status.success(),
        "maintenance run should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]

/// Tests `maintenance run --task gc` runs only the gc task.
/// Verifies that selective task execution works.
fn test_maintenance_run_gc_only() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["maintenance", "run", "--task", "gc"], repo.path());
    assert!(
        output.status.success(),
        "maintenance run --task gc should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("gc"),
        "output should mention gc task, got: {stdout}"
    );
}

#[test]

/// Tests `maintenance register` followed by `maintenance status`.
/// Verifies registration and status reporting.
fn test_maintenance_register_and_status() {
    let repo = create_committed_repo_via_cli();

    let register_output = run_libra_command(&["maintenance", "register"], repo.path());
    assert!(
        register_output.status.success(),
        "register should succeed, stderr: {}",
        String::from_utf8_lossy(&register_output.stderr)
    );

    let status_output = run_libra_command(&["maintenance", "status"], repo.path());
    assert!(
        status_output.status.success(),
        "status should succeed, stderr: {}",
        String::from_utf8_lossy(&status_output.stderr)
    );
    let stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(
        stdout.contains("registered"),
        "status should show registered, got: {stdout}"
    );
}

#[test]

/// Tests `maintenance unregister` removes registration.
/// Verifies the unregister happy path.
fn test_maintenance_unregister() {
    let repo = create_committed_repo_via_cli();

    run_libra_command(&["maintenance", "register"], repo.path());

    let output = run_libra_command(&["maintenance", "unregister"], repo.path());
    assert!(
        output.status.success(),
        "unregister should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status_output = run_libra_command(&["maintenance", "status"], repo.path());
    let stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(
        stdout.contains("not registered"),
        "status should show not registered after unregister, got: {stdout}"
    );
}

#[test]

/// Tests `maintenance run --dry-run` reports without modifying the repository.
/// Verifies dry-run mode produces output and exits successfully.
fn test_maintenance_run_dry_run() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["maintenance", "run", "--dry-run"], repo.path());
    assert!(
        output.status.success(),
        "dry-run should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("would") || stdout.contains("skipping") || stdout.contains("skipped"),
        "dry-run should indicate no changes, got: {stdout}"
    );
}

#[test]

/// Tests `maintenance run --task loose-objects` on a repository with few objects.
/// Verifies that the threshold check prevents unnecessary packing.
fn test_maintenance_run_loose_objects_few() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(
        &["maintenance", "run", "--task", "loose-objects"],
        repo.path(),
    );
    assert!(
        output.status.success(),
        "loose-objects should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("skipping") || stdout.contains("threshold"),
        "few loose objects should skip packing, got: {stdout}"
    );
}

#[test]

/// Regression test: `maintenance run --task loose-objects` writes a standard
/// `.pack`/`.idx` pair and leaves objects readable.
///
/// Previously the task wrote `pack-maintenance-{timestamp}` without a `.pack`
/// extension, which `LocalStorage::list_all_packs` never discovers. After the
/// task deleted the original loose objects, those objects became unreadable.
fn test_maintenance_run_loose_objects_pack_is_readable() {
    let repo = create_committed_repo_via_cli();

    // Create enough files in a single commit to exceed the loose-object
    // threshold (100 objects). Each file becomes a blob; together with the
    // commit and tree this gives us >100 loose objects.
    for i in 0..105 {
        fs::write(
            repo.path().join(format!("file_{i:03}.txt")),
            format!("content {i}\n"),
        )
        .unwrap();
    }
    run_libra_command(&["add", "."], repo.path());
    run_libra_command(&["commit", "-m", "many files", "--no-verify"], repo.path());

    // Age all loose object files so the loose-objects task considers them old.
    let objects_dir = repo.path().join(".libra").join("objects");
    let old_time = SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60);
    for entry in WalkDir::new(&objects_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let file = fs::File::open(entry.path()).unwrap();
        file.set_modified(old_time).unwrap();
    }

    // Run the loose-objects task and verify it packed objects.
    let output = run_libra_command(
        &["--json", "maintenance", "run", "--task", "loose-objects"],
        repo.path(),
    );
    assert!(
        output.status.success(),
        "loose-objects should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let tasks = json
        .get("data")
        .and_then(|d| d.get("tasks"))
        .expect("tasks array");
    let loose_task = tasks
        .as_array()
        .expect("tasks is array")
        .iter()
        .find(|t| t.get("task").and_then(|v| v.as_str()) == Some("loose-objects"))
        .expect("loose-objects task in results");
    let packed = loose_task
        .get("objects_packed")
        .and_then(|v| v.as_u64())
        .expect("objects_packed field");
    assert!(
        packed >= 100,
        "loose-objects should pack at least 100 objects, got task: {loose_task}"
    );

    // Verify the pack is discoverable: a .pack file should exist under
    // .libra/objects/pack.
    let pack_dir = objects_dir.join("pack");
    let pack_files: Vec<_> = fs::read_dir(&pack_dir)
        .expect("pack directory should exist")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "pack"))
        .collect();
    assert!(
        !pack_files.is_empty(),
        "a .pack file should be created under {pack_dir:?}"
    );

    // Verify history remains readable after the loose objects were removed.
    let log_output = run_libra_command(&["log", "--pretty=%s"], repo.path());
    assert!(
        log_output.status.success(),
        "log should succeed after packing, stderr: {}",
        String::from_utf8_lossy(&log_output.stderr)
    );
    let log_stdout = String::from_utf8_lossy(&log_output.stdout);
    assert!(
        log_stdout.contains("many files") && log_stdout.contains("base"),
        "history must remain intact after packing, got: {log_stdout}"
    );
}

#[test]

/// Regression test: `maintenance run --task incremental-repack` creates a valid
/// `.pack`/`.idx` pair and only deletes source packs after verification.
///
/// Previously the consolidated file was named without a `.pack` extension, so
/// `LocalStorage::list_all_packs` ignored it. After the task deleted the source
/// `.pack` files, objects that only lived in those packs became unreadable.
fn test_maintenance_run_incremental_repack_verifies_new_pack() {
    let repo = create_committed_repo_via_cli();

    // Create 5 separate packs by repeatedly adding old loose objects and
    // running the loose-objects task.
    for pack_idx in 0..5 {
        for file_idx in 0..105 {
            fs::write(
                repo.path()
                    .join(format!("pack{pack_idx}_file{file_idx:03}.txt")),
                format!("content {pack_idx} {file_idx}\n"),
            )
            .unwrap();
        }
        run_libra_command(&["add", "."], repo.path());
        run_libra_command(
            &[
                "commit",
                "-m",
                &format!("pack commit {pack_idx}"),
                "--no-verify",
            ],
            repo.path(),
        );

        // Age all loose objects so the next loose-objects run packs them.
        let objects_dir = repo.path().join(".libra").join("objects");
        let old_time = SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60);
        for entry in WalkDir::new(&objects_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let file = fs::File::open(entry.path()).unwrap();
            file.set_modified(old_time).unwrap();
        }

        let output = run_libra_command(
            &["maintenance", "run", "--task", "loose-objects"],
            repo.path(),
        );
        assert!(
            output.status.success(),
            "loose-objects run {pack_idx} should pass, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Verify we have at least 5 .pack files before incremental repack.
    let pack_dir = repo.path().join(".libra").join("objects").join("pack");
    let initial_packs: Vec<_> = fs::read_dir(&pack_dir)
        .expect("pack directory should exist")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "pack"))
        .collect();
    assert!(
        initial_packs.len() >= 5,
        "should have at least 5 packs before incremental repack, got {}",
        initial_packs.len()
    );

    // Run incremental repack and verify it succeeds.
    let output = run_libra_command(
        &[
            "--json",
            "maintenance",
            "run",
            "--task",
            "incremental-repack",
        ],
        repo.path(),
    );
    assert!(
        output.status.success(),
        "incremental-repack should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let tasks = json
        .get("data")
        .and_then(|d| d.get("tasks"))
        .expect("tasks array");
    let repack_task = tasks
        .as_array()
        .expect("tasks is array")
        .iter()
        .find(|t| t.get("task").and_then(|v| v.as_str()) == Some("incremental-repack"))
        .expect("incremental-repack task in results");
    let packs_repacked = repack_task
        .get("packs_repacked")
        .and_then(|v| v.as_u64())
        .expect("packs_repacked field");
    assert!(
        packs_repacked >= 5,
        "incremental-repack should repack at least 5 packs, got task: {repack_task}"
    );

    // Verify a single consolidated .pack remains.
    let final_packs: Vec<_> = fs::read_dir(&pack_dir)
        .expect("pack directory should exist")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "pack"))
        .collect();
    assert_eq!(
        final_packs.len(),
        1,
        "should be left with exactly one consolidated .pack, got {}",
        final_packs.len()
    );

    // Verify history remains readable after source packs were removed.
    let log_output = run_libra_command(&["log", "--pretty=%s"], repo.path());
    assert!(
        log_output.status.success(),
        "log should succeed after repack, stderr: {}",
        String::from_utf8_lossy(&log_output.stderr)
    );
    let log_stdout = String::from_utf8_lossy(&log_output.stdout);
    for pack_idx in 0..5 {
        let msg = format!("pack commit {pack_idx}");
        assert!(
            log_stdout.contains(&msg),
            "history must contain '{msg}' after repack, got: {log_stdout}"
        );
    }
    assert!(
        log_stdout.contains("base"),
        "history must contain 'base' after repack, got: {log_stdout}"
    );
}

#[test]

/// Tests `maintenance run --task pack-refs` packs loose refs.
/// Verifies pack-refs task execution.
fn test_maintenance_run_pack_refs() {
    let repo = create_committed_repo_via_cli();

    // Create a branch to have refs to pack
    run_libra_command(&["branch", "test-branch"], repo.path());

    let output = run_libra_command(&["maintenance", "run", "--task", "pack-refs"], repo.path());
    assert!(
        output.status.success(),
        "pack-refs should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]

/// Regression test: `maintenance run --task pack-refs` must write fully-qualified
/// refnames such as `refs/heads/main` instead of bare `main`.
///
/// Previously collect_refs was rooted at `refs/heads` and stored relative names,
/// so the resulting packed-refs file could not be resolved by standard readers.
fn test_maintenance_pack_refs_writes_full_refnames() {
    let repo = create_committed_repo_via_cli();

    // Libra stores refs in SQLite by default; pack-refs operates on file-backed
    // loose refs under .libra/refs/heads. Materialize two loose refs manually so
    // the task has something to collapse into packed-refs.
    let main_hash = rev_parse(repo.path(), "main");
    let feature_hash = rev_parse(repo.path(), "main");
    let refs_heads = repo.path().join(".libra").join("refs").join("heads");
    fs::create_dir_all(&refs_heads).unwrap();
    fs::write(refs_heads.join("main"), format!("{main_hash}\n")).unwrap();
    fs::write(
        refs_heads.join("feature-branch"),
        format!("{feature_hash}\n"),
    )
    .unwrap();

    let output = run_libra_command(&["maintenance", "run", "--task", "pack-refs"], repo.path());
    assert!(
        output.status.success(),
        "pack-refs should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let packed_refs = repo.path().join(".libra").join("packed-refs");
    assert!(packed_refs.exists(), "packed-refs file should exist");
    let content = fs::read_to_string(&packed_refs).unwrap();
    assert!(
        content.contains("refs/heads/main"),
        "packed-refs must contain fully-qualified main ref, got: {content}"
    );
    assert!(
        content.contains("refs/heads/feature-branch"),
        "packed-refs must contain fully-qualified branch ref, got: {content}"
    );
    assert!(
        !content.lines().any(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with('#')
                && (trimmed.ends_with(" main") || trimmed.ends_with(" feature-branch"))
        }),
        "packed-refs must not contain bare relative refnames, got: {content}"
    );

    // Loose ref files should have been removed after successful packing.
    assert!(
        !refs_heads.join("main").exists(),
        "loose main ref should be removed after packing"
    );
    assert!(
        !refs_heads.join("feature-branch").exists(),
        "loose feature-branch ref should be removed after packing"
    );
}

/// Resolve a ref to its full hex hash via `rev-parse`.
fn rev_parse(repo: &std::path::Path, rev: &str) -> String {
    let output = run_libra_command(&["rev-parse", rev], repo);
    assert!(
        output.status.success(),
        "rev-parse {rev} should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]

/// Regression test: `maintenance run --task pack-refs` must update packed-refs
/// atomically and preserve entries that only exist in the existing file.
///
/// Previously `File::create(packed-refs)` truncated the file before the new
/// content was durable, so a crash/IO error during the write could lose packed
/// refs. The task now writes to a temp file and atomically renames it into place.
fn test_maintenance_pack_refs_preserves_existing_entries() {
    let repo = create_committed_repo_via_cli();
    let main_hash = rev_parse(repo.path(), "main");

    // Pre-populate packed-refs with an entry that has no corresponding loose ref.
    let libra_dir = repo.path().join(".libra");
    fs::write(
        libra_dir.join("packed-refs"),
        "# packed-refs with peeled tags\nabcd1234 refs/heads/legacy\n",
    )
    .unwrap();

    // Add a new loose ref to be packed.
    let refs_heads = libra_dir.join("refs").join("heads");
    fs::create_dir_all(&refs_heads).unwrap();
    fs::write(refs_heads.join("main"), format!("{main_hash}\n")).unwrap();

    let output = run_libra_command(&["maintenance", "run", "--task", "pack-refs"], repo.path());
    assert!(
        output.status.success(),
        "pack-refs should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let packed_refs = libra_dir.join("packed-refs");
    let content = fs::read_to_string(&packed_refs).unwrap();
    assert!(
        content.contains("refs/heads/main"),
        "packed-refs must contain the new ref, got: {content}"
    );
    assert!(
        content.contains("refs/heads/legacy"),
        "packed-refs must preserve the pre-existing entry, got: {content}"
    );
    assert!(
        !libra_dir.join("packed-refs.tmp").exists(),
        "temporary packed-refs file should be cleaned up"
    );
}

#[test]

/// Regression test: `maintenance run --task gc` must preserve objects reachable
/// from annotated tags.
///
/// Previously `walk_reachable` ignored `ObjectType::Tag`, so only the tag object
/// itself was marked reachable and its target commit (and tree/blobs) could be
/// pruned, leaving the tag dangling.
fn test_maintenance_gc_preserves_annotated_tag_target() {
    let repo = create_committed_repo_via_cli();

    // Create a commit to tag.
    fs::write(repo.path().join("tagged.txt"), "tagged content\n").unwrap();
    run_libra_command(&["add", "tagged.txt"], repo.path());
    run_libra_command(
        &["commit", "-m", "tagged commit", "--no-verify"],
        repo.path(),
    );

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let tagged_commit = stdout.lines().next().unwrap().trim();

    // Create an annotated tag pointing to the tagged commit.
    run_libra_command(&["tag", "-m", "annotated tag", "v-tagged"], repo.path());

    // Reset to the base commit so the tagged commit is no longer reachable
    // from HEAD, but remains reachable via the tag.
    let base_commit = stdout.lines().nth(1).unwrap().trim();
    run_libra_command(&["reset", "--hard", base_commit], repo.path());

    // Run GC and verify the tagged commit is still readable.
    let output = run_libra_command(
        &["--json", "maintenance", "run", "--task", "gc"],
        repo.path(),
    );
    assert!(
        output.status.success(),
        "gc should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cat_output = run_libra_command(&["cat-file", "-t", tagged_commit], repo.path());
    assert!(
        cat_output.status.success(),
        "tagged commit should still be readable after gc, stderr: {}",
        String::from_utf8_lossy(&cat_output.stderr)
    );
    let cat_stdout = String::from_utf8_lossy(&cat_output.stdout);
    assert!(
        cat_stdout.contains("commit"),
        "tagged commit object type should still be readable, got: {cat_stdout}"
    );
}

#[test]

/// Regression test: `maintenance run --task gc` must treat reflog old OIDs as
/// reachability roots.
///
/// Previously GC only walked new_oid from reflog entries, so after a force reset
/// the previous tip could be pruned even though users expect to recover it from
/// the reflog.
fn test_maintenance_gc_preserves_reflog_old_oid() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("second.txt"), "second content\n").unwrap();
    run_libra_command(&["add", "second.txt"], repo.path());
    run_libra_command(
        &["commit", "-m", "second commit", "--no-verify"],
        repo.path(),
    );

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let second_commit = stdout.lines().next().unwrap().trim();
    let first_commit = stdout.lines().nth(1).unwrap().trim();

    // Reset hard to the first commit so the second commit is only reachable
    // through reflog old_oid.
    run_libra_command(&["reset", "--hard", first_commit], repo.path());

    let output = run_libra_command(
        &["--json", "maintenance", "run", "--task", "gc"],
        repo.path(),
    );
    assert!(
        output.status.success(),
        "gc should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cat_output = run_libra_command(&["cat-file", "-t", second_commit], repo.path());
    assert!(
        cat_output.status.success(),
        "reflog old OID commit should still be readable after gc, stderr: {}",
        String::from_utf8_lossy(&cat_output.stderr)
    );
    let cat_stdout = String::from_utf8_lossy(&cat_output.stdout);
    assert!(
        cat_stdout.contains("commit"),
        "old OID object type should still be readable, got: {cat_stdout}"
    );
}

#[test]

/// Tests `maintenance status --json` returns structured output.
/// Verifies JSON output for the status subcommand.
fn test_maintenance_status_json() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "maintenance", "status"], repo.path());
    assert!(
        output.status.success(),
        "json status should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.stdout.is_empty(),
        "json status should produce stdout"
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let data = json.get("data").expect("json should have data field");
    assert!(
        data.get("registered").is_some(),
        "json data should contain registered field"
    );
}

// ---------------------------------------------------------------------------
// Boundary Condition Tests (≥ 8 required)
// ---------------------------------------------------------------------------

#[test]

/// Tests `maintenance run` on an empty (newly initialized) repository.
/// Verifies graceful handling of repositories with minimal objects.
fn test_maintenance_run_empty_repo() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["maintenance", "run"], repo.path());
    assert!(
        output.status.success(),
        "maintenance on empty repo should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]

/// Tests `maintenance run` on a repository with only a root commit.
/// Verifies minimal repository structure handling.
fn test_maintenance_run_single_commit_repo() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    fs::write(repo.path().join("only.txt"), "only commit\n").unwrap();
    run_libra_command(&["add", "."], repo.path());
    run_libra_command(&["commit", "-m", "only", "--no-verify"], repo.path());

    let output = run_libra_command(&["maintenance", "run"], repo.path());
    assert!(
        output.status.success(),
        "maintenance on single-commit repo should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]

/// Tests `maintenance run --task loose-objects` when there are no loose objects.
/// Verifies threshold-based skip logic on empty object sets.
fn test_maintenance_run_with_no_loose_objects() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(
        &["maintenance", "run", "--task", "loose-objects"],
        repo.path(),
    );
    assert!(
        output.status.success(),
        "should pass even with no loose objects, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("skipping") || stdout.contains("only"),
        "should indicate skipping, got: {stdout}"
    );
}

#[test]

/// Tests `maintenance run --task incremental-repack` when there are no pack files.
/// Verifies graceful handling of missing pack directory.
fn test_maintenance_run_with_few_packs() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(
        &["maintenance", "run", "--task", "incremental-repack"],
        repo.path(),
    );
    assert!(
        output.status.success(),
        "should pass with few packs, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]

/// Tests `maintenance status` before any registration.
/// Verifies default unregistered state.
fn test_maintenance_status_before_register() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["maintenance", "status"], repo.path());
    assert!(
        output.status.success(),
        "status should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("not registered"),
        "default status should be not registered, got: {stdout}"
    );
}

#[test]

/// Tests `maintenance run --quiet` suppresses progress output.
/// Verifies quiet mode reduces stdout.
fn test_maintenance_run_quiet() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["maintenance", "run", "--quiet"], repo.path());
    assert!(
        output.status.success(),
        "quiet run should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]

/// Tests `maintenance run --task commit-graph` reports skip gracefully.
/// Verifies handling of unsupported tasks.
fn test_maintenance_run_commit_graph_skipped() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(
        &["maintenance", "run", "--task", "commit-graph"],
        repo.path(),
    );
    assert!(
        output.status.success(),
        "commit-graph should pass (skipped), stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("skipped") || stdout.contains("not yet supported"),
        "should indicate skipped, got: {stdout}"
    );
}

#[test]

/// Tests `maintenance run --task prefetch` reports skip gracefully.
/// Verifies handling of tasks requiring remote configuration.
fn test_maintenance_run_prefetch_skipped() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["maintenance", "run", "--task", "prefetch"], repo.path());
    assert!(
        output.status.success(),
        "prefetch should pass (skipped), stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("skipped") || stdout.contains("requires remote"),
        "should indicate skipped, got: {stdout}"
    );
}

#[test]

/// Tests `maintenance run --dry-run --task gc` with a dangling object.
/// Verifies dry-run correctly reports what would be removed.
fn test_maintenance_run_dry_run_gc_with_dangling() {
    let repo = create_committed_repo_via_cli();

    // Create a second commit and then reset, leaving a dangling commit
    fs::write(repo.path().join("file2.txt"), "second file\n").unwrap();
    run_libra_command(&["add", "file2.txt"], repo.path());
    run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();
    run_libra_command(&["reset", "--hard", first_commit], repo.path());

    let output = run_libra_command(
        &["maintenance", "run", "--dry-run", "--task", "gc"],
        repo.path(),
    );
    assert!(
        output.status.success(),
        "dry-run gc should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("would") || stdout.contains("unreachable"),
        "dry-run should mention would remove or unreachable, got: {stdout}"
    );
}

#[test]

/// Regression test: `maintenance run --task gc` must preserve reachable objects.
///
/// Previously `collect_reachable_objects` pre-inserted ref/reflog commit hashes
/// into the reachable set before calling `walk_reachable`, so the walker
/// returned immediately and never protected the commit's tree, parents, or
/// historical blobs. This could cause GC to delete reachable loose objects and
/// corrupt history.
fn test_maintenance_gc_preserves_reachable_objects() {
    let repo = create_committed_repo_via_cli();

    // Create additional commits so the history contains multiple trees/blobs.
    fs::write(repo.path().join("file_a.txt"), "content a\n").unwrap();
    run_libra_command(&["add", "file_a.txt"], repo.path());
    run_libra_command(&["commit", "-m", "commit a", "--no-verify"], repo.path());

    fs::write(repo.path().join("file_b.txt"), "content b\n").unwrap();
    run_libra_command(&["add", "file_b.txt"], repo.path());
    run_libra_command(&["commit", "-m", "commit b", "--no-verify"], repo.path());

    // Run GC and verify it does not remove any reachable object.
    let output = run_libra_command(
        &["--json", "maintenance", "run", "--task", "gc"],
        repo.path(),
    );
    assert!(
        output.status.success(),
        "gc should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let tasks = json
        .get("data")
        .and_then(|d| d.get("tasks"))
        .expect("tasks array");
    let gc_task = tasks
        .as_array()
        .expect("tasks is array")
        .iter()
        .find(|t| t.get("task").and_then(|v| v.as_str()) == Some("gc"))
        .expect("gc task in results");
    let removed = gc_task
        .get("objects_removed")
        .and_then(|v| v.as_u64())
        .expect("objects_removed field");
    assert_eq!(
        removed, 0,
        "gc must not remove reachable objects, got task: {gc_task}"
    );

    // Verify history is still readable after GC.
    let log_output = run_libra_command(&["log", "--pretty=%s"], repo.path());
    assert!(
        log_output.status.success(),
        "log should succeed after gc, stderr: {}",
        String::from_utf8_lossy(&log_output.stderr)
    );
    let log_stdout = String::from_utf8_lossy(&log_output.stdout);
    assert!(
        log_stdout.contains("commit b")
            && log_stdout.contains("commit a")
            && log_stdout.contains("base"),
        "history must remain intact after gc, got: {log_stdout}"
    );
}

#[test]

/// Regression test: `maintenance run --task gc` must leave fresh unreachable
/// objects alone until a grace period has passed.
///
/// Without a grace period, GC can race with commands that have just written
/// loose objects before updating refs/reflogs/index, corrupting the concurrent
/// operation. Objects older than the grace period can still be pruned.
fn test_maintenance_gc_skips_fresh_unreachable_objects() {
    let repo = create_committed_repo_via_cli();

    // Write a loose blob that is not referenced by any ref/index/reflog.
    let blob_file = repo.path().join("dangling.txt");
    fs::write(&blob_file, "dangling content\n").unwrap();
    let hash_output = run_libra_command(&["hash-object", "-w", "dangling.txt"], repo.path());
    assert!(
        hash_output.status.success(),
        "hash-object should succeed, stderr: {}",
        String::from_utf8_lossy(&hash_output.stderr)
    );
    let blob_hash = String::from_utf8_lossy(&hash_output.stdout)
        .trim()
        .to_string();
    fs::remove_file(&blob_file).unwrap();
    let obj_path = loose_object_path(repo.path(), &blob_hash);
    assert!(
        obj_path.exists(),
        "written blob should exist as loose object"
    );

    // A fresh unreachable object must not be deleted.
    let fresh_output = run_libra_command(
        &["--json", "maintenance", "run", "--task", "gc"],
        repo.path(),
    );
    assert!(
        fresh_output.status.success(),
        "gc should pass on fresh object, stderr: {}",
        String::from_utf8_lossy(&fresh_output.stderr)
    );
    assert!(
        obj_path.exists(),
        "fresh unreachable blob should survive gc grace period"
    );

    // Age the blob beyond the grace period, then GC should remove it.
    let old_time = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    let file = fs::File::create(&obj_path).unwrap();
    file.set_modified(old_time).unwrap();

    let aged_output = run_libra_command(
        &["--json", "maintenance", "run", "--task", "gc"],
        repo.path(),
    );
    assert!(
        aged_output.status.success(),
        "gc should pass on aged object, stderr: {}",
        String::from_utf8_lossy(&aged_output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&aged_output.stdout).expect("valid json");
    let gc_task = json
        .get("data")
        .and_then(|d| d.get("tasks"))
        .and_then(|t| t.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|t| t.get("task").and_then(|v| v.as_str()) == Some("gc"))
        })
        .expect("gc task in results");
    let removed = gc_task
        .get("objects_removed")
        .and_then(|v| v.as_u64())
        .expect("objects_removed field");
    assert!(
        removed >= 1,
        "aged unreachable objects should be removed, got task: {gc_task}"
    );
    assert!(
        !obj_path.exists(),
        "aged unreachable object file should be deleted"
    );
}

// ---------------------------------------------------------------------------
// Error Handling Tests (≥ 8 required)
// ---------------------------------------------------------------------------

#[test]

/// Tests `maintenance run` outside a repository returns fatal error.
/// Verifies proper error handling when not in a repository.
fn test_maintenance_outside_repository() {
    let temp = tempdir().unwrap();
    let output = run_libra_command(&["maintenance", "run"], temp.path());
    assert_eq!(
        output.status.code(),
        Some(128),
        "maintenance outside repo should exit 128"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal") || stderr.contains("not a libra repository"),
        "should show fatal error, stderr: {stderr}"
    );
}

#[test]

/// Tests `maintenance run` with an invalid flag returns usage error.
/// Verifies CLI argument validation.
fn test_maintenance_run_invalid_flag() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["maintenance", "run", "--invalid-flag"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(129),
        "invalid flag should exit 129"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error") || stderr.contains("unexpected"),
        "should report argument error, stderr: {stderr}"
    );
}

#[test]

/// Tests `maintenance register` outside a repository returns fatal error.
/// Verifies repo validation for register subcommand.
fn test_maintenance_register_outside_repo() {
    let temp = tempdir().unwrap();
    let output = run_libra_command(&["maintenance", "register"], temp.path());
    assert!(
        !output.status.success(),
        "register outside repo should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal") || stderr.contains("not a libra repository"),
        "should show fatal error, stderr: {stderr}"
    );
}

#[test]

/// Tests `maintenance status` outside a repository returns fatal error.
/// Verifies repo validation for status subcommand.
fn test_maintenance_status_outside_repo() {
    let temp = tempdir().unwrap();
    let output = run_libra_command(&["maintenance", "status"], temp.path());
    assert!(!output.status.success(), "status outside repo should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal") || stderr.contains("not a libra repository"),
        "should show fatal error, stderr: {stderr}"
    );
}

#[test]

/// Tests `maintenance run --task gc` actually removes dangling objects.
/// Verifies gc task performs expected cleanup.
fn test_maintenance_run_gc_removes_dangling() {
    let repo = create_committed_repo_via_cli();

    // Create dangling commit
    fs::write(repo.path().join("file2.txt"), "second file\n").unwrap();
    run_libra_command(&["add", "file2.txt"], repo.path());
    run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();
    run_libra_command(&["reset", "--hard", first_commit], repo.path());

    let output = run_libra_command(&["maintenance", "run", "--task", "gc"], repo.path());
    assert!(
        output.status.success(),
        "gc should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("removed") || stdout.contains("unreachable"),
        "gc should report removal, got: {stdout}"
    );
}

#[test]

/// Tests `maintenance run --json` returns structured output envelope.
/// Verifies JSON output format for the run subcommand.
fn test_maintenance_run_json_output() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(
        &["--json", "maintenance", "run", "--task", "gc"],
        repo.path(),
    );
    assert!(
        output.status.success(),
        "json run should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.is_empty(), "json run should produce stdout");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let data = json.get("data").expect("json should have data field");
    assert!(
        data.get("dry_run").is_some(),
        "json data should contain dry_run field"
    );
    assert!(
        data.get("tasks").is_some(),
        "json data should contain tasks field"
    );
}

/// Regression test: `maintenance run --json` must exit non-zero when a task fails.
///
/// Previously the JSON path returned `Ok(())` immediately after emitting the
/// `overall_success: false` payload, so automation received exit code 0 even
/// when tasks failed. The JSON should still be emitted, but the process must
/// return a non-zero exit code.
#[cfg(unix)]
#[test]
fn test_maintenance_run_json_exits_nonzero_on_task_failure() {
    if skip_permission_denied_test_if_root(
        "test_maintenance_run_json_exits_nonzero_on_task_failure",
    ) {
        return;
    }

    let repo = create_committed_repo_via_cli();

    // Make the objects directory unreadable so `run_gc` fails while listing
    // loose objects.
    let objects_dir = repo.path().join(".libra").join("objects");
    let mut permissions = fs::metadata(&objects_dir).unwrap().permissions();
    permissions.set_mode(0o000);
    fs::set_permissions(&objects_dir, permissions).unwrap();

    let output = run_libra_command(
        &["--json", "maintenance", "run", "--task", "gc"],
        repo.path(),
    );

    // Restore permissions so the temp directory can be cleaned up.
    let mut permissions = fs::metadata(&objects_dir).unwrap().permissions();
    permissions.set_mode(0o755);
    let _ = fs::set_permissions(&objects_dir, permissions);

    assert!(
        !output.status.success(),
        "json maintenance run should fail when task fails, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "json maintenance run should exit with code 1"
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let data = json.get("data").expect("json should have data field");
    assert_eq!(
        data.get("overall_success"),
        Some(&serde_json::Value::Bool(false)),
        "json should report overall_success: false"
    );
    let tasks = data
        .get("tasks")
        .expect("tasks array")
        .as_array()
        .expect("tasks is array");
    let gc_task = tasks
        .iter()
        .find(|t| t.get("task").and_then(|v| v.as_str()) == Some("gc"))
        .expect("gc task in results");
    assert_eq!(
        gc_task.get("success"),
        Some(&serde_json::Value::Bool(false)),
        "gc task should report success: false, got: {gc_task}"
    );
}

#[test]

/// Tests `maintenance run --task gc --task loose-objects` runs multiple tasks.
/// Verifies multiple --task flags are accepted.
fn test_maintenance_run_multiple_tasks() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(
        &[
            "maintenance",
            "run",
            "--task",
            "gc",
            "--task",
            "loose-objects",
        ],
        repo.path(),
    );
    assert!(
        output.status.success(),
        "multiple tasks should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("gc") && stdout.contains("loose-objects"),
        "output should mention both tasks, got: {stdout}"
    );
}

#[test]

/// Tests `maintenance unregister` on a repository that was never registered.
/// Verifies graceful handling of unregister without prior register.
fn test_maintenance_unregister_not_registered() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["maintenance", "unregister"], repo.path());
    assert!(
        output.status.success(),
        "unregister on unregistered repo should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]

/// Tests `maintenance run --dry-run` does not modify repository state.
/// Verifies that dry-run leaves objects untouched.
fn test_maintenance_dry_run_no_changes() {
    let repo = create_committed_repo_via_cli();

    // Count loose objects before
    let objects_dir = repo.path().join(".libra").join("objects");
    let before_count = count_loose_objects(&objects_dir);

    let output = run_libra_command(&["maintenance", "run", "--dry-run"], repo.path());
    assert!(
        output.status.success(),
        "dry-run should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Count loose objects after
    let after_count = count_loose_objects(&objects_dir);
    assert_eq!(
        before_count, after_count,
        "dry-run should not change object count"
    );
}

// ---------------------------------------------------------------------------
// Bug-fix regression tests
// ---------------------------------------------------------------------------

#[test]

/// Regression test: `maintenance run --task gc` must preserve file-backed stash
/// refs and reflogs as GC roots.
///
/// Stash refs live as plain files under `.libra/refs/stash` (not in the SQLite
/// reference table), so `collect_reachable_objects` previously missed them.
/// After a stash push, GC could delete the stash commit/tree/blobs, making
/// `stash apply`/`pop` fail with missing-object errors.
fn test_maintenance_gc_preserves_stash_objects() {
    let repo = create_committed_repo_via_cli();

    // Create a file change and stash it.
    fs::write(repo.path().join("stashed.txt"), "stash me\n").unwrap();
    run_libra_command(&["add", "stashed.txt"], repo.path());
    let stash_output = run_libra_command(&["stash", "push", "-m", "test stash"], repo.path());
    assert!(
        stash_output.status.success(),
        "stash push should succeed, stderr: {}",
        String::from_utf8_lossy(&stash_output.stderr)
    );

    // Verify the stash ref file exists on disk.
    let stash_ref_path = repo.path().join(".libra").join("refs").join("stash");
    assert!(
        stash_ref_path.exists(),
        "stash ref file should exist at {}",
        stash_ref_path.display()
    );

    // Verify the stash reflog exists on disk.
    let stash_log_path = repo
        .path()
        .join(".libra")
        .join("logs")
        .join("refs")
        .join("stash");
    assert!(
        stash_log_path.exists(),
        "stash reflog should exist at {}",
        stash_log_path.display()
    );

    // Run GC. The stash objects must not be deleted.
    let gc_output = run_libra_command(
        &["--json", "maintenance", "run", "--task", "gc"],
        repo.path(),
    );
    assert!(
        gc_output.status.success(),
        "gc should pass, stderr: {}",
        String::from_utf8_lossy(&gc_output.stderr)
    );

    // Verify stash list still works (stash objects are reachable).
    let list_output = run_libra_command(&["stash", "list"], repo.path());
    assert!(
        list_output.status.success(),
        "stash list should succeed after gc, stderr: {}",
        String::from_utf8_lossy(&list_output.stderr)
    );
    let stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(
        stdout.contains("test stash"),
        "stash entry should still be listed after gc, got: {stdout}"
    );

    // Verify stash apply works (the blob data is intact).
    let pop_output = run_libra_command(&["stash", "pop"], repo.path());
    assert!(
        pop_output.status.success(),
        "stash pop should succeed after gc, stderr: {}",
        String::from_utf8_lossy(&pop_output.stderr)
    );
    assert!(
        repo.path().join("stashed.txt").exists(),
        "stashed file should be restored after pop"
    );
}

#[test]

/// Regression test: `maintenance run --task gc` must treat previous stash
/// entries (stored in the stash reflog) as GC roots.
///
/// When a user creates multiple stashes, `stash pop`/`stash drop` removes the
/// top entry but leaves older entries referenced only via `logs/refs/stash`.
/// Without reflog scanning, GC can delete the second stash's commit/tree/blobs.
fn test_maintenance_gc_preserves_multiple_stash_entries() {
    let repo = create_committed_repo_via_cli();

    // Create first stash.
    fs::write(repo.path().join("first.txt"), "first\n").unwrap();
    run_libra_command(&["add", "first.txt"], repo.path());
    run_libra_command(&["stash", "push", "-m", "first stash"], repo.path());

    // Create second stash on top.
    fs::write(repo.path().join("second.txt"), "second\n").unwrap();
    run_libra_command(&["add", "second.txt"], repo.path());
    run_libra_command(&["stash", "push", "-m", "second stash"], repo.path());

    // Pop the top stash, so the first stash is only reachable via reflog.
    let pop_output = run_libra_command(&["stash", "pop"], repo.path());
    assert!(
        pop_output.status.success(),
        "stash pop should succeed, stderr: {}",
        String::from_utf8_lossy(&pop_output.stderr)
    );

    // Run GC and verify both stashes' data is preserved.
    let gc_output = run_libra_command(
        &["--json", "maintenance", "run", "--task", "gc"],
        repo.path(),
    );
    assert!(
        gc_output.status.success(),
        "gc should pass, stderr: {}",
        String::from_utf8_lossy(&gc_output.stderr)
    );

    // The first stash should still be listable via stash@{1}.
    let list_output = run_libra_command(&["stash", "list"], repo.path());
    assert!(
        list_output.status.success(),
        "stash list should succeed after gc, stderr: {}",
        String::from_utf8_lossy(&list_output.stderr)
    );
    let stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(
        stdout.contains("first stash"),
        "first stash entry should still be listed after gc, got: {stdout}"
    );

    // Pop the first stash to verify the blob is intact.
    let pop2_output = run_libra_command(&["stash", "pop"], repo.path());
    assert!(
        pop2_output.status.success(),
        "second stash pop should succeed after gc, stderr: {}",
        String::from_utf8_lossy(&pop2_output.stderr)
    );
    assert!(
        repo.path().join("first.txt").exists(),
        "first stashed file should be restored after pop"
    );
}

#[test]

/// Regression test: `maintenance run --task pack-refs` must skip `.lock` files
/// under `refs/heads`.
///
/// Concurrent ref updates create temporary lock files (e.g.
/// `refs/heads/main.lock`). Without filtering, `collect_refs` treats the lock
/// file as a valid ref and writes a bogus entry into `packed-refs`. Worse,
/// `remove_packed_refs` can delete the lock file if its content matches,
/// breaking the concurrent update.
fn test_maintenance_pack_refs_skips_lock_files() {
    let repo = create_committed_repo_via_cli();
    let main_hash = rev_parse(repo.path(), "main");

    // Materialize a loose ref.
    let refs_heads = repo.path().join(".libra").join("refs").join("heads");
    fs::create_dir_all(&refs_heads).unwrap();
    fs::write(refs_heads.join("main"), format!("{main_hash}\n")).unwrap();

    // Create a fake lock file alongside the real ref.
    fs::write(refs_heads.join("main.lock"), format!("{main_hash}\n")).unwrap();

    // Create a lock file in a subdirectory.
    let nested_dir = refs_heads.join("feature");
    fs::create_dir_all(&nested_dir).unwrap();
    fs::write(nested_dir.join("branch.lock"), format!("{main_hash}\n")).unwrap();

    let output = run_libra_command(&["maintenance", "run", "--task", "pack-refs"], repo.path());
    assert!(
        output.status.success(),
        "pack-refs should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The packed-refs file must NOT contain any .lock entry.
    let packed_refs = repo.path().join(".libra").join("packed-refs");
    let content = fs::read_to_string(&packed_refs).unwrap();
    assert!(
        !content.contains(".lock"),
        "packed-refs must not contain lock file entries, got: {content}"
    );
    assert!(
        content.contains("refs/heads/main"),
        "packed-refs must still contain the real ref, got: {content}"
    );

    // Lock files must NOT be deleted by pack-refs.
    assert!(
        refs_heads.join("main.lock").exists(),
        "main.lock must survive pack-refs"
    );
    assert!(
        nested_dir.join("branch.lock").exists(),
        "nested branch.lock must survive pack-refs"
    );
}

#[test]

/// Regression test: `maintenance run --task pack-refs` must validate ref names
/// and object hashes before inserting them into packed-refs.
///
/// Without validation, a file with a bogus name or non-hex content under
/// `refs/heads` produces a corrupted packed-refs entry.
fn test_maintenance_pack_refs_validates_refs() {
    let repo = create_committed_repo_via_cli();
    let main_hash = rev_parse(repo.path(), "main");

    // Create a valid loose ref.
    let refs_heads = repo.path().join(".libra").join("refs").join("heads");
    fs::create_dir_all(&refs_heads).unwrap();
    fs::write(refs_heads.join("main"), format!("{main_hash}\n")).unwrap();

    // Create a file with non-hex content that should be rejected.
    fs::write(refs_heads.join("bogus"), "not-a-hash\n").unwrap();

    // Create an empty file that should be skipped.
    fs::write(refs_heads.join("empty"), "\n").unwrap();

    let output = run_libra_command(&["maintenance", "run", "--task", "pack-refs"], repo.path());
    assert!(
        output.status.success(),
        "pack-refs should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let packed_refs = repo.path().join(".libra").join("packed-refs");
    let content = fs::read_to_string(&packed_refs).unwrap();
    assert!(
        content.contains("refs/heads/main"),
        "packed-refs must contain valid ref, got: {content}"
    );
    assert!(
        !content.contains("bogus") && !content.contains("empty"),
        "packed-refs must not contain invalid ref entries, got: {content}"
    );
}

#[test]

/// Regression test: `maintenance run --task gc` must treat packed-refs and
/// file-backed loose refs as GC roots.
///
/// When `pack-refs` removes loose ref files under `.libra/refs/heads/`, the
/// only remaining copy of those refs lives in `.libra/packed-refs`. Without
/// `collect_reachable_objects` reading `packed-refs`, a later `gc` would see
/// zero refs pointing to those commits and could delete their objects once the
/// mtime grace period expires. File-backed loose refs that were never packed
/// (e.g. freshly created refs) must also be reachable.
fn test_maintenance_gc_preserves_packed_refs_reachability() {
    let repo = create_committed_repo_via_cli();

    // Record the commit hashes in the history.
    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let base_commit = stdout.lines().next().unwrap().trim().to_string();

    // Create a second commit on a branch.
    run_libra_command(&["branch", "side"], repo.path());
    run_libra_command(&["switch", "side"], repo.path());
    fs::write(repo.path().join("side.txt"), "side content\n").unwrap();
    run_libra_command(&["add", "side.txt"], repo.path());
    run_libra_command(&["commit", "-m", "side commit", "--no-verify"], repo.path());

    let side_hash = rev_parse(repo.path(), "HEAD");

    // Reset main to the base commit so the side commit is not reachable
    // from HEAD or any SQLite-tracked ref.
    run_libra_command(&["switch", "main"], repo.path());
    run_libra_command(&["reset", "--hard", &base_commit], repo.path());

    // Materialize a file-backed loose ref for the side branch under
    // .libra/refs/heads/side so pack-refs can process it.
    let refs_heads = repo.path().join(".libra").join("refs").join("heads");
    fs::create_dir_all(&refs_heads).unwrap();
    fs::write(refs_heads.join("side"), format!("{side_hash}\n")).unwrap();

    // Run pack-refs to collapse the loose ref into packed-refs and delete
    // the loose file. After this, the side branch ref exists *only* in
    // packed-refs.
    let pack_refs_output =
        run_libra_command(&["maintenance", "run", "--task", "pack-refs"], repo.path());
    assert!(
        pack_refs_output.status.success(),
        "pack-refs should pass, stderr: {}",
        String::from_utf8_lossy(&pack_refs_output.stderr)
    );

    let packed_refs_path = repo.path().join(".libra").join("packed-refs");
    assert!(
        packed_refs_path.exists(),
        "packed-refs must exist after pack-refs task"
    );
    let packed_content = fs::read_to_string(&packed_refs_path).unwrap();
    assert!(
        packed_content.contains("refs/heads/side"),
        "packed-refs must contain side branch, got: {packed_content}"
    );
    assert!(
        !refs_heads.join("side").exists(),
        "loose refs/heads/side must be deleted after packing"
    );

    // Age all loose objects past the GC grace period so that GC would
    // delete them if they were unreachable.
    let objects_dir = repo.path().join(".libra").join("objects");
    let old_time = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    for entry in WalkDir::new(&objects_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let file = fs::File::open(entry.path()).unwrap();
        file.set_modified(old_time).unwrap();
    }

    // Run GC. The side commit must survive because packed-refs keeps it
    // reachable.
    let gc_output = run_libra_command(
        &["--json", "maintenance", "run", "--task", "gc"],
        repo.path(),
    );
    assert!(
        gc_output.status.success(),
        "gc should pass, stderr: {}",
        String::from_utf8_lossy(&gc_output.stderr)
    );

    // Verify the side commit is still readable after GC.
    let cat_output = run_libra_command(&["cat-file", "-t", &side_hash], repo.path());
    assert!(
        cat_output.status.success(),
        "side commit ({side_hash}) should still be readable after gc (reachable via packed-refs), stderr: {}",
        String::from_utf8_lossy(&cat_output.stderr)
    );
    let cat_stdout = String::from_utf8_lossy(&cat_output.stdout);
    assert!(
        cat_stdout.contains("commit"),
        "side commit object type should be readable, got: {cat_stdout}"
    );
}

#[test]

/// Regression test: `maintenance run --task incremental-repack` must be able to
/// rename the consolidated pack into the allocated path without a placeholder
/// file blocking the rename.
///
/// Previously `allocate_unique_pack_path` pre-created an empty file via
/// `create_new(true)`, causing `std::fs::rename` to fail on Windows where the
/// destination must not exist. After the fix, the path is reserved but not
/// pre-created, and the consolidated pack is renamed into place cleanly.
fn test_maintenance_incremental_repack_path_not_precreated() {
    let repo = create_committed_repo_via_cli();
    let pack_dir = repo.path().join(".libra").join("objects").join("pack");

    // Run incremental-repack on a repo with few packs — it will try to
    // allocate a name and rename into it.
    let output = run_libra_command(
        &["maintenance", "run", "--task", "incremental-repack"],
        repo.path(),
    );
    assert!(
        output.status.success(),
        "incremental-repack should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify no orphaned placeholder files remain in the pack directory.
    // The only .pack files should be actual pack files with matching .idx.
    if pack_dir.exists() {
        for entry in fs::read_dir(&pack_dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "pack") {
                // Every .pack must be a real pack (non-empty, with header).
                let metadata = fs::metadata(&path).unwrap();
                assert!(
                    metadata.len() > 0,
                    "pack file {} must not be empty (placeholder)",
                    path.display()
                );
                // Must have a matching .idx.
                let idx = path.with_extension("idx");
                assert!(
                    idx.exists(),
                    "pack file {} must have matching index",
                    path.display()
                );
            }
        }
    }
}

/// Helper: create enough packs so incremental-repack actually repacks.
fn create_many_packs(repo: &std::path::Path) {
    for pack_idx in 0..5 {
        for file_idx in 0..105 {
            fs::write(
                repo.join(format!("p{pack_idx}_f{file_idx:03}.txt")),
                format!("c {pack_idx} {file_idx}\n"),
            )
            .unwrap();
        }
        run_libra_command(&["add", "."], repo);
        run_libra_command(
            &["commit", "-m", &format!("pc {pack_idx}"), "--no-verify"],
            repo,
        );

        let objects_dir = repo.join(".libra").join("objects");
        let old_time = SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60);
        for entry in WalkDir::new(&objects_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let file = fs::File::open(entry.path()).unwrap();
            file.set_modified(old_time).unwrap();
        }

        let output = run_libra_command(&["maintenance", "run", "--task", "loose-objects"], repo);
        assert!(
            output.status.success(),
            "loose-objects run {pack_idx} should pass, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]

/// Regression test: `maintenance run --task incremental-repack` streams pack
/// objects instead of loading them all into memory.
///
/// Previously `read_pack_objects` returned a `Vec` containing every decoded
/// object from a pack, risking OOM on large packs. After the fix, objects are
/// streamed through a callback and only the dedup set grows with the unique
/// object count.
fn test_maintenance_incremental_repack_streams_objects() {
    let repo = create_committed_repo_via_cli();

    // Create 5 packs so incremental-repack triggers.
    create_many_packs(repo.path());

    let pack_dir = repo.path().join(".libra").join("objects").join("pack");
    let initial_packs: Vec<_> = fs::read_dir(&pack_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "pack"))
        .collect();
    assert!(
        initial_packs.len() >= 5,
        "should have at least 5 packs before repack, got {}",
        initial_packs.len()
    );

    // Run incremental repack.
    let output = run_libra_command(
        &[
            "--json",
            "maintenance",
            "run",
            "--task",
            "incremental-repack",
        ],
        repo.path(),
    );
    assert!(
        output.status.success(),
        "incremental-repack should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let tasks = json
        .get("data")
        .and_then(|d| d.get("tasks"))
        .expect("tasks array");
    let repack_task = tasks
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t.get("task").and_then(|v| v.as_str()) == Some("incremental-repack"))
        .expect("incremental-repack task");
    let packs_repacked = repack_task
        .get("packs_repacked")
        .and_then(|v| v.as_u64())
        .unwrap();
    assert!(
        packs_repacked >= 5,
        "should repack all packs, got task: {repack_task}"
    );

    // Verify a single consolidated .pack remains and is valid.
    let final_packs: Vec<_> = fs::read_dir(&pack_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "pack"))
        .collect();
    assert_eq!(
        final_packs.len(),
        1,
        "should be left with exactly one consolidated .pack"
    );

    // Verify history remains readable.
    let log_output = run_libra_command(&["log", "--pretty=%s"], repo.path());
    assert!(
        log_output.status.success(),
        "log should succeed after repack, stderr: {}",
        String::from_utf8_lossy(&log_output.stderr)
    );
    let log_stdout = String::from_utf8_lossy(&log_output.stdout);
    for pack_idx in 0..5 {
        assert!(
            log_stdout.contains(&format!("pc {pack_idx}")),
            "history must contain commit for pack {pack_idx}, got: {log_stdout}"
        );
    }
}

#[test]

/// Regression test: `collect_file_ref_hashes` must fail GC when a file-backed
/// ref contains a non-hex object hash instead of silently skipping it.
///
/// Previously the helper used `if let Some(hash) = parse_object_hash(…)` and
/// dropped the ref on parse failure, letting GC proceed with an incomplete
/// root set. Now it returns a fatal error so that GC aborts rather than
/// risking deletion of objects reachable only from a malformed ref.
fn test_maintenance_gc_fails_on_malformed_ref_content() {
    let repo = create_committed_repo_via_cli();

    // Materialize a file-backed ref with bogus (non-hex) content.
    let refs_heads = repo.path().join(".libra").join("refs").join("heads");
    fs::create_dir_all(&refs_heads).unwrap();
    fs::write(refs_heads.join("corrupt-branch"), "not-a-hex-hash\n").unwrap();

    // Age all loose objects past the GC grace period.
    let objects_dir = repo.path().join(".libra").join("objects");
    let old_time = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    for entry in WalkDir::new(&objects_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let file = fs::File::open(entry.path()).unwrap();
        file.set_modified(old_time).unwrap();
    }

    // GC must fail when a ref file contains an invalid hash. Use JSON
    // output so we can inspect the per-task error message.
    let gc_output = run_libra_command(
        &["--json", "maintenance", "run", "--task", "gc"],
        repo.path(),
    );
    assert!(
        !gc_output.status.success(),
        "gc should fail when ref file is malformed, but exited 0"
    );

    let json: serde_json::Value = serde_json::from_slice(&gc_output.stdout).expect("valid json");
    let tasks = json
        .get("data")
        .and_then(|d| d.get("tasks"))
        .expect("tasks array")
        .as_array()
        .expect("tasks is array");
    let gc_task = tasks
        .iter()
        .find(|t| t.get("task").and_then(|v| v.as_str()) == Some("gc"))
        .expect("gc task in results");
    assert_eq!(
        gc_task.get("success"),
        Some(&serde_json::Value::Bool(false)),
        "gc task must report success: false, got: {gc_task}"
    );
    let msg = gc_task
        .get("message")
        .and_then(|v| v.as_str())
        .expect("gc task message");
    assert!(
        msg.contains("corrupt-branch")
            && (msg.contains("does not contain a valid object hash")
                || msg.contains("failed to read loose ref")),
        "gc error message must mention the malformed ref, got: {msg}"
    );
}

#[test]

/// Regression test: `maintenance run --task pack-refs` must successfully
/// replace an existing `packed-refs` file on all platforms.
///
/// On Unix `std::fs::rename` atomically replaces the destination file, but
/// on Windows the rename fails when the destination already exists. After
/// the fix the old `packed-refs` is removed before the rename so that a
/// second `pack-refs` run succeeds on Windows-supported installs.
fn test_maintenance_pack_refs_replaces_existing_file() {
    let repo = create_committed_repo_via_cli();
    let main_hash = rev_parse(repo.path(), "main");

    // Pre-populate packed-refs as if a previous run already wrote it.
    let libra_dir = repo.path().join(".libra");
    let packed_refs_path = libra_dir.join("packed-refs");
    fs::write(
        &packed_refs_path,
        "# packed-refs with peeled tags\ndeadbeef refs/heads/legacy\n",
    )
    .unwrap();

    // Create a new loose ref to be packed.
    let refs_heads = libra_dir.join("refs").join("heads");
    fs::create_dir_all(&refs_heads).unwrap();
    fs::write(refs_heads.join("main"), format!("{main_hash}\n")).unwrap();

    // First pack-refs run — succeeds, legacy entry persists.
    let output1 = run_libra_command(&["maintenance", "run", "--task", "pack-refs"], repo.path());
    assert!(
        output1.status.success(),
        "first pack-refs should pass, stderr: {}",
        String::from_utf8_lossy(&output1.stderr)
    );

    let content1 = fs::read_to_string(&packed_refs_path).unwrap();
    assert!(
        content1.contains("refs/heads/legacy"),
        "legacy entry must be preserved after first pack-refs, got: {content1}"
    );
    assert!(
        content1.contains("refs/heads/main"),
        "main entry must be present after first pack-refs, got: {content1}"
    );

    // Second pack-refs run — must succeed even though packed-refs already
    // exists (regression test for Windows rename failure).
    let output2 = run_libra_command(&["maintenance", "run", "--task", "pack-refs"], repo.path());
    assert!(
        output2.status.success(),
        "second pack-refs must pass when packed-refs already exists, stderr: {}",
        String::from_utf8_lossy(&output2.stderr)
    );

    // Both entries must still be present after the second run.
    let content2 = fs::read_to_string(&packed_refs_path).unwrap();
    assert!(
        content2.contains("refs/heads/legacy"),
        "legacy entry must survive second pack-refs, got: {content2}"
    );
    assert!(
        content2.contains("refs/heads/main"),
        "main entry must survive second pack-refs, got: {content2}"
    );

    // No stale temp file must be left behind.
    assert!(
        !libra_dir.join("packed-refs.tmp").exists(),
        "temporary packed-refs file must be cleaned up"
    );
}

#[test]

/// Regression test: `maintenance run --task pack-refs` must not leave a stale
/// backup file behind after a successful run.
///
/// After the atomic install fix, the old packed-refs is renamed to a `.bak`
/// backup, the new file is installed, and the backup is removed on success.
/// This prevents data loss if the install step fails while also ensuring no
/// stale `.bak` files accumulate.
fn test_maintenance_pack_refs_no_stale_backup_after_success() {
    let repo = create_committed_repo_via_cli();
    let main_hash = rev_parse(repo.path(), "main");

    // Pre-populate packed-refs with an existing entry.
    let libra_dir = repo.path().join(".libra");
    let packed_refs_path = libra_dir.join("packed-refs");
    fs::write(
        &packed_refs_path,
        "# packed-refs with peeled tags\ndeadbeef refs/heads/old\n",
    )
    .unwrap();

    // Create a loose ref to pack.
    let refs_heads = libra_dir.join("refs").join("heads");
    fs::create_dir_all(&refs_heads).unwrap();
    fs::write(refs_heads.join("main"), format!("{main_hash}\n")).unwrap();

    let output = run_libra_command(&["maintenance", "run", "--task", "pack-refs"], repo.path());
    assert!(
        output.status.success(),
        "pack-refs should pass, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The backup file must be cleaned up after a successful install.
    assert!(
        !libra_dir.join("packed-refs.bak").exists(),
        "backup packed-refs.bak must be removed after successful install"
    );

    // The packed-refs must contain both entries.
    let content = fs::read_to_string(&packed_refs_path).unwrap();
    assert!(
        content.contains("refs/heads/old"),
        "existing entry must survive"
    );
    assert!(
        content.contains("refs/heads/main"),
        "new entry must be present"
    );
}

#[test]

/// Regression test: `maintenance run --task gc` must abort when the stash
/// reflog (`logs/refs/stash`) exists but cannot be read.
///
/// Previously `if let Ok(content) = fs::read_to_string(…)` silently skipped
/// an unreadable stash reflog, letting GC proceed with only the current
/// `refs/stash` entry. Older stash entries reachable only through the reflog
/// could then be pruned.
#[cfg(unix)]
fn test_maintenance_gc_fails_on_unreadable_stash_reflog() {
    if skip_permission_denied_test_if_root("test_maintenance_gc_fails_on_unreadable_stash_reflog") {
        return;
    }

    let repo = create_committed_repo_via_cli();

    // Create a stash to populate the reflog.
    fs::write(repo.path().join("stashed.txt"), "stash content\n").unwrap();
    run_libra_command(&["add", "stashed.txt"], repo.path());
    let stash_output = run_libra_command(&["stash", "push", "-m", "test stash"], repo.path());
    assert!(
        stash_output.status.success(),
        "stash push should succeed, stderr: {}",
        String::from_utf8_lossy(&stash_output.stderr)
    );

    let stash_log_path = repo
        .path()
        .join(".libra")
        .join("logs")
        .join("refs")
        .join("stash");
    assert!(
        stash_log_path.exists(),
        "stash reflog should exist at {}",
        stash_log_path.display()
    );

    // Make the reflog file unreadable.
    let mut permissions = fs::metadata(&stash_log_path).unwrap().permissions();
    permissions.set_mode(0o000);
    fs::set_permissions(&stash_log_path, permissions).unwrap();

    // GC must fail because the stash reflog exists but is unreadable.
    // Use JSON output to inspect the per-task error message.
    let gc_output = run_libra_command(
        &["--json", "maintenance", "run", "--task", "gc"],
        repo.path(),
    );
    assert!(
        !gc_output.status.success(),
        "gc should fail when stash reflog is unreadable, but exited 0"
    );

    let json: serde_json::Value = serde_json::from_slice(&gc_output.stdout).expect("valid json");
    let tasks = json
        .get("data")
        .and_then(|d| d.get("tasks"))
        .expect("tasks array")
        .as_array()
        .expect("tasks is array");
    let gc_task = tasks
        .iter()
        .find(|t| t.get("task").and_then(|v| v.as_str()) == Some("gc"))
        .expect("gc task in results");
    assert_eq!(
        gc_task.get("success"),
        Some(&serde_json::Value::Bool(false)),
        "gc task must report success: false, got: {gc_task}"
    );
    let msg = gc_task
        .get("message")
        .and_then(|v| v.as_str())
        .expect("gc task message");
    assert!(
        msg.contains("stash reflog") || msg.contains("failed to read"),
        "gc error must mention stash reflog, got: {msg}"
    );

    // Restore permissions for cleanup.
    let mut permissions = fs::metadata(&stash_log_path).unwrap().permissions();
    permissions.set_mode(0o644);
    let _ = fs::set_permissions(&stash_log_path, permissions);
}

#[test]

/// Regression test: `maintenance run --task gc` must abort when a non-comment
/// packed-refs line contains an invalid object hash.
///
/// Previously `if let Some(hash) = parse_object_hash(…)` silently skipped such
/// lines. After pack-refs deletes the loose ref files, a branch or tag that
/// exists only in packed-refs would be omitted from GC roots and its old loose
/// objects could be deleted.
fn test_maintenance_gc_fails_on_malformed_packed_refs_entry() {
    let repo = create_committed_repo_via_cli();

    // Pre-populate packed-refs with a malformed entry.
    let libra_dir = repo.path().join(".libra");
    let packed_refs_path = libra_dir.join("packed-refs");
    fs::write(
        &packed_refs_path,
        "# packed-refs with peeled tags\nnot-a-hex-hash refs/heads/broken\n",
    )
    .unwrap();

    // GC must fail because packed-refs contains a malformed entry.
    let gc_output = run_libra_command(
        &["--json", "maintenance", "run", "--task", "gc"],
        repo.path(),
    );
    assert!(
        !gc_output.status.success(),
        "gc should fail when packed-refs is malformed, but exited 0"
    );

    let json: serde_json::Value = serde_json::from_slice(&gc_output.stdout).expect("valid json");
    let tasks = json
        .get("data")
        .and_then(|d| d.get("tasks"))
        .expect("tasks array")
        .as_array()
        .expect("tasks is array");
    let gc_task = tasks
        .iter()
        .find(|t| t.get("task").and_then(|v| v.as_str()) == Some("gc"))
        .expect("gc task in results");
    assert_eq!(
        gc_task.get("success"),
        Some(&serde_json::Value::Bool(false)),
        "gc task must report success: false, got: {gc_task}"
    );
    let msg = gc_task
        .get("message")
        .and_then(|v| v.as_str())
        .expect("gc task message");
    assert!(
        msg.contains("packed-refs") && msg.contains("invalid object hash"),
        "gc error must mention packed-refs invalid hash, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Count loose objects in the objects directory.
fn count_loose_objects(objects_dir: &std::path::Path) -> usize {
    let mut count = 0;
    for entry in fs::read_dir(objects_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy();
        if name.len() != 2 {
            continue;
        }
        for sub in fs::read_dir(&path).unwrap() {
            let sub = sub.unwrap();
            if sub.path().is_file() {
                count += 1;
            }
        }
    }
    count
}
