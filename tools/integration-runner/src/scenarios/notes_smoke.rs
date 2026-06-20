use super::prelude::*;

pub(crate) fn scenario_notes_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;

    ctx.command(&["notes", "add", "-m", "Reviewed-by: Alice"], repo.clone(), true)?;
    let show = ctx.command(&["notes", "show"], repo.clone(), true)?;
    assert_stdout_contains(&show, "Reviewed-by: Alice")?;

    let show_json = ctx.command(&["--json", "notes", "show"], repo.clone(), true)?;
    assert_json_ok(&show_json, "notes")?;

    let list_json = ctx.command(&["--json", "notes", "list"], repo.clone(), true)?;
    assert_json_ok(&list_json, "notes")?;

    ctx.command(
        &["notes", "--ref", "refs/notes/review", "add", "-m", "LGTM"],
        repo.clone(),
        true,
    )?;
    let review_show = ctx.command(
        &["notes", "--ref", "refs/notes/review", "show"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&review_show, "LGTM")?;
    ctx.command(
        &["notes", "--ref", "refs/notes/review", "remove", "HEAD"],
        repo.clone(),
        true,
    )?;

    let duplicate = ctx.command(
        &["notes", "add", "-m", "duplicate without force"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&duplicate, "already has a note")?;

    ctx.command(&["notes", "remove", "HEAD"], repo.clone(), true)?;
    let empty_list = ctx.command(&["notes", "list"], repo.clone(), true)?;
    assert_not_contains(&empty_list, "Reviewed-by: Alice")?;

    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
