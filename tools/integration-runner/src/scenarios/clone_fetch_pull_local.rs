use super::prelude::*;

pub(crate) fn scenario_clone_fetch_pull_local(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let remote_dir = ctx
        .run
        .run_root
        .join("fixtures")
        .join(&ctx.id)
        .join("git-source");
    let clone_dir = ctx.run_dir.join("clone");
    fs::create_dir_all(&remote_dir).context("create git fixture dir")?;
    ctx.gitfix(&["init", "-b", "main"], remote_dir.clone(), true)?;
    ctx.gitfix(
        &["config", "user.name", "Libra Remote Seed"],
        remote_dir.clone(),
        true,
    )?;
    ctx.gitfix(
        &["config", "user.email", "remote-seed@example.invalid"],
        remote_dir.clone(),
        true,
    )?;
    fs::write(remote_dir.join("README.md"), "first\n").context("write first remote commit")?;
    ctx.gitfix(&["add", "README.md"], remote_dir.clone(), true)?;
    ctx.gitfix(
        &["commit", "-m", "test: seed remote"],
        remote_dir.clone(),
        true,
    )?;

    let remote = remote_dir.to_string_lossy().to_string();
    let clone = clone_dir.to_string_lossy().to_string();
    let ls_remote = ctx.command(&["ls-remote", &remote], ctx.run_dir.clone(), true)?;
    assert_stdout_contains(&ls_remote, "refs/heads/main")?;
    ctx.command(
        &["ls-remote", "--heads", &remote, "main"],
        ctx.run_dir.clone(),
        true,
    )?;
    ctx.command(&["clone", &remote, &clone], ctx.run_dir.clone(), true)?;
    let remotes = ctx.command(&["remote", "-v"], clone_dir.clone(), true)?;
    assert_stdout_contains(&remotes, &remote)?;
    let origin = ctx.command(&["remote", "get-url", "origin"], clone_dir.clone(), true)?;
    assert_stdout_contains(&origin, &remote)?;
    ctx.command(
        &["remote", "add", "mirror", &remote],
        clone_dir.clone(),
        true,
    )?;
    let mirror = ctx.command(&["remote", "get-url", "mirror"], clone_dir.clone(), true)?;
    assert_stdout_contains(&mirror, &remote)?;
    ctx.command(
        &["config", "set", "user.name", "Libra Clone Local"],
        clone_dir.clone(),
        true,
    )?;
    ctx.command(
        &["config", "set", "user.email", "clone-local@example.invalid"],
        clone_dir.clone(),
        true,
    )?;
    let log = ctx.command(&["log", "--oneline"], clone_dir.clone(), true)?;
    assert_stdout_contains(&log, "test: seed remote")?;
    let readme = fs::read_to_string(clone_dir.join("README.md")).context("read cloned README")?;
    if !readme.contains("first") {
        bail!("cloned README did not contain first commit content: {readme}");
    }
    let bare_clone = ctx.run_dir.join("bare-clone.git");
    let bare_clone_arg = bare_clone.to_string_lossy().to_string();
    ctx.command(
        &["clone", "--bare", &remote, &bare_clone_arg],
        ctx.run_dir.clone(),
        true,
    )?;
    ensure_file(bare_clone.join("libra.db"))?;

    let single_branch = ctx.run_dir.join("single-branch");
    let single_branch_arg = single_branch.to_string_lossy().to_string();
    ctx.command(
        &[
            "clone",
            "--single-branch",
            "-b",
            "main",
            &remote,
            &single_branch_arg,
        ],
        ctx.run_dir.clone(),
        true,
    )?;
    assert_stdout_contains(
        &ctx.command(&["branch", "--show-current"], single_branch.clone(), true)?,
        "main",
    )?;

    let no_checkout = ctx.run_dir.join("no-checkout");
    let no_checkout_arg = no_checkout.to_string_lossy().to_string();
    ctx.command(
        &[
            "clone",
            "--origin",
            "upstream",
            "--no-checkout",
            &remote,
            &no_checkout_arg,
        ],
        ctx.run_dir.clone(),
        true,
    )?;
    let upstream = ctx.command(
        &["config", "get", "remote.upstream.url"],
        no_checkout.clone(),
        true,
    )?;
    assert_stdout_contains(&upstream, &remote)?;
    if no_checkout.join("README.md").exists() {
        bail!("--no-checkout clone unexpectedly materialized README.md");
    }

    let jobs_clone = ctx.run_dir.join("jobs-clone");
    let jobs_clone_arg = jobs_clone.to_string_lossy().to_string();
    ctx.command(
        &["clone", "--jobs", "2", &remote, &jobs_clone_arg],
        ctx.run_dir.clone(),
        true,
    )?;

    let reference_clone = ctx.run_dir.join("reference-clone");
    let reference_clone_arg = reference_clone.to_string_lossy().to_string();
    ctx.command(
        &[
            "clone",
            "--reference",
            &clone,
            &remote,
            &reference_clone_arg,
        ],
        ctx.run_dir.clone(),
        true,
    )?;
    ctx.command(&["fsck", "--connectivity-only"], reference_clone, true)?;

    let local_copy = ctx.run_dir.join("local-copy");
    let local_copy_arg = local_copy.to_string_lossy().to_string();
    ctx.command(
        &["clone", "--local", "--no-hardlinks", &remote, &local_copy_arg],
        ctx.run_dir.clone(),
        true,
    )?;

    let shared_copy = ctx.run_dir.join("shared-copy");
    let shared_copy_arg = shared_copy.to_string_lossy().to_string();
    ctx.command(
        &["clone", "--shared", &remote, &shared_copy_arg],
        ctx.run_dir.clone(),
        true,
    )?;
    ctx.command(&["fsck", "--connectivity-only"], shared_copy, true)?;

    let json_clone = ctx.run_dir.join("clone-json");
    let json_clone_arg = json_clone.to_string_lossy().to_string();
    assert_json_ok(
        &ctx.command(
            &["--json", "clone", &remote, &json_clone_arg],
            ctx.run_dir.clone(),
            true,
        )?,
        "clone",
    )?;

    fs::write(remote_dir.join("README.md"), "first\nsecond\n")
        .context("write second remote commit")?;
    ctx.gitfix(&["add", "README.md"], remote_dir.clone(), true)?;
    ctx.gitfix(
        &["commit", "-m", "test: second remote commit"],
        remote_dir.clone(),
        true,
    )?;

    ctx.command(&["fetch", "origin", "main"], clone_dir.clone(), true)?;
    ctx.command(&["fetch", "--all"], clone_dir.clone(), true)?;
    ctx.command(&["show-ref", "--heads"], clone_dir.clone(), true)?;
    ctx.command(
        &["pull", "--ff-only", "origin", "main"],
        clone_dir.clone(),
        true,
    )?;
    let readme = fs::read_to_string(clone_dir.join("README.md")).context("read pulled README")?;
    if !readme.contains("second") {
        bail!("pulled README did not contain second commit content: {readme}");
    }
    ctx.command(&["fsck", "--connectivity-only"], clone_dir.clone(), true)?;
    assert_json_ok(
        &ctx.command(&["--json", "log", "--oneline"], clone_dir.clone(), true)?,
        "log",
    )?;
    assert_json_ok(
        &ctx.command(&["--json", "fetch", "origin"], json_clone.clone(), true)?,
        "fetch",
    )?;
    assert_json_ok(
        &ctx.command(
            &["--json", "pull", "--ff-only", "origin", "main"],
            json_clone.clone(),
            true,
        )?,
        "pull",
    )?;

    fs::write(clone_dir.join("clone-local.txt"), "local only\n")
        .context("write clone local commit")?;
    ctx.command(&["add", "clone-local.txt"], clone_dir.clone(), true)?;
    ctx.command(
        &["commit", "-m", "test: clone local commit"],
        clone_dir.clone(),
        true,
    )?;
    fs::write(remote_dir.join("README.md"), "first\nsecond\nthird\n")
        .context("write third remote commit")?;
    ctx.gitfix(&["add", "README.md"], remote_dir.clone(), true)?;
    ctx.gitfix(
        &["commit", "-m", "test: third remote commit"],
        remote_dir.clone(),
        true,
    )?;
    ctx.command(
        &["pull", "--rebase", "origin", "main"],
        clone_dir.clone(),
        true,
    )?;
    let readme = fs::read_to_string(clone_dir.join("README.md")).context("read rebased README")?;
    if !readme.contains("third") {
        bail!("rebased README did not contain third commit content: {readme}");
    }
    ensure_file(clone_dir.join("clone-local.txt"))?;
    assert_json_ok(
        &ctx.command(&["--json", "log", "-n", "5"], clone_dir.clone(), true)?,
        "log",
    )?;

    let bad_fetch = ctx.command(
        &["fetch", "origin", "no-such-branch"],
        clone_dir.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_fetch, "couldn't find remote ref")?;
    let bad_clone_target = ctx
        .run_dir
        .join("missing-clone")
        .to_string_lossy()
        .to_string();
    let missing = ctx
        .run
        .run_root
        .join("fixtures")
        .join(&ctx.id)
        .join("missing.git");
    let missing_remote = missing.to_string_lossy().to_string();
    let bad_clone = ctx.command(
        &["clone", &missing_remote, &bad_clone_target],
        ctx.run_dir.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_clone, "No such file")?;
    Ok(())
}
