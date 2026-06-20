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

    super::object_readback_show_ref::assert_initial_show_ref_readback(ctx, &repo, &head_id)?;
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
    let no_filters_blob = ctx.command(
        &["hash-object", "--no-filters", "loose.txt"],
        repo.clone(),
        true,
    )?;
    if stdout_trim(&no_filters_blob) != blob_id {
        bail!("hash-object --no-filters id did not match default blob id");
    }
    let stdin_blob = ctx.command_with_stdin(
        &["hash-object", "--stdin"],
        repo.clone(),
        "loose blob\n",
        true,
    )?;
    if stdout_trim(&stdin_blob) != blob_id {
        bail!("hash-object --stdin id did not match file blob id");
    }
    let stdin_path_json = ctx.command_with_stdin(
        &["--json", "hash-object", "--stdin", "--path", "loose.txt"],
        repo.clone(),
        "loose blob\n",
        true,
    )?;
    assert_json_ok(&stdin_path_json, "hash-object")?;
    assert_stdout_contains(&stdin_path_json, "\"source\": \"loose.txt\"")?;
    let path_no_filters = ctx.command_with_stdin(
        &[
            "hash-object",
            "--stdin",
            "--path",
            "loose.txt",
            "--no-filters",
        ],
        repo.clone(),
        "loose blob\n",
        false,
    )?;
    assert_lbr_or_text(&path_no_filters, "cannot be used with")?;
    let bad_type = ctx.command(
        &["hash-object", "-t", "bogus", "loose.txt"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_type, "unsupported object type")?;

    super::object_readback_rev_list::assert_rev_list_readback(ctx, &repo, &head_id)?;
    ctx.command(&["fsck"], repo.clone(), true)?;
    ctx.command(&["fsck", "--connectivity-only"], repo.clone(), true)?;
    ctx.command(&["fsck", &head_id], repo.clone(), true)?;
    let latest_head = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?);
    ctx.command(
        &["tag", "-m", "release fixture", "v1.0"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["tag", "v1-light"], repo.clone(), true)?;
    super::object_readback_show_ref::assert_tagged_show_ref_readback(ctx, &repo, &latest_head)?;
    let refs_at_head = ctx.command(
        &[
            "for-each-ref",
            "--points-at",
            &latest_head,
            "--format=%(refname) %(objecttype)",
        ],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&refs_at_head, "refs/heads/main commit")?;
    assert_stdout_contains(&refs_at_head, "refs/tags/v1.0 tag")?;
    assert_stdout_contains(&refs_at_head, "refs/tags/v1-light commit")?;
    assert_json_ok(
        &ctx.command(
            &["--json", "for-each-ref", "--points-at", &latest_head],
            repo.clone(),
            true,
        )?,
        "for-each-ref",
    )?;
    let missing = ctx.command(&["cat-file", "-t", "deadbeef"], repo, false)?;
    assert_lbr_or_text(&missing, "object not found")?;
    Ok(())
}
