use super::prelude::*;

pub(crate) fn scenario_restore_reset_diff(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    fs::write(repo.join("tracked.txt"), "modified\n").context("modify tracked file")?;
    assert_stdout_contains(&ctx.command(&["diff"], repo.clone(), true)?, "modified")?;
    assert_stdout_contains(
        &ctx.command(&["diff", "tracked.txt"], repo.clone(), true)?,
        "modified",
    )?;
    assert_stdout_contains(
        &ctx.command(&["diff", "--name-only"], repo.clone(), true)?,
        "tracked.txt",
    )?;
    assert_stdout_contains(
        &ctx.command(&["diff", "--stat"], repo.clone(), true)?,
        "tracked.txt",
    )?;
    assert_stdout_contains(
        &ctx.command(&["diff", "--raw"], repo.clone(), true)?,
        "M\ttracked.txt",
    )?;
    assert_stdout_contains(
        &ctx.command(&["diff", "-w", "-U0"], repo.clone(), true)?,
        "modified",
    )?;
    let exit_diff = ctx.command(&["diff", "--exit-code"], repo.clone(), false)?;
    if exit_diff.status.code() != Some(1) {
        bail!(
            "diff --exit-code should exit 1 when changes exist, got {:?}",
            exit_diff.status.code()
        );
    }
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    assert_stdout_contains(
        &ctx.command(&["diff", "--staged"], repo.clone(), true)?,
        "modified",
    )?;
    assert_stdout_contains(
        &ctx.command(&["diff", "--staged", "--name-status"], repo.clone(), true)?,
        "M\ttracked.txt",
    )?;
    ctx.command(&["restore", "--staged", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["restore", "--worktree", "tracked.txt"],
        repo.clone(),
        true,
    )?;
    fs::write(repo.join("tracked.txt"), "restore pathspec file\n")
        .context("modify tracked for restore pathspec file")?;
    fs::write(repo.join("restore-paths.txt"), "tracked.txt\n")
        .context("write restore pathspec file")?;
    let restore_from_file = ctx.command(
        &[
            "--json",
            "restore",
            "--pathspec-from-file=restore-paths.txt",
        ],
        repo.clone(),
        true,
    )?;
    assert_json_ok(&restore_from_file, "restore")?;
    let restore_from_file_content =
        fs::read_to_string(repo.join("tracked.txt")).context("read restored pathspec file")?;
    if restore_from_file_content != "base\n" {
        bail!("restore --pathspec-from-file did not restore base content");
    }
    fs::write(repo.join("tracked.txt"), "restore pathspec nul\n")
        .context("modify tracked for NUL restore pathspec file")?;
    fs::write(repo.join("restore-paths-nul.txt"), b"tracked.txt\0")
        .context("write NUL restore pathspec file")?;
    let restore_from_nul = ctx.command(
        &[
            "--json",
            "restore",
            "--pathspec-from-file=restore-paths-nul.txt",
            "--pathspec-file-nul",
        ],
        repo.clone(),
        true,
    )?;
    assert_json_ok(&restore_from_nul, "restore")?;
    let restore_from_nul_content =
        fs::read_to_string(repo.join("tracked.txt")).context("read restored NUL pathspec file")?;
    if restore_from_nul_content != "base\n" {
        bail!("restore --pathspec-file-nul did not restore base content");
    }
    ctx.command(&["reset", "--hard", "HEAD"], repo.clone(), true)?;
    let restored = fs::read_to_string(repo.join("tracked.txt")).context("read restored file")?;
    if restored != "base\n" {
        bail!("restore did not return tracked.txt to base content: {restored:?}");
    }
    fs::write(repo.join("tracked.txt"), "second\n").context("modify tracked second")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(&["reset", "HEAD", "--", "tracked.txt"], repo.clone(), true)?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    fs::write(repo.join("reset-paths.txt"), "tracked.txt\n")
        .context("write reset pathspec file")?;
    let reset_from_file = ctx.command(
        &["--json", "reset", "--pathspec-from-file=reset-paths.txt"],
        repo.clone(),
        true,
    )?;
    assert_json_ok(&reset_from_file, "reset")?;
    assert_stdout_contains(&reset_from_file, "\"files_unstaged\": 1")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    fs::write(repo.join("reset-paths-nul.txt"), b"tracked.txt\0")
        .context("write NUL reset pathspec file")?;
    let reset_from_nul = ctx.command(
        &[
            "--json",
            "reset",
            "--pathspec-from-file=reset-paths-nul.txt",
            "--pathspec-file-nul",
        ],
        repo.clone(),
        true,
    )?;
    assert_json_ok(&reset_from_nul, "reset")?;
    assert_stdout_contains(&reset_from_nul, "\"pathspecs\": [")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    let reset_no_refresh = ctx.command(
        &["--json", "reset", "--no-refresh", "HEAD"],
        repo.clone(),
        true,
    )?;
    assert_json_ok(&reset_no_refresh, "reset")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "second", "--no-verify"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(
        &ctx.command(
            &["diff", "--old", "HEAD~1", "--new", "HEAD", "--numstat"],
            repo.clone(),
            true,
        )?,
        "tracked.txt",
    )?;
    ctx.command(&["reset", "--soft", "HEAD~1"], repo.clone(), true)?;
    ctx.command(&["reset", "--mixed", "HEAD"], repo.clone(), true)?;
    ctx.command(&["reset", "--hard", "HEAD"], repo.clone(), true)?;
    fs::write(repo.join("keep.txt"), "keep\n").context("write keep reset file")?;
    ctx.command(&["add", "keep.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "keep reset target", "--no-verify"],
        repo.clone(),
        true,
    )?;
    fs::write(repo.join("tracked.txt"), "local keep\n").context("write keep-preserved change")?;
    let keep_reset = ctx.command(&["--json", "reset", "--keep", "HEAD~1"], repo.clone(), true)?;
    assert_json_ok(&keep_reset, "reset")?;
    assert_stdout_contains(&keep_reset, "\"mode\": \"keep\"")?;
    let kept = fs::read_to_string(repo.join("tracked.txt")).context("read keep-preserved file")?;
    if kept != "local keep\n" {
        bail!("reset --keep did not preserve tracked.txt: {kept:?}");
    }
    ctx.command(&["reset", "--hard", "HEAD"], repo.clone(), true)?;
    fs::write(repo.join("merge.txt"), "merge\n").context("write merge reset file")?;
    ctx.command(&["add", "merge.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "merge reset target", "--no-verify"],
        repo.clone(),
        true,
    )?;
    let merge_reset = ctx.command(
        &["--json", "reset", "--merge", "HEAD~1"],
        repo.clone(),
        true,
    )?;
    assert_json_ok(&merge_reset, "reset")?;
    assert_stdout_contains(&merge_reset, "\"mode\": \"merge\"")?;
    if repo.join("merge.txt").exists() {
        bail!("reset --merge should remove merge.txt when resetting to HEAD~1");
    }
    fs::write(repo.join("overlay.txt"), "overlay\n").context("write overlay restore file")?;
    ctx.command(&["add", "overlay.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "overlay restore target", "--no-verify"],
        repo.clone(),
        true,
    )?;
    let overlay_restore = ctx.command(
        &[
            "--json",
            "restore",
            "--source",
            "HEAD~1",
            "--overlay",
            "overlay.txt",
        ],
        repo.clone(),
        true,
    )?;
    assert_json_ok(&overlay_restore, "restore")?;
    let overlay_content =
        fs::read_to_string(repo.join("overlay.txt")).context("read overlay-preserved file")?;
    if overlay_content != "overlay\n" {
        bail!("restore --overlay should preserve overlay.txt: {overlay_content:?}");
    }
    let no_overlay_restore = ctx.command(
        &["--json", "restore", "--source", "HEAD~1", "overlay.txt"],
        repo.clone(),
        true,
    )?;
    assert_json_ok(&no_overlay_restore, "restore")?;
    if repo.join("overlay.txt").exists() {
        bail!("default restore no-overlay should remove overlay.txt");
    }
    ctx.command(&["reset", "--hard", "HEAD"], repo.clone(), true)?;
    fs::write(repo.join("orig.txt"), "l1\nl2\nl3\nl4\n").context("write rename source")?;
    ctx.command(&["add", "orig.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "rename source", "--no-verify"],
        repo.clone(),
        true,
    )?;
    fs::remove_file(repo.join("orig.txt")).context("remove rename source")?;
    fs::write(repo.join("new.txt"), "l1\nl2\nl3\nCHANGED\n").context("write rename dest")?;
    assert_stdout_contains(
        &ctx.command(&["diff", "-M70", "--name-status"], repo.clone(), true)?,
        "R075\torig.txt\tnew.txt",
    )?;
    ctx.command(&["reset", "--hard", "HEAD"], repo.clone(), true)?;
    assert_json_ok(
        &ctx.command(&["--json", "diff"], repo.clone(), true)?,
        "diff",
    )?;
    let bad_diff = ctx.command(
        &["diff", "--old", "no-such-revision", "--new", "HEAD"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_diff, "invalid revision")?;
    let bad_restore = ctx.command(&["restore", "nonexistent.txt"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_restore, "pathspec")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;

    let conflict_repo = ctx.repo("restore-conflict");
    create_committed_repo(ctx, &conflict_repo)?;
    ctx.command(&["switch", "-c", "feature"], conflict_repo.clone(), true)?;
    fs::write(conflict_repo.join("tracked.txt"), "feature\n").context("write feature conflict")?;
    ctx.command(&["add", "tracked.txt"], conflict_repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "feature conflict", "--no-verify"],
        conflict_repo.clone(),
        true,
    )?;
    ctx.command(&["switch", "main"], conflict_repo.clone(), true)?;
    fs::write(conflict_repo.join("tracked.txt"), "main\n").context("write main conflict")?;
    ctx.command(&["add", "tracked.txt"], conflict_repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "main conflict", "--no-verify"],
        conflict_repo.clone(),
        true,
    )?;
    let conflict = ctx.command(&["merge", "feature"], conflict_repo.clone(), false)?;
    assert_lbr_or_text(&conflict, "conflict")?;
    let conflicted =
        fs::read_to_string(conflict_repo.join("tracked.txt")).context("read restore conflict")?;
    if !conflicted.contains("<<<<<<<") {
        bail!("restore conflict fixture did not write conflict markers: {conflicted}");
    }
    let blocked = ctx.command(
        &["--json", "restore", "tracked.txt"],
        conflict_repo.clone(),
        false,
    )?;
    if blocked.status.code() != Some(128) {
        bail!(
            "plain restore over unmerged path should exit 128, got {:?}",
            blocked.status.code()
        );
    }
    assert_lbr_or_text(&blocked, "is unmerged")?;
    let ignored = ctx.command(
        &[
            "--json",
            "restore",
            "--ignore-unmerged",
            "--source",
            "HEAD",
            "tracked.txt",
        ],
        conflict_repo.clone(),
        true,
    )?;
    assert_json_ok(&ignored, "restore")?;
    let ignored_content = fs::read_to_string(conflict_repo.join("tracked.txt"))
        .context("read ignored restore conflict")?;
    if ignored_content != conflicted {
        bail!("--ignore-unmerged should leave conflict file untouched");
    }
    let ours = ctx.command(
        &["--json", "restore", "--ours", "tracked.txt"],
        conflict_repo.clone(),
        true,
    )?;
    assert_json_ok(&ours, "restore")?;
    let ours_content =
        fs::read_to_string(conflict_repo.join("tracked.txt")).context("read ours restore")?;
    if ours_content != "main\n" {
        bail!("restore --ours wrote unexpected content: {ours_content:?}");
    }
    let still_unmerged = ctx.command(&["restore", "tracked.txt"], conflict_repo.clone(), false)?;
    assert_lbr_or_text(&still_unmerged, "is unmerged")?;
    let theirs = ctx.command(
        &["--json", "restore", "--theirs", "tracked.txt"],
        conflict_repo.clone(),
        true,
    )?;
    assert_json_ok(&theirs, "restore")?;
    let theirs_content =
        fs::read_to_string(conflict_repo.join("tracked.txt")).context("read theirs restore")?;
    if theirs_content != "feature\n" {
        bail!("restore --theirs wrote unexpected content: {theirs_content:?}");
    }
    let merge_markers = ctx.command(
        &["--json", "restore", "--merge", "tracked.txt"],
        conflict_repo.clone(),
        true,
    )?;
    assert_json_ok(&merge_markers, "restore")?;
    let merge_marker_content =
        fs::read_to_string(conflict_repo.join("tracked.txt")).context("read restore --merge")?;
    if !merge_marker_content.contains("<<<<<<<") || !merge_marker_content.contains(">>>>>>>") {
        bail!("restore --merge did not re-create conflict markers: {merge_marker_content}");
    }
    let diff3_markers = ctx.command(
        &["--json", "restore", "--conflict=diff3", "tracked.txt"],
        conflict_repo.clone(),
        true,
    )?;
    assert_json_ok(&diff3_markers, "restore")?;
    let diff3_marker_content =
        fs::read_to_string(conflict_repo.join("tracked.txt")).context("read restore diff3")?;
    if !diff3_marker_content.contains("||||||| base") {
        bail!("restore --conflict=diff3 did not include base marker: {diff3_marker_content}");
    }
    fs::write(conflict_repo.join("tracked.txt"), "resolved\n")
        .context("resolve restore conflict")?;
    ctx.command(&["add", "tracked.txt"], conflict_repo.clone(), true)?;
    ctx.command(&["merge", "--continue"], conflict_repo.clone(), true)?;
    assert_json_ok(
        &ctx.command(&["--json", "status"], conflict_repo.clone(), true)?,
        "status",
    )?;
    ctx.command(&["fsck", "--connectivity-only"], conflict_repo, true)?;
    Ok(())
}
