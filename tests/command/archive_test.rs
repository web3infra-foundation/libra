//! Integration smoke tests for the `archive` command output formats.

use std::{fs, io::Read, path::Path};

use super::*;

fn create_archive_test_repo() -> tempfile::TempDir {
    let repo = tempdir().expect("failed to create archive test repository");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());

    fs::create_dir_all(repo.path().join("src")).expect("failed to create src directory");
    fs::write(repo.path().join("README.md"), "# Test\n").expect("failed to write README");
    fs::write(repo.path().join("src/main.rs"), "fn main() {}\n").expect("failed to write main.rs");

    let output = run_libra_command(
        &["add", ".libraignore", "README.md", "src/main.rs"],
        repo.path(),
    );
    assert_cli_success(&output, "failed to add archive fixture files");

    let output = run_libra_command(&["commit", "-m", "initial", "--no-verify"], repo.path());
    assert_cli_success(&output, "failed to commit archive fixture files");

    repo
}

fn read_bytes(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .expect("failed to open archive output")
        .read_to_end(&mut bytes)
        .expect("failed to read archive output");
    bytes
}

fn is_tar(data: &[u8]) -> bool {
    data.len() >= 263
        && (&data[257..263] == b"ustar\0".as_slice() || &data[257..263] == b"ustar ".as_slice())
}

fn is_gzip(data: &[u8]) -> bool {
    data.starts_with(&[0x1f, 0x8b])
}

fn is_bzip2(data: &[u8]) -> bool {
    data.starts_with(b"BZh")
}

fn is_zip(data: &[u8]) -> bool {
    data.starts_with(b"PK\x03\x04")
}

#[test]
fn archive_default_produces_tar() {
    let repo = create_archive_test_repo();

    let output = run_libra_command(&["archive"], repo.path());

    assert_cli_success(&output, "archive default");
    assert!(is_tar(&output.stdout), "expected tar output on stdout");
}

#[test]
fn archive_lists_supported_formats_without_repository() {
    let temp = tempdir().expect("failed to create non-repository archive list test directory");

    let output = run_libra_command(&["archive", "--list"], temp.path());

    assert_cli_success(&output, "archive --list");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("tar\n"), "format list should include tar");
    assert!(
        stdout.contains("tar.gz"),
        "format list should include tar.gz"
    );
    assert!(
        stdout.contains("tar.bz2"),
        "format list should include tar.bz2"
    );
    assert!(stdout.contains("zip"), "format list should include zip");
}

#[test]
fn archive_pathspec_limits_tar_entries_to_matching_directory() {
    let repo = create_archive_test_repo();
    let out = repo.path().join("src-only.tar");
    let out_str = out.to_str().expect("archive output path should be UTF-8");

    let output = run_libra_command(&["archive", "-o", out_str, "HEAD", "src"], repo.path());

    assert_cli_success(&output, "archive HEAD src");
    let text = String::from_utf8_lossy(&read_bytes(&out)).to_string();
    assert!(
        text.contains("src/main.rs"),
        "tar should contain matched path"
    );
    assert!(
        !text.contains("README.md"),
        "tar should omit unmatched root file"
    );
}

#[test]
fn archive_pathspec_limits_tar_entries_to_matching_file() {
    let repo = create_archive_test_repo();
    let out = repo.path().join("readme-only.tar");
    let out_str = out.to_str().expect("archive output path should be UTF-8");

    let output = run_libra_command(
        &["archive", "-o", out_str, "HEAD", "README.md"],
        repo.path(),
    );

    assert_cli_success(&output, "archive HEAD README.md");
    let text = String::from_utf8_lossy(&read_bytes(&out)).to_string();
    assert!(
        text.contains("README.md"),
        "tar should contain matched file"
    );
    assert!(
        !text.contains("src/main.rs"),
        "tar should omit unmatched nested file"
    );
}

