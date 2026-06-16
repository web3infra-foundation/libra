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

    fs::write(repo.join("docs/rev-list.md"), "rev-list second\n")
        .context("write rev-list second fixture")?;
    ctx.command(&["add", "docs/rev-list.md"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "test: rev-list second", "--no-verify"],
        repo.clone(),
        true,
    )?;
    let latest_id = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?);
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
    let rev_parents = ctx.command(&["rev-list", "--parents", "HEAD"], repo.clone(), true)?;
    let rev_parents_output = String::from_utf8_lossy(&rev_parents.stdout);
    let Some(first_parent_line) = rev_parents_output.lines().next() else {
        bail!("rev-list --parents HEAD returned empty output");
    };
    if !first_parent_line.starts_with(&latest_id) || !first_parent_line.ends_with(&head_id) {
        bail!("rev-list --parents HEAD did not include the current HEAD followed by its parent");
    }
    let rev_timestamp = ctx.command(&["rev-list", "--timestamp", "HEAD"], repo.clone(), true)?;
    let rev_timestamp_output = String::from_utf8_lossy(&rev_timestamp.stdout);
    let Some(first_timestamp_line) = rev_timestamp_output.lines().next() else {
        bail!("rev-list --timestamp HEAD returned empty output");
    };
    let timestamp_fields = first_timestamp_line.split_whitespace().collect::<Vec<_>>();
    if timestamp_fields.len() != 2
        || timestamp_fields[0].parse::<u64>().is_err()
        || timestamp_fields[1] != latest_id
    {
        bail!("rev-list --timestamp HEAD did not use Git-compatible `timestamp commit` output");
    }
    let rev_timestamp_parents = ctx.command(
        &["rev-list", "--timestamp", "--parents", "HEAD"],
        repo.clone(),
        true,
    )?;
    let rev_timestamp_parents_output =
        String::from_utf8_lossy(&rev_timestamp_parents.stdout);
    let Some(first_timestamp_parent_line) = rev_timestamp_parents_output.lines().next() else {
        bail!("rev-list --timestamp --parents HEAD returned empty output");
    };
    let timestamp_parent_fields = first_timestamp_parent_line
        .split_whitespace()
        .collect::<Vec<_>>();
    if timestamp_parent_fields.len() != 3
        || timestamp_parent_fields[0].parse::<u64>().is_err()
        || timestamp_parent_fields[1] != latest_id
        || timestamp_parent_fields[2] != head_id
    {
        bail!(
            "rev-list --timestamp --parents HEAD did not use Git-compatible `timestamp commit parent` output"
        );
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
    ctx.command(&["tag", "v1-light"], repo.clone(), true)?;
    let dereferenced_tag = ctx.command(
        &["show-ref", "--dereference", "--tags", "v1.0"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&dereferenced_tag, "refs/tags/v1.0^{}")?;
    assert_stdout_contains(&dereferenced_tag, &latest_head)?;
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
