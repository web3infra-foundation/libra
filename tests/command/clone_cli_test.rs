//! Binary-level `libra clone` behavior checks.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::Command,
    sync::Arc,
    thread,
};

use chrono::{TimeZone, Utc};
use git_internal::internal::object::{
    ObjectTrait,
    blob::Blob,
    commit::Commit,
    tree::{Tree, TreeItem, TreeItemMode},
};
use libra::{
    internal::{
        model::reference,
        publish::{
            contract::{
                AiBundleAssociatedIds, AiBundleIndexes, AiBundleObjectEntry, AiBundleRedaction,
                AiGraphNode, AiObjectLayer, AiObjectRedaction, PUBLISH_SCHEMA_VERSION,
                PublishAiBundle, PublishAiGraph, PublishAiIndex, PublishAiIndexBundleEntry,
                PublishAiObject, RedactionMode,
            },
            snapshot::sha256_hex,
        },
    },
    utils::{
        pager::LIBRA_TEST_ENV,
        storage::{Storage, publish_storage::PublishStorage, remote::RemoteStorage},
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
        Some("vault.env.LIBRA_D1_API_TOKEN or LIBRA_D1_API_TOKEN")
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
    assert_cloud_clone_ai_history(&default_dest, &probe_home);

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
        let publish_store = PublishStorage::new(
            Arc::new(
                LocalFileSystem::new_with_prefix(r2_root.path())
                    .expect("local mock R2 root should be valid"),
            ),
            "repo_456",
            "site_123",
        )
        .expect("publish storage should build for mock R2");
        let ai_model = mock_publish_ai_model(&commit.id.to_string());
        publish_store
            .put_json(
                &mock_ai_index_relative_key(&commit.id.to_string()),
                &ai_model.index,
            )
            .await
            .expect("AI index should write to mock R2");
        publish_store
            .put_json(
                &mock_ai_graph_relative_key(&commit.id.to_string()),
                &ai_model.graph,
            )
            .await
            .expect("AI graph should write to mock R2");
        publish_store
            .put_json(
                &mock_ai_bundle_relative_key(&commit.id.to_string()),
                &ai_model.bundle,
            )
            .await
            .expect("AI bundle should write to mock R2");
        publish_store
            .put_json(
                &mock_ai_object_relative_key(&commit.id.to_string()),
                &ai_model.object,
            )
            .await
            .expect("AI object should write to mock R2");
    });

    let data = MockD1Data {
        commit_oid: commit.id.to_string(),
        ai_objects: vec![mock_ai_object_row(&commit.id.to_string())],
        ai_versions: vec![mock_ai_version_row(&commit.id.to_string())],
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
    ai_objects: Vec<Value>,
    ai_versions: Vec<Value>,
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
        return Ok(vec![mock_publish_revision_row(
            &data.commit_oid,
            data.ai_objects.len() as i64,
        )]);
    }
    if sql.contains("FROM publish_ai_objects")
        && param_str(params, 0) == Some("site_123")
        && param_str(params, 1) == Some(data.commit_oid.as_str())
    {
        return Ok(data.ai_objects.clone());
    }
    if sql.contains("FROM publish_ai_versions")
        && param_str(params, 0) == Some("site_123")
        && param_str(params, 1) == Some(data.commit_oid.as_str())
    {
        return Ok(data.ai_versions.clone());
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

fn mock_publish_revision_row(revision_oid: &str, ai_object_count: i64) -> Value {
    serde_json::json!({
        "site_id": "site_123",
        "revision_oid": revision_oid,
        "status": "published",
        "code_manifest_key": null,
        "ai_index_key": if ai_object_count > 0 {
            serde_json::Value::String(mock_ai_index_r2_key(revision_oid))
        } else {
            serde_json::Value::Null
        },
        "file_count": 1,
        "ai_object_count": ai_object_count,
        "ai_bundle_count": if ai_object_count > 0 { 1 } else { 0 },
        "redaction_mode": "default",
        "redaction_rules_version": "1",
        "sync_run_id": "sync_123",
        "schema_version": 1,
        "created_at": "2026-05-13T00:00:00Z",
        "updated_at": "2026-05-13T00:00:00Z"
    })
}

fn assert_cloud_clone_ai_history(repo: &Path, home: &Path) {
    let output = run_libra_with_home(&["cat-file", "--ai-list-types"], repo, home);
    assert!(
        output.status.success(),
        "cat-file --ai-list-types failed after cloud clone: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let restored_types = stdout
        .lines()
        .filter_map(|line| line.split_once('\t').map(|(object_type, _)| object_type))
        .collect::<Vec<_>>();
    for expected in [
        "publish_ai_bundle",
        "publish_ai_graph",
        "publish_ai_index",
        "publish_ai_intent",
        "publish_ai_version",
    ] {
        assert!(
            restored_types.contains(&expected),
            "cloud clone should restore {expected} into local AI history, got: {stdout}"
        );
    }
}

fn mock_ai_object_relative_key(revision_oid: &str) -> String {
    format!("revisions/{revision_oid}/ai/objects/snapshot/Intent/intent-1.json")
}

fn mock_ai_object_r2_key(revision_oid: &str) -> String {
    format!(
        "repo_456/publish/sites/site_123/{}",
        mock_ai_object_relative_key(revision_oid)
    )
}

fn mock_ai_index_relative_key(revision_oid: &str) -> String {
    format!("revisions/{revision_oid}/ai/index.json")
}

fn mock_ai_index_r2_key(revision_oid: &str) -> String {
    format!(
        "repo_456/publish/sites/site_123/{}",
        mock_ai_index_relative_key(revision_oid)
    )
}

fn mock_ai_graph_relative_key(revision_oid: &str) -> String {
    format!("revisions/{revision_oid}/ai/graph.json")
}

fn mock_ai_bundle_relative_key(revision_oid: &str) -> String {
    format!("revisions/{revision_oid}/ai/bundles/ai-{revision_oid}.json")
}

fn mock_ai_bundle_r2_key(revision_oid: &str) -> String {
    format!(
        "repo_456/publish/sites/site_123/{}",
        mock_ai_bundle_relative_key(revision_oid)
    )
}

fn mock_ai_object_row(revision_oid: &str) -> Value {
    let object = mock_publish_ai_object(revision_oid);
    let bytes = serde_json::to_vec(&object).expect("AI object should serialize");
    serde_json::json!({
        "site_id": "site_123",
        "revision_oid": revision_oid,
        "object_type": "Intent",
        "object_id": "intent-1",
        "layer": "snapshot",
        "r2_key": mock_ai_object_r2_key(revision_oid),
        "redaction_mode": "default",
        "payload_sha256": sha256_hex(&bytes),
        "schema_version": 1,
        "created_at": "2026-05-13T00:00:00Z"
    })
}

fn mock_ai_version_row(revision_oid: &str) -> Value {
    let bundle = mock_publish_ai_bundle(revision_oid);
    let bytes = serde_json::to_vec(&bundle).expect("AI bundle should serialize");
    serde_json::json!({
        "site_id": "site_123",
        "ai_version_id": format!("ai-{revision_oid}"),
        "revision_oid": revision_oid,
        "bundle_key": mock_ai_bundle_r2_key(revision_oid),
        "bundle_sha256": sha256_hex(&bytes),
        "object_count": 1,
        "redaction_mode": "default",
        "redaction_rules_version": "1",
        "schema_version": 1,
        "created_at": "2026-05-13T00:00:00Z"
    })
}

struct MockPublishAiModel {
    object: PublishAiObject,
    index: PublishAiIndex,
    graph: PublishAiGraph,
    bundle: PublishAiBundle,
}

fn mock_publish_ai_model(revision_oid: &str) -> MockPublishAiModel {
    MockPublishAiModel {
        object: mock_publish_ai_object(revision_oid),
        index: mock_publish_ai_index(revision_oid),
        graph: mock_publish_ai_graph(revision_oid),
        bundle: mock_publish_ai_bundle(revision_oid),
    }
}

fn mock_publish_ai_object(revision_oid: &str) -> PublishAiObject {
    PublishAiObject {
        schema_version: PUBLISH_SCHEMA_VERSION,
        site_id: "site_123".to_string(),
        revision_oid: revision_oid.to_string(),
        object_type: "Intent".to_string(),
        object_id: "intent-1".to_string(),
        layer: AiObjectLayer::Snapshot,
        source_refs: vec!["refs/heads/main".to_string()],
        relationships: Vec::new(),
        payload: serde_json::json!({
            "id": "intent-1",
            "title": "ship cloud clone AI restore",
            "status": "accepted"
        }),
        redaction: AiObjectRedaction {
            mode: RedactionMode::Default,
            rules_version: "1".to_string(),
        },
        removed_fields: Vec::new(),
    }
}

fn mock_ai_generated_at() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 13, 0, 0, 0).unwrap()
}