#[test]
fn archive_add_file_includes_untracked_working_tree_file() {
    let repo = create_archive_test_repo();
    let p = repo.path();
    // An untracked file in the working tree (never committed).
    std::fs::write(p.join("EXTRA.txt"), "added bytes\n").expect("write untracked file");

    // `--add-file` must precede the tree-ish (the `paths` positional is
    // trailing_var_arg), matching `git archive --add-file=<file> <tree>`.
    let out = p.join("with-extra.tar");
    let output = run_libra_command(
        &[
            "archive",
            "-o",
            out.to_str().unwrap(),
            "--add-file=EXTRA.txt",
            "HEAD",
        ],
        p,
    );
    assert_cli_success(&output, "archive --add-file");
    let text = String::from_utf8_lossy(&read_bytes(&out)).to_string();
    assert!(
        text.contains("README.md"),
        "tracked tree files must still be archived"
    );
    assert!(
        text.contains("EXTRA.txt") && text.contains("added bytes"),
        "the added file's name and content must be in the archive"
    );

    // Under --prefix the added file sits at <prefix><basename>.
    let prefixed = p.join("prefixed.tar");
    let output = run_libra_command(
        &[
            "archive",
            "-o",
            prefixed.to_str().unwrap(),
            "--prefix=proj/",
            "--add-file=EXTRA.txt",
            "HEAD",
        ],
        p,
    );
    assert_cli_success(&output, "archive --add-file --prefix");
    let prefixed_text = String::from_utf8_lossy(&read_bytes(&prefixed)).to_string();
    assert!(
        prefixed_text.contains("proj/EXTRA.txt"),
        "added file must sit under the prefix at its basename"
    );

    // A missing --add-file path is a hard error, not a silent skip.
    let missing = run_libra_command(
        &[
            "archive",
            "-o",
            out.to_str().unwrap(),
            "--add-file=nope.txt",
            "HEAD",
        ],
        p,
    );
    assert!(
        !missing.status.success(),
        "a non-existent --add-file path must fail"
    );
}

#[test]
fn archive_supports_compressed_and_zip_formats() {
    let repo = create_archive_test_repo();

    let gzip = run_libra_command(&["archive", "--format=tar.gz"], repo.path());
    assert_cli_success(&gzip, "archive tar.gz");
    assert!(is_gzip(&gzip.stdout), "expected gzip output");

    let bzip2 = run_libra_command(&["archive", "--format=tar.bz2"], repo.path());
    assert_cli_success(&bzip2, "archive tar.bz2");
    assert!(is_bzip2(&bzip2.stdout), "expected bzip2 output");

    let zip = run_libra_command(&["archive", "--format=zip"], repo.path());
    assert_cli_success(&zip, "archive zip");
    assert!(is_zip(&zip.stdout), "expected zip output");
}

#[test]
fn archive_accepts_compression_format_aliases() {
    let repo = create_archive_test_repo();

    let gzip = run_libra_command(&["archive", "--format=tgz"], repo.path());
    assert_cli_success(&gzip, "archive tgz");
    assert!(is_gzip(&gzip.stdout), "tgz should produce gzip output");

    let bzip2 = run_libra_command(&["archive", "--format=tbz2"], repo.path());
    assert_cli_success(&bzip2, "archive tbz2");
    assert!(is_bzip2(&bzip2.stdout), "tbz2 should produce bzip2 output");

    let short_bzip2 = run_libra_command(&["archive", "--format=tbz"], repo.path());
    assert_cli_success(&short_bzip2, "archive tbz");
    assert!(
        is_bzip2(&short_bzip2.stdout),
        "tbz should produce bzip2 output"
    );
}

#[test]
fn archive_writes_output_file() {
    let repo = create_archive_test_repo();
    let out = repo.path().join("out.tar");
    let out_str = out.to_str().expect("archive output path should be UTF-8");

    let output = run_libra_command(&["archive", "-o", out_str], repo.path());

    assert_cli_success(&output, "archive -o");
    assert!(
        output.stdout.is_empty(),
        "file output should not write archive bytes to stdout"
    );
    assert!(
        is_tar(&read_bytes(&out)),
        "output file should contain tar data"
    );
}

