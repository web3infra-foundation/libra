use super::prelude::*;

pub(crate) fn scenario_clone_fetch_pull_local(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let remote_dir = ctx
        .run
        .run_root
        .join("fixtures")
        .join(&ctx.id)
        .join("git-source");
    let clone_dir = ctx.run_dir.join("clone");
    fs::create_dir_all(&remote_dir).context("create git fixture dir")?;
    ctx.gitfix(&["init", "-b", "main"], remote_dir.clone(), true)?;
    ctx.gitfix(
        &["config", "user.name", "Libra Remote Seed"],
        remote_dir.clone(),
        true,
    )?;
    ctx.gitfix(
        &["config", "user.email", "remote-seed@example.invalid"],
        remote_dir.clone(),
        true,
    )?;
    fs::write(remote_dir.join("README.md"), "first\n").context("write first remote commit")?;
    ctx.gitfix(&["add", "README.md"], remote_dir.clone(), true)?;
    ctx.gitfix(
        &["commit", "-m", "test: seed remote"],
        remote_dir.clone(),
        true,
    )?;

    let remote = remote_dir.to_string_lossy().to_string();
    let clone = clone_dir.to_string_lossy().to_string();
    let ls_remote = ctx.command(&["ls-remote", &remote], ctx.run_dir.clone(), true)?;
    assert_stdout_contains(&ls_remote, "refs/heads/main")?;
    ctx.command(
        &["ls-remote", "--heads", &remote, "main"],
        ctx.run_dir.clone(),
        true,
    )?;
    ctx.command(&["clone", &remote, &clone], ctx.run_dir.clone(), true)?;
    let remotes = ctx.command(&["remote", "-v"], clone_dir.clone(), true)?;
    assert_stdout_contains(&remotes, &remote)?;
    let origin = ctx.command(&["remote", "get-url", "origin"], clone_dir.clone(), true)?;
    assert_stdout_contains(&origin, &remote)?;
    ctx.command(
        &["remote", "add", "mirror", &remote],
        clone_dir.clone(),
        true,
    )?;
    let mirror = ctx.command(&["remote", "get-url", "mirror"], clone_dir.clone(), true)?;
    assert_stdout_contains(&mirror, &remote)?;
    let log = ctx.command(&["log", "--oneline"], clone_dir.clone(), true)?;
    assert_stdout_contains(&log, "test: seed remote")?;
    let readme = fs::read_to_string(clone_dir.join("README.md")).context("read cloned README")?;
    if !readme.contains("first") {
        bail!("cloned README did not contain first commit content: {readme}");
    }

    fs::write(remote_dir.join("README.md"), "first\nsecond\n")
        .context("write second remote commit")?;
    ctx.gitfix(&["add", "README.md"], remote_dir.clone(), true)?;
    ctx.gitfix(
        &["commit", "-m", "test: second remote commit"],
        remote_dir.clone(),
        true,
    )?;

    ctx.command(&["fetch", "origin", "main"], clone_dir.clone(), true)?;
    ctx.command(&["fetch", "--all"], clone_dir.clone(), true)?;
    ctx.command(&["show-ref", "--heads"], clone_dir.clone(), true)?;
    ctx.command(
        &["pull", "--ff-only", "origin", "main"],
        clone_dir.clone(),
        true,
    )?;
    let readme = fs::read_to_string(clone_dir.join("README.md")).context("read pulled README")?;
    if !readme.contains("second") {
        bail!("pulled README did not contain second commit content: {readme}");
    }
    ctx.command(&["fsck", "--connectivity-only"], clone_dir.clone(), true)?;
    assert_json_ok(
        &ctx.command(&["--json", "log", "--oneline"], clone_dir.clone(), true)?,
        "log",
    )?;

    let bad_fetch = ctx.command(
        &["fetch", "origin", "no-such-branch"],
        clone_dir.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_fetch, "couldn't find remote ref")?;
    let bad_clone_target = ctx
        .run_dir
        .join("missing-clone")
        .to_string_lossy()
        .to_string();
    let missing = ctx
        .run
        .run_root
        .join("fixtures")
        .join(&ctx.id)
        .join("missing.git");
    let missing_remote = missing.to_string_lossy().to_string();
    let bad_clone = ctx.command(
        &["clone", &missing_remote, &bad_clone_target],
        ctx.run_dir.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_clone, "No such file")?;
    Ok(())
}
