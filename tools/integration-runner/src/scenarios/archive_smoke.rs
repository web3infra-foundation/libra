use super::prelude::*;

pub(crate) fn scenario_archive_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    fs::create_dir_all(repo.join("docs")).context("create archive docs fixture dir")?;
    fs::write(repo.join("docs/guide.md"), "archive docs\n")
        .context("write archive docs fixture")?;
    ctx.command(&["add", "docs/guide.md"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "test: archive docs", "--no-verify"],
        repo.clone(),
        true,
    )?;

    let tar_path = ctx.run_dir.join("release.tar");
    let tar_arg = tar_path.to_string_lossy().to_string();
    ctx.command(
        &["archive", "--output", &tar_arg, "--prefix", "release/"],
        repo.clone(),
        true,
    )?;
    ensure_file(tar_path.clone())?;
    let tar = fs::read(&tar_path).with_context(|| format!("read {}", tar_path.display()))?;
    if !is_tar(&tar) {
        bail!("archive tar output did not contain a tar header");
    }
    let tar_text = String::from_utf8_lossy(&tar);
    if !tar_text.contains("release/tracked.txt") || !tar_text.contains("release/docs/guide.md") {
        bail!("archive tar output missed expected prefixed paths: {tar_text}");
    }

    let zip_path = ctx.run_dir.join("release.zip");
    let zip_arg = zip_path.to_string_lossy().to_string();
    ctx.command(
        &["archive", "--format=zip", "--output", &zip_arg],
        repo.clone(),
        true,
    )?;
    ensure_file(zip_path.clone())?;
    let zip = fs::read(&zip_path).with_context(|| format!("read {}", zip_path.display()))?;
    if !zip.starts_with(b"PK") {
        bail!("archive zip output did not contain a zip header");
    }

    let bad_prefix = ctx.command(&["archive", "--prefix", "../escape"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_prefix, "invalid archive prefix")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}

fn is_tar(data: &[u8]) -> bool {
    data.len() >= 263
        && (&data[257..263] == b"ustar\0".as_slice() || &data[257..263] == b"ustar ".as_slice())
}