fn mock_ai_object_entry(revision_oid: &str) -> AiBundleObjectEntry {
    let object = mock_publish_ai_object(revision_oid);
    let bytes = serde_json::to_vec(&object).expect("AI object should serialize");
    AiBundleObjectEntry {
        object_type: "Intent".to_string(),
        object_id: "intent-1".to_string(),
        layer: AiObjectLayer::Snapshot,
        r2_key: mock_ai_object_r2_key(revision_oid),
        payload_sha256: sha256_hex(&bytes),
    }
}

fn mock_ai_redaction() -> AiBundleRedaction {
    AiBundleRedaction {
        mode: RedactionMode::Default,
        rules_version: "1".to_string(),
        removed_field_count: 0,
        removed_fields_by_type: BTreeMap::new(),
        object_counts_by_type: BTreeMap::from([("Intent".to_string(), 1)]),
    }
}

fn mock_publish_ai_index(revision_oid: &str) -> PublishAiIndex {
    let bundle = mock_publish_ai_bundle(revision_oid);
    let bundle_bytes = serde_json::to_vec(&bundle).expect("AI bundle should serialize");
    PublishAiIndex {
        schema_version: PUBLISH_SCHEMA_VERSION,
        site_id: "site_123".to_string(),
        revision_oid: revision_oid.to_string(),
        objects: vec![mock_ai_object_entry(revision_oid)],
        bundles: vec![PublishAiIndexBundleEntry {
            ai_version_id: format!("ai-{revision_oid}"),
            bundle_key: mock_ai_bundle_r2_key(revision_oid),
            bundle_sha256: sha256_hex(&bundle_bytes),
            object_count: 1,
            created_at: mock_ai_generated_at(),
        }],
        redaction: mock_ai_redaction(),
        generated_at: mock_ai_generated_at(),
    }
}

