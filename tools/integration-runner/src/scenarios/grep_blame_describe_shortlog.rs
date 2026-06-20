use super::prelude::*;

pub(crate) fn scenario_grep_blame_describe_shortlog(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    fs::write(
        repo.join("search.txt"),
        "needle\na.b literal\naxb regex\nMixedCase Needle\nsecond\n",
    )
    .context("write search file")?;
    fs::create_dir_all(repo.join("sub")).context("create grep subdir")?;
    fs::write(repo.join("sub/inner.txt"), "subneedle\n").context("write grep subdir file")?;
    ctx.command(&["add", "search.txt", "sub/inner.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "searchable", "--no-verify"],
        repo.clone(),
        true,
    )?;
    let searchable_rev = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?);
    ctx.command(
        &["tag", "-m", "inspect release", "v1.0.0"],
        repo.clone(),
        true,
    )?;

    assert_stdout_contains(
        &ctx.command(&["grep", "needle"], repo.clone(), true)?,
        "search.txt",
    )?;
    let regex_grep = ctx.command(&["grep", "a.b"], repo.clone(), true)?;
    assert_stdout_contains(&regex_grep, "axb regex")?;
    assert_stdout_contains(&regex_grep, "a.b literal")?;
    let fixed_grep = ctx.command(&["grep", "-F", "a.b"], repo.clone(), true)?;
    assert_stdout_contains(&fixed_grep, "a.b literal")?;
    assert_not_contains(&fixed_grep, "axb")?;
    assert_stdout_contains(
        &ctx.command(&["grep", "-i", "mixedcase"], repo.clone(), true)?,
        "MixedCase Needle",
    )?;
    assert_stdout_contains(
        &ctx.command(&["grep", "-n", "needle"], repo.clone(), true)?,
        "search.txt:1:needle",
    )?;
    assert_stdout_contains(
        &ctx.command(&["grep", "-c", "needle"], repo.clone(), true)?,
        "search.txt:1",
    )?;
    let names_only = ctx.command(&["grep", "-l", "needle"], repo.clone(), true)?;
    assert_stdout_contains(&names_only, "search.txt")?;
    assert_not_contains(&names_only, "needle")?;
    let without_match = ctx.command(&["grep", "-L", "needle"], repo.clone(), true)?;
    assert_stdout_contains(&without_match, "tracked.txt")?;
    assert_not_contains(&without_match, "search.txt")?;
    let nul_list = ctx.command(
        &["grep", "-z", "-l", "needle", "search.txt"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&nul_list, "-z")?;
    assert_stdout_contains(
        &ctx.command(&["grep", "-e", "subneedle"], repo.clone(), true)?,
        "sub/inner.txt",
    )?;
    fs::write(repo.join("pats.txt"), "subneedle\n").context("write grep pattern file")?;
    assert_stdout_contains(
        &ctx.command(&["grep", "-f", "pats.txt"], repo.clone(), true)?,
        "sub/inner.txt:subneedle",
    )?;
    fs::remove_file(repo.join("pats.txt")).context("remove grep pattern file")?;
    let scoped = ctx.command(&["grep", "needle", "sub"], repo.clone(), true)?;
    assert_stdout_contains(&scoped, "sub/inner.txt:subneedle")?;
    assert_not_contains(&scoped, "search.txt")?;
    assert_stdout_contains(
        &ctx.command(&["grep", "--tree", "HEAD~1", "base"], repo.clone(), true)?,
        "tracked.txt:base",
    )?;
    let tree_miss = ctx.command(&["grep", "--tree", "HEAD~1", "needle"], repo.clone(), false)?;
    assert_lbr_or_text(&tree_miss, "not found")?;

    fs::write(
        repo.join("search.txt"),
        "needle\na.b literal\naxb regex\nMixedCase Needle\nsecond\nstaged-only-marker\n",
    )
    .context("stage grep marker")?;
    ctx.command(&["add", "search.txt"], repo.clone(), true)?;
    fs::write(
        repo.join("search.txt"),
        "needle\na.b literal\naxb regex\nMixedCase Needle\nsecond\n",
    )
    .context("revert grep worktree file")?;
    assert_stdout_contains(
        &ctx.command(
            &["grep", "--cached", "staged-only-marker"],
            repo.clone(),
            true,
        )?,
        "search.txt:staged-only-marker",
    )?;
    let worktree_miss = ctx.command(&["grep", "staged-only-marker"], repo.clone(), false)?;
    assert_lbr_or_text(&worktree_miss, "not found")?;
    ctx.command(&["restore", "--staged", "search.txt"], repo.clone(), true)?;

    fs::write(repo.join("loose.txt"), "needle untracked\n").context("write untracked grep file")?;
    let tracked_only = ctx.command(&["grep", "-l", "needle"], repo.clone(), true)?;
    assert_not_contains(&tracked_only, "loose.txt")?;
    let untracked = ctx.command(&["grep", "--untracked", "needle"], repo.clone(), false)?;
    assert_lbr_or_text(&untracked, "--untracked")?;
    fs::remove_file(repo.join("loose.txt")).context("remove untracked grep file")?;

    let tree_bad = ctx.command(
        &["grep", "--tree", "no-such-revision", "needle"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&tree_bad, "invalid revision")?;
    assert_stdout_contains(
        &ctx.command(&["blame", "-L", "1,1", "search.txt"], repo.clone(), true)?,
        "needle",
    )?;
    assert_stdout_contains(
        &ctx.command(
            &["blame", "search.txt", searchable_rev.as_str()],
            repo.clone(),
            true,
        )?,
        "needle",
    )?;
    let porcelain = ctx.command(&["blame", "--porcelain", "search.txt"], repo.clone(), false)?;
    assert_lbr_or_text(&porcelain, "--porcelain")?;
    let bad_range = ctx.command(&["blame", "-L", "bad", "search.txt"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_range, "invalid line range")?;
    let missing_file = ctx.command(&["blame", "missing.txt"], repo.clone(), false)?;
    assert_lbr_or_text(&missing_file, "not found")?;

    if stdout_trim(&ctx.command(&["describe", "--always"], repo.clone(), true)?).is_empty() {
        bail!("describe --always returned empty output");
    }
    assert_stdout_contains(
        &ctx.command(&["describe", "--tags", "HEAD"], repo.clone(), true)?,
        "v1.0.0",
    )?;
    let long_describe = stdout_trim(&ctx.command(
        &["describe", "--long", "--tags", "HEAD"],
        repo.clone(),
        true,
    )?);
    if !long_describe.starts_with("v1.0.0-0-g") {
        bail!("describe --long --tags HEAD returned {long_describe}");
    }
    if stdout_trim(&ctx.command(
        &["describe", "--always", "--abbrev", "12", "HEAD"],
        repo.clone(),
        true,
    )?)
    .is_empty()
    {
        bail!("describe --always --abbrev 12 returned empty output");
    }
    assert_stdout_contains(
        &ctx.command(&["describe", "--exact-match", "HEAD"], repo.clone(), true)?,
        "v1.0.0",
    )?;
    let long_abbrev_zero =
        ctx.command(&["describe", "--long", "--abbrev=0"], repo.clone(), false)?;
    assert_lbr_or_text(&long_abbrev_zero, "--abbrev=0")?;
    fs::write(
        repo.join("search.txt"),
        "needle\na.b literal\naxb regex\nMixedCase Needle\nsecond\ndirty\n",
    )
    .context("dirty describe tracked file")?;
    assert_stdout_contains(
        &ctx.command(&["describe", "--tags", "--dirty"], repo.clone(), true)?,
        "-dirty",
    )?;
    ctx.command(&["restore", "search.txt"], repo.clone(), true)?;

    ctx.command(
        &["config", "user.name", "Second Author"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["config", "user.email", "second@example.invalid"],
        repo.clone(),
        true,
    )?;
    fs::write(repo.join("extra.txt"), "extra\n").context("write second-author file")?;
    ctx.command(&["add", "extra.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "second author", "--no-verify"],
        repo.clone(),
        true,
    )?;
    let exact_after_move = ctx.command(&["describe", "--exact-match", "HEAD"], repo.clone(), false)?;
    assert_lbr_or_text(&exact_after_move, "--exact-match")?;
    ctx.command(
        &["config", "user.name", "Libra Integration"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["config", "user.email", "integration@example.invalid"],
        repo.clone(),
        true,
    )?;
    fs::write(repo.join("third.txt"), "third\n").context("write third file")?;
    ctx.command(&["add", "third.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "third", "--no-verify"],
        repo.clone(),
        true,
    )?;

    let summary = ctx.command(&["shortlog", "-s", "-n"], repo.clone(), true)?;
    assert_stdout_contains(&summary, "Libra Integration")?;
    assert_stdout_contains(&summary, "Second Author")?;
    let with_email = ctx.command(&["shortlog", "-s", "-e"], repo.clone(), true)?;
    assert_stdout_contains(&with_email, "<integration@example.invalid>")?;
    assert_stdout_contains(&with_email, "<second@example.invalid>")?;
    let limited = ctx.command(
        &["shortlog", "-s", searchable_rev.as_str()],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&limited, "Libra Integration")?;
    assert_not_contains(&limited, "Second Author")?;
    let top = ctx.command(&["shortlog", "-s", "-n", "--top", "1"], repo.clone(), false)?;
    assert_lbr_or_text(&top, "--top")?;
    let min_count = ctx.command(&["shortlog", "-s", "--min-count", "2"], repo.clone(), false)?;
    assert_lbr_or_text(&min_count, "--min-count")?;
    let format = ctx.command(&["shortlog", "--format", "%an %s"], repo.clone(), false)?;
    assert_lbr_or_text(&format, "--format")?;

    assert_json_ok(
        &ctx.command(&["--json", "grep", "needle"], repo.clone(), true)?,
        "grep",
    )?;
    assert_json_ok(
        &ctx.command(&["--json", "blame", "search.txt"], repo.clone(), true)?,
        "blame",
    )?;
    assert_json_ok(
        &ctx.command(
            &["--json", "describe", "--always", "HEAD"],
            repo.clone(),
            true,
        )?,
        "describe",
    )?;
    assert_json_ok(
        &ctx.command(&["--json", "shortlog"], repo.clone(), true)?,
        "shortlog",
    )?;
    assert_lbr_or_text(
        &ctx.command(&["describe", "no-such-revision"], repo.clone(), false)?,
        "invalid",
    )?;
    assert_lbr_or_text(
        &ctx.command(&["grep", "no-such-pattern"], repo, false)?,
        "not found",
    )?;
    Ok(())
}
