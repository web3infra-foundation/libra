//! Binary-level `libra clone` behavior checks.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::Command,
    sync::Arc,
    thread,
};

use git_internal::internal::object::{
    ObjectTrait,
    blob::Blob,
    commit::Commit,
    tree::{Tree, TreeItem, TreeItemMode},
};
use libra::{
    internal::model::reference,
    utils::{
        pager::LIBRA_TEST_ENV,
        storage::{Storage, remote::RemoteStorage},
    },
};
use object_store::local::LocalFileSystem;
use serde_json::Value;
use tempfile::{TempDir, tempdir};

use super::parse_cli_error_stderr;

fn run_libra(args: &[&str], cwd: &Path) -> std::process::Output {
    let home = cwd.join(".home");
    let config_home = home.join(".config");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&config_home).unwrap();

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", home)
        .env("USERPROFILE", cwd.join(".home"))
        .env("XDG_CONFIG_HOME", config_home)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env(LIBRA_TEST_ENV, "1")
        .output()
        .unwrap()
}

fn run_libra_with_env(
    args: &[&str],
    cwd: &Path,
    extra_env: &[(&str, &str)],
) -> std::process::Output {
    let home = cwd.join(".home");
    let config_home = home.join(".config");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&config_home).unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_libra"));
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", home)
        .env("USERPROFILE", cwd.join(".home"))
        .env("XDG_CONFIG_HOME", config_home)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env(LIBRA_TEST_ENV, "1");
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.output().unwrap()
}

fn run_libra_with_home(args: &[&str], cwd: &Path, home: &Path) -> std::process::Output {
    let config_home = home.join(".config");
    fs::create_dir_all(&config_home).unwrap();

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", config_home)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env(LIBRA_TEST_ENV, "1")
        .output()
        .unwrap()
}

#[test]
fn clone_cloud_missing_clone_domain_config_fails_before_restore_stub() {
    let cwd = tempdir().unwrap();
    let dest = cwd.path().join("restored");

    let output = run_libra(
        &[
            "clone",
            "libra+cloud://code.example.com/kepler-ledger",
            dest.to_str().unwrap(),
        ],
        cwd.path(),
    );

    assert!(
        !output.status.success(),
        "cloud clone without clone-domain config should fail"
    );
    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-AUTH-001");
    assert!(
        report
            .message
            .contains("clone domain 'code.example.com' is not configured"),
        "error should identify the missing clone-domain config: {:?}",
        report.message
    );
    assert!(
        report
            .hints
            .iter()
            .any(|hint| hint.contains("cloud.clone_domains.code.example.com.account_id")),
        "hint should point at the clone-domain config keys: {:?}",
        report.hints
    );
    assert!(
        !dest.exists(),
        "cloud clone config preflight must not create the destination"
    );
}

#[test]
fn clone_cloud_rejects_unsupported_git_style_options_before_config_lookup() {
    let cwd = tempdir().unwrap();
    let source = "libra+cloud://code.example.com/kepler-ledger";

    for (name, leading_args, needle) in [
        ("branch", vec!["clone", "--branch", "main"], "--branch"),
        ("depth", vec!["clone", "--depth", "1"], "--depth"),
        (
            "single-branch",
            vec!["clone", "--single-branch"],
            "--single-branch",
        ),
        ("bare", vec!["clone", "--bare"], "--bare"),
    ] {
        let dest = cwd.path().join(format!("restored-{name}"));
        let mut args = leading_args;
        args.push(source);
        args.push(dest.to_str().unwrap());

        let output = run_libra(&args, cwd.path());
        assert!(
            !output.status.success(),
            "{needle} cloud clone should fail before restore"
        );
        let (_, report) = parse_cli_error_stderr(&output.stderr);
        assert_eq!(report.error_code, "LBR-CLI-002");
        assert!(
            report.message.contains(needle),
            "error should identify the unsupported option: {:?}",
            report.message
        );
        assert!(
            report.message.contains("libra+cloud://"),
            "error should identify the cloud source surface: {:?}",
            report.message
        );
        assert!(
            !report
                .message
                .contains("clone domain 'code.example.com' is not configured"),
            "unsupported option should be rejected before config lookup: {:?}",
            report.message
        );
        assert!(
            !dest.exists(),
            "unsupported cloud clone option must not create the destination"
        );
    }
}

