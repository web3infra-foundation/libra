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
