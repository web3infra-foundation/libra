use super::prelude::*;

pub(crate) fn scenario_stash_bisect_worktree(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    let orig_branch =
        stdout_trim(&ctx.command(&["branch", "--show-current"], repo.clone(), true)?);
    fs::write(repo.join("tracked.txt"), "stashed\n").context("write stashed change")?;
    ctx.command(&["stash", "push", "-m", "save work"], repo.clone(), true)?;
    assert_stdout_contains(
        &ctx.command(&["stash", "list"], repo.clone(), true)?,
        "save work",
    )?;
    assert_json_ok(
        &ctx.command(&["--json", "stash", "list"], repo.clone(), true)?,
        "stash",
    )?;
    // Bare `stash show` summarises the stashed file; `--name-status` with an
    // explicit positional ref emits `<status>\t<path>` records.
    assert_stdout_contains(
        &ctx.command(&["stash", "show"], repo.clone(), true)?,
        "tracked.txt",
    )?;
    assert_stdout_contains(
        &ctx.command(
            &["stash", "show", "--name-status", "stash@{0}"],
            repo.clone(),
            true,
        )?,
        "\ttracked.txt",
    )?;
    // Out-of-range positional stash ref on pop must fail while entries exist.
    let bad_pop = ctx.command(&["stash", "pop", "stash@{999}"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_pop, "stash")?;
    ctx.command(&["stash", "apply"], repo.clone(), true)?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "apply stash", "--no-verify"],
        repo.clone(),
        true,
    )?;
    // `apply` kept the entry; drop it via an explicit positional ref.
    assert_stdout_contains(
        &ctx.command(&["stash", "drop", "stash@{0}"], repo.clone(), true)?,
        "Dropped stash@{0}",
    )?;
    assert_not_contains(
        &ctx.command(&["stash", "list"], repo.clone(), true)?,
        "save work",
    )?;

    // `stash apply <stash>` and `stash branch <branch> <stash>` positional refs.
    fs::write(repo.join("tracked.txt"), "branch wip\n").context("write branch stash fixture")?;
    ctx.command(
        &["stash", "push", "-m", "wip: stash branch"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(
        &ctx.command(&["stash", "apply", "stash@{0}"], repo.clone(), true)?,
        "Applied stash@{0}",
    )?;
    ctx.command(&["reset", "--hard"], repo.clone(), true)?;
    assert_stdout_contains(
        &ctx.command(
            &["stash", "branch", "stash-branch-test", "stash@{0}"],
            repo.clone(),
            true,
        )?,
        "Switched to a new branch 'stash-branch-test'",
    )?;
    assert_stdout_contains(
        &ctx.command(&["branch", "--show-current"], repo.clone(), true)?,
        "stash-branch-test",
    )?;
    ctx.command(&["reset", "--hard"], repo.clone(), true)?;
    ctx.command(&["switch", &orig_branch], repo.clone(), true)?;
    ctx.command(&["branch", "-D", "stash-branch-test"], repo.clone(), true)?;
    ctx.command(&["stash", "clear", "--force"], repo.clone(), true)?;
    let empty_pop = ctx.command(&["stash", "pop"], repo.clone(), false)?;
    assert_lbr_or_text(&empty_pop, "stash")?;

    let include_untracked = ctx.command(&["--json", "stash", "push", "-u"], repo.clone(), false)?;
    assert_json_error_code(&include_untracked, "LBR-CLI-002")?;
    let include_all = ctx.command(&["--json", "stash", "push", "--all"], repo.clone(), false)?;
    assert_json_error_code(&include_all, "LBR-CLI-002")?;
    let keep_index = ctx.command(
        &["--json", "stash", "push", "--keep-index"],
        repo.clone(),
        false,
    )?;
    assert_json_error_code(&keep_index, "LBR-CLI-002")?;

    // Build a short linear history for bisect: good -> middle -> bad.
    let good_commit = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?);
    fs::write(repo.join("tracked.txt"), "bisect middle\n")
        .context("write bisect middle fixture")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "bisect middle", "--no-verify"],
        repo.clone(),
        true,
    )?;
    let middle_commit = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?);
    fs::write(repo.join("tracked.txt"), "bisect bad\n").context("write bisect bad fixture")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "bisect bad", "--no-verify"],
        repo.clone(),
        true,
    )?;
    let bad_commit = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?);

    // Session A: bare start, bare bad, good <rev>.
    ctx.command(&["bisect", "start"], repo.clone(), true)?;
    ctx.command(&["bisect", "bad"], repo.clone(), true)?;
    ctx.command(&["bisect", "good", "HEAD~1"], repo.clone(), true)?;
    ctx.command(&["bisect", "log"], repo.clone(), true)?;
    ctx.command(&["bisect", "reset"], repo.clone(), true)?;

    // Session B: --good flag form, view, bad <rev> (negative + positive),
    // and reset <rev> landing HEAD on the requested commit.
    ctx.command(
        &["bisect", "start", &bad_commit, "--good", &good_commit],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(
        &ctx.command(&["bisect", "view"], repo.clone(), true)?,
        "Remaining:",
    )?;
    let bad_rev = ctx.command(&["bisect", "bad", "no-such-revision"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_rev, "no-such-revision")?;
    assert_stdout_contains(
        &ctx.command(&["bisect", "bad", &middle_commit], repo.clone(), true)?,
        "Marked",
    )?;
    ctx.command(&["bisect", "log"], repo.clone(), true)?;
    ctx.command(&["bisect", "reset", &good_commit], repo.clone(), true)?;
    let reset_head = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?);
    if reset_head != good_commit {
        bail!("bisect reset <rev> left HEAD at {reset_head}, expected {good_commit}");
    }
    // reset <rev> detaches HEAD and repaints the worktree only; realign the
    // index before switching back to the original branch.
    ctx.command(&["reset", "--hard"], repo.clone(), true)?;
    ctx.command(&["switch", &orig_branch], repo.clone(), true)?;

    let multi_good = ctx.command(
        &["bisect", "start", &bad_commit, &middle_commit, &good_commit],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&multi_good, "unexpected argument")?;

    // Session D: skip (bare current candidate + explicit positional rev).
    ctx.command(
        &["bisect", "start", &bad_commit, "--good", "HEAD~3"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(
        &ctx.command(&["bisect", "skip"], repo.clone(), true)?,
        "Skipped",
    )?;
    assert_stdout_contains(
        &ctx.command(&["bisect", "skip", &middle_commit], repo.clone(), true)?,
        "Skipped",
    )?;
    ctx.command(&["bisect", "reset"], repo.clone(), true)?;

    let wt = ctx.run_dir.join("wt").to_string_lossy().to_string();
    ctx.command(&["worktree", "add", &wt], repo.clone(), true)?;
    let wt_display = Path::new(&wt)
        .canonicalize()
        .with_context(|| format!("canonicalize worktree path {wt}"))?
        .to_string_lossy()
        .to_string();
    let list = ctx.command(&["worktree", "list"], repo.clone(), true)?;
    assert_stdout_contains(&list, &wt_display)?;
    assert_stdout_contains(&list, "worktree ")?;
    assert_json_ok(
        &ctx.command(&["--json", "worktree", "list"], repo.clone(), true)?,
        "worktree.list",
    )?;
    ensure_file(Path::new(&wt).join("tracked.txt"))?;
    ctx.command(
        &["worktree", "lock", &wt, "--reason", "integration smoke"],
        repo.clone(),
        true,
    )?;
    let locked = ctx.command(&["worktree", "list"], repo.clone(), true)?;
    assert_stdout_contains(&locked, "[locked: integration smoke]")?;
    ctx.command(&["worktree", "unlock", &wt], repo.clone(), true)?;
    let moved = ctx.run_dir.join("wt-moved").to_string_lossy().to_string();
    ctx.command(&["worktree", "move", &wt, &moved], repo.clone(), true)?;
    ctx.command(&["worktree", "remove", &moved], repo.clone(), true)?;
    if !Path::new(&moved).exists() {
        bail!("worktree remove unexpectedly deleted directory by default");
    }

    let stale = ctx.run_dir.join("wt-stale").to_string_lossy().to_string();
    ctx.command(&["worktree", "add", &stale], repo.clone(), true)?;
    let stale_display = Path::new(&stale)
        .canonicalize()
        .with_context(|| format!("canonicalize stale worktree path {stale}"))?
        .to_string_lossy()
        .to_string();
    fs::remove_dir_all(&stale).with_context(|| format!("remove stale worktree {stale}"))?;
    let prune = ctx.command(&["worktree", "prune"], repo.clone(), true)?;
    assert_stdout_contains(&prune, &stale_display)?;

    let no_checkout = ctx
        .run_dir
        .join("wt-no-checkout")
        .to_string_lossy()
        .to_string();
    let no_checkout_result = ctx.command(
        &["worktree", "add", "--no-checkout", &no_checkout],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&no_checkout_result, "--no-checkout")?;

    let dirty_delete = ctx
        .run_dir
        .join("wt-dirty-delete")
        .to_string_lossy()
        .to_string();
    ctx.command(&["worktree", "add", &dirty_delete], repo.clone(), true)?;
    fs::write(Path::new(&dirty_delete).join("dirty.txt"), "dirty\n")
        .context("write dirty worktree fixture")?;
    let dirty_refused = ctx.command(
        &["worktree", "remove", "--delete-dir", &dirty_delete],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&dirty_refused, "dirty worktree")?;
    fs::remove_file(Path::new(&dirty_delete).join("dirty.txt"))
        .context("remove dirty worktree fixture")?;
    ctx.command(
        &["worktree", "remove", "--delete-dir", &dirty_delete],
        repo.clone(),
        true,
    )?;
    if Path::new(&dirty_delete).exists() {
        bail!("worktree remove --delete-dir did not delete clean worktree");
    }

    ctx.command(&["fsck", "--connectivity-only"], repo.clone(), true)?;
    Ok(())
}