#[test]
fn clone_cloud_configured_domain_requires_d1_api_token_before_site_lookup() {
    let cwd = tempdir().unwrap();
    let dest = cwd.path().join("restored");

    for (key, value) in [
        (
            "cloud.clone_domains.code.example.com.account_id",
            "acct_123",
        ),
        (
            "cloud.clone_domains.code.example.com.d1_database_id",
            "d1_pub_456",
        ),
        (
            "cloud.clone_domains.code.example.com.r2_bucket",
            "publish-r2",
        ),
        (
            "cloud.clone_domains.code.example.com.credential_profile",
            "prod",
        ),
    ] {
        let output = run_libra(&["config", "set", "--global", key, value], cwd.path());
        assert!(
            output.status.success(),
            "config set should succeed for {key}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = run_libra(
        &[
            "clone",
            "libra+cloud://code.example.com/kepler-ledger?ref=refs/tags/v1.0.0",
            dest.to_str().unwrap(),
        ],
        cwd.path(),
    );

    assert!(
        !output.status.success(),
        "configured cloud clone should fail"
    );
    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-AUTH-001");
    assert!(
        report.message.contains("D1 API token"),
        "configured clone-domain should ask for the D1 API token before site lookup: {:?}",
        report.message
    );
    assert!(
        !report
            .message
            .contains("clone domain 'code.example.com' is not configured"),
        "configured clone-domain should not fail at the config preflight: {:?}",
        report.message
    );
    assert_eq!(
        report.details.get("clone_domain").and_then(Value::as_str),
        Some("code.example.com")
    );
    assert_eq!(
        report.details.get("missing_keys").and_then(Value::as_str),
        Some("LIBRA_D1_API_TOKEN (env or vault)")
    );
    assert!(
        !dest.exists(),
        "cloud clone D1 credential preflight must not create the destination"
    );
}

#[test]
fn clone_cloud_mock_d1_and_r2_restores_slug_tag_and_repo_id_sources() {
    let cwd = tempdir().unwrap();
    configure_cloud_clone_domain(cwd.path());
    let fixture = create_cloud_clone_cli_fixture();
    let r2_root = fixture.r2_root.path().to_str().unwrap();
    let d1_base = fixture.d1.base_url.as_str();
    let probe_home = cwd.path().join("probe-home");
    fs::create_dir_all(&probe_home).unwrap();

    let default_dest = cwd.path().join("restored-default");
    let output = run_libra_with_env(
        &[
            "--json",
            "clone",
            "libra+cloud://code.example.com/kepler-ledger",
            default_dest.to_str().unwrap(),
        ],
        cwd.path(),
        &[
            ("LIBRA_D1_API_TOKEN", "token_123"),
            ("LIBRA_D1_API_BASE_URL", d1_base),
            ("LIBRA_CLOUD_CLONE_TEST_R2_ROOT", r2_root),
        ],
    );
    assert!(
        output.status.success(),
        "default cloud clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_cloud_clone_json(
        &output.stdout,
        "main",
        "refs/heads/main",
        "kepler-ledger",
        &fixture.commit_oid,
    );
    assert_eq!(
        fs::read_to_string(default_dest.join("README.md")).unwrap(),
        "# cloud\n"
    );
    assert_rev_parse(&default_dest, &probe_home, "HEAD", &fixture.commit_oid);
    assert_rev_parse(&default_dest, &probe_home, "--abbrev-ref", "main");

    let tag_dest = cwd.path().join("restored-tag");
    let output = run_libra_with_env(
        &[
            "--json",
            "clone",
            "libra+cloud://code.example.com/kepler-ledger?ref=refs/tags/v1.0.0",
            tag_dest.to_str().unwrap(),
        ],
        cwd.path(),
        &[
            ("LIBRA_D1_API_TOKEN", "token_123"),
            ("LIBRA_D1_API_BASE_URL", d1_base),
            ("LIBRA_CLOUD_CLONE_TEST_R2_ROOT", r2_root),
        ],
    );
    assert!(
        output.status.success(),
        "tag cloud clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_cloud_clone_json(
        &output.stdout,
        Value::Null,
        "refs/tags/v1.0.0",
        "kepler-ledger",
        &fixture.commit_oid,
    );
    assert_eq!(
        fs::read_to_string(tag_dest.join("README.md")).unwrap(),
        "# cloud\n"
    );
    assert_rev_parse(&tag_dest, &probe_home, "HEAD", &fixture.commit_oid);
    assert_rev_parse(&tag_dest, &probe_home, "--abbrev-ref", "HEAD");

    let repo_dest = cwd.path().join("restored-repo-id");
    let output = run_libra_with_env(
        &[
            "--json",
            "clone",
            "libra+cloud://code.example.com/repo/repo_456",
            repo_dest.to_str().unwrap(),
        ],
        cwd.path(),
        &[
            ("LIBRA_D1_API_TOKEN", "token_123"),
            ("LIBRA_D1_API_BASE_URL", d1_base),
            ("LIBRA_CLOUD_CLONE_TEST_R2_ROOT", r2_root),
        ],
    );
    assert!(
        output.status.success(),
        "repo id cloud clone failed after slug rename: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_cloud_clone_json(
        &output.stdout,
        "main",
        "refs/heads/main",
        "renamed-ledger",
        &fixture.commit_oid,
    );
    assert_eq!(
        fs::read_to_string(repo_dest.join("README.md")).unwrap(),
        "# cloud\n"
    );
    assert_rev_parse(&repo_dest, &probe_home, "HEAD", &fixture.commit_oid);
}

fn configure_cloud_clone_domain(cwd: &Path) {
    for (key, value) in [
        (
            "cloud.clone_domains.code.example.com.account_id",
            "acct_123",
        ),
        (
            "cloud.clone_domains.code.example.com.d1_database_id",
            "d1_pub_456",
        ),
        (
            "cloud.clone_domains.code.example.com.r2_bucket",
            "publish-r2",
        ),
    ] {
        let output = run_libra(&["config", "set", "--global", key, value], cwd);
        assert!(
            output.status.success(),
            "config set should succeed for {key}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn assert_cloud_clone_json(
    stdout: &[u8],
    expected_branch: impl Into<Value>,
    expected_ref: &str,
    expected_slug: &str,
    expected_revision: &str,
) {
    let stdout = String::from_utf8_lossy(stdout);
    let json: Value = serde_json::from_str(stdout.trim()).expect("stdout should be JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "clone");
    let data = &json["data"];
    assert_eq!(data["branch"], expected_branch.into());
    assert_eq!(data["source_kind"], "cloudflare");
    assert_eq!(data["cloud_site"]["clone_domain"], "code.example.com");
    assert_eq!(data["cloud_site"]["site_id"], "site_123");
    assert_eq!(data["cloud_site"]["slug"], expected_slug);
    assert_eq!(data["cloud_site"]["repo_id"], "repo_456");
    assert_eq!(data["cloud_site"]["ref"], expected_ref);
    assert_eq!(data["cloud_site"]["revision"], expected_revision);
}

fn assert_rev_parse(repo: &Path, home: &Path, arg: &str, expected: &str) {
    let args = if arg == "HEAD" {
        vec!["rev-parse", "HEAD"]
    } else {
        vec!["rev-parse", arg, "HEAD"]
    };
    let output = run_libra_with_home(&args, repo, home);
    assert!(
        output.status.success(),
        "rev-parse {arg} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
}

struct CloudCloneCliFixture {
    r2_root: TempDir,
    d1: MockD1Server,
    commit_oid: String,
}

fn create_cloud_clone_cli_fixture() -> CloudCloneCliFixture {
    let r2_root = tempdir().unwrap();
    let blob = Blob::from_content("# cloud\n");
    let tree = Tree::from_tree_items(vec![TreeItem::new(
        TreeItemMode::Blob,
        blob.id,
        "README.md".to_string(),
    )])
    .expect("tree should build");
    let commit = Commit::from_tree_id(tree.id, Vec::new(), "cloud clone cli fixture");

    let local_store = LocalFileSystem::new_with_prefix(r2_root.path())
        .expect("local mock R2 root should be valid");
    let remote = RemoteStorage::new_with_prefix(Arc::new(local_store), "repo_456".to_string());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        put_remote_object(&remote, &blob).await;
        put_remote_object(&remote, &tree).await;
        put_remote_object(&remote, &commit).await;
        let refs = vec![
            reference::Model {
                id: 0,
                name: Some("main".to_string()),
                kind: reference::ConfigKind::Head,
                commit: None,
                remote: None,
            },
            reference::Model {
                id: 0,
                name: Some("main".to_string()),
                kind: reference::ConfigKind::Branch,
                commit: Some(commit.id.to_string()),
                remote: None,
            },
            reference::Model {
                id: 0,
                name: Some("refs/tags/v1.0.0".to_string()),
                kind: reference::ConfigKind::Tag,
                commit: Some(commit.id.to_string()),
                remote: None,
            },
        ];
        let metadata = serde_json::to_vec(&refs).expect("refs metadata should serialize");
        remote
            .put_metadata(&metadata)
            .await
            .expect("metadata should write to mock R2");
    });

    let data = MockD1Data {
        commit_oid: commit.id.to_string(),
        objects: vec![
            mock_object_row(&blob.id.to_string(), "blob", blob.to_data().unwrap().len()),
            mock_object_row(&tree.id.to_string(), "tree", tree.to_data().unwrap().len()),
            mock_object_row(
                &commit.id.to_string(),
                "commit",
                commit.to_data().unwrap().len(),
            ),
        ],
    };

    CloudCloneCliFixture {
        r2_root,
        d1: MockD1Server::start(data),
        commit_oid: commit.id.to_string(),
    }
}

async fn put_remote_object<T>(remote: &RemoteStorage, object: &T)
where
    T: ObjectTrait,
{
    let data = object.to_data().expect("object data should serialize");
    let hash = object.object_hash().expect("object hash should compute");
    remote
        .put(&hash, &data, object.get_type())
        .await
        .expect("object should write to mock R2");
}

fn mock_object_row(o_id: &str, o_type: &str, o_size: usize) -> Value {
    serde_json::json!({
        "o_id": o_id,
        "o_type": o_type,
        "o_size": o_size as i64,
        "repo_id": "repo_456",
        "created_at": 1778620800,
        "is_synced": 1
    })
}

#[derive(Clone)]
struct MockD1Data {
    commit_oid: String,
    objects: Vec<Value>,
}

struct MockD1Server {
    base_url: String,
    _handle: thread::JoinHandle<()>,
}

impl MockD1Server {
    fn start(data: MockD1Data) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock D1 should bind");
        let addr = listener
            .local_addr()
            .expect("mock D1 address should resolve");
        let base_url = format!("http://{addr}/client/v4");
        let handle = thread::spawn(move || {
            for stream in listener.incoming().take(24).flatten() {
                handle_mock_d1_request(stream, &data);
            }
        });
        Self {
            base_url,
            _handle: handle,
        }
    }
}

fn handle_mock_d1_request(mut stream: TcpStream, data: &MockD1Data) {
    let request = read_http_request(&mut stream);
    let body = request
        .split("\r\n\r\n")
        .nth(1)
        .expect("request should contain a body");
    let statement: Value = serde_json::from_str(body).expect("request body should be JSON");
    let sql = statement["sql"]
        .as_str()
        .expect("D1 statement should include SQL");
    let params = statement["params"].as_array().cloned().unwrap_or_default();

    let response = match mock_d1_rows(sql, &params, data) {
        Ok(rows) => serde_json::json!({
            "success": true,
            "errors": [],
            "messages": [],
            "result": [{ "results": rows, "success": true, "meta": {} }]
        }),
        Err(message) => serde_json::json!({
            "success": false,
            "errors": [{ "code": 3999, "message": message }],
            "messages": [],
            "result": []
        }),
    }
    .to_string();

    let http = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        response.len(),
        response
    );
    stream
        .write_all(http.as_bytes())
        .expect("mock D1 response should write");
}

fn read_http_request(stream: &mut TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let n = stream
            .read(&mut chunk)
            .expect("mock D1 request should read");
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
        if let Some(header_end) = find_header_end(&buffer) {
            let headers = String::from_utf8_lossy(&buffer[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if buffer.len() >= header_end + content_length {
                break;
            }
        }
    }
    String::from_utf8(buffer).expect("mock D1 request should be UTF-8")
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}

fn mock_d1_rows(sql: &str, params: &[Value], data: &MockD1Data) -> Result<Vec<Value>, String> {
    if sql.contains("FROM publish_sites WHERE clone_domain = ?1 AND slug = ?2") {
        return Ok(
            if param_str(params, 0) == Some("code.example.com")
                && param_str(params, 1) == Some("kepler-ledger")
            {
                vec![mock_publish_site_row("kepler-ledger", &data.commit_oid)]
            } else {
                Vec::new()
            },
        );
    }
    if sql.contains("FROM publish_sites WHERE clone_domain = ?1 AND repo_id = ?2") {
        return Ok(
            if param_str(params, 0) == Some("code.example.com")
                && param_str(params, 1) == Some("repo_456")
            {
                vec![mock_publish_site_row("renamed-ledger", &data.commit_oid)]
            } else {
                Vec::new()
            },
        );
    }
    if sql.contains("FROM repositories WHERE repo_id = ?1") {
        return Ok(if param_str(params, 0) == Some("repo_456") {
            vec![serde_json::json!({
                "repo_id": "repo_456",
                "name": "Kepler Ledger",
                "created_at": 1778620800,
                "updated_at": 1778620800
            })]
        } else {
            Vec::new()
        });
    }
    if sql.contains("FROM publish_refs WHERE site_id = ?1") {
        return Ok(if param_str(params, 0) == Some("site_123") {
            vec![
                mock_publish_ref_row("refs/heads/main", "branch", "main", 1, &data.commit_oid),
                mock_publish_ref_row("refs/tags/v1.0.0", "tag", "v1.0.0", 0, &data.commit_oid),
            ]
        } else {
            Vec::new()
        });
    }
    if sql.contains("FROM publish_revisions")
        && sql.contains("status = 'published'")
        && param_str(params, 0) == Some("site_123")
        && param_str(params, 1) == Some(data.commit_oid.as_str())
    {
        return Ok(vec![mock_publish_revision_row(&data.commit_oid)]);
    }
    if sql.contains("FROM object_index WHERE repo_id = ?1") {
        return Ok(if param_str(params, 0) == Some("repo_456") {
            data.objects.clone()
        } else {
            Vec::new()
        });
    }

    Err(format!("unexpected D1 SQL: {sql}"))
}

fn param_str(params: &[Value], index: usize) -> Option<&str> {
    params.get(index).and_then(Value::as_str)
}

fn mock_publish_site_row(slug: &str, revision_oid: &str) -> Value {
    serde_json::json!({
        "site_id": "site_123",
        "repo_id": "repo_456",
        "clone_domain": "code.example.com",
        "slug": slug,
        "display_origin": "https://code.example.com",
        "name": "Kepler Ledger",
        "visibility": "public",
        "status": "active",
        "worker_name": "libra-publish",
        "default_ref": "refs/heads/main",
        "latest_revision_oid": revision_oid,
        "refs_generation": 7,
        "max_preview_bytes": 1024,
        "schema_version": 1,
        "created_at": "2026-05-13T00:00:00Z",
        "updated_at": "2026-05-13T00:00:00Z"
    })
}

fn mock_publish_ref_row(
    ref_name: &str,
    ref_type: &str,
    short_name: &str,
    is_default: i64,
    revision_oid: &str,
) -> Value {
    serde_json::json!({
        "site_id": "site_123",
        "ref_name": ref_name,
        "ref_type": ref_type,
        "short_name": short_name,
        "target_oid": revision_oid,
        "revision_oid": revision_oid,
        "is_default": is_default,
        "sync_run_id": "sync_123",
        "schema_version": 1,
        "updated_at": "2026-05-13T00:00:00Z"
    })
}

fn mock_publish_revision_row(revision_oid: &str) -> Value {
    serde_json::json!({
        "site_id": "site_123",
        "revision_oid": revision_oid,
        "status": "published",
        "code_manifest_key": null,
        "ai_index_key": null,
        "file_count": 1,
        "ai_object_count": 0,
        "ai_bundle_count": 0,
        "redaction_mode": "default",
        "redaction_rules_version": "1",
        "sync_run_id": "sync_123",
        "schema_version": 1,
        "created_at": "2026-05-13T00:00:00Z",
        "updated_at": "2026-05-13T00:00:00Z"
    })
}

fn run_git(args: &[&str], cwd: &Path) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap()
}

fn create_remote_with_main(base: &Path) -> std::path::PathBuf {
    let remote = base.join("remote.git");
    assert!(
        run_git(&["init", "--bare", remote.to_str().unwrap()], base)
            .status
            .success()
    );

    let work = base.join("work");
    fs::create_dir_all(&work).unwrap();
    assert!(run_git(&["init"], &work).status.success());
    assert!(
        run_git(&["config", "user.name", "T"], &work)
            .status
            .success()
    );
    assert!(
        run_git(&["config", "user.email", "t@example.com"], &work)
            .status
            .success()
    );
    assert!(
        run_git(&["config", "commit.gpgsign", "false"], &work)
            .status
            .success()
    );
    fs::write(work.join("README.md"), "hello\n").unwrap();
    assert!(run_git(&["add", "README.md"], &work).status.success());
    assert!(
        run_git(&["commit", "-m", "initial"], &work)
            .status
            .success()
    );
    assert!(run_git(&["branch", "-M", "main"], &work).status.success());
    assert!(
        run_git(
            &["remote", "add", "origin", remote.to_str().unwrap()],
            &work
        )
        .status
        .success()
    );
    assert!(run_git(&["push", "origin", "main"], &work).status.success());
    assert!(
        run_git(&["symbolic-ref", "HEAD", "refs/heads/main"], &remote)
            .status
            .success()
    );
    remote
}

fn create_remote_with_gitignore(base: &Path) -> std::path::PathBuf {
    let remote = base.join("remote-with-ignore.git");
    assert!(
        run_git(&["init", "--bare", remote.to_str().unwrap()], base)
            .status
            .success()
    );

    let work = base.join("work-with-ignore");
    fs::create_dir_all(work.join("nested")).unwrap();
    assert!(run_git(&["init"], &work).status.success());
    assert!(
        run_git(&["config", "user.name", "T"], &work)
            .status
            .success()
    );
    assert!(
        run_git(&["config", "user.email", "t@example.com"], &work)
            .status
            .success()
    );
    fs::write(work.join("README.md"), "hello\n").unwrap();
    fs::write(work.join(".gitignore"), "ignored-root.log\n").unwrap();
    fs::write(work.join("nested").join(".gitignore"), "*.tmp\n").unwrap();
    assert!(
        run_git(
            &["add", "README.md", ".gitignore", "nested/.gitignore"],
            &work
        )
        .status
        .success()
    );
    assert!(
        run_git(&["commit", "-m", "initial with ignore files"], &work)
            .status
            .success()
    );
    assert!(run_git(&["branch", "-M", "main"], &work).status.success());
    assert!(
        run_git(
            &["remote", "add", "origin", remote.to_str().unwrap()],
            &work
        )
        .status
        .success()
    );
    assert!(run_git(&["push", "origin", "main"], &work).status.success());
    assert!(
        run_git(&["symbolic-ref", "HEAD", "refs/heads/main"], &remote)
            .status
            .success()
    );
    remote
}

fn create_empty_remote(base: &Path) -> std::path::PathBuf {
    let remote = base.join("empty-remote.git");
    assert!(
        run_git(&["init", "--bare", remote.to_str().unwrap()], base)
            .status
            .success()
    );
    remote
}

// =========================================================================
// Existing tests (updated for new output behavior)
// =========================================================================

#[test]
fn invalid_source_does_not_panic() {
    let temp = tempdir().unwrap();
    let dest = temp.path().join("dest");
    let output = run_libra(&["clone", "/", dest.to_str().unwrap()], temp.path());
    assert_eq!(output.status.code(), Some(128));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        stderr.contains("fatal:"),
        "expected fatal message, got: {stderr}"
    );
    assert!(
        stderr.contains("LBR-REPO-001"),
        "expected error code, got: {stderr}"
    );
    assert!(
        stderr.to_ascii_lowercase().contains("hint"),
        "expected hint, got: {stderr}"
    );
    assert_eq!(report.error_code, "LBR-REPO-001");
    assert_eq!(report.exit_code, 128);
    assert!(!stderr.contains("thread 'main' panicked"));
}

