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
    fs::create_dir_all(repo.join("src")).context("create src fixture dir")?;
    fs::write(repo.join("README.md"), "object root\n").context("write README fixture")?;
    fs::write(repo.join("docs/guide.md"), "object docs\n").context("write docs fixture")?;
    fs::write(repo.join("src/main.rs"), "fn main() {}\n").context("write src fixture")?;
    ctx.command(
        &["add", "README.md", "docs/guide.md", "src/main.rs"],
        repo.clone(),
        true,
    )?;
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
    ctx.command(&["rev-parse", "--short", "HEAD"], repo.clone(), true)?;
    let verify = ctx.command(&["rev-parse", "--verify", "HEAD"], repo.clone(), true)?;
    let verify_id = stdout_trim(&verify);
    if verify_id != head_id {
        bail!("rev-parse --verify HEAD returned {verify_id:?}, expected {head_id}");
    }
    let verify_short = ctx.command(
        &["rev-parse", "--verify", "--short", "HEAD"],
        repo.clone(),
        true,
    )?;
    let verify_short_id = stdout_trim(&verify_short);
    if verify_short_id.is_empty() || !head_id.starts_with(&verify_short_id) {
        bail!("rev-parse --verify --short HEAD returned invalid prefix {verify_short_id:?}");
    }
    let verify_default = ctx.command(
        &["rev-parse", "--verify", "--default", "HEAD"],
        repo.clone(),
        true,
    )?;
    let verify_default_id = stdout_trim(&verify_default);
    if verify_default_id != head_id {
        bail!(
            "rev-parse --verify --default HEAD returned {verify_default_id:?}, expected {head_id}"
        );
    }
    assert_json_ok(
        &ctx.command(
            &["--json", "rev-parse", "--verify", "HEAD"],
            repo.clone(),
            true,
        )?,
        "rev-parse",
    )?;
    let quiet_verify = ctx.command(
        &["--quiet", "rev-parse", "--verify", "no-such-revision"],
        repo.clone(),
        false,
    )?;
    if quiet_verify.status.code() != Some(1) {
        bail!(
            "rev-parse --verify under --quiet exited {:?}, expected 1",
            quiet_verify.status.code()
        );
    }
    if !quiet_verify.stdout.is_empty() || !quiet_verify.stderr.is_empty() {
        bail!("rev-parse --verify under --quiet must be silent");
    }
    let top = ctx.command(&["rev-parse", "--show-toplevel"], repo.clone(), true)?;
    assert_stdout_contains(&top, repo.to_string_lossy().as_ref())?;
    let git_dir = ctx.command(&["rev-parse", "--git-dir"], repo.clone(), true)?;
    assert_stdout_contains(&git_dir, ".libra")?;
    let prefix = ctx.command(&["rev-parse", "--show-prefix"], repo.join("src"), true)?;
    if stdout_trim(&prefix) != "src/" {
        bail!(
            "rev-parse --show-prefix from src returned {:?}, expected src/",
            stdout_trim(&prefix)
        );
    }
    let cdup = ctx.command(&["rev-parse", "--show-cdup"], repo.join("src"), true)?;
    if stdout_trim(&cdup) != "../" {
        bail!(
            "rev-parse --show-cdup from src returned {:?}, expected ../",
            stdout_trim(&cdup)
        );
    }
    let inside_work = ctx.command(&["rev-parse", "--is-inside-work-tree"], repo.clone(), true)?;
    if stdout_trim(&inside_work) != "true" {
        bail!("rev-parse --is-inside-work-tree returned non-true output");
    }
    let inside_storage = ctx.command(
        &["rev-parse", "--is-inside-git-dir"],
        repo.join(".libra"),
        true,
    )?;
    if stdout_trim(&inside_storage) != "true" {
        bail!("rev-parse --is-inside-git-dir in .libra returned non-true output");
    }
    let sq_quote = ctx.command(
        &["rev-parse", "--sq-quote", "-x", "a b"],
        ctx.run_dir.clone(),
        true,
    )?;
    if stdout_trim(&sq_quote) != "'-x' 'a b'" {
        bail!("rev-parse --sq-quote returned {:?}", stdout_trim(&sq_quote));
    }
    let rev_list = ctx.command(&["rev-list", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&rev_list, &head_id)?;
    let show = ctx.command(&["show", "--no-patch", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&show, "test: object readback")?;
    let guide = ctx.command(&["show", "HEAD:docs/guide.md"], repo.clone(), true)?;
    assert_stdout_contains(&guide, "object docs")?;
    ctx.command(&["show-ref", "--head"], repo.clone(), true)?;
    ctx.command(&["show-ref", "--heads"], repo.clone(), true)?;
    ctx.command(
        &["show-ref", "--exists", "refs/heads/main"],
        repo.clone(),
        true,
    )?;
    let verified_show_ref = ctx.command(
        &["show-ref", "--verify", "refs/heads/main"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&verified_show_ref, "refs/heads/main")?;
    let verified_hash = ctx.command(
        &["show-ref", "--hash", "--verify", "refs/heads/main"],
        repo.clone(),
        true,
    )?;
    let verified_hash_stdout = stdout_trim(&verified_hash);
    if verified_hash_stdout != head_id {
        bail!("show-ref --hash --verify returned {verified_hash_stdout:?}, expected {head_id}");
    }
    let refs = ctx.command(&["for-each-ref", "--heads"], repo.clone(), true)?;
    assert_stdout_contains(&refs, "refs/heads/main")?;
    let formatted_refs = ctx.command(
        &[
            "for-each-ref",
            "--heads",
            "--format=%(refname) %(objecttype)",
        ],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&formatted_refs, "refs/heads/main commit")?;
    assert_json_ok(
        &ctx.command(&["--json", "for-each-ref", "--heads"], repo.clone(), true)?,
        "for-each-ref",
    )?;
    let indexed = ctx.command(&["ls-files"], repo.clone(), true)?;
    assert_stdout_contains(&indexed, "README.md")?;
    assert_stdout_contains(&indexed, "docs/guide.md")?;
    let cached = ctx.command(&["ls-files", "--cached"], repo.clone(), true)?;
    assert_stdout_contains(&cached, "README.md")?;
    assert_stdout_contains(&cached, "src/main.rs")?;
    let staged = ctx.command(&["ls-files", "--stage"], repo.clone(), true)?;
    assert_stdout_contains(&staged, "README.md")?;
    assert_stdout_contains(&staged, " 0\tREADME.md")?;
    fs::write(repo.join("README.md"), "object root changed\n").context("modify README fixture")?;
    let modified = ctx.command(&["ls-files", "--modified"], repo.clone(), true)?;
    assert_stdout_contains(&modified, "README.md")?;
    fs::remove_file(repo.join("src/main.rs")).context("delete indexed src fixture")?;
    let deleted = ctx.command(&["ls-files", "--deleted"], repo.clone(), true)?;
    assert_stdout_contains(&deleted, "src/main.rs")?;
    fs::write(repo.join("untracked.txt"), "untracked\n").context("write untracked fixture")?;
    fs::write(repo.join("ignored.tmp"), "ignored\n").context("write ignored fixture")?;
    fs::write(repo.join(".libraignore"), "ignored.tmp\n").context("write ignore fixture")?;
    let all_others = ctx.command(&["ls-files", "--others"], repo.clone(), true)?;
    assert_stdout_contains(&all_others, "ignored.tmp")?;
    let others = ctx.command(&["ls-files", "--others", "--exclude-standard"], repo.clone(), true)?;
    assert_stdout_contains(&others, "untracked.txt")?;
    assert_not_contains(&others, "ignored.tmp")?;
    assert_json_ok(
        &ctx.command(&["--json", "ls-files", "--modified"], repo.clone(), true)?,
        "ls-files",
    )?;
    let missing_show_ref = ctx.command(
        &["show-ref", "--exists", "refs/heads/nope"],
        repo.clone(),
        false,
    )?;
    if missing_show_ref.status.code() != Some(2) {
        bail!(
            "show-ref --exists missing ref exited {:?}, expected 2",
            missing_show_ref.status.code()
        );
    }
    assert_lbr_or_text(&missing_show_ref, "reference does not exist")?;
    let missing_verified_show_ref = ctx.command(
        &["show-ref", "--verify", "refs/heads/nope"],
        repo.clone(),
        false,
    )?;
    if missing_verified_show_ref.status.code() != Some(128) {
        bail!(
            "show-ref --verify missing ref exited {:?}, expected 128",
            missing_verified_show_ref.status.code()
        );
    }
    assert_lbr_or_text(&missing_verified_show_ref, "not a valid ref")?;
    let quiet_missing_verified_show_ref = ctx.command(
        &["--quiet", "show-ref", "--verify", "refs/heads/nope"],
        repo.clone(),
        false,
    )?;
    if quiet_missing_verified_show_ref.status.code() != Some(1) {
        bail!(
            "quiet show-ref --verify missing ref exited {:?}, expected 1",
            quiet_missing_verified_show_ref.status.code()
        );
    }
    if !quiet_missing_verified_show_ref.stdout.is_empty()
        || !quiet_missing_verified_show_ref.stderr.is_empty()
    {
        bail!("quiet show-ref --verify missing ref must be silent");
    }
    let object_type = ctx.command(&["cat-file", "-t", &head_id], repo.clone(), true)?;
    assert_stdout_contains(&object_type, "commit")?;
    ctx.command(&["cat-file", "-s", &head_id], repo.clone(), true)?;
    let pretty = ctx.command(&["cat-file", "-p", &head_id], repo.clone(), true)?;
    assert_stdout_contains(&pretty, "tree ")?;
    ctx.command(&["cat-file", "-e", &head_id], repo.clone(), true)?;
    fs::write(repo.join("loose.txt"), "loose blob\n").context("write loose blob fixture")?;
    let blob = ctx.command(&["hash-object", "-w", "loose.txt"], repo.clone(), true)?;
    let blob_id = stdout_trim(&blob);
    let blob_type = ctx.command(&["cat-file", "-t", &blob_id], repo.clone(), true)?;
    assert_stdout_contains(&blob_type, "blob")?;
    let blob_content = ctx.command(&["cat-file", "-p", &blob_id], repo.clone(), true)?;
    assert_stdout_contains(&blob_content, "loose blob")?;
    let shown_blob = ctx.command(&["show", &blob_id], repo.clone(), true)?;
    assert_stdout_contains(&shown_blob, "loose blob")?;
    fs::write(repo.join("binary.bin"), b"binary\0blob").context("write binary blob fixture")?;
    let binary_blob = ctx.command(&["hash-object", "-w", "binary.bin"], repo.clone(), true)?;
    let binary_blob_id = stdout_trim(&binary_blob);
    let shown_binary = ctx.command(&["show", &binary_blob_id], repo.clone(), true)?;
    assert_stdout_contains(&shown_binary, "Binary file")?;
    assert_json_ok(
        &ctx.command(&["--json", "show", &binary_blob_id], repo.clone(), true)?,
        "show",
    )?;
    fs::write(repo.join("docs/rev-list.md"), "rev-list second\n")
        .context("write rev-list second fixture")?;
    ctx.command(&["add", "docs/rev-list.md"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "test: rev-list second", "--no-verify"],
        repo.clone(),
        true,
    )?;
    let second = ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?;
    let second_id = stdout_trim(&second);
    let limited = ctx.command(&["rev-list", "-n", "1", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&limited, &second_id)?;
    let skipped = ctx.command(&["rev-list", "--skip", "1", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&skipped, &head_id)?;
    let range_spec = format!("{head_id}..HEAD");
    let parsed_range = ctx.command(&["rev-parse", &range_spec], repo.clone(), true)?;
    assert_stdout_contains(&parsed_range, &second_id)?;
    assert_stdout_contains(&parsed_range, &format!("^{head_id}"))?;
    let parsed_range_json =
        ctx.command(&["--json", "rev-parse", &range_spec], repo.clone(), true)?;
    assert_json_ok(&parsed_range_json, "rev-parse")?;
    assert_stdout_contains(&parsed_range_json, "\"values\"")?;
    let quoted = ctx.command(&["rev-parse", "--sq", "HEAD", "HEAD~1"], repo.clone(), true)?;
    assert_stdout_contains(&quoted, &format!("'{second_id}'"))?;
    let range = ctx.command(&["rev-list", &range_spec], repo.clone(), true)?;
    let range_stdout = stdout_trim(&range);
    if range_stdout != second_id {
        bail!("rev-list range {range_spec} returned {range_stdout:?}, expected {second_id}");
    }
    let exclude_spec = format!("^{head_id}");
    let excluded = ctx.command(&["rev-list", "HEAD", &exclude_spec], repo.clone(), true)?;
    let excluded_stdout = stdout_trim(&excluded);
    if excluded_stdout != second_id {
        bail!("rev-list exclusion returned {excluded_stdout:?}, expected {second_id}");
    }
    let count = ctx.command(&["rev-list", "--count", "HEAD"], repo.clone(), true)?;
    let count_stdout = stdout_trim(&count);
    if count_stdout != "2" {
        bail!("rev-list --count HEAD returned {count_stdout:?}, expected 2");
    }
    let parents = ctx.command(
        &["rev-list", "--parents", "-n", "1", "HEAD"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&parents, &second_id)?;
    assert_stdout_contains(&parents, &head_id)?;
    let timestamp = ctx.command(
        &["rev-list", "--timestamp", "-n", "1", "HEAD"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&timestamp, &second_id)?;
    let objects = ctx.command(
        &["rev-list", "--objects", "-n", "1", "HEAD"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&objects, &second_id)?;
    assert_stdout_contains(&objects, "docs/rev-list.md")?;
    assert_json_ok(
        &ctx.command(
            &["--json", "rev-list", "--count", "HEAD"],
            repo.clone(),
            true,
        )?,
        "rev-list",
    )?;
    let json_objects = ctx.command(
        &["--json", "rev-list", "--objects", "-n", "1", "HEAD"],
        repo.clone(),
        true,
    )?;
    assert_json_ok(&json_objects, "rev-list")?;
    assert_stdout_contains(&json_objects, "\"objects\"")?;
    // fsck flag coverage on the healthy committed history: default (`--full`),
    // `--no-full` (loose objects only), `--strict` (commit/tree structural checks),
    // `--connectivity-only`, and a single-object check.
    ctx.command(&["fsck"], repo.clone(), true)?;
    ctx.command(&["fsck", "--no-full"], repo.clone(), true)?;
    ctx.command(&["fsck", "--strict"], repo.clone(), true)?;
    ctx.command(&["fsck", "--connectivity-only"], repo.clone(), true)?;
    ctx.command(&["fsck", &head_id], repo.clone(), true)?;
    assert_json_ok(
        &ctx.command(&["--json", "show-ref", "--heads"], repo.clone(), true)?,
        "show-ref",
    )?;
    ctx.command(&["tag", "-m", "object tag", "v-object"], repo.clone(), true)?;
    let tag_refs = ctx.command(&["for-each-ref", "--tags"], repo.clone(), true)?;
    assert_stdout_contains(&tag_refs, "refs/tags/v-object")?;
    let all_refs = ctx.command(&["for-each-ref", "--all"], repo.clone(), true)?;
    assert_stdout_contains(&all_refs, "refs/heads/main")?;
    assert_stdout_contains(&all_refs, "refs/tags/v-object")?;
    let dereferenced_tag = ctx.command(
        &["show-ref", "--dereference", "--tags", "v-object"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&dereferenced_tag, "refs/tags/v-object")?;
    assert_stdout_contains(&dereferenced_tag, "refs/tags/v-object^{}")?;
    let verified_dereferenced_tag = ctx.command(
        &[
            "show-ref",
            "--verify",
            "--dereference",
            "refs/tags/v-object",
        ],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&verified_dereferenced_tag, "refs/tags/v-object^{}")?;
    ctx.command(&["branch", "main-2"], repo.clone(), true)?;
    let counted_refs = ctx.command(
        &["for-each-ref", "--heads", "--sort=-refname", "--count=1"],
        repo.clone(),
        true,
    )?;
    let counted_stdout = stdout_trim(&counted_refs);
    if !counted_stdout.contains("refs/heads/main-2") || counted_stdout.contains("refs/heads/main\n") {
        bail!("for-each-ref --sort=-refname --count=1 returned {counted_stdout:?}");
    }
    let pattern_refs = ctx.command(&["for-each-ref", "main-2"], repo.clone(), true)?;
    assert_stdout_contains(&pattern_refs, "refs/heads/main-2")?;
    assert_not_contains(&pattern_refs, "refs/heads/main\n")?;
    let main_pattern = ctx.command(&["show-ref", "--heads", "main"], repo.clone(), true)?;
    assert_stdout_contains(&main_pattern, "refs/heads/main")?;
    assert_not_contains(&main_pattern, "refs/heads/main-2")?;

    // --- hash-object parameter coverage (beyond the `-w <file>` blob writes above) ---
    // `--stdin` computes the id for piped bytes; identical content to loose.txt
    // must yield the identical object id.
    let stdin_blob = ctx.command_with_stdin(
        &["hash-object", "--stdin"],
        repo.clone(),
        "loose blob\n",
        true,
    )?;
    let stdin_blob_id = stdout_trim(&stdin_blob);
    if stdin_blob_id != blob_id {
        bail!("hash-object --stdin id {stdin_blob_id:?} did not match -w id {blob_id}");
    }
    // Explicit `-t blob` with `--stdin -w` persists and reads back as a blob.
    let typed_blob = ctx.command_with_stdin(
        &["hash-object", "-t", "blob", "-w", "--stdin"],
        repo.clone(),
        "typed stdin blob\n",
        true,
    )?;
    let typed_blob_id = stdout_trim(&typed_blob);
    assert_stdout_contains(
        &ctx.command(&["cat-file", "-t", &typed_blob_id], repo.clone(), true)?,
        "blob",
    )?;
    // `--stdin-paths` hashes newline-separated worktree paths read from stdin.
    let paths_out = ctx.command_with_stdin(
        &["hash-object", "--stdin-paths"],
        repo.clone(),
        "loose.txt\n",
        true,
    )?;
    assert_stdout_contains(&paths_out, &blob_id)?;
    // `--path` is an accepted source label for `--stdin`.
    ctx.command_with_stdin(
        &["hash-object", "--path", "labelled.txt", "--stdin"],
        repo.clone(),
        "labelled stdin blob\n",
        true,
    )?;
    // `--no-filters` is an accepted no-op (Libra has no clean/smudge filters).
    ctx.command_with_stdin(
        &["hash-object", "--no-filters", "--stdin"],
        repo.clone(),
        "unfiltered stdin blob\n",
        true,
    )?;
    // `--literally` skips object-format validation, allowing a non-standard body.
    // Computed only (no `-w`), so the malformed object never enters the store and
    // cannot break the fsck runs below.
    ctx.command_with_stdin(
        &["hash-object", "-t", "tag", "--literally", "--stdin"],
        repo.clone(),
        "not a real tag object\n",
        true,
    )?;
    // Without `--literally` the same malformed tag body is rejected (exit 128).
    let bad_tag = ctx.command_with_stdin(
        &["hash-object", "-t", "tag", "--stdin"],
        repo.clone(),
        "not a real tag object\n",
        false,
    )?;
    assert_lbr_or_text(&bad_tag, "literally")?;
    // An unsupported `-t <type>` is a usage error (intentionally-different exit 129).
    let bad_type = ctx.command(
        &["hash-object", "-t", "bogus", "loose.txt"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_type, "unsupported object type")?;

    let missing = ctx.command(&["cat-file", "-t", "deadbeef"], repo.clone(), false)?;
    assert_lbr_or_text(&missing, "object not found")?;
    Ok(())
}
