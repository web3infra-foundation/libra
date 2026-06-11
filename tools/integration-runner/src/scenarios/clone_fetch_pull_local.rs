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
    for tag in ["v1.10.0", "v1.1.0", "v1.2.0"] {
        ctx.gitfix(&["tag", tag], remote_dir.clone(), true)?;
    }

    let remote = remote_dir.to_string_lossy().to_string();
    let clone = clone_dir.to_string_lossy().to_string();
    let ls_remote = ctx.command(&["ls-remote", &remote], ctx.run_dir.clone(), true)?;
    assert_stdout_contains(&ls_remote, "refs/heads/main")?;
    ctx.command(
        &["ls-remote", "--heads", &remote, "main"],
        ctx.run_dir.clone(),
        true,
    )?;
    let json_ls_remote = ctx.command(
        &["--json", "ls-remote", "--heads", &remote],
        ctx.run_dir.clone(),
        true,
    )?;
    assert_json_ok(&json_ls_remote, "ls-remote")?;
    assert_stdout_contains(&json_ls_remote, "refs/heads/main")?;
    let sorted_tags = ctx.command(
        &["ls-remote", "--sort=version:refname", "--tags", &remote],
        ctx.run_dir.clone(),
        true,
    )?;
    let sorted_stdout = String::from_utf8_lossy(&sorted_tags.stdout);
    let v1 = sorted_stdout
        .find("refs/tags/v1.1.0")
        .context("missing v1.1.0 in sorted ls-remote tags")?;
    let v2 = sorted_stdout
        .find("refs/tags/v1.2.0")
        .context("missing v1.2.0 in sorted ls-remote tags")?;
    let v10 = sorted_stdout
        .find("refs/tags/v1.10.0")
        .context("missing v1.10.0 in sorted ls-remote tags")?;
    if !(v1 < v2 && v2 < v10) {
        bail!("ls-remote version sort order was not natural: {sorted_stdout}");
    }
    let no_match = ctx.command(
        &[
            "ls-remote",
            "--exit-code",
            "--heads",
            &remote,
            "no-such-branch",
        ],
        ctx.run_dir.clone(),
        false,
    )?;
    if no_match.status.code() != Some(2) {
        bail!(
            "ls-remote --exit-code no-match returned {:?}, expected 2",
            no_match.status.code()
        );
    }
    let get_url = ctx.command(
        &["ls-remote", "--get-url", &remote],
        ctx.run_dir.clone(),
        true,
    )?;
    assert_stdout_contains(&get_url, &remote)?;
    // `--symref` renders the HEAD symref advertised by the git fixture's
    // upload-pack capabilities; `-o/--server-option` is parsed-but-not-
    // forwarded and must not fail.
    let symref = ctx.command(
        &["ls-remote", "--symref", "-o", "trace=1", &remote],
        ctx.run_dir.clone(),
        true,
    )?;
    assert_stdout_contains(&symref, "ref: refs/heads/main")?;
    assert_stdout_contains(&symref, "refs/heads/main")?;
    let invalid_sort = ctx.command(
        &["ls-remote", "--sort=objectname", &remote],
        ctx.run_dir.clone(),
        false,
    )?;
    assert_lbr_or_text(&invalid_sort, "invalid sort key")?;
    ctx.command(&["clone", &remote, &clone], ctx.run_dir.clone(), true)?;
    let remotes = ctx.command(&["remote", "-v"], clone_dir.clone(), true)?;
    assert_stdout_contains(&remotes, &remote)?;
    let origin = ctx.command(&["remote", "get-url", "origin"], clone_dir.clone(), true)?;
    assert_stdout_contains(&origin, &remote)?;
    let set_branches = ctx.command(
        &["--json", "remote", "set-branches", "origin", "main"],
        clone_dir.clone(),
        true,
    )?;
    assert_json_ok(&set_branches, "remote")?;
    assert_stdout_contains(&set_branches, "refs/remotes/origin/main")?;
    let set_head = ctx.command(
        &["--json", "remote", "set-head", "origin", "main"],
        clone_dir.clone(),
        true,
    )?;
    assert_json_ok(&set_head, "remote")?;
    assert_stdout_contains(&set_head, "\"target\": \"main\"")?;
    let show_origin = ctx.command(
        &["remote", "show", "--no-query", "origin"],
        clone_dir.clone(),
        true,
    )?;
    assert_stdout_contains(&show_origin, "* remote origin")?;
    assert_stdout_contains(&show_origin, "HEAD branch: main")?;
    // `show -v` is accepted for Git compatibility and currently does not alter
    // the named-remote detail output; lock that contract black-box.
    let show_origin_verbose = ctx.command(
        &["remote", "show", "-v", "--no-query", "origin"],
        clone_dir.clone(),
        true,
    )?;
    assert_stdout_contains(&show_origin_verbose, "* remote origin")?;
    assert_stdout_contains(&show_origin_verbose, "HEAD branch: main")?;
    ctx.command(
        &["remote", "set-head", "origin", "-d"],
        clone_dir.clone(),
        true,
    )?;
    let set_head_auto = ctx.command(
        &["--json", "remote", "set-head", "origin", "--auto"],
        clone_dir.clone(),
        true,
    )?;
    assert_json_ok(&set_head_auto, "remote")?;
    assert_stdout_contains(&set_head_auto, "\"mode\": \"auto\"")?;
    assert_stdout_contains(&set_head_auto, "\"target\": \"main\"")?;
    let update_origin = ctx.command(
        &["--json", "remote", "update", "origin"],
        clone_dir.clone(),
        true,
    )?;
    assert_json_ok(&update_origin, "remote")?;
    assert_stdout_contains(&update_origin, "\"action\": \"update\"")?;
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
    let clone_tags = ctx.command(&["show-ref", "--tags"], clone_dir.clone(), true)?;
    assert_stdout_contains(&clone_tags, "refs/tags/v1.1.0")?;
    assert_stdout_contains(&clone_tags, "refs/tags/v1.2.0")?;
    assert_stdout_contains(&clone_tags, "refs/tags/v1.10.0")?;
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
            "--dissociate",
            &remote,
            &reference_clone_arg,
        ],
        ctx.run_dir.clone(),
        true,
    )?;
    ctx.command(&["fsck", "--connectivity-only"], reference_clone, true)?;

    // `--reference-if-able` with a missing path must degrade to a normal clone.
    let missing_reference = ctx
        .run
        .run_root
        .join("fixtures")
        .join(&ctx.id)
        .join("missing-reference");
    let missing_reference_arg = missing_reference.to_string_lossy().to_string();
    let ref_if_able = ctx.run_dir.join("ref-if-able-clone");
    let ref_if_able_arg = ref_if_able.to_string_lossy().to_string();
    ctx.command(
        &[
            "clone",
            "--reference-if-able",
            &missing_reference_arg,
            &remote,
            &ref_if_able_arg,
        ],
        ctx.run_dir.clone(),
        true,
    )?;
    ctx.command(&["fsck", "--connectivity-only"], ref_if_able, true)?;

    let local_copy = ctx.run_dir.join("local-copy");
    let local_copy_arg = local_copy.to_string_lossy().to_string();
    ctx.command(
        &[
            "clone",
            "--local",
            "--no-hardlinks",
            &remote,
            &local_copy_arg,
        ],
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
    ctx.gitfix(&["tag", "v2.0.0"], remote_dir.clone(), true)?;

    ctx.command(&["fetch", "origin", "main"], clone_dir.clone(), true)?;
    let fetched_tags = ctx.command(&["show-ref", "--tags"], clone_dir.clone(), true)?;
    assert_stdout_contains(&fetched_tags, "refs/tags/v2.0.0")?;
    ctx.command(&["fetch", "--all"], clone_dir.clone(), true)?;
    ctx.command(&["show-ref", "--heads"], clone_dir.clone(), true)?;

    // advanced fetch flags for plan maintenance (prune/porcelain/dry-run/tags per "继续维护" in improvement/fetch.md)
    ctx.command(&["fetch", "--prune", "origin"], clone_dir.clone(), true)?;
    // Advance the remote so the porcelain fetch observes a real fast-forward
    // (`--porcelain` prints nothing when every ref is already up to date).
    fs::write(remote_dir.join("porcelain.txt"), "porcelain seed\n")
        .context("write porcelain seed commit")?;
    ctx.gitfix(&["add", "porcelain.txt"], remote_dir.clone(), true)?;
    ctx.gitfix(
        &["commit", "-m", "test: porcelain seed commit"],
        remote_dir.clone(),
        true,
    )?;
    let porcelain = ctx.command(
        &["fetch", "--porcelain", "origin", "main"],
        clone_dir.clone(),
        true,
    )?;
    assert_stdout_contains(&porcelain, "refs/remotes/origin/main")?;
    ctx.command(
        &["fetch", "--dry-run", "origin", "main"],
        clone_dir.clone(),
        true,
    )?;
    ctx.command(
        &["fetch", "--tags", "--force", "origin"],
        clone_dir.clone(),
        true,
    )?;
    ctx.command(&["fetch", "--no-tags", "origin"], clone_dir.clone(), true)?;
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

    let squash_clone = ctx.run_dir.join("pull-squash-clone");
    let squash_clone_arg = squash_clone.to_string_lossy().to_string();
    ctx.command(
        &["clone", &remote, &squash_clone_arg],
        ctx.run_dir.clone(),
        true,
    )?;
    ctx.command(
        &["config", "set", "user.name", "Libra Pull Squash"],
        squash_clone.clone(),
        true,
    )?;
    ctx.command(
        &["config", "set", "user.email", "pull-squash@example.invalid"],
        squash_clone.clone(),
        true,
    )?;
    fs::write(squash_clone.join("squash-local.txt"), "squash local\n")
        .context("write squash local commit")?;
    ctx.command(&["add", "squash-local.txt"], squash_clone.clone(), true)?;
    ctx.command(
        &["commit", "-m", "test: squash local commit"],
        squash_clone.clone(),
        true,
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
    let squash_pull = ctx.command(
        &["pull", "--squash", "origin", "main"],
        squash_clone.clone(),
        true,
    )?;
    assert_stdout_contains(&squash_pull, "Squash commit -- not updating HEAD.")?;
    assert_not_contains(&squash_pull, "Fast-forward")?;
    let squash_readme =
        fs::read_to_string(squash_clone.join("README.md")).context("read squash README")?;
    if !squash_readme.contains("third") {
        bail!("squash pull README did not contain third commit content: {squash_readme}");
    }
    ensure_file(squash_clone.join("squash-local.txt"))?;

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

    // `pull --ff` must override a configured `pull.ff=false` and fast-forward;
    // `--no-squash` / `--commit` ride along as accepted merge-flag overrides
    // (no-ops once the fast-forward resolution wins).
    ctx.command(
        &["config", "set", "pull.ff", "false"],
        json_clone.clone(),
        true,
    )?;
    let ff_pull = ctx.command(
        &["pull", "--ff", "--no-squash", "--commit", "origin", "main"],
        json_clone.clone(),
        true,
    )?;
    assert_stdout_contains(&ff_pull, "Fast-forward")?;
    let ff_readme =
        fs::read_to_string(json_clone.join("README.md")).context("read ff-pulled README")?;
    if !ff_readme.contains("third") {
        bail!("pull --ff README did not contain third commit content: {ff_readme}");
    }
    ctx.command(&["fsck", "--connectivity-only"], json_clone.clone(), true)?;

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