#[test]
fn archive_applies_prefix_to_tar_paths() {
    let repo = create_archive_test_repo();
    let out = repo.path().join("prefixed.tar");
    let out_str = out.to_str().expect("archive output path should be UTF-8");

    let output = run_libra_command(
        &["archive", "-o", out_str, "--prefix", "myapp/"],
        repo.path(),
    );

    assert_cli_success(&output, "archive --prefix");
    let data = read_bytes(&out);
    let text = String::from_utf8_lossy(&data);
    assert!(
        text.contains("myapp/README.md"),
        "tar should contain prefixed README path"
    );
    assert!(
        text.contains("myapp/src/main.rs"),
        "tar should contain prefixed source path"
    );
}

#[test]
fn archive_empty_repo_reports_invalid_target() {
    let repo = tempdir().expect("failed to create empty archive test repository");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["archive"], repo.path());

    assert!(
        !output.status.success(),
        "archive should fail without commits"
    );
    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(
        report.message.contains("failed to resolve"),
        "unexpected empty repo message: {}",
        report.message
    );
}

#[test]
fn archive_rejects_invalid_treeish() {
    let repo = create_archive_test_repo();

    let output = run_libra_command(&["archive", "nonexistent-branch"], repo.path());

    assert!(
        !output.status.success(),
        "archive should reject an unknown tree-ish"
    );
    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
}

#[test]
fn archive_rejects_invalid_format() {
    let repo = create_archive_test_repo();

    let output = run_libra_command(&["archive", "--format=bogus"], repo.path());

    assert!(
        !output.status.success(),
        "archive should reject unknown formats"
    );
    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report.message.contains("unknown archive format"),
        "unexpected format error message: {}",
        report.message
    );
}

#[test]
fn archive_rejects_empty_format() {
    let repo = create_archive_test_repo();

    let output = run_libra_command(&["archive", "--format="], repo.path());

    assert!(
        !output.status.success(),
        "archive should reject an empty format"
    );
    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report.message.contains("unknown archive format"),
        "unexpected empty format error message: {}",
        report.message
    );
}

#[test]
fn archive_rejects_archive_slip_prefix() {
    let repo = create_archive_test_repo();

    let output = run_libra_command(&["archive", "--prefix", "../release"], repo.path());

    assert!(
        !output.status.success(),
        "archive should reject parent-directory prefixes"
    );
    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report.message.contains("invalid archive prefix"),
        "unexpected prefix error message: {}",
        report.message
    );
}

#[test]
fn archive_rejects_output_in_missing_directory() {
    let repo = create_archive_test_repo();
    let out = repo.path().join("missing").join("out.tar");
    let out_str = out.to_str().expect("archive output path should be UTF-8");

    let output = run_libra_command(&["archive", "-o", out_str], repo.path());

    assert!(
        !output.status.success(),
        "archive should fail when output parent directory is missing"
    );
    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-IO-002");
}

#[test]
fn archive_outside_repository_reports_repo_not_found() {
    let temp = tempdir().expect("failed to create non-repository archive test directory");

    let output = run_libra_command(&["archive"], temp.path());

    assert!(
        !output.status.success(),
        "archive should fail outside a repository"
    );
    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-001");
}

#[test]
fn archive_preserves_unicode_filenames() {
    let repo = tempdir().expect("failed to create unicode archive test repository");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    fs::write(repo.path().join("你好世界.txt"), "unicode content\n")
        .expect("failed to write unicode file");

    let output = run_libra_command(&["add", ".libraignore", "你好世界.txt"], repo.path());
    assert_cli_success(&output, "failed to add unicode file");
    let output = run_libra_command(&["commit", "-m", "unicode", "--no-verify"], repo.path());
    assert_cli_success(&output, "failed to commit unicode file");

    let out = repo.path().join("unicode.tar");
    let out_str = out.to_str().expect("archive output path should be UTF-8");
    let output = run_libra_command(&["archive", "-o", out_str], repo.path());

    assert_cli_success(&output, "archive unicode");
    let text = String::from_utf8_lossy(&read_bytes(&out)).to_string();
    assert!(
        text.contains("你好世界.txt"),
        "tar should contain unicode filename"
    );
}

