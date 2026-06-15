use super::prelude::*;

pub(crate) fn scenario_object_readback(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("object-repo");
    ctx.command(&["init", "object-repo"], ctx.run_dir.clone(), true)?;
    ctx.command(
        &["config", "user.name", "Libra Object Test"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["config", "user.email", "object@example.invalid"],
        repo.clone(),
        true,
    )?;
    fs::create_dir_all(repo.join("docs")).context("create docs fixture dir")?;
    fs::write(repo.join("README.md"), "object root\n").context("write README fixture")?;
    fs::write(repo.join("docs/guide.md"), "object docs\n").context("write docs fixture")?;
    ctx.command(&["add", "README.md", "docs/guide.md"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "test: object readback", "--no-verify"],
        repo.clone(),
        true,
    )?;

    let head = ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?;
    let head_id = stdout_trim(&head);
    if head_id.len() < 40 {
        bail!("rev-parse HEAD returned an unexpectedly short id: {head_id}");
    }
    let short = ctx.command(&["rev-parse", "--short", "HEAD"], repo.clone(), true)?;
    if !head_id.starts_with(&stdout_trim(&short)) {
        bail!("rev-parse --short HEAD was not a prefix of HEAD");
    }
    assert_stdout_contains(
        &ctx.command(&["rev-parse", "--show-toplevel"], repo.clone(), true)?,
        repo.to_string_lossy().as_ref(),
    )?;
    assert_json_ok(
        &ctx.command(&["--json", "rev-parse", "HEAD"], repo.clone(), true)?,
        "rev-parse",
    )?;
    let missing_rev = ctx.command(&["rev-parse", "no-such-revision"], repo.clone(), false)?;
    assert_lbr_or_text(&missing_rev, "failed to resolve")?;

    assert_stdout_contains(
        &ctx.command(&["show", "--no-patch", "HEAD"], repo.clone(), true)?,
        "test: object readback",
    )?;
    assert_stdout_contains(
        &ctx.command(&["show", "HEAD:docs/guide.md"], repo.clone(), true)?,
        "object docs",
    )?;
    assert_json_ok(
        &ctx.command(&["--json", "show", "HEAD"], repo.clone(), true)?,
        "show",
    )?;

    assert_stdout_contains(
        &ctx.command(&["show-ref", "--head"], repo.clone(), true)?,
        "HEAD",
    )?;
    assert_stdout_contains(
        &ctx.command(&["show-ref", "--heads"], repo.clone(), true)?,
        "refs/heads/main",
    )?;
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
    assert_json_ok(
        &ctx.command(
            &["--json", "show-ref", "--abbrev=12", "--heads"],
            repo.clone(),
            true,
        )?,
        "show-ref",
    )?;
    let object_type = ctx.command(&["cat-file", "-t", &head_id], repo.clone(), true)?;
    assert_stdout_contains(&object_type, "commit")?;
    ctx.command(&["cat-file", "-s", &head_id], repo.clone(), true)?;
    assert_stdout_contains(
        &ctx.command(&["cat-file", "-p", &head_id], repo.clone(), true)?,
        "tree ",
    )?;
    ctx.command(&["cat-file", "-e", &head_id], repo.clone(), true)?;

    fs::write(repo.join("loose.txt"), "loose blob\n").context("write loose blob fixture")?;
    let blob = ctx.command(&["hash-object", "-w", "loose.txt"], repo.clone(), true)?;
    let blob_id = stdout_trim(&blob);
    assert_stdout_contains(
        &ctx.command(&["cat-file", "-t", &blob_id], repo.clone(), true)?,
        "blob",
    )?;
    assert_stdout_contains(
        &ctx.command(&["cat-file", "-p", &blob_id], repo.clone(), true)?,
        "loose blob",
    )?;
    assert_stdout_contains(
        &ctx.command(&["show", &blob_id], repo.clone(), true)?,
        "loose blob",
    )?;
    assert_json_ok(
        &ctx.command(&["--json", "hash-object", "loose.txt"], repo.clone(), true)?,
        "hash-object",
    )?;
    let stdin_blob = ctx.command_with_stdin(
        &["hash-object", "--stdin"],
        repo.clone(),
        "loose blob\n",
        true,
    )?;
    if stdout_trim(&stdin_blob) != blob_id {
        bail!("hash-object --stdin id did not match file blob id");
    }
    let bad_type = ctx.command(
        &["hash-object", "-t", "bogus", "loose.txt"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_type, "unsupported object type")?;

    fs::write(repo.join("docs/rev-list.md"), "rev-list second\n")
        .context("write rev-list second fixture")?;
    ctx.command(&["add", "docs/rev-list.md"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "test: rev-list second", "--no-verify"],
        repo.clone(),
        true,
    )?;
    let rev_list = ctx.command(&["rev-list", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&rev_list, &head_id)?;
    let rev_count = ctx.command(&["rev-list", "--count", "HEAD"], repo.clone(), true)?;
    if stdout_trim(&rev_count) != "2" {
        bail!("rev-list --count HEAD returned unexpected count");
    }
    let rev_limit = ctx.command(&["rev-list", "-n", "1", "HEAD"], repo.clone(), true)?;
    let rev_limit_stdout = String::from_utf8_lossy(&rev_limit.stdout);
    if rev_limit_stdout.lines().count() != 1 {
        bail!("rev-list -n 1 HEAD returned more than one commit");
    }
    let rev_skip = ctx.command(
        &["rev-list", "--skip", "1", "--max-count", "1", "HEAD"],
        repo.clone(),
        true,
    )?;
    if stdout_trim(&rev_skip) != head_id {
        bail!("rev-list --skip 1 --max-count 1 HEAD did not return the parent commit");
    }
    assert_json_ok(
        &ctx.command(&["--json", "rev-list", "HEAD"], repo.clone(), true)?,
        "rev-list",
    )?;
    ctx.command(&["fsck"], repo.clone(), true)?;
    ctx.command(&["fsck", "--connectivity-only"], repo.clone(), true)?;
    ctx.command(&["fsck", &head_id], repo.clone(), true)?;
    let latest_head = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?);
    ctx.command(
        &["tag", "-m", "release fixture", "v1.0"],
        repo.clone(),
        true,
    )?;
    let dereferenced_tag = ctx.command(
        &["show-ref", "--dereference", "--tags", "v1.0"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&dereferenced_tag, "refs/tags/v1.0^{}")?;
    assert_stdout_contains(&dereferenced_tag, &latest_head)?;
    let missing = ctx.command(&["cat-file", "-t", "deadbeef"], repo, false)?;
    assert_lbr_or_text(&missing, "object not found")?;
    Ok(())
}
