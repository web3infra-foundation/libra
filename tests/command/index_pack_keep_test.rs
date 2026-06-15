use std::{
    fs,
    path::{Path, PathBuf},
};

use serial_test::serial;
use tempfile::tempdir;

use super::{assert_cli_success, init_repo_via_cli, parse_json_stdout, run_libra_command};

fn find_pack(prefix: &str) -> PathBuf {
    let packs_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/packs");
    let mut matches = Vec::new();
    for entry in fs::read_dir(&packs_dir).expect("read packs dir failed") {
        let entry = entry.expect("dir entry error");
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(prefix) && name.ends_with(".pack") {
            matches.push(entry.path());
        }
    }
    match matches.len() {
        0 => panic!("pack with prefix `{prefix}` not found in {:?}", packs_dir),
        1 => matches.remove(0),
        _ => panic!(
            "multiple packs with prefix `{prefix}` found in {:?}",
            packs_dir
        ),
    }
}

fn copy_pack_to_dir(prefix: &str, dir: &Path) -> PathBuf {
    let pack_src = find_pack(prefix);
    let file_name = pack_src
        .file_name()
        .expect("pack file should have a filename");
    let pack_dst = dir.join(file_name);
    fs::copy(&pack_src, &pack_dst).expect("failed to stage pack fixture");
    pack_dst
}

#[test]
#[serial]
fn index_pack_keep_message_writes_keep_file_and_reports_json_path() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    let pack_dir = tempdir().unwrap();
    let pack_path = copy_pack_to_dir("small-sha1", pack_dir.path());
    let keep_path = pack_path.with_extension("keep");

    let output = run_libra_command(
        &[
            "index-pack",
            "--keep=keep from compatibility test",
            pack_path.to_str().unwrap(),
            "--json",
        ],
        repo.path(),
    );

    assert_cli_success(&output, "index-pack --keep --json should succeed");
    assert_eq!(
        fs::read_to_string(&keep_path).expect("keep file should be readable"),
        "keep from compatibility test\n"
    );

    let json = parse_json_stdout(&output);
    assert_eq!(
        json["data"]["keep_file"],
        keep_path.to_string_lossy().as_ref()
    );
}

#[test]
#[serial]
fn index_pack_keep_without_message_writes_empty_keep_file() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    let pack_dir = tempdir().unwrap();
    let pack_path = copy_pack_to_dir("small-sha1", pack_dir.path());
    let keep_path = pack_path.with_extension("keep");

    let output = run_libra_command(
        &["index-pack", "--keep", pack_path.to_str().unwrap()],
        repo.path(),
    );

    assert_cli_success(&output, "index-pack --keep should succeed");
    assert_eq!(
        fs::metadata(&keep_path)
            .expect("keep file should exist")
            .len(),
        0
    );
}