#[test]
fn archive_preserves_spaces_in_filenames() {
    let repo = tempdir().expect("failed to create spaced filename archive test repository");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    fs::create_dir_all(repo.path().join("my docs")).expect("failed to create spaced directory");
    fs::write(
        repo.path().join("my docs").join("hello world.txt"),
        "hello\n",
    )
    .expect("failed to write spaced filename");

    let output = run_libra_command(
        &["add", ".libraignore", "my docs/hello world.txt"],
        repo.path(),
    );
    assert_cli_success(&output, "failed to add spaced filename");
    let output = run_libra_command(&["commit", "-m", "spaces", "--no-verify"], repo.path());
    assert_cli_success(&output, "failed to commit spaced filename");

    let out = repo.path().join("spaces.tar");
    let out_str = out.to_str().expect("archive output path should be UTF-8");
    let output = run_libra_command(&["archive", "-o", out_str], repo.path());

    assert_cli_success(&output, "archive spaces");
    let text = String::from_utf8_lossy(&read_bytes(&out)).to_string();
    assert!(
        text.contains("my docs/hello world.txt"),
        "tar should contain filename with spaces"
    );
}

#[test]
fn archive_preserves_deeply_nested_paths() {
    let repo = tempdir().expect("failed to create deep archive test repository");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    let deep = repo.path().join("a/b/c/d/e/f/g");
    fs::create_dir_all(&deep).expect("failed to create deep directory");
    fs::write(deep.join("deep.txt"), "bottom\n").expect("failed to write deep file");

    let output = run_libra_command(&["add", ".libraignore", "a/"], repo.path());
    assert_cli_success(&output, "failed to add deep path");
    let output = run_libra_command(&["commit", "-m", "deep", "--no-verify"], repo.path());
    assert_cli_success(&output, "failed to commit deep path");

    let out = repo.path().join("deep.tar");
    let out_str = out.to_str().expect("archive output path should be UTF-8");
    let output = run_libra_command(&["archive", "-o", out_str], repo.path());

    assert_cli_success(&output, "archive deep path");
    let text = String::from_utf8_lossy(&read_bytes(&out)).to_string();
    assert!(
        text.contains("a/b/c/d/e/f/g/deep.txt"),
        "tar should contain full nested path"
    );
}

#[test]
fn archive_preserves_empty_files() {
    let repo = tempdir().expect("failed to create empty file archive test repository");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    fs::write(repo.path().join("empty.txt"), "").expect("failed to write empty file");

    let output = run_libra_command(&["add", ".libraignore", "empty.txt"], repo.path());
    assert_cli_success(&output, "failed to add empty file");
    let output = run_libra_command(&["commit", "-m", "empty", "--no-verify"], repo.path());
    assert_cli_success(&output, "failed to commit empty file");

    let out = repo.path().join("empty.tar");
    let out_str = out.to_str().expect("archive output path should be UTF-8");
    let output = run_libra_command(&["archive", "-o", out_str], repo.path());

    assert_cli_success(&output, "archive empty file");
    let text = String::from_utf8_lossy(&read_bytes(&out)).to_string();
    assert!(text.contains("empty.txt"), "tar should contain empty file");
}

#[test]
fn archive_short_format_flag_writes_zip() {
    let repo = create_archive_test_repo();

    let output = run_libra_command(&["archive", "-f", "zip"], repo.path());

    assert_cli_success(&output, "archive -f zip");
    assert!(is_zip(&output.stdout), "expected zip output from -f zip");
}

