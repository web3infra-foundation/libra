use super::prelude::*;

pub(crate) fn scenario_gc_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;

    fs::write(repo.join("unreachable.txt"), "gc unreachable blob\n")
        .context("write unreachable blob fixture")?;
    let hash = ctx.command(
        &["hash-object", "-w", "unreachable.txt"],
        repo.clone(),
        true,
    )?;
    let object_id = stdout_trim(&hash);
    if object_id.len() < 40 {
        bail!("hash-object returned an unexpectedly short id: {object_id}");
    }

    let object_type = ctx.command(&["cat-file", "-t", &object_id], repo.clone(), true)?;
    assert_stdout_contains(&object_type, "blob")?;

    assert_json_ok(
        &ctx.command(
            &["--json", "gc", "--dry-run", "--prune=now"],
            repo.clone(),
            true,
        )?,
        "gc",
    )?;
    let still_present = ctx.command(&["cat-file", "-t", &object_id], repo.clone(), true)?;
    assert_stdout_contains(&still_present, "blob")?;

    // --no-prune disables loose-object pruning entirely: a real (non-dry-run)
    // gc pass must leave the unreachable object in place.
    ctx.command(&["gc", "--no-prune"], repo.clone(), true)?;
    let no_prune_kept = ctx.command(&["cat-file", "-t", &object_id], repo.clone(), true)?;
    assert_stdout_contains(&no_prune_kept, "blob")?;

    ctx.command(&["gc", "--prune=now"], repo.clone(), true)?;
    let missing = ctx.command(&["cat-file", "-t", &object_id], repo.clone(), false)?;
    assert_lbr_or_text(&missing, "object not found")?;

    ctx.command(&["fsck", "--connectivity-only"], repo.clone(), true)?;

    // Compatibility flags: --auto and --aggressive are accepted no-op passes
    // (warnings surfaced in the JSON envelope), and --force succeeds even
    // right after a previous gc finished (no stale lock left behind).
    let auto_run = ctx.command(&["--json", "gc", "--auto"], repo.clone(), true)?;
    assert_json_ok(&auto_run, "gc")?;
    assert_stdout_contains(&auto_run, "--auto is accepted for compatibility")?;

    let aggressive_run = ctx.command(&["--json", "gc", "--aggressive"], repo.clone(), true)?;
    assert_json_ok(&aggressive_run, "gc")?;
    assert_stdout_contains(&aggressive_run, "does not repack")?;
    ctx.command(&["fsck", "--connectivity-only"], repo.clone(), true)?;

    let force_run = ctx.command(&["--json", "gc", "--force"], repo.clone(), true)?;
    assert_json_ok(&force_run, "gc")?;
    assert_stdout_contains(&force_run, "gc lock was available")?;

    // --- standalone `libra prune` (sibling maintenance command of gc) ---
    // gc internally prunes; `prune` is the dedicated unreachable-loose-object
    // collector. Build a fresh unreachable blob (the gc fixture above is gone)
    // and exercise prune's dry-run / --expire keep / verbose real-prune paths.
    fs::write(repo.join("prune-me.txt"), "prune unreachable blob\n")
        .context("write prune unreachable blob fixture")?;
    let prune_blob = ctx.command(&["hash-object", "-w", "prune-me.txt"], repo.clone(), true)?;
    let prune_oid = stdout_trim(&prune_blob);
    if prune_oid.len() < 40 {
        bail!("hash-object returned an unexpectedly short id: {prune_oid}");
    }

    // --dry-run + JSON: lists the unreachable object but must not delete it.
    let prune_dry = ctx.command(&["--json", "prune", "--dry-run"], repo.clone(), true)?;
    assert_json_ok(&prune_dry, "prune")?;
    assert_stdout_contains(&prune_dry, &prune_oid)?;
    let still_present = ctx.command(&["cat-file", "-t", &prune_oid], repo.clone(), true)?;
    assert_stdout_contains(&still_present, "blob")?;

    // --expire with a far-past cutoff keeps the freshly written object (only
    // objects older than the cutoff expire), exercising the date-parse path.
    ctx.command(&["prune", "--expire=2000-01-01"], repo.clone(), true)?;
    let kept = ctx.command(&["cat-file", "-t", &prune_oid], repo.clone(), true)?;
    assert_stdout_contains(&kept, "blob")?;

    // Verbose real prune (default "expire all unreachable" policy) removes it.
    ctx.command(&["prune", "-v"], repo.clone(), true)?;
    let pruned = ctx.command(&["cat-file", "-t", &prune_oid], repo.clone(), false)?;
    assert_lbr_or_text(&pruned, "object not found")?;

    // Positional <head> arguments add extra reachability roots: objects
    // reachable from the supplied head survive while the other planted
    // unreachable loose object is pruned in the same run.
    fs::write(repo.join("prune-keep.txt"), "prune keep blob\n")
        .context("write prune keep blob fixture")?;
    let keep_blob = ctx.command(&["hash-object", "-w", "prune-keep.txt"], repo.clone(), true)?;
    let keep_oid = stdout_trim(&keep_blob);
    if keep_oid.len() < 40 {
        bail!("hash-object returned an unexpectedly short id: {keep_oid}");
    }
    fs::write(repo.join("prune-drop.txt"), "prune drop blob\n")
        .context("write prune drop blob fixture")?;
    let drop_blob = ctx.command(&["hash-object", "-w", "prune-drop.txt"], repo.clone(), true)?;
    let drop_oid = stdout_trim(&drop_blob);
    if drop_oid.len() < 40 {
        bail!("hash-object returned an unexpectedly short id: {drop_oid}");
    }

    let head_prune = ctx.command(&["--json", "prune", &keep_oid], repo.clone(), true)?;
    assert_json_ok(&head_prune, "prune")?;
    assert_stdout_contains(&head_prune, &drop_oid)?;
    let head_kept = ctx.command(&["cat-file", "-t", &keep_oid], repo.clone(), true)?;
    assert_stdout_contains(&head_kept, "blob")?;
    let head_dropped = ctx.command(&["cat-file", "-t", &drop_oid], repo.clone(), false)?;
    assert_lbr_or_text(&head_dropped, "object not found")?;

    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
