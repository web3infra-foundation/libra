use super::prelude::*;

pub(crate) fn scenario_clone_fetch_pull_local(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let remote_dir = ctx
        .run
        .run_root
        .join("fixtures")
        .join(&ctx.id)
        .join("git-source");
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
    for tag in ["v1.1.0", "v1.2.0"] {
        ctx.gitfix(&["tag", tag], remote_dir.clone(), true)?;
    }

    let remote = remote_dir.to_string_lossy().to_string();
    let clone_dir = ctx.run_dir.join("clone");
    let clone = clone_dir.to_string_lossy().to_string();
    let ls_remote = ctx.command(&["ls-remote", &remote], ctx.run_dir.clone(), true)?;
    assert_stdout_contains(&ls_remote, "refs/heads/main")?;
    assert_stdout_contains(
        &ctx.command(
            &["ls-remote", "--get-url", &remote],
            ctx.run_dir.clone(),
            true,
        )?,
        &remote,
    )?;
    assert_stdout_contains(
        &ctx.command(&["ls-remote", "--tags", &remote], ctx.run_dir.clone(), true)?,
        "refs/tags/v1.1.0",
    )?;
    let sorted_tags = ctx.command(
        &["ls-remote", "--tags", "--sort=version:refname", &remote],
        ctx.run_dir.clone(),
        true,
    )?;
    let sorted_tags_stdout = String::from_utf8_lossy(&sorted_tags.stdout);
    let v1_1 = sorted_tags_stdout
        .find("refs/tags/v1.1.0")
        .context("sorted ls-remote tags omitted v1.1.0")?;
    let v1_2 = sorted_tags_stdout
        .find("refs/tags/v1.2.0")
        .context("sorted ls-remote tags omitted v1.2.0")?;
    if v1_1 >= v1_2 {
        bail!("ls-remote --sort=version:refname returned unexpected tag order: {sorted_tags_stdout}");
    }
    ctx.command(
        &["ls-remote", "--exit-code", &remote, "main"],
        ctx.run_dir.clone(),
        true,
    )?;
    let missing_ref = ctx.command(
        &["ls-remote", "--exit-code", &remote, "no-match"],
        ctx.run_dir.clone(),
        false,
    )?;
    if missing_ref.status.code() != Some(2) {
        bail!(
            "ls-remote --exit-code no-match returned {:?}, expected 2",
            missing_ref.status.code()
        );
    }
    // `--symref` against a local *Git* remote (this fixture is a `git init`
    // repo, served via `git-upload-pack`, which advertises
    // `symref=HEAD:refs/heads/main`): the `ref:` line for HEAD must appear above
    // HEAD's own OID line. (Local *Libra* repos advertise no `symref=`
    // capability and print no `ref:` line — covered by ls_remote unit tests.)
    let symref = ctx.command(&["ls-remote", "--symref", &remote], ctx.run_dir.clone(), true)?;
    assert_stdout_contains(&symref, "ref: refs/heads/main\tHEAD")?;
    let json_ls_remote = ctx.command(
        &["--json", "ls-remote", "--heads", &remote, "main"],
        ctx.run_dir.clone(),
        true,
    )?;
    assert_json_ok(&json_ls_remote, "ls-remote")?;
    assert_stdout_contains(&json_ls_remote, "refs/heads/main")?;

    ctx.command(&["clone", &remote, &clone], ctx.run_dir.clone(), true)?;
    assert_stdout_contains(
        &ctx.command(&["remote", "-v"], clone_dir.clone(), true)?,
        &remote,
    )?;
    assert_stdout_contains(
        &ctx.command(&["remote", "get-url", "origin"], clone_dir.clone(), true)?,
        &remote,
    )?;
    assert_stdout_contains(
        &ctx.command(&["ls-remote", "--get-url", "origin"], clone_dir.clone(), true)?,
        &remote,
    )?;
    assert_stdout_contains(
        &ctx.command(&["remote", "show"], clone_dir.clone(), true)?,
        "origin",
    )?;
    assert_stdout_contains(
        &ctx.command(&["log", "--oneline"], clone_dir.clone(), true)?,
        "test: seed remote",
    )?;
    let readme = fs::read_to_string(clone_dir.join("README.md")).context("read cloned README")?;
    if !readme.contains("first") {
        bail!("cloned README did not contain first commit content: {readme}");
    }
    ctx.command(&["fsck", "--connectivity-only"], clone_dir.clone(), true)?;

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

    let rebase_clone = ctx.run_dir.join("rebase-clone");
    let rebase_clone_arg = rebase_clone.to_string_lossy().to_string();
    ctx.command(
        &["clone", &remote, &rebase_clone_arg],
        ctx.run_dir.clone(),
        true,
    )?;
    ctx.command(
        &["config", "set", "user.name", "Libra Pull Rebase"],
        rebase_clone.clone(),
        true,
    )?;
    ctx.command(
        &["config", "set", "user.email", "pull-rebase@example.invalid"],
        rebase_clone.clone(),
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
    fs::write(rebase_clone.join("local.txt"), "local only\n").context("write local commit")?;
    ctx.command(&["add", "local.txt"], rebase_clone.clone(), true)?;
    ctx.command(
        &["commit", "-m", "test: local commit", "--no-verify"],
        rebase_clone.clone(),
        true,
    )?;
    ctx.command(
        &["pull", "--rebase", "origin", "main"],
        rebase_clone.clone(),
        true,
    )?;
    ensure_file(rebase_clone.join("local.txt"))?;
    let readme =
        fs::read_to_string(rebase_clone.join("README.md")).context("read rebased README")?;
    if !readme.contains("third") {
        bail!("rebased README did not contain third commit content: {readme}");
    }
    ctx.command(&["fsck", "--connectivity-only"], rebase_clone, true)?;

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
