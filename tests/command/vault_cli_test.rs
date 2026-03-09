//! Integration tests for `libra vault` CLI commands.

use std::{fs, process::Command};

use serial_test::serial;
use tempfile::tempdir;

fn run_libra(cwd: &std::path::Path, home: &std::path::Path, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    cmd.current_dir(cwd).env("HOME", home).args(args);
    #[cfg(windows)]
    cmd.env("USERPROFILE", home);
    cmd.output().expect("failed to execute libra")
}

#[test]
#[serial]
fn test_cli_vault_gpg_public_key_after_init() {
    let temp_root = tempdir().unwrap();
    let workdir = temp_root.path().join("work");
    let home = temp_root.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let init_out = run_libra(
        temp_root.path(),
        &home,
        &["init", "--vault", workdir.to_str().unwrap()],
    );
    assert!(
        init_out.status.success(),
        "init --vault should succeed, stderr: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );

    let pub_out = run_libra(&workdir, &home, &["vault", "gpg-public-key"]);
    assert!(
        pub_out.status.success(),
        "vault gpg-public-key should succeed, stderr: {}",
        String::from_utf8_lossy(&pub_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&pub_out.stdout);
    assert!(
        !stdout.trim().is_empty(),
        "gpg public key output should not be empty"
    );
    assert!(
        stdout.contains("BEGIN PGP PUBLIC KEY BLOCK"),
        "expected armored PGP public key, got: {stdout}"
    );
}

#[test]
#[serial]
fn test_cli_vault_generate_ssh_key_and_show_public_key() {
    let temp_root = tempdir().unwrap();
    let workdir = temp_root.path().join("work");
    let home = temp_root.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let init_out = run_libra(
        temp_root.path(),
        &home,
        &["init", "--vault", workdir.to_str().unwrap()],
    );
    assert!(
        init_out.status.success(),
        "init --vault should succeed, stderr: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );

    let gen_out = run_libra(
        &workdir,
        &home,
        &["vault", "generate-ssh-key", "--name", "libra-tester"],
    );
    assert!(
        gen_out.status.success(),
        "vault generate-ssh-key should succeed, stderr: {}",
        String::from_utf8_lossy(&gen_out.stderr)
    );
    let show_out = run_libra(&workdir, &home, &["vault", "ssh-public-key"]);
    assert!(
        show_out.status.success(),
        "vault ssh-public-key should succeed, stderr: {}",
        String::from_utf8_lossy(&show_out.stderr)
    );
    let show_stdout = String::from_utf8_lossy(&show_out.stdout);
    assert!(
        show_stdout.contains("ssh-"),
        "stored ssh public key should look like SSH format: {show_stdout}"
    );

    let repoid_out = run_libra(
        &workdir,
        &home,
        &["config", "--local", "--get", "libra.repoid"],
    );
    assert!(
        repoid_out.status.success(),
        "config --get libra.repoid should succeed, stderr: {}",
        String::from_utf8_lossy(&repoid_out.stderr)
    );
    let repo_id = String::from_utf8_lossy(&repoid_out.stdout)
        .trim()
        .to_string();
    assert!(!repo_id.is_empty(), "repo id should not be empty");

    assert!(
        home.join(".libra")
            .join("ssh-keys")
            .join(&repo_id)
            .join("id_ed25519")
            .is_file(),
        "vault ssh private key should be written to ~/.libra/ssh-keys/<repoid>/id_ed25519"
    );

    #[cfg(unix)]
    {
        let key_path = home
            .join(".libra")
            .join("ssh-keys")
            .join(&repo_id)
            .join("id_ed25519");
        let key_check = Command::new("ssh-keygen")
            .args(["-y", "-f", key_path.to_str().unwrap()])
            .output()
            .expect("failed to run ssh-keygen -y");
        assert!(
            key_check.status.success(),
            "vault-generated SSH private key should be parseable by OpenSSH, stderr: {}",
            String::from_utf8_lossy(&key_check.stderr)
        );
        let derived_pub = String::from_utf8_lossy(&key_check.stdout);
        assert!(
            derived_pub.starts_with("ssh-"),
            "ssh-keygen -y output should be an SSH public key, got: {derived_pub}"
        );
        assert_eq!(
            show_stdout.trim(),
            derived_pub.trim(),
            "vault ssh-public-key should match the generated private key"
        );
    }
}