#[test]
fn archive_zip_writes_output_file() {
    let repo = create_archive_test_repo();
    let out = repo.path().join("out.zip");
    let out_str = out.to_str().expect("archive output path should be UTF-8");

    let output = run_libra_command(&["archive", "--format=zip", "-o", out_str], repo.path());

    assert_cli_success(&output, "archive zip -o");
    assert!(
        output.stdout.is_empty(),
        "zip file output should not write archive bytes to stdout"
    );
    assert!(
        is_zip(&read_bytes(&out)),
        "output file should contain zip data"
    );
}

#[test]
fn archive_help_mentions_archive_options() {
    let repo = create_archive_test_repo();

    let output = run_libra_command(&["archive", "--help"], repo.path());

    assert_cli_success(&output, "archive --help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--format"), "help should mention --format");
    assert!(stdout.contains("--output"), "help should mention --output");
    assert!(stdout.contains("--prefix"), "help should mention --prefix");
}

#[test]
fn archive_preserves_subdirectory_paths() {
    let repo = create_archive_test_repo();
    let out = repo.path().join("dirs.tar");
    let out_str = out.to_str().expect("archive output path should be UTF-8");

    let output = run_libra_command(&["archive", "-o", out_str], repo.path());

    assert_cli_success(&output, "archive subdirectories");
    let text = String::from_utf8_lossy(&read_bytes(&out)).to_string();
    assert!(text.contains("README.md"), "tar should contain root file");
    assert!(
        text.contains("src/main.rs"),
        "tar should contain nested source file"
    );
}

#[test]
fn archive_prefix_accepts_trailing_slash() {
    let repo = create_archive_test_repo();
    let out = repo.path().join("prefixed-slash.tar");
    let out_str = out.to_str().expect("archive output path should be UTF-8");

    let output = run_libra_command(
        &["archive", "-o", out_str, "--prefix", "release/"],
        repo.path(),
    );

    assert_cli_success(&output, "archive prefix with trailing slash");
    let text = String::from_utf8_lossy(&read_bytes(&out)).to_string();
    assert!(
        text.contains("release/README.md"),
        "tar should contain prefix with trailing slash"
    );
}

#[test]
fn archive_accepts_tags() {
    let repo = create_archive_test_repo();

    let output = run_libra_command(&["tag", "v0.1.0"], repo.path());
    assert_cli_success(&output, "failed to create archive fixture tag");

    let output = run_libra_command(&["archive", "v0.1.0"], repo.path());

    assert_cli_success(&output, "archive tag");
    assert!(is_tar(&output.stdout), "expected tar output from tag");
}

#[test]
fn archive_accepts_short_commit_hashes() {
    let repo = create_archive_test_repo();

    let output = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert_cli_success(&output, "failed to resolve HEAD");
    let full_hash = String::from_utf8_lossy(&output.stdout);
    let short_hash = full_hash
        .trim()
        .get(..8)
        .expect("archive fixture commit hash should be at least 8 chars");

    let output = run_libra_command(&["archive", short_hash], repo.path());

    assert_cli_success(&output, "archive short hash");
    assert!(
        is_tar(&output.stdout),
        "expected tar output from short hash"
    );
}

#[test]
fn archive_head_uses_latest_commit_tree() {
    let repo = create_archive_test_repo();
    fs::write(repo.path().join("NEW.txt"), "second commit\n")
        .expect("failed to write second commit fixture");

    let output = run_libra_command(&["add", "NEW.txt"], repo.path());
    assert_cli_success(&output, "failed to add second commit fixture");
    let output = run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());
    assert_cli_success(&output, "failed to create second commit");

    let out = repo.path().join("head.tar");
    let out_str = out.to_str().expect("archive output path should be UTF-8");
    let output = run_libra_command(&["archive", "-o", out_str, "HEAD"], repo.path());

    assert_cli_success(&output, "archive HEAD after second commit");
    let text = String::from_utf8_lossy(&read_bytes(&out)).to_string();
    assert!(
        text.contains("NEW.txt"),
        "archive should contain the latest committed file"
    );
}

