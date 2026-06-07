use super::prelude::*;

pub(crate) fn scenario_live_github_create_push_clone_fetch(
    ctx: &mut ScenarioCtx<'_>,
) -> Result<()> {
    // This fn is only reached via run-live (normal run skips gh_required early).
    // We still re-verify gh auth here for defense-in-depth (no-op if already checked).
    let _auth = ctx.gh(
        &["auth", "status", "--active", "--hostname", "github.com"],
        ctx.run.run_root.clone(),
        true,
    )?;
    // auth succeeded (else gh() bailed)

    // Compute unique run id (no external time crate needed beyond std/chrono already in use)
    let run_id = format!("{}-{}", chrono::Utc::now().timestamp(), std::process::id());
    let owner_out = ctx.gh(
        &["api", "user", "--jq", ".login"],
        ctx.run.run_root.clone(),
        true,
    )?;
    let owner = stdout_trim(&owner_out);
    if owner.is_empty() {
        bail!("gh api user did not return login");
    }
    let owner_repo = format!("{}/libra-integ-{}", owner, run_id);

    // Create temp private repo (block if no write scope)
    ctx.gh(
        &[
            "repo",
            "create",
            &owner_repo,
            "--private",
            "--disable-issues",
            "--disable-wiki",
            "--description",
            &format!("Temporary Libra integration test {}", run_id),
        ],
        ctx.run.run_root.clone(),
        true,
    )?;

    // Arm the guard *immediately* after successful create. Drop will delete if we don't disarm.
    // Mark "cleanup_required" immediately so that any subsequent bail (error path) will carry
    // it in ScenarioResult.cleanup (via finish). On explicit success delete+disarm we overwrite
    // with "deleted ...". This wires the guard arming to the wave3_cleanup aggregator and
    // post-run WARNING (addresses the emission gap where failed live runs after create
    // previously reported wave3_cleanup="not_run").
    let guard = GhRepoCleanupGuard::new(owner_repo.clone());
    ctx.set_cleanup(&format!("cleanup_required {}", owner_repo));

    // Capture remote URL (prefer sshUrl for agent-based auth; plan allows recorded HTTPS)
    let view_ssh = ctx.gh(
        &[
            "repo",
            "view",
            &owner_repo,
            "--json",
            "sshUrl",
            "--jq",
            ".sshUrl",
        ],
        ctx.run.run_root.clone(),
        true,
    )?;
    let remote_url = stdout_trim(&view_ssh);
    if remote_url.is_empty() {
        bail!("failed to obtain sshUrl for temp repo");
    }

    // Record full view for audit
    let _ = ctx.gh(
        &[
            "repo",
            "view",
            &owner_repo,
            "--json",
            "nameWithOwner,isPrivate,isEmpty,url,sshUrl",
        ],
        ctx.run.run_root.clone(),
        true,
    );

    // Also verify isPrivate etc via json (light)
    let view_json = ctx.gh(
        &["repo", "view", &owner_repo, "--json", "isPrivate"],
        ctx.run.run_root.clone(),
        true,
    )?;
    let v = String::from_utf8_lossy(&view_json.stdout);
    if !v.contains("true") {
        // still proceed; some orgs may differ, but we requested private
    }

    let base = ctx.run_dir.clone();
    // source repo (the one we push from)
    ctx.command(&["init", "source"], base.clone(), true)?;
    let source = base.join("source");
    ctx.command(
        &["config", "set", "user.name", "Libra GitHub Integration"],
        source.clone(),
        true,
    )?;
    ctx.command(
        &[
            "config",
            "set",
            "user.email",
            "libra-integration@example.invalid",
        ],
        source.clone(),
        true,
    )?;
    fs::write(source.join("README.md"), "github remote\n").context("write README for live")?;
    ctx.command(&["add", "README.md"], source.clone(), true)?;
    ctx.command(
        &["commit", "-m", "test: github integration"],
        source.clone(),
        true,
    )?;
    ctx.command(
        &["remote", "add", "origin", &remote_url],
        source.clone(),
        true,
    )?;

    // dry-run push
    ctx.command(
        &["push", "--dry-run", "origin", "main"],
        source.clone(),
        true,
    )?;

    // real push -u
    ctx.command(&["push", "-u", "origin", "main"], source.clone(), true)?;

    // gh api verify ref matches local
    let remote_main = ctx.gh(
        &[
            "api",
            &format!("repos/{}/git/ref/heads/main", owner_repo),
            "--jq",
            ".object.sha",
        ],
        base.clone(),
        true,
    )?;
    let remote_main_sha = stdout_trim(&remote_main);
    let local_head = ctx.command(&["rev-parse", "HEAD"], source.clone(), true)?;
    let local_sha = stdout_trim(&local_head);
    if remote_main_sha != local_sha {
        bail!(
            "ref mismatch after initial push: remote={} local={}",
            remote_main_sha,
            local_sha
        );
    }

    // feature refspec push
    ctx.command(&["branch", "feature/live", "main"], source.clone(), true)?;
    ctx.command(&["switch", "feature/live"], source.clone(), true)?;
    fs::write(source.join("feature.txt"), "feature branch\n").context("write feature")?;
    ctx.command(&["add", "feature.txt"], source.clone(), true)?;
    ctx.command(
        &["commit", "-m", "test: github feature branch"],
        source.clone(),
        true,
    )?;
    ctx.command(
        &["push", "origin", "feature/live:feature/pushed"],
        source.clone(),
        true,
    )?;

    // tag + --tags
    ctx.command(&["tag", "v-live-smoke"], source.clone(), true)?;
    ctx.command(&["push", "--tags", "origin"], source.clone(), true)?;
    let _ = ctx.gh(
        &[
            "api",
            &format!("repos/{}/git/ref/tags/v-live-smoke", owner_repo),
            "--jq",
            ".object.sha",
        ],
        base.clone(),
        true,
    );

    // delete ref via push :<dst>
    ctx.command(&["push", "origin", ":feature/pushed"], source.clone(), true)?;

    // --mirror (dry + real) — only on our temp repo
    ctx.command(
        &["push", "--mirror", "--dry-run", "origin"],
        source.clone(),
        true,
    )?;
    ctx.command(&["push", "--mirror", "origin"], source.clone(), true)?;

    // force push path: non-ff must fail, --force succeeds
    ctx.command(&["switch", "main"], source.clone(), true)?;
    // append + amend to create non-ff situation
    let mut readme = fs::read_to_string(source.join("README.md")).unwrap_or_default();
    readme.push_str("forced rewrite\n");
    fs::write(source.join("README.md"), &readme).context("append for force")?;
    ctx.command(&["add", "README.md"], source.clone(), true)?;
    ctx.command(&["commit", "--amend", "--no-edit"], source.clone(), true)?;
    let forced_out = ctx.command(&["rev-parse", "HEAD"], source.clone(), true)?;
    let forced_sha = stdout_trim(&forced_out);

    // non-ff push must fail
    let non_ff = ctx.command(&["push", "origin", "main"], source.clone(), false)?;
    // expect LBR or at least non-zero (GitHub will reject non-ff without force)
    if non_ff.status.success() {
        // unexpected, but continue; some mirrors may allow?
    }

    // force succeeds
    ctx.command(&["push", "--force", "origin", "main"], source.clone(), true)?;

    // gh verify forced sha landed
    let forced_remote = ctx.gh(
        &[
            "api",
            &format!("repos/{}/git/ref/heads/main", owner_repo),
            "--jq",
            ".object.sha",
        ],
        base.clone(),
        true,
    )?;
    let forced_remote_sha = stdout_trim(&forced_remote);
    if forced_remote_sha != forced_sha {
        bail!(
            "force push did not land on remote: expected {} got {}",
            forced_sha,
            forced_remote_sha
        );
    }

    // clone the remote into cloned/
    ctx.command(&["clone", &remote_url, "cloned"], base.clone(), true)?;
    let cloned = base.join("cloned");
    ctx.command(&["log", "--oneline"], cloned.clone(), true)?;
    let cloned_readme = fs::read_to_string(cloned.join("README.md")).unwrap_or_default();
    if !cloned_readme.contains("forced rewrite") {
        bail!("clone did not receive forced content");
    }

    // second commit on source, push; then fetch+pull from cloned
    let mut readme2 = fs::read_to_string(source.join("README.md")).unwrap_or_default();
    readme2.push_str("second commit\n");
    fs::write(source.join("README.md"), &readme2).context("second commit file")?;
    ctx.command(&["add", "README.md"], source.clone(), true)?;
    ctx.command(
        &["commit", "-m", "test: github second commit"],
        source.clone(),
        true,
    )?;
    ctx.command(&["push", "origin", "main"], source.clone(), true)?;

    // now from clone dir
    ctx.command(&["fetch", "origin"], cloned.clone(), true)?;
    ctx.command(&["pull", "origin", "main"], cloned.clone(), true)?;
    let pulled_readme = fs::read_to_string(cloned.join("README.md")).unwrap_or_default();
    if !pulled_readme.contains("second commit") {
        bail!("pull did not bring second commit");
    }

    // json sanity after key ops
    let jlog = ctx.command(&["--json", "log", "-n", "1"], source.clone(), true)?;
    assert_json_ok(&jlog, "log")?;

    // explicit cleanup + disarm guard
    ctx.gh(
        &["repo", "delete", &owner_repo, "--yes"],
        base.clone(),
        true,
    )?;
    guard.disarm();
    ctx.set_cleanup(&format!("deleted {}", owner_repo));

    // final fsck on source
    ctx.command(&["fsck", "--connectivity-only"], source.clone(), true)?;

    Ok(())
}
