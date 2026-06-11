use super::prelude::*;

pub(crate) fn scenario_stash_bisect_worktree(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    let orig_branch = stdout_trim(&ctx.command(
        &["branch", "--show-current"],
        repo.clone(),
        true,
    )?);
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

    fs::write(repo.join("visible-untracked.txt"), "visible\n")
        .context("write visible untracked stash fixture")?;
    let include_untracked = ctx.command(&["--json", "stash", "push", "-u"], repo.clone(), true)?;
    assert_json_ok(&include_untracked, "stash")?;
    assert_stdout_contains(&include_untracked, "included_untracked")?;
    if repo.join("visible-untracked.txt").exists() {
        bail!("stash push -u did not remove included untracked file");
    }
    ctx.command(&["stash", "pop"], repo.clone(), true)?;
    ensure_file(repo.join("visible-untracked.txt"))?;
    fs::remove_file(repo.join("visible-untracked.txt"))
        .context("remove restored visible untracked fixture")?;

    fs::write(repo.join(".libraignore"), "ignored.log\n").context("write stash ignore fixture")?;
    fs::write(repo.join("ignored.log"), "ignored\n").context("write ignored stash fixture")?;
    let include_all = ctx.command(&["--json", "stash", "push", "--all"], repo.clone(), true)?;
    assert_json_ok(&include_all, "stash")?;
    assert_stdout_contains(&include_all, "included_untracked")?;
    if repo.join(".libraignore").exists() || repo.join("ignored.log").exists() {
        bail!("stash push --all did not remove visible and ignored untracked files");
    }
    ctx.command(&["stash", "pop"], repo.clone(), true)?;
    ensure_file(repo.join(".libraignore"))?;
    ensure_file(repo.join("ignored.log"))?;
    fs::remove_file(repo.join(".libraignore")).context("remove restored ignore fixture")?;
    fs::remove_file(repo.join("ignored.log")).context("remove restored ignored fixture")?;

    fs::write(repo.join("tracked.txt"), "staged\n").context("write staged stash fixture")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    fs::write(repo.join("tracked.txt"), "unstaged\n").context("write unstaged stash fixture")?;
    let keep_index = ctx.command(
        &["--json", "stash", "push", "--keep-index"],
        repo.clone(),
        true,
    )?;
    assert_json_ok(&keep_index, "stash")?;
    assert_stdout_contains(&keep_index, "kept_index")?;
    let kept_index =
        fs::read_to_string(repo.join("tracked.txt")).context("read keep-index fixture")?;
    if kept_index != "staged\n" {
        bail!("stash push --keep-index did not keep staged content in worktree: {kept_index}");
    }
    ctx.command(&["reset", "--hard"], repo.clone(), true)?;
    ctx.command(&["stash", "clear", "--force"], repo.clone(), true)?;

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

    // Session C: multi-good positional surface (git parity for
    // start <bad> <good1> <good2>...). Exercises the Vec revs path.
    ctx.command(
        &["bisect", "start", &bad_commit, &middle_commit, &good_commit],
        repo.clone(),
        true,
    )?;
    ctx.command(&["bisect", "log"], repo.clone(), true)?;
    ctx.command(&["bisect", "reset"], repo.clone(), true)?;

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
    ctx.command(
        &["worktree", "add", "-b", "workflow-linked", &wt],
        repo.clone(),
        true,
    )?;
    let wt_display = Path::new(&wt)
        .canonicalize()
        .with_context(|| format!("canonicalize worktree path {wt}"))?
        .to_string_lossy()
        .to_string();
    let list = ctx.command(&["worktree", "list", "--verbose"], repo.clone(), true)?;
    assert_stdout_contains(&list, &wt_display)?;
    assert_stdout_contains(&list, "[HEAD ")?;
    let porcelain = ctx.command(&["worktree", "list", "--porcelain"], repo.clone(), true)?;
    assert_stdout_contains(&porcelain, &format!("worktree {wt_display}"))?;
    assert_stdout_contains(&porcelain, "HEAD ")?;
    assert_not_contains(&porcelain, "branch ")?;
    assert_not_contains(&porcelain, "detached")?;
    ctx.command(
        &["worktree", "lock", &wt, "--reason", "integration smoke"],
        repo.clone(),
        true,
    )?;
    let locked_porcelain = ctx.command(&["worktree", "list", "--porcelain"], repo.clone(), true)?;
    assert_stdout_contains(&locked_porcelain, "locked")?;
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
    let prune_dry_run = ctx.command(&["worktree", "prune", "--dry-run"], repo.clone(), true)?;
    assert_stdout_contains(&prune_dry_run, &stale_display)?;
    let prune_expire = ctx.command(
        &["worktree", "prune", "--verbose", "--expire", "now"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&prune_expire, &stale_display)?;

    let no_checkout = ctx
        .run_dir
        .join("wt-no-checkout")
        .to_string_lossy()
        .to_string();
    ctx.command(
        &[
            "worktree",
            "add",
            "--no-checkout",
            "--lock",
            "--reason",
            "integration no checkout",
            &no_checkout,
        ],
        repo.clone(),
        true,
    )?;
    if Path::new(&no_checkout).join("tracked.txt").exists() {
        bail!("worktree add --no-checkout unexpectedly restored tracked.txt");
    }
    ctx.command(
        &["worktree", "remove", "-f", "-f", &no_checkout],
        repo.clone(),
        true,
    )?;
    if !Path::new(&no_checkout).exists() {
        bail!("worktree remove -f -f without --delete-dir unexpectedly deleted directory");
    }

    let dirty_delete = ctx
        .run_dir
        .join("wt-dirty-delete")
        .to_string_lossy()
        .to_string();
    ctx.command(&["worktree", "add", &dirty_delete], repo.clone(), true)?;
    fs::write(Path::new(&dirty_delete).join("dirty.txt"), "dirty\n")
        .context("write dirty worktree fixture")?;
    ctx.command(
        &[
            "worktree",
            "remove",
            "--delete-dir",
            "--force",
            &dirty_delete,
        ],
        repo.clone(),
        true,
    )?;
    if Path::new(&dirty_delete).exists() {
        bail!("worktree remove --delete-dir --force did not delete dirty worktree");
    }

    ctx.command(&["fsck", "--connectivity-only"], repo.clone(), true)?;
    Ok(())
}
