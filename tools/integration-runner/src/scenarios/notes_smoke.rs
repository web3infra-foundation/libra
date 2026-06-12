use super::prelude::*;

/// `cli.notes-smoke` — black-box coverage for `libra notes` (commit annotations).
///
/// Covers add/list/show/remove, explicit-object targeting (`add <object>`,
/// `list <object>`, `show <object>`), `-m`/`-F`/`-f`, `--ref` custom notes refs,
/// and the negative paths (re-add without `-f`, show-after-remove, empty message).
/// `notes` is a Git-compatible command (`git notes`); refs/notes/* are local-only
/// in Libra.
pub(crate) fn scenario_notes_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;

    let base = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?);

    // Add a note to HEAD (the default object) and read it back.
    ctx.command(&["notes", "add", "-m", "first note"], repo.clone(), true)?;
    let shown = ctx.command(&["notes", "show"], repo.clone(), true)?;
    assert_stdout_contains(&shown, "first note")?;
    // JSON envelopes for show + list.
    assert_json_ok(
        &ctx.command(&["--json", "notes", "show"], repo.clone(), true)?,
        "notes",
    )?;
    let listed = ctx.command(&["--json", "notes", "list"], repo.clone(), true)?;
    assert_json_ok(&listed, "notes")?;
    assert_stdout_contains(&listed, &base)?;

    // Re-adding without -f must fail (a note already exists); -f overwrites it.
    let dup = ctx.command(&["notes", "add", "-m", "second note"], repo.clone(), false)?;
    assert_lbr_or_text(&dup, "already")?;
    ctx.command(
        &["notes", "add", "-f", "-m", "overwritten note"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(
        &ctx.command(&["notes", "show"], repo.clone(), true)?,
        "overwritten note",
    )?;

    // Explicit-object show / list target the base commit directly.
    assert_stdout_contains(
        &ctx.command(&["notes", "show", &base], repo.clone(), true)?,
        "overwritten note",
    )?;
    let scoped = ctx.command(&["--json", "notes", "list", &base], repo.clone(), true)?;
    assert_json_ok(&scoped, "notes")?;
    assert_stdout_contains(&scoped, &base)?;

    // -F reads the note body from a file; attach it to a second commit.
    fs::write(repo.join("note.txt"), "file note body\n").context("write note body file")?;
    fs::write(repo.join("tracked.txt"), "base\nmore\n").context("extend tracked file")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "test: notes second", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["notes", "add", "-F", "note.txt"], repo.clone(), true)?;
    assert_stdout_contains(
        &ctx.command(&["notes", "show"], repo.clone(), true)?,
        "file note body",
    )?;

    // A custom notes ref is isolated from the default refs/notes/commits;
    // `add` also accepts an explicit object instead of defaulting to HEAD.
    ctx.command(
        &[
            "notes",
            "--ref",
            "refs/notes/review",
            "add",
            "-m",
            "review note",
            &base,
        ],
        repo.clone(),
        true,
    )?;
    let review = ctx.command(
        &["--json", "notes", "--ref", "refs/notes/review", "list"],
        repo.clone(),
        true,
    )?;
    assert_json_ok(&review, "notes")?;
    assert_stdout_contains(&review, &base)?;

    // Remove the HEAD note; a subsequent show must fail with not-found.
    ctx.command(&["notes", "remove"], repo.clone(), true)?;
    let after_remove = ctx.command(&["notes", "show"], repo.clone(), false)?;
    assert_lbr_or_text(&after_remove, "note")?;
    // An explicit-object remove still works for the base commit's note.
    ctx.command(&["notes", "remove", &base], repo.clone(), true)?;

    // An empty note body is rejected (usage error).
    let empty = ctx.command(&["notes", "add", "-m", "", &base], repo.clone(), false)?;
    assert_lbr_or_text(&empty, "empty")?;

    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
