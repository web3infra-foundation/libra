use super::prelude::*;

pub(crate) fn scenario_verify_pack_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    let packs_dir = repo_root()?.join("tests/data/packs");
    let pack_src = fs::read_dir(&packs_dir)
        .with_context(|| format!("read packs dir {}", packs_dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("small-sha1") && name.ends_with(".pack"))
        })
        .context("find small-sha1 pack fixture")?;
    let pack_dst = ctx.run_dir.join("fixture.pack");
    fs::copy(&pack_src, &pack_dst).with_context(|| format!("copy pack {}", pack_src.display()))?;
    let pack = pack_dst.to_string_lossy().to_string();
    ctx.command(
        &["index-pack", &pack, "--index-version", "1"],
        repo.clone(),
        true,
    )?;
    let idx = pack_dst.with_extension("idx");
    let idx_arg = idx.to_string_lossy().to_string();
    assert_stdout_contains(
        &ctx.command(&["verify-pack", &idx_arg], repo.clone(), true)?,
        ": ok",
    )?;
    assert_json_ok(
        &ctx.command(&["--json", "verify-pack", &idx_arg], repo.clone(), true)?,
        "verify-pack",
    )?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
