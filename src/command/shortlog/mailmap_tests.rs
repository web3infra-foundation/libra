use std::fs;

use tempfile::tempdir;

use super::mailmap::load_mailmap;

const MAILMAP_FILE: &str = ".mailmap";
const MAX_MAILMAP_LINE_BYTES: usize = 256;

#[test]
fn load_mailmap_missing_file_is_empty() {
    let dir = tempdir().unwrap();
    let loaded = load_mailmap(dir.path());
    assert!(loaded.warnings.is_empty());
    assert_eq!(
        loaded.mailmap.resolve("Test User", "test@example.com"),
        ("Test User".to_string(), "test@example.com".to_string())
    );
}

#[test]
fn parse_mailmap_maps_name_and_email_by_commit_identity() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join(MAILMAP_FILE),
        "Proper Name <proper@example.com> Commit Name <commit@example.com>\n",
    )
    .unwrap();

    let loaded = load_mailmap(dir.path());
    assert!(loaded.warnings.is_empty());
    assert_eq!(
        loaded.mailmap.resolve("Commit Name", "commit@example.com"),
        ("Proper Name".to_string(), "proper@example.com".to_string())
    );
}

#[test]
fn parse_mailmap_maps_email_only_form() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join(MAILMAP_FILE),
        "<proper@example.com> <commit@example.com>\n",
    )
    .unwrap();

    let loaded = load_mailmap(dir.path());
    assert_eq!(
        loaded.mailmap.resolve("Commit Name", "commit@example.com"),
        ("Commit Name".to_string(), "proper@example.com".to_string())
    );
}

#[test]
fn parse_mailmap_reports_malformed_and_long_lines() {
    let dir = tempdir().unwrap();
    let long_line = "x".repeat(MAX_MAILMAP_LINE_BYTES + 1);
    fs::write(
        dir.path().join(MAILMAP_FILE),
        format!("bad line\n{long_line}\nProper Name <commit@example.com>\n"),
    )
    .unwrap();

    let loaded = load_mailmap(dir.path());
    assert_eq!(loaded.warnings.len(), 2);
    assert_eq!(
        loaded.mailmap.resolve("Old", "commit@example.com"),
        ("Proper Name".to_string(), "commit@example.com".to_string())
    );
}

#[cfg(unix)]
#[test]
fn load_mailmap_rejects_symlink() {
    use std::os::unix::fs::symlink;

    let dir = tempdir().unwrap();
    let target = dir.path().join("outside-mailmap");
    fs::write(
        &target,
        "Proper <proper@example.com> <commit@example.com>\n",
    )
    .unwrap();
    symlink(&target, dir.path().join(MAILMAP_FILE)).unwrap();

    let loaded = load_mailmap(dir.path());
    assert_eq!(loaded.warnings.len(), 1);
    assert_eq!(
        loaded.mailmap.resolve("Commit", "commit@example.com"),
        ("Commit".to_string(), "commit@example.com".to_string())
    );
}
