use super::prelude::*;

pub(crate) fn scenario_archive_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    fs::create_dir_all(repo.join("docs")).context("create archive fixturedir")?;
    fs::write(repo.join("docs/guide.md"), "archive docs\n")
        .context("write archive fixture")?;
    ensure_file(repo.join("docs/guide.md"))?;
    ctx.command(
        &["add", "docs/guide.md"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["commit", "-m", "test: archive docs", "--no-verify"],
        repo.clone(),
        true,
    )?;
    let tar_arg = ctx
        .run_dir
        .join("release.tar")
        .to_string_lossy()
        .to_string();
    ctx.command(
        &["archive", "--output", &tar_arg, "--prefix", "release/"],
        repo.clone(),
        true,
    )?;
    let tar_bytes = fs::read(&tar_arg).context("read archive tar output")?;
    assert!(
        tar_bytes.len() >= 263,
        "tar output too short: {} bytes",
        tar_bytes.len()
    );
    assert!(
        tar_bytes[257..263] == b"ustar\0" || tar_bytes[257..263] == b"ustar ",
        "tar magic not found"
    );
    let zip_arg = ctx
        .run_dir
        .join("release.zip")
        .to_string_lossy()
        .to_string();
    ctx.command(
        &["archive", "--format=zip", "--output", &zip_arg],
        repo.clone(),
        true,
    )?;
    let zip_bytes = fs::read(&zip_arg).context("read archive zip output")?;
    assert!(zip_bytes.starts_with(b"PK"), "zip magic not found");
    let json = ctx.command(&["--json", "archive"], repo.clone(), true)?;
    assert_json_ok(&json, "archive")?;
    let fail = ctx.command(
        &["archive", "--prefix", "../escape"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&fail, "invalid archive prefix")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