#[test]
fn missing_branch_keeps_preexisting_empty_destination() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let existing = temp.path().join("existing");
    fs::create_dir_all(&existing).unwrap();

    let output = run_libra(
        &[
            "clone",
            "-b",
            "nope",
            remote.to_str().unwrap(),
            existing.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(128));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(stderr.contains("remote branch"));
    assert!(stderr.contains("nope"));
    assert!(stderr.contains("LBR-REPO-003"));
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(existing.is_dir());
    assert_eq!(fs::read_dir(&existing).unwrap().count(), 0);
}

#[test]
fn successful_clone_output_has_no_debug_noise() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone");

    let output = run_libra(
        &["clone", remote.to_str().unwrap(), dest.to_str().unwrap()],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("Cloned into"),
        "expected clone summary on stdout, got: {stdout}"
    );
    assert!(
        stdout.contains("branch: main"),
        "expected branch info, got: {stdout}"
    );
    assert!(stderr.contains("Connecting to"));
    assert!(!stderr.contains(" INFO "));
    assert!(!stderr.contains(" WARN "));
    assert!(!stderr.contains("fatal: fatal:"));
    assert!(!stderr.contains('\u{2}'));
    assert!(dest.join("README.md").exists());
}

#[test]
fn successful_clone_initializes_vault() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone");
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let output = run_libra_with_home(
        &["clone", remote.to_str().unwrap(), dest.to_str().unwrap()],
        temp.path(),
        &home,
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        dest.join(".libra").join("vault.db").exists(),
        "clone should initialize .libra/vault.db for vault-backed workflows"
    );

    let signing_output = run_libra_with_home(&["config", "--get", "vault.signing"], &dest, &home);
    assert_eq!(
        signing_output.status.code(),
        Some(0),
        "failed to read vault.signing: {}",
        String::from_utf8_lossy(&signing_output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&signing_output.stdout).trim(),
        "true",
    );

    let gpg_output = run_libra_with_home(&["config", "--get", "vault.gpg.pubkey"], &dest, &home);
    assert_eq!(gpg_output.status.code(), Some(0));
    assert!(
        !String::from_utf8_lossy(&gpg_output.stdout)
            .trim()
            .is_empty()
    );
}