#[test]
fn archive_tar_gz_file_output_is_not_truncated() {
    let repo = create_archive_test_repo();
    let out = repo.path().join("out.tar.gz");
    let out_str = out.to_str().expect("archive output path should be UTF-8");

    let output = run_libra_command(&["archive", "-o", out_str, "--format=tar.gz"], repo.path());

    assert_cli_success(&output, "archive tar.gz file");
    let data = read_bytes(&out);
    assert!(is_gzip(&data), "archive output should be gzip");
    assert!(data.len() > 20, "archive output should not be truncated");
}

#[test]
fn test_archive_verbose_lists_paths_on_stderr() {
    let repo = create_archive_test_repo();
    let out = repo.path().join("verbose.tar");
    let out_str = out.to_str().unwrap();

    let output = run_libra_command(&["archive", "-v", "-o", out_str, "HEAD"], repo.path());
    assert_cli_success(&output, "archive -v");

    // `-v` reports each archived path to stderr (mirroring git archive -v); the
    // archive bytes go to the -o file, so stdout stays empty.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("README.md"),
        "verbose should list README.md on stderr: {stderr}"
    );
    assert!(
        stderr.contains("src/main.rs"),
        "verbose should list src/main.rs on stderr: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "with -o, the archive bytes must not leak to stdout: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );

    // `--prefix` is applied to the reported paths too.
    let output = run_libra_command(
        &["archive", "-v", "-o", out_str, "--prefix", "app/", "HEAD"],
        repo.path(),
    );
    assert_cli_success(&output, "archive -v --prefix");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("app/README.md"),
        "verbose should list the prefixed path on stderr: {stderr}"
    );
}

/// `--compression-level <0-9>` is threaded into the gzip/bzip2/zip encoders:
/// for highly compressible data, level 9 produces a smaller archive than level
/// 0, and an out-of-range level is rejected by the parser.
#[test]
fn archive_compression_level_affects_output() {
    let repo = create_archive_test_repo();
    let p = repo.path();
    // A large, highly compressible committed file so the level is observable.
    fs::write(p.join("big.txt"), "a".repeat(20_000)).unwrap();
    assert_cli_success(&run_libra_command(&["add", "big.txt"], p), "add big.txt");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "big", "--no-verify"], p),
        "commit big.txt",
    );

    let out0 = p.join("l0.tar.gz");
    let out9 = p.join("l9.tar.gz");
    assert_cli_success(
        &run_libra_command(
            &[
                "archive",
                "--format=tar.gz",
                "--compression-level=0",
                "-o",
                out0.to_str().unwrap(),
                "HEAD",
            ],
            p,
        ),
        "archive level 0",
    );
    assert_cli_success(
        &run_libra_command(
            &[
                "archive",
                "--format=tar.gz",
                "--compression-level=9",
                "-o",
                out9.to_str().unwrap(),
                "HEAD",
            ],
            p,
        ),
        "archive level 9",
    );
    let s0 = read_bytes(&out0);
    let s9 = read_bytes(&out9);
    assert!(
        is_gzip(&s0) && is_gzip(&s9),
        "both levels produce gzip output"
    );
    assert!(
        s9.len() < s0.len(),
        "level 9 ({}) must be smaller than level 0 ({}) for compressible data",
        s9.len(),
        s0.len()
    );

    // An out-of-range level is rejected by the parser.
    assert!(
        !run_libra_command(
            &[
                "archive",
                "--format=tar.gz",
                "--compression-level=10",
                "HEAD"
            ],
            p
        )
        .status
        .success(),
        "--compression-level=10 must be rejected"
    );

    // zip also honors the level and stays a valid zip.
    let z = run_libra_command(&["archive", "--format=zip", "--compression-level=9"], p);
    assert_cli_success(&z, "zip with --compression-level");
    assert!(is_zip(&z.stdout), "zip output is valid");
}