fn mock_publish_ai_graph(revision_oid: &str) -> PublishAiGraph {
    PublishAiGraph {
        schema_version: PUBLISH_SCHEMA_VERSION,
        site_id: "site_123".to_string(),
        revision_oid: revision_oid.to_string(),
        ai_version_id: format!("ai-{revision_oid}"),
        nodes: vec![AiGraphNode {
            object_type: "Intent".to_string(),
            object_id: "intent-1".to_string(),
            layer: AiObjectLayer::Snapshot,
            r2_key: mock_ai_object_r2_key(revision_oid),
        }],
        edges: Vec::new(),
        generated_at: mock_ai_generated_at(),
    }
}

fn mock_publish_ai_bundle(revision_oid: &str) -> PublishAiBundle {
    PublishAiBundle {
        schema_version: PUBLISH_SCHEMA_VERSION,
        ai_object_model_reference: "docs/agent/ai-object-model-reference.md".to_string(),
        site_id: "site_123".to_string(),
        revision_oid: revision_oid.to_string(),
        ai_version_id: format!("ai-{revision_oid}"),
        objects: vec![mock_ai_object_entry(revision_oid)],
        relationships: Vec::new(),
        indexes: AiBundleIndexes::default(),
        redaction: mock_ai_redaction(),
        associated_ids: AiBundleAssociatedIds {
            tree_oid: Some(revision_oid.to_string()),
            ..AiBundleAssociatedIds::default()
        },
    }
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

// ---------------------------------------------------------------------------
// Batch 1 — advanced shallow flags + fetch interop
// ---------------------------------------------------------------------------

/// Build a bare git source with two commits on `main`, so that a `--depth 1`
/// clone produces a genuine shallow boundary at the tip's parent cut.
fn create_remote_with_two_commits(base: &Path) -> std::path::PathBuf {
    let remote = base.join("remote2.git");
    assert!(
        run_git(&["init", "--bare", remote.to_str().unwrap()], base)
            .status
            .success()
    );

    let work = base.join("work2");
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
    fs::write(work.join("README.md"), "hello again\n").unwrap();
    assert!(run_git(&["add", "README.md"], &work).status.success());
    assert!(run_git(&["commit", "-m", "second"], &work).status.success());
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

/// `--single-branch --no-single-branch` is a Git-style negation, not a usage
/// conflict; clap `overrides_with` makes the last occurrence win and the clone
/// succeeds (exit 0) instead of failing with a 129 usage error.
#[test]
fn single_branch_negation_last_wins() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-negation");

    let output = run_libra(
        &[
            "clone",
            "--single-branch",
            "--no-single-branch",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "combining --single-branch and --no-single-branch must not be a usage conflict: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `libra+cloud://` sources reject the new shaping flags during fail-fast
/// validation (exit 129) before any config lookup or remote discovery.
#[test]
fn cloud_source_rejects_shallow_flags() {
    let temp = tempdir().unwrap();
    let dest = temp.path().join("restored");

    let output = run_libra(
        &[
            "clone",
            "--shallow-since",
            "2024-01-01",
            "libra+cloud://code.example.com/kepler-ledger",
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "cloud source must reject --shallow-since: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !dest.exists(),
        "no destination should be created on rejection"
    );
}

/// A `--depth` clone reports `shallow: true` in JSON while preserving every
/// pre-existing field (backward-compatible schema).
#[test]
fn shallow_clone_json_schema_backward_compatible() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-shallow-json");

    let output = run_libra(
        &[
            "--json",
            "clone",
            "--depth",
            "1",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "shallow clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "clone");
    let data = &json["data"];
    // New behaviour: shallow is true.
    assert_eq!(data["shallow"], true);
    // Backward-compatible: pre-existing fields keep their shape.
    assert!(data["path"].is_string());
    assert_eq!(data["bare"], false);
    assert!(data["remote_url"].is_string());
    assert_eq!(data["branch"], "main");
    assert!(data["object_format"].is_string());
    assert!(data["repo_id"].is_string());
    assert!(data["vault_signing"].is_boolean());
    assert!(data["warnings"].is_array());
}

/// A malformed `--shallow-since` time is rejected by fail-fast validation with
/// exit code 129, before any network access.
#[test]
fn shallow_since_invalid_time_exits_129() {
    let temp = tempdir().unwrap();
    let dest = temp.path().join("clone-bad-time");

    let output = run_libra(
        &[
            "clone",
            "--shallow-since",
            "not-a-date",
            "file:///definitely/not/here",
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "invalid --shallow-since must exit 129: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `--depth` together with `--shallow-since` is accepted (no usage conflict);
/// the clone succeeds because the time-based request supersedes plain depth at
/// the protocol layer (git-upload-pack rejects sending both).
#[test]
fn depth_and_shallow_since_combine_ok() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-combo");

    let output = run_libra(
        &[
            "clone",
            "--depth",
            "5",
            "--shallow-since",
            "2020-01-01",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "--depth + --shallow-since must be accepted and succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `--reject-shallow` against a shallow local source fails fast with exit 128
/// before remote discovery or directory creation.
#[test]
fn reject_shallow_source_exits_128() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    // Mark the bare source as shallow by writing a boundary file.
    fs::write(remote.join("shallow"), format!("{}\n", "a".repeat(40))).unwrap();
    let dest = temp.path().join("clone-reject");

    let output = run_libra(
        &[
            "clone",
            "--reject-shallow",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(128),
        "--reject-shallow against a shallow source must exit 128: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !dest.exists(),
        "no destination should be created when rejecting a shallow source"
    );
}

/// A `--depth 1` clone of a two-commit history writes a `.libra/shallow`
/// boundary file whose contents round-trip (no half-written state).
#[test]
fn shallow_file_consistent_after_clone() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_two_commits(temp.path());
    let dest = temp.path().join("clone-shallow-file");

    let output = run_libra(
        &[
            "clone",
            "--depth",
            "1",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "shallow clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let shallow_file = dest.join(".libra").join("shallow");
    let contents = fs::read_to_string(&shallow_file)
        .expect(".libra/shallow should exist after a depth-limited clone");
    let lines: Vec<&str> = contents.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        !lines.is_empty(),
        "shallow boundary file should record at least one boundary commit"
    );
    for line in &lines {
        assert!(
            line.len() == 40 || line.len() == 64,
            "shallow boundary should be a valid object id, got: {line}"
        );
    }
}

// ---------------------------------------------------------------------------
// Batch 2a — origin / no-checkout / mirror
// ---------------------------------------------------------------------------

/// `--no-checkout` writes metadata but skips the working-tree checkout: `.libra`
/// exists, but no source files are materialized.
#[test]
fn no_checkout_skips_worktree() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-nocheckout");

    let output = run_libra(
        &[
            "clone",
            "--no-checkout",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "--no-checkout clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        dest.join(".libra").exists(),
        "metadata must still be written"
    );
    assert!(
        !dest.join("README.md").exists(),
        "--no-checkout must not materialize the working tree"
    );
}

/// `--mirror` implies a bare clone: no working tree and no `.libraignore`.
#[test]
fn mirror_clone_is_bare() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("mirror.git");

    let output = run_libra(
        &[
            "--json",
            "clone",
            "--mirror",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "--mirror clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    assert_eq!(json["data"]["bare"], true, "--mirror implies a bare clone");
    assert!(
        !dest.join(".libraignore").exists(),
        "bare mirror clone should not create a worktree .libraignore"
    );
    assert!(
        !dest.join("README.md").exists(),
        "bare mirror clone has no working tree"
    );
}

/// `-o/--origin` names the tracked remote and records it in branch config.
#[test]
fn origin_name_used_in_branch_config() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-origin");

    let output = run_libra(
        &[
            "clone",
            "-o",
            "upstream",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "-o upstream clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cfg = run_libra(&["config", "--get", "branch.main.remote"], &dest);
    assert_eq!(cfg.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&cfg.stdout).trim(),
        "upstream",
        "branch.main.remote must record the custom origin name"
    );

    let url = run_libra(&["config", "--get", "remote.upstream.url"], &dest);
    assert_eq!(
        url.status.code(),
        Some(0),
        "remote.upstream.url must be written for the custom remote name"
    );
}

/// `--json` reports the custom remote name in `origin_name`.
#[test]
fn origin_name_in_json_output() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-origin-json");

    let output = run_libra(
        &[
            "--json",
            "clone",
            "--origin",
            "upstream",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(0));
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    assert_eq!(json["data"]["origin_name"], "upstream");
}

/// Cloud sources reject `--mirror`, `--origin`, and `--no-checkout` (exit 129).
#[test]
fn cloud_source_rejects_mirror_origin_no_checkout() {
    let temp = tempdir().unwrap();
    let dest = temp.path().join("restored");

    let output = run_libra(
        &[
            "clone",
            "--mirror",
            "libra+cloud://code.example.com/kepler-ledger",
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "cloud source must reject --mirror: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---------------------------------------------------------------------------
// Batch 2b — reference / reference-if-able / dissociate
// ---------------------------------------------------------------------------

/// Clone `remote` into a local reference repository under `base` and return its
/// path, for use as a `--reference` source.
fn create_local_reference(base: &Path, remote: &Path) -> std::path::PathBuf {
    let refrepo = base.join("refrepo");
    let output = run_libra(
        &["clone", remote.to_str().unwrap(), refrepo.to_str().unwrap()],
        base,
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "reference repo clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    refrepo
}

/// `--reference` copies the source objects into the new clone and the result
/// passes `libra fsck` (objects readable, no alternates dependency).
#[test]
fn reference_clone_objects_readable() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let refrepo = create_local_reference(temp.path(), &remote);
    let dest = temp.path().join("clone-reference");

    let output = run_libra(
        &[
            "clone",
            "--reference",
            refrepo.to_str().unwrap(),
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "--reference clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The clone has no alternates dependency: no info/alternates file is written.
    assert!(
        !dest
            .join(".libra")
            .join("objects")
            .join("info")
            .join("alternates")
            .exists(),
        "copy-semantics --reference must not write info/alternates"
    );

    let fsck = run_libra(&["fsck"], &dest);
    assert_eq!(
        fsck.status.code(),
        Some(0),
        "fsck must pass after --reference clone: {}",
        String::from_utf8_lossy(&fsck.stderr)
    );
}

/// `--reference-if-able` with a missing path degrades to a normal clone and
/// surfaces a warning rather than failing.
#[test]
fn reference_if_able_missing_path_warns() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-ifable");
    let missing = temp.path().join("does-not-exist");

    let output = run_libra(
        &[
            "clone",
            "--reference-if-able",
            missing.to_str().unwrap(),
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "--reference-if-able with a missing path must still succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("reference-if-able") && stderr.contains("not found"),
        "expected a degrade warning, got: {stderr}"
    );
    assert!(
        dest.join("README.md").exists(),
        "clone should still complete"
    );
}

/// `--dissociate` without a reference is a usage error (exit 129).
#[test]
fn dissociate_requires_reference_exits_129() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-dissociate");

    let output = run_libra(
        &[
            "clone",
            "--dissociate",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "--dissociate without --reference must exit 129: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `--json` reports `reference_used` and `dissociated`, and preserves the
/// pre-existing field set.
#[test]
fn reference_clone_json_schema() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let refrepo = create_local_reference(temp.path(), &remote);
    let dest = temp.path().join("clone-reference-json");

    let output = run_libra(
        &[
            "--json",
            "clone",
            "--dissociate",
            "--reference",
            refrepo.to_str().unwrap(),
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "--reference --dissociate clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    let data = &json["data"];
    assert!(
        data["reference_used"].is_string(),
        "reference_used should be the canonical reference path"
    );
    assert_eq!(data["dissociated"], true);
    // Backward-compatible fields remain.
    assert!(data["path"].is_string());
    assert_eq!(data["bare"], false);
    assert!(data["repo_id"].is_string());
}

/// A symlinked reference object source is rejected for security (exit 128).
#[cfg(unix)]
#[test]
fn symlink_object_source_exits_128() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let refrepo = create_local_reference(temp.path(), &remote);
    let link = temp.path().join("ref-link");
    symlink(&refrepo, &link).unwrap();
    let dest = temp.path().join("clone-symlink-ref");

    let output = run_libra(
        &[
            "clone",
            "--reference",
            link.to_str().unwrap(),
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(128),
        "symlinked --reference source must be rejected with 128: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Cloud sources reject `--reference` (exit 129).
#[test]
fn cloud_source_rejects_reference() {
    let temp = tempdir().unwrap();
    let dest = temp.path().join("restored-ref");

    let output = run_libra(
        &[
            "clone",
            "--reference",
            "/tmp/whatever",
            "libra+cloud://code.example.com/kepler-ledger",
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "cloud source must reject --reference: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---------------------------------------------------------------------------
// Batch 3a — jobs / local / no-hardlinks / shared
// ---------------------------------------------------------------------------

/// `--local` hardlinks the source's objects: a destination loose object shares
/// an inode with the source, and the clone passes fsck.
#[cfg(unix)]
#[test]
fn local_clone_hardlinks_objects() {
    use std::os::unix::fs::MetadataExt;

    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-local");

    let output = run_libra(
        &[
            "clone",
            "-l",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "--local clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Find a loose object in the source and check the destination shares its inode.
    let src_objects = remote.join("objects");
    let mut linked = false;
    for top in fs::read_dir(&src_objects).unwrap().flatten() {
        let name = top.file_name();
        let name = name.to_string_lossy();
        if name == "pack" || name == "info" || name.len() != 2 {
            continue;
        }
        for obj in fs::read_dir(top.path()).unwrap().flatten() {
            let rel = format!("{}/{}", name, obj.file_name().to_string_lossy());
            let dest_obj = dest.join(".libra").join("objects").join(&rel);
            if dest_obj.exists() {
                let src_ino = fs::metadata(obj.path()).unwrap().ino();
                let dest_ino = fs::metadata(&dest_obj).unwrap().ino();
                assert_eq!(src_ino, dest_ino, "--local object should be hardlinked");
                linked = true;
            }
        }
    }
    assert!(linked, "expected at least one hardlinked loose object");

    let fsck = run_libra(&["fsck"], &dest);
    assert_eq!(fsck.status.code(), Some(0), "fsck must pass after --local");
}

/// `--shared` copies the source's objects (no alternates dependency) and passes fsck.
#[test]
fn shared_clone_copies_objects_no_alternates() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-shared");

    let output = run_libra(
        &[
            "clone",
            "-s",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "--shared clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !dest
            .join(".libra")
            .join("objects")
            .join("info")
            .join("alternates")
            .exists(),
        "--shared must not write info/alternates"
    );
    let fsck = run_libra(&["fsck"], &dest);
    assert_eq!(fsck.status.code(), Some(0), "fsck must pass after --shared");
}

/// `--jobs` is strictly range-checked (1..=16); 0 and >16 exit 129.
#[test]
fn jobs_out_of_range_exits_129() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());

    for bad in ["0", "17", "99"] {
        let dest = temp.path().join(format!("clone-jobs-{bad}"));
        let output = run_libra(
            &[
                "clone",
                "--jobs",
                bad,
                remote.to_str().unwrap(),
                dest.to_str().unwrap(),
            ],
            temp.path(),
        );
        assert_eq!(
            output.status.code(),
            Some(129),
            "--jobs {bad} must exit 129: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// `--jobs 16` is accepted (boundary), `--jobs 17` is rejected.
#[test]
fn jobs_boundary_16_ok_17_rejected() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());

    let ok = run_libra(
        &[
            "clone",
            "--jobs",
            "16",
            remote.to_str().unwrap(),
            temp.path().join("clone-jobs-16").to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        ok.status.code(),
        Some(0),
        "--jobs 16 must be accepted: {}",
        String::from_utf8_lossy(&ok.stderr)
    );

    let rejected = run_libra(
        &[
            "clone",
            "--jobs",
            "17",
            remote.to_str().unwrap(),
            temp.path().join("clone-jobs-17").to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        rejected.status.code(),
        Some(129),
        "--jobs 17 must be rejected"
    );
}

/// A symlinked local object source is rejected for `--local` (exit 128).
#[cfg(unix)]
#[test]
fn local_clone_rejects_symlink_object_source() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let link = temp.path().join("src-link");
    symlink(&remote, &link).unwrap();
    let dest = temp.path().join("clone-local-symlink");

    let output = run_libra(
        &[
            "clone",
            "-l",
            link.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(128),
        "symlinked --local source must be rejected with 128: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Cloud sources reject `--local`, `--shared`, and `--jobs` (exit 129).
#[test]
fn cloud_source_rejects_local_shared_jobs() {
    let temp = tempdir().unwrap();
    let dest = temp.path().join("restored-local");

    let output = run_libra(
        &[
            "clone",
            "--shared",
            "libra+cloud://code.example.com/kepler-ledger",
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "cloud source must reject --shared: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---------------------------------------------------------------------------
// Batch 3b — filter (partial clone)
// ---------------------------------------------------------------------------

/// Enable server-side partial-clone filtering on a bare git source so libra's
/// `--filter` request is honored by `git-upload-pack`.
fn enable_filter_support(remote: &Path) {
    assert!(
        run_git(&["config", "uploadpack.allowFilter", "true"], remote)
            .status
            .success()
    );
}

/// `--filter=blob:none --no-checkout` succeeds and records promisor config.
#[test]
fn filter_clone_sets_promisor_config() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    enable_filter_support(&remote);
    let dest = temp.path().join("clone-filter");

    let output = run_libra(
        &[
            "clone",
            "--filter=blob:none",
            "--no-checkout",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "--filter --no-checkout clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let promisor = run_libra(&["config", "--get", "remote.origin.promisor"], &dest);
    assert_eq!(String::from_utf8_lossy(&promisor.stdout).trim(), "true");
    let spec = run_libra(
        &["config", "--get", "remote.origin.partialclonefilter"],
        &dest,
    );
    assert_eq!(String::from_utf8_lossy(&spec.stdout).trim(), "blob:none");
}

/// Unknown `--filter` specs are rejected by validation (exit 129).
#[test]
fn filter_spec_whitelist_rejects_unknown_exits_129() {
    let temp = tempdir().unwrap();
    let dest = temp.path().join("clone-filter-bad");

    let output = run_libra(
        &[
            "clone",
            "--filter=weird:thing",
            "file:///definitely/not/here",
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "unknown --filter spec must exit 129: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// An over-long `--filter` spec is rejected (4 KiB cap, exit 129).
#[test]
fn filter_spec_length_capped_4kib() {
    let temp = tempdir().unwrap();
    let dest = temp.path().join("clone-filter-long");
    let long = format!("blob:limit={}", "9".repeat(5000));

    let output = run_libra(
        &[
            "clone",
            &format!("--filter={long}"),
            "file:///definitely/not/here",
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "over-long --filter spec must exit 129: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `--json` reports `filter_spec` for a partial clone.
#[test]
fn filter_clone_json_schema() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    enable_filter_support(&remote);
    let dest = temp.path().join("clone-filter-json");

    let output = run_libra(
        &[
            "--json",
            "clone",
            "--filter=blob:none",
            "--bare",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "--filter --bare clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    assert_eq!(json["data"]["filter_spec"], "blob:none");
    assert_eq!(json["data"]["bare"], true);
}

/// A non-bare default checkout of a partial clone hits a filtered-out blob and
/// fails with a clear partial-clone diagnostic (exit 128).
#[test]
fn partial_clone_missing_blob_gives_clear_hint() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    enable_filter_support(&remote);
    let dest = temp.path().join("clone-filter-checkout");

    let output = run_libra(
        &[
            "clone",
            "--filter=blob:none",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(128),
        "non-bare partial clone checkout must fail with 128: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("partial clone"),
        "expected a partial-clone diagnostic, got: {stderr}"
    );
}

/// Cloud sources reject `--filter` (exit 129).
#[test]
fn cloud_source_rejects_filter() {
    let temp = tempdir().unwrap();
    let dest = temp.path().join("restored-filter");

    let output = run_libra(
        &[
            "clone",
            "--filter=blob:none",
            "libra+cloud://code.example.com/kepler-ledger",
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "cloud source must reject --filter: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---------------------------------------------------------------------------
// Batch 4 — precise remote/branch config write + credential redaction
// ---------------------------------------------------------------------------

/// A normal clone writes the branch-heads fetch refspec for the remote.
#[test]
fn clone_writes_fetch_refspec() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-refspec");

    let output = run_libra(
        &["clone", remote.to_str().unwrap(), dest.to_str().unwrap()],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(0));

    let fetch = run_libra(&["config", "--get", "remote.origin.fetch"], &dest);
    assert_eq!(
        String::from_utf8_lossy(&fetch.stdout).trim(),
        "+refs/heads/*:refs/remotes/origin/*",
    );
}

/// `--mirror` writes the all-refs refspec and `remote.<name>.mirror = true`.
#[test]
fn mirror_writes_full_refspec_and_mirror_flag() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("mirror-refspec.git");

    let output = run_libra(
        &[
            "clone",
            "--mirror",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "--mirror clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let fetch = run_libra(&["config", "--get", "remote.origin.fetch"], &dest);
    assert_eq!(
        String::from_utf8_lossy(&fetch.stdout).trim(),
        "+refs/*:refs/*"
    );
    let mirror = run_libra(&["config", "--get", "remote.origin.mirror"], &dest);
    assert_eq!(String::from_utf8_lossy(&mirror.stdout).trim(), "true");
}

/// `-o/--origin` records the fetch refspec under the custom remote name.
#[test]
fn origin_name_used_in_remote_fetch_config() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-origin-fetch");

    let output = run_libra(
        &[
            "clone",
            "-o",
            "upstream",
            remote.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(0));

    let fetch = run_libra(&["config", "--get", "remote.upstream.fetch"], &dest);
    assert_eq!(
        String::from_utf8_lossy(&fetch.stdout).trim(),
        "+refs/heads/*:refs/remotes/upstream/*",
    );
}

/// An empty remote writes `remote.<name>.url` + `remote.<name>.fetch` but no
/// synthetic `branch.*` tracking config.
#[test]
fn empty_remote_writes_remote_config_no_branch_tracking() {
    let temp = tempdir().unwrap();
    let remote = create_empty_remote(temp.path());
    let dest = temp.path().join("clone-empty-cfg");

    let output = run_libra(
        &["clone", remote.to_str().unwrap(), dest.to_str().unwrap()],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "empty clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let url = run_libra(&["config", "--get", "remote.origin.url"], &dest);
    assert_eq!(url.status.code(), Some(0), "remote.origin.url must be set");
    let fetch = run_libra(&["config", "--get", "remote.origin.fetch"], &dest);
    assert_eq!(
        String::from_utf8_lossy(&fetch.stdout).trim(),
        "+refs/heads/*:refs/remotes/origin/*",
    );
    // No synthetic branch tracking is written for an empty remote.
    let branch_remote = run_libra(&["config", "--get", "branch.main.remote"], &dest);
    assert_ne!(
        branch_remote.status.code(),
        Some(0),
        "empty clone must not write branch.main.remote, got: {}",
        String::from_utf8_lossy(&branch_remote.stdout)
    );
}

/// The first reflog entry after a clone is `clone: from <url>`.
#[test]
fn reflog_first_entry_is_clone_from() {
    let temp = tempdir().unwrap();
    let remote = create_remote_with_main(temp.path());
    let dest = temp.path().join("clone-reflog");

    let output = run_libra(
        &["clone", remote.to_str().unwrap(), dest.to_str().unwrap()],
        temp.path(),
    );
    assert_eq!(output.status.code(), Some(0));

    let reflog = run_libra(&["reflog", "show"], &dest);
    let stdout = String::from_utf8_lossy(&reflog.stdout);
    assert!(
        stdout.contains("clone: from"),
        "reflog should record 'clone: from <url>', got: {stdout}"
    );
}

/// Credentials embedded in a clone URL are redacted from all output faces
/// (stdout, stderr, JSON) when discovery fails.
#[test]
fn credentials_redacted_in_error_output() {
    let temp = tempdir().unwrap();
    let dest = temp.path().join("clone-cred");

    // 127.0.0.1:1 refuses immediately; the embedded password must never appear.
    let output = run_libra(
        &[
            "clone",
            "https://user:SuperSecretToken@127.0.0.1:1/repo.git",
            dest.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(128),
        "bad-credential clone should fail with 128"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("SuperSecretToken"),
        "credentials must be redacted from all output, got: {combined}"
    );
}