#[test]
fn clone_converts_gitignore_files_to_visible_libraignore_files() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_gitignore(temp.path());
    let dest = temp.path().join("clone-ignore");

    let output = run_libra(
        &["clone", remote.to_str().unwrap(), dest.to_str().unwrap()],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        fs::read_to_string(dest.join(".libraignore")).unwrap(),
        "ignored-root.log\n"
    );
    assert_eq!(
        fs::read_to_string(dest.join("nested").join(".libraignore")).unwrap(),
        "*.tmp\n"
    );

    fs::write(dest.join("ignored-root.log"), "ignored\n").unwrap();
    fs::write(dest.join("nested").join("ignored.tmp"), "ignored\n").unwrap();
    fs::write(dest.join("visible.txt"), "visible\n").unwrap();

    let status = run_libra(&["status", "--short"], &dest);
    assert_eq!(
        status.status.code(),
        Some(0),
        "status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        stdout.contains("?? .libraignore") && stdout.contains("?? nested/.libraignore"),
        "converted .libraignore files should remain visible, got: {stdout}"
    );
    assert!(
        stdout.contains("?? visible.txt"),
        "non-ignored untracked files should remain visible, got: {stdout}"
    );
    assert!(
        !stdout.contains("ignored-root.log") && !stdout.contains("ignored.tmp"),
        "converted ignore rules should hide matching files, got: {stdout}"
    );
}

