use super::prelude::*;

pub(crate) fn scenario_archive_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    ensure_file(repo.join("tracked.txt"))?;
    let tar_arg = ctx
        .run_dir
        .join("release.tar")
        .to_string_lossy()
        .to_string();
    let missing = ctx.command(
        &[
            "--json", "archive", "--output", &tar_arg, "--prefix", "release/",
        ],
        repo.clone(),
        false,
    )?;
    assert_json_error_code(&missing, "LBR-CLI-001")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