/// Parse the modification time (octal) from the first tar header in `data`.
/// The ustar/gnu `mtime` field is 12 bytes at offset 136.
fn first_tar_entry_mtime(data: &[u8]) -> u64 {
    assert!(data.len() >= 148, "tar data too small for a header");
    let field = &data[136..148];
    let digits: String = field
        .iter()
        .take_while(|&&b| b != 0 && b != b' ')
        .map(|&b| b as char)
        .collect();
    u64::from_str_radix(digits.trim(), 8).expect("tar mtime is octal")
}

/// Decode `(year, month, day)` from the first zip local file header's MS-DOS
/// mod-date (little-endian u16 at offset 12: bits 0-4 day, 5-8 month, 9-15
/// year-since-1980).
fn first_zip_entry_date(data: &[u8]) -> (u16, u16, u16) {
    assert_eq!(
        &data[0..4],
        b"PK\x03\x04",
        "zip local file header signature"
    );
    let dos_date = u16::from_le_bytes([data[12], data[13]]);
    let day = dos_date & 0x1f;
    let month = (dos_date >> 5) & 0xf;
    let year = ((dos_date >> 9) & 0x7f) + 1980;
    (year, month, day)
}

/// Read the committer Unix timestamp of `HEAD` via `cat-file -p`.
fn head_committer_time(repo: &Path) -> u64 {
    let out = run_libra_command(&["cat-file", "-p", "HEAD"], repo);
    assert_cli_success(&out, "cat-file -p HEAD");
    let text = String::from_utf8_lossy(&out.stdout);
    let committer = text
        .lines()
        .find(|l| l.starts_with("committer "))
        .expect("commit has a committer line");
    // "committer Name <email> <unixts> <tz>"
    let fields: Vec<&str> = committer.split_whitespace().collect();
    fields[fields.len() - 2]
        .parse()
        .expect("committer timestamp is numeric")
}

#[test]
fn archive_default_mtime_uses_commit_committer_time() {
    // Previously the mtime was hard-coded to epoch 0; it must now be exactly the
    // archived commit's committer time, matching Git.
    let repo = create_archive_test_repo();
    let out = run_libra_command(&["archive", "--format=tar", "HEAD"], repo.path());
    assert_cli_success(&out, "archive HEAD");
    assert!(is_tar(&out.stdout), "tar output");
    assert_eq!(
        first_tar_entry_mtime(&out.stdout),
        head_committer_time(repo.path()),
        "default mtime equals the commit's committer time (not epoch 0)"
    );
}

#[test]
fn archive_zip_mtime_flag_sets_entry_date() {
    // `--mtime` also drives the zip entry's MS-DOS mod-date.
    let repo = create_archive_test_repo();
    let out = run_libra_command(
        &[
            "archive",
            "--format=zip",
            "--mtime=2020-01-02 03:04:05 +0000",
            "HEAD",
        ],
        repo.path(),
    );
    assert_cli_success(&out, "zip --mtime");
    assert!(is_zip(&out.stdout), "zip output");
    assert_eq!(
        first_zip_entry_date(&out.stdout),
        (2020, 1, 2),
        "--mtime sets the zip entry's mod-date"
    );
}

#[test]
fn archive_mtime_flag_overrides_entry_time() {
    // `--mtime` sets every entry's modification time. 2020-01-02 03:04:05 UTC.
    let repo = create_archive_test_repo();
    let out = run_libra_command(
        &[
            "archive",
            "--format=tar",
            "--mtime=2020-01-02 03:04:05 +0000",
            "HEAD",
        ],
        repo.path(),
    );
    assert_cli_success(&out, "archive --mtime");
    assert_eq!(
        first_tar_entry_mtime(&out.stdout),
        1_577_934_245,
        "--mtime sets the tar entry time"
    );
}

#[test]
fn archive_mtime_rejects_invalid_value() {
    let repo = create_archive_test_repo();
    let out = run_libra_command(
        &["archive", "--format=tar", "--mtime=not-a-date", "HEAD"],
        repo.path(),
    );
    assert_eq!(
        out.status.code(),
        Some(129),
        "an unparseable --mtime is a usage error: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