#[test]
fn bare_clone_does_not_create_libraignore() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_gitignore(temp.path());
    let dest = temp.path().join("bare-ignore.git");

    let output = run_libra(
        &[
            "clone",
            "--bare",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "bare clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !dest.join(".libraignore").exists(),
        "bare clone should not create a worktree .libraignore"
    );
}

#[test]
fn machine_clone_suppresses_decorative_stderr() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-machine");

    let output = run_libra(
        &[
            "--machine",
            "clone",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "machine clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.trim().is_empty(),
        "machine clone should emit JSON on stdout"
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout should be valid JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "clone");
    assert!(
        stderr.trim().is_empty(),
        "machine clone should suppress decorative stderr, got: {stderr}"
    );
    assert!(dest.join("README.md").exists());
}

#[test]
fn json_clone_does_not_leak_init_output() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-json");

    let output = run_libra(
        &[
            "--json",
            "clone",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "json clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout should be valid JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "clone");
    assert!(
        !stderr.contains("\"command\":\"init\"")
            && !stderr.contains("Creating repository layout ..."),
        "clone stderr should not leak init output, got: {stderr}"
    );
    assert!(dest.join("README.md").exists());
}

// =========================================================================
// New tests
// =========================================================================

#[test]
fn json_clone_success_schema() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-schema");

    let output = run_libra(
        &[
            "--json",
            "clone",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();

    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "clone");
    let data = &json["data"];
    assert!(data["path"].is_string());
    assert_eq!(data["bare"], false);
    assert!(data["remote_url"].is_string());
    assert_eq!(data["branch"], "main");
    assert!(data["object_format"].is_string());
    assert!(data["repo_id"].is_string());
    assert!(data["vault_signing"].is_boolean());
    assert_eq!(data["shallow"], false);
    assert!(data["warnings"].is_array());
    assert_eq!(data["warnings"].as_array().unwrap().len(), 0);
    assert!(data.get("source_kind").is_none());
    assert!(data.get("cloud_site").is_none());
}

#[test]
fn json_clone_empty_remote() {
    let temp = tempdir().unwrap();
    let remote = create_empty_remote(temp.path());
    let dest = temp.path().join("clone-empty");

    let output = run_libra(
        &[
            "--json",
            "clone",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "json clone of empty repo failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();

    assert_eq!(json["ok"], true);
    let data = &json["data"];
    assert!(
        data["branch"].is_null(),
        "empty remote should have branch: null"
    );
    let warnings = data["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("empty repository")),
        "expected empty repo warning, got: {warnings:?}"
    );
}

#[test]
fn machine_clone_single_line_json() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-machine-line");

    let output = run_libra(
        &[
            "--machine",
            "clone",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let non_empty_lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        non_empty_lines.len(),
        1,
        "machine stdout should be exactly 1 non-empty line, got: {non_empty_lines:?}"
    );
    let _json: serde_json::Value =
        serde_json::from_str(non_empty_lines[0]).expect("single line should be valid JSON");
}

#[test]
fn quiet_clone_no_output_on_success() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-quiet");

    let output = run_libra(
        &[
            "--quiet",
            "clone",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.trim().is_empty(),
        "quiet clone should produce no stdout, got: {stdout}"
    );
    assert!(
        stderr.trim().is_empty(),
        "quiet clone should produce no stderr, got: {stderr}"
    );
    assert!(dest.join("README.md").exists());
}

#[test]
fn error_code_cannot_infer_destination() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["clone", "///"], temp.path());
    assert_eq!(output.status.code(), Some(129));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        stderr.contains("LBR-CLI-002"),
        "expected LBR-CLI-002, got: {stderr}"
    );
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert_eq!(report.exit_code, 129);
}

