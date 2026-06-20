use super::prelude::*;

pub(crate) fn assert_initial_show_ref_readback(
    ctx: &mut ScenarioCtx<'_>,
    repo: &std::path::PathBuf,
    head_id: &str,
) -> Result<()> {
    assert_stdout_contains(&ctx.command(&["show-ref", "--head"], repo.clone(), true)?, "HEAD")?;
    assert_not_contains(
        &ctx.command(&["show-ref", "--head", "--no-head"], repo.clone(), true)?,
        " HEAD",
    )?;

    let heads_ref = ctx.command(&["show-ref", "--heads"], repo.clone(), true)?;
    assert_stdout_contains(&heads_ref, "refs/heads/main")?;
    let branches_ref = ctx.command(&["show-ref", "--branches"], repo.clone(), true)?;
    if branches_ref.stdout != heads_ref.stdout {
        bail!("show-ref --branches did not match --heads output");
    }

    let hash_only = ctx.command(&["show-ref", "--hash", "--heads"], repo.clone(), true)?;
    if stdout_trim(&hash_only) != head_id {
        bail!("show-ref --hash --heads returned unexpected hash");
    }
    let Some(head_short_12) = head_id.get(..12) else {
        bail!("rev-parse HEAD returned an id shorter than 12 characters: {head_id}");
    };
    let abbreviated = ctx.command(&["show-ref", "--abbrev=12", "--heads"], repo.clone(), true)?;
    let abbreviated_output = stdout_trim(&abbreviated);
    let Some(abbreviated_hash) = abbreviated_output.split_whitespace().next() else {
        bail!("show-ref --abbrev=12 --heads returned empty output");
    };
    if abbreviated_hash != head_short_12 {
        bail!("show-ref --abbrev=12 --heads returned unexpected hash");
    }
    let hash_width = ctx.command(&["show-ref", "--hash=12", "--heads"], repo.clone(), true)?;
    if stdout_trim(&hash_width) != head_short_12 {
        bail!("show-ref --hash=12 --heads returned unexpected hash");
    }
    let no_hash = ctx.command(&["show-ref", "--no-hash", "--heads"], repo.clone(), true)?;
    if stdout_trim(&no_hash) != head_id {
        bail!("show-ref --no-hash --heads returned unexpected hash");
    }
    let no_abbrev = ctx.command(
        &["show-ref", "--abbrev=12", "--no-abbrev", "--heads"],
        repo.clone(),
        true,
    )?;
    let Some(no_abbrev_hash) = stdout_trim(&no_abbrev)
        .split_whitespace()
        .next()
        .map(str::to_string)
    else {
        bail!("show-ref --abbrev=12 --no-abbrev --heads returned empty output");
    };
    if no_abbrev_hash != head_id {
        bail!("show-ref --abbrev=12 --no-abbrev did not restore full hash");
    }

    assert_json_ok(
        &ctx.command(&["--json", "show-ref", "--abbrev=12", "--heads"], repo.clone(), true)?,
        "show-ref",
    )?;
    assert_stdout_contains(
        &ctx.command(&["show-ref", "--verify", "refs/heads/main"], repo.clone(), true)?,
        "refs/heads/main",
    )?;
    let exists_ref = ctx.command(&["show-ref", "--exists", "refs/heads/main"], repo.clone(), true)?;
    if !exists_ref.stdout.is_empty() {
        bail!("show-ref --exists refs/heads/main should be silent on success");
    }
    assert_stdout_contains(
        &ctx.command(&["show-ref", "--verify", "--no-verify", "main"], repo.clone(), true)?,
        "refs/heads/main",
    )?;
    assert_stdout_contains(
        &ctx.command(
            &["show-ref", "--exists", "--no-exists", "refs/heads/main"],
            repo.clone(),
            true,
        )?,
        "refs/heads/main",
    )?;

    Ok(())
}

pub(crate) fn assert_tagged_show_ref_readback(
    ctx: &mut ScenarioCtx<'_>,
    repo: &std::path::PathBuf,
    latest_head: &str,
) -> Result<()> {
    let branch_scope_reset = ctx.command(
        &["show-ref", "--branches", "--no-branches"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&branch_scope_reset, "refs/heads/main")?;
    assert_stdout_contains(&branch_scope_reset, "refs/tags/v1.0")?;

    let tag_scope_reset = ctx.command(&["show-ref", "--tags", "--no-tags"], repo.clone(), true)?;
    assert_stdout_contains(&tag_scope_reset, "refs/heads/main")?;
    assert_stdout_contains(&tag_scope_reset, "refs/tags/v1.0")?;

    let dereferenced_tag = ctx.command(
        &["show-ref", "--dereference", "--tags", "v1.0"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&dereferenced_tag, "refs/tags/v1.0^{}")?;
    assert_stdout_contains(&dereferenced_tag, latest_head)?;
    assert_not_contains(
        &ctx.command(
            &[
                "show-ref",
                "--dereference",
                "--no-dereference",
                "--tags",
                "v1.0",
            ],
            repo.clone(),
            true,
        )?,
        "^{}",
    )?;

    Ok(())
}
