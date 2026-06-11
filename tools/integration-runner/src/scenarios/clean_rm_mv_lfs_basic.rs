use super::prelude::*;

/// Full sha256 OID of the fixed LFS payload `libra lfs payload\n` (18 bytes),
/// asserted verbatim by the `lfs ls-files --long --size` step.
const LFS_BLOB_OID: &str = "33fe424d00faa1aad5529c6b3bf2a461ebc0fc91b46a7df5ce409141a81f23b4";

pub(crate) fn scenario_clean_rm_mv_lfs_basic(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    fs::write(repo.join("temp.tmp"), "temp\n").context("write temp")?;
    assert_stdout_contains(
        &ctx.command(&["clean", "-n"], repo.clone(), true)?,
        "temp.tmp",
    )?;
    ctx.command(&["clean", "-f"], repo.clone(), true)?;
    if repo.join("temp.tmp").exists() {
        bail!("clean -f did not remove temp.tmp");
    }
    // clean -fd: untracked directory removal.
    fs::create_dir_all(repo.join("tmpdir")).context("create tmpdir")?;
    fs::write(repo.join("tmpdir/dir-file.txt"), "dir scratch\n").context("write dir-file")?;
    assert_stdout_contains(
        &ctx.command(&["clean", "-fd"], repo.clone(), true)?,
        "tmpdir",
    )?;
    if repo.join("tmpdir").exists() {
        bail!("clean -fd did not remove tmpdir");
    }
    // clean -nX / -fX: only-ignored preview + removal driven by .libraignore;
    // .libraignore itself is untracked but NOT ignored, so it must survive -fX.
    fs::write(repo.join(".libraignore"), "*.ignored\n").context("write .libraignore")?;
    fs::write(repo.join("scratch.ignored"), "ignored\n").context("write scratch.ignored")?;
    let only_ignored_dry = ctx.command(&["clean", "-nX"], repo.clone(), true)?;
    assert_stdout_contains(&only_ignored_dry, "scratch.ignored")?;
    assert_not_contains(&only_ignored_dry, ".libraignore")?;
    ensure_file(repo.join("scratch.ignored"))?;
    ctx.command(&["clean", "-fX"], repo.clone(), true)?;
    if repo.join("scratch.ignored").exists() {
        bail!("clean -fX did not remove scratch.ignored");
    }
    ensure_file(repo.join(".libraignore"))?;
    fs::write(repo.join("old.txt"), "old\n").context("write old")?;
    fs::write(repo.join("dry.txt"), "dry\n").context("write dry")?;
    fs::write(repo.join("verbose.txt"), "verbose\n").context("write verbose")?;
    fs::write(repo.join("json.txt"), "json\n").context("write json")?;
    ctx.command(
        &["add", "old.txt", "dry.txt", "verbose.txt", "json.txt"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["commit", "-m", "mv fixtures", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["mv", "old.txt", "new.txt"], repo.clone(), true)?;
    ensure_file(repo.join("new.txt"))?;
    let dry_run = ctx.command(&["mv", "-n", "dry.txt", "dry-new.txt"], repo.clone(), true)?;
    assert_stdout_contains(&dry_run, "Checking rename of 'dry.txt' to 'dry-new.txt'")?;
    assert_stdout_contains(&dry_run, "Renaming dry.txt to dry-new.txt")?;
    ensure_file(repo.join("dry.txt"))?;
    if repo.join("dry-new.txt").exists() {
        bail!("mv -n unexpectedly materialized dry-new.txt");
    }
    let verbose = ctx.command(
        &["mv", "-v", "verbose.txt", "verbose-new.txt"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&verbose, "Renaming verbose.txt to verbose-new.txt")?;
    assert_not_contains(&verbose, "Checking rename of")?;
    ensure_file(repo.join("verbose-new.txt"))?;
    let json_mv = ctx.command(
        &["--json", "mv", "--sparse", "json.txt", "json-new.txt"],
        repo.clone(),
        true,
    )?;
    assert_json_ok(&json_mv, "mv")?;
    assert_stdout_contains(&json_mv, "json-new.txt")?;
    assert_not_contains(&json_mv, "\"sparse\"")?;
    ensure_file(repo.join("json-new.txt"))?;
    ctx.command(
        &["commit", "-m", "rename", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["rm", "new.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "remove", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["lfs", "track", "*.bin"], repo.clone(), true)?;
    ensure_file(repo.join(".libra_attributes"))?;
    let attributes =
        fs::read_to_string(repo.join(".libra_attributes")).context("read .libra_attributes")?;
    if !attributes.contains("*.bin") {
        bail!(".libra_attributes does not contain the tracked pattern: {attributes:?}");
    }
    // `lfs track` with no patterns is list mode: header + tracked patterns.
    let track_list = ctx.command(&["lfs", "track"], repo.clone(), true)?;
    assert_stdout_contains(&track_list, "Listing tracked patterns")?;
    assert_stdout_contains(&track_list, "*.bin")?;
    // Commit a deterministic LFS-tracked payload so ls-files variants are observable.
    fs::create_dir_all(repo.join("assets")).context("create assets")?;
    fs::write(repo.join("assets/blob.bin"), "libra lfs payload\n").context("write blob.bin")?;
    ctx.command(
        &["add", ".libra_attributes", "assets/blob.bin"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["commit", "-m", "lfs tracked file", "--no-verify"],
        repo.clone(),
        true,
    )?;
    let lfs = ctx.command(&["lfs", "ls-files"], repo.clone(), true)?;
    assert_not_contains(&lfs, "PRIVATE KEY")?;
    assert_stdout_contains(&lfs, "assets/blob.bin")?;
    let lfs_long = ctx.command(
        &["lfs", "ls-files", "--long", "--size"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&lfs_long, LFS_BLOB_OID)?;
    assert_stdout_contains(&lfs_long, "assets/blob.bin (18 B)")?;
    let lfs_names = ctx.command(&["lfs", "ls-files", "--name-only"], repo.clone(), true)?;
    let names = stdout_trim(&lfs_names);
    if names != "assets/blob.bin" {
        bail!("lfs ls-files --name-only expected bare path, got {names:?}");
    }
    assert_json_ok(
        &ctx.command(&["--json", "lfs", "ls-files"], repo.clone(), true)?,
        "lfs ls-files",
    )?;
    ctx.command(&["lfs", "untrack", "*.bin"], repo.clone(), true)?;
    let track_list_after = ctx.command(&["lfs", "track"], repo.clone(), true)?;
    assert_stdout_contains(&track_list_after, "Listing tracked patterns")?;
    assert_not_contains(&track_list_after, "*.bin")?;
    assert_json_ok(
        &ctx.command(&["--json", "status"], repo.clone(), true)?,
        "status",
    )?;
    let bad_clean = ctx.command(&["clean"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_clean, "requires -f")?;
    let bad_clean_xx = ctx.command(&["clean", "-xX"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_clean_xx, "cannot use -x and -X together")?;
    let bad_rm = ctx.command(&["rm", "nonexistent.txt"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_rm, "pathspec")?;
    let bad_mv = ctx.command(
        &["mv", "no-such-source.txt", "docs/dest.txt"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_mv, "bad source")?;
    // `lfs lock` requires a remote LFS endpoint; with no remote configured it
    // must fail fast and never leak credential material.
    let bad_lock = ctx.command(&["lfs", "lock", "assets/blob.bin"], repo.clone(), false)?;
    assert_not_contains(&bad_lock, "PRIVATE KEY")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