#[test]
fn error_code_destination_exists_non_empty() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("non-empty-dest");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("blocker.txt"), "exists").unwrap();

    let output = run_libra(
        &["clone", remote.to_str().unwrap(), dest.to_str().unwrap()],
        temp.path(),
    );
    assert_ne!(output.status.code(), Some(0));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        stderr.contains("LBR-CLI-003"),
        "expected LBR-CLI-003, got: {stderr}"
    );
    assert_eq!(report.exit_code, 129);
}

#[test]
fn error_code_missing_local_repo() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["clone", "/nonexistent/path/to/repo"], temp.path());
    assert_eq!(output.status.code(), Some(128));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        stderr.contains("LBR-REPO-001"),
        "expected LBR-REPO-001 for missing local repo, got: {stderr}"
    );
    assert_eq!(report.error_code, "LBR-REPO-001");
}

#[test]
fn error_code_remote_branch_not_found() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-bad-branch");

    let output = run_libra(
        &[
            "clone",
            "-b",
            "nonexistent-branch",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(128));

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(stderr.contains("nonexistent-branch"));
}

#[test]
fn hint_present_on_network_like_errors() {
    let temp = tempdir().unwrap();
    let output = run_libra(&["clone", "/nonexistent/path/to/repo"], temp.path());
    assert_ne!(output.status.code(), Some(0));

    let (stderr, _report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        stderr.to_ascii_lowercase().contains("hint"),
        "expected a hint in error output, got: {stderr}"
    );
}

#[test]
fn json_clone_init_output_isolation() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-isolation");

    let output = run_libra(
        &[
            "--json",
            "clone",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout should be a single valid JSON object");
    assert_eq!(
        json["command"], "clone",
        "unexpected command in JSON envelope"
    );
    assert_eq!(json["ok"], true);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("\"progress\""),
        "json clone stderr should not contain fetch NDJSON progress, got: {stderr}"
    );
}
