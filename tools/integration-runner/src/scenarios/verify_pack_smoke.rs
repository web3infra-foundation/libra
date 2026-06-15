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
        &[
            "index-pack",
            "--keep=integration keep",
            &pack,
            "--index-version",
            "1",
        ],
        repo.clone(),
        true,
    )?;
    let idx = pack_dst.with_extension("idx");
    let keep = pack_dst.with_extension("keep");
    let keep_message =
        fs::read_to_string(&keep).with_context(|| format!("read keep {}", keep.display()))?;
    anyhow::ensure!(keep_message == "integration keep\n");
    let idx_arg = idx.to_string_lossy().to_string();
    assert_stdout_contains(
        &ctx.command(&["verify-pack", &idx_arg], repo.clone(), true)?,
        ": ok",
    )?;

    // --pack: an explicit pack path must verify the same idx instead of the
    // derived `.pack` sibling.
    assert_stdout_contains(
        &ctx.command(
            &["verify-pack", "--pack", &pack, &idx_arg],
            repo.clone(),
            true,
        )?,
        ": ok",
    )?;

    // -v prints one `<oid> <type> <size> <size-in-pack> <offset>` row per
    // indexed object before the trailing ok line.
    let verbose = ctx.command(&["verify-pack", "-v", &idx_arg], repo.clone(), true)?;
    assert_stdout_contains(&verbose, " commit ")?;
    assert_stdout_contains(&verbose, " blob ")?;
    assert_stdout_contains(&verbose, ": ok")?;

    let stats = ctx.command(&["verify-pack", "-s", &idx_arg], repo.clone(), true)?;
    assert_stdout_contains(&stats, "non delta:")?;
    assert_not_contains(&stats, ": ok")?;

    assert_json_ok(
        &ctx.command(&["--json", "verify-pack", &idx_arg], repo.clone(), true)?,
        "verify-pack",
    )?;

    // Negative paths: a missing idx and a corrupted idx must both fail with a
    // stable error naming the affected path.
    let missing_arg = ctx
        .run_dir
        .join("missing.idx")
        .to_string_lossy()
        .to_string();
    let missing = ctx.command(&["verify-pack", &missing_arg], repo.clone(), false)?;
    assert_lbr_or_text(&missing, "could not open pack index")?;

    let corrupt_idx = ctx.run_dir.join("corrupt.idx");
    fs::copy(&idx, &corrupt_idx)
        .with_context(|| format!("copy idx to {}", corrupt_idx.display()))?;
    let mut corrupt_bytes =
        fs::read(&corrupt_idx).with_context(|| format!("read {}", corrupt_idx.display()))?;
    corrupt_bytes.extend_from_slice(b"corrupt");
    fs::write(&corrupt_idx, corrupt_bytes)
        .with_context(|| format!("append corruption to {}", corrupt_idx.display()))?;
    let corrupt_arg = corrupt_idx.to_string_lossy().to_string();
    let corrupt = ctx.command(&["verify-pack", &corrupt_arg], repo.clone(), false)?;
    assert_lbr_or_text(&corrupt, "invalid pack index")?;

    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
