use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use libra::utils::pager::LIBRA_TEST_ENV;
use serde_json::Value;
use tempfile::tempdir;

fn base_libra_command(args: &[&str], cwd: &Path) -> Command {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    let global_db = home.join(".libra").join("config.db");
    fs::create_dir_all(&config_home).expect("failed to create isolated config directory");

    let mut command = Command::new(env!("CARGO_BIN_EXE_libra"));
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("LIBRA_CONFIG_GLOBAL_DB", &global_db)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env(LIBRA_TEST_ENV, "1");
    command
}

fn run_libra(args: &[&str], cwd: &Path) -> Output {
    base_libra_command(args, cwd)
        .output()
        .expect("failed to execute libra binary")
}

fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context}: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn copy_pack_fixture(workdir: &Path, stem: &str) -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/packs/small-sha1.pack");
    let destination = workdir.join(format!("{stem}.pack"));
    fs::copy(&source, &destination)
        .unwrap_or_else(|err| panic!("failed to copy {}: {err}", source.display()));
    destination
}

fn build_index(workdir: &Path, pack: &Path) -> PathBuf {
    let pack_arg = pack.to_string_lossy().to_string();
    let output = run_libra(&["index-pack", &pack_arg, "--index-version", "1"], workdir);
    assert_success(&output, "index-pack failed");
    pack.with_extension("idx")
}

fn prepare_two_indexes(workdir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let init = run_libra(&["init"], workdir);
    assert_success(&init, "failed to initialize fixture repository");

    let first_pack = copy_pack_fixture(workdir, "first");
    let second_pack = copy_pack_fixture(workdir, "second");
    let first_idx = build_index(workdir, &first_pack);
    let second_idx = build_index(workdir, &second_pack);
    (first_pack, first_idx, second_idx)
}

#[test]
fn verify_pack_accepts_multiple_index_files_in_order() {
    let workdir = tempdir().expect("failed to create tempdir");
    let (_pack, first_idx, second_idx) = prepare_two_indexes(workdir.path());
    let first_arg = first_idx.to_string_lossy().to_string();
    let second_arg = second_idx.to_string_lossy().to_string();

    let output = run_libra(&["verify-pack", &first_arg, &second_arg], workdir.path());
    assert_success(&output, "verify-pack multi-index failed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let expected = format!(
        "{}: ok\n{}: ok\n",
        first_idx.display(),
        second_idx.display()
    );
    assert_eq!(stdout, expected);
}

#[test]
fn verify_pack_stat_only_accepts_multiple_index_files() {
    let workdir = tempdir().expect("failed to create tempdir");
    let (_pack, first_idx, second_idx) = prepare_two_indexes(workdir.path());
    let first_arg = first_idx.to_string_lossy().to_string();
    let second_arg = second_idx.to_string_lossy().to_string();

    let output = run_libra(
        &["verify-pack", "-s", &first_arg, &second_arg],
        workdir.path(),
    );
    assert_success(&output, "verify-pack -s multi-index failed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.matches("non delta:").count(), 2, "{stdout}");
    assert!(!stdout.contains(": ok"), "{stdout}");
}

#[test]
fn verify_pack_json_preserves_single_shape_and_wraps_multi_shape() {
    let workdir = tempdir().expect("failed to create tempdir");
    let (_pack, first_idx, second_idx) = prepare_two_indexes(workdir.path());
    let first_arg = first_idx.to_string_lossy().to_string();
    let second_arg = second_idx.to_string_lossy().to_string();

    let single = run_libra(&["--json", "verify-pack", &first_arg], workdir.path());
    assert_success(&single, "single-index json verify-pack failed");
    let single_json: Value =
        serde_json::from_slice(&single.stdout).expect("single-index output should be json");
    assert_eq!(single_json["command"], "verify-pack");
    assert_eq!(single_json["data"]["idx_file"], first_arg);
    assert!(single_json["data"]["results"].is_null(), "{single_json}");

    let multi = run_libra(
        &["--json", "verify-pack", &first_arg, &second_arg],
        workdir.path(),
    );
    assert_success(&multi, "multi-index json verify-pack failed");
    let multi_json: Value =
        serde_json::from_slice(&multi.stdout).expect("multi-index output should be json");
    assert_eq!(multi_json["command"], "verify-pack");
    assert_eq!(multi_json["data"]["verified"], true);
    assert_eq!(multi_json["data"]["count"], 2);
    assert_eq!(multi_json["data"]["results"][0]["idx_file"], first_arg);
    assert_eq!(multi_json["data"]["results"][1]["idx_file"], second_arg);
}

#[test]
fn verify_pack_rejects_explicit_pack_with_multiple_indexes() {
    let workdir = tempdir().expect("failed to create tempdir");
    let (pack, first_idx, second_idx) = prepare_two_indexes(workdir.path());
    let pack_arg = pack.to_string_lossy().to_string();
    let first_arg = first_idx.to_string_lossy().to_string();
    let second_arg = second_idx.to_string_lossy().to_string();

    let output = run_libra(
        &["verify-pack", "--pack", &pack_arg, &first_arg, &second_arg],
        workdir.path(),
    );
    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-CLI-002"), "{stderr}");
    assert!(
        stderr.contains("cannot use --pack with multiple index files"),
        "{stderr}"
    );
}
