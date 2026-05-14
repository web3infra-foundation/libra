//! Live Cloudflare gate for the publish surface.
//!
//! This test is intentionally named with the `publish_live` prefix so the documented
//! command below always runs at least one concrete test instead of compiling the
//! suite with a vacuous `0 tests` result:
//!
//! `LIBRA_ENABLE_TEST_LIVE_CLOUD=1 cargo test --features test-live-cloud publish_live -- --test-threads=1`
//!
//! The gate never starts `libra code` or an MCP server. It first validates that the
//! live D1 API token and R2 S3 credentials can be used by the same clients publish
//! sync/clone use, then performs a real `cloud sync`, all-refs `publish sync`,
//! and `libra+cloud://` clone restore against unique live D1/R2 rows. A deployed
//! Worker API smoke can be layered on by setting `LIBRA_PUBLISH_LIVE_WORKER_ORIGIN`.
//! The smoke reuses the slug created by this live gate and derives the clone
//! domain from the Worker origin host unless `LIBRA_PUBLISH_LIVE_CLONE_DOMAIN`
//! is set. `LIBRA_PUBLISH_LIVE_SLUG` remains available when a pre-existing
//! deployed site must be probed instead.

use std::{
    collections::HashMap,
    fs,
    path::Path,
    process::{Command, Output},
    sync::{Arc, OnceLock},
    time::Duration,
};

use git_internal::internal::object::{ObjectTrait, blob::Blob};
use libra::utils::{
    d1_client::{D1Client, PublishSiteRow},
    storage::{Storage, remote::RemoteStorage},
};
use object_store::aws::AmazonS3Builder;
use serde_json::Value;
use serial_test::serial;
use tempfile::tempdir;
use url::Url;
use uuid::Uuid;

const REQUIRED_CLOUD_VARS: &[&str] = &[
    "LIBRA_D1_ACCOUNT_ID",
    "LIBRA_D1_API_TOKEN",
    "LIBRA_D1_DATABASE_ID",
    "LIBRA_STORAGE_ENDPOINT",
    "LIBRA_STORAGE_BUCKET",
    "LIBRA_STORAGE_ACCESS_KEY",
    "LIBRA_STORAGE_SECRET_KEY",
];

const DEPLOY_SMOKE_REQUIRED_VARS: &[&str] = &["LIBRA_PUBLISH_LIVE_WORKER_ORIGIN"];

static DOT_ENV_TEST: OnceLock<HashMap<String, String>> = OnceLock::new();

#[derive(Clone, Debug)]
struct LivePublishSite {
    slug: String,
    clone_domain: String,
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn config_value(name: &str) -> Option<String> {
    env_value(name).or_else(|| dot_env_test_values().get(name).cloned())
}

fn dot_env_test_values() -> &'static HashMap<String, String> {
    DOT_ENV_TEST.get_or_init(load_dot_env_test)
}

fn load_dot_env_test() -> HashMap<String, String> {
    let Ok(contents) = fs::read_to_string(".env.test") else {
        return HashMap::new();
    };

    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (key, value) = line.split_once('=')?;
            let value = value.trim().trim_matches(['"', '\'']).to_string();
            if key.trim().is_empty() || value.is_empty() {
                return None;
            }
            Some((key.trim().to_string(), value))
        })
        .collect()
}

fn required_env(name: &str) -> String {
    config_value(name).unwrap_or_else(|| panic!("missing required publish live env var: {name}"))
}

fn missing_vars(names: &'static [&'static str]) -> Vec<&'static str> {
    names
        .iter()
        .copied()
        .filter(|name| config_value(name).is_none())
        .collect()
}

fn publish_live_enabled() -> bool {
    cfg!(feature = "test-live-cloud")
        && config_value("LIBRA_ENABLE_TEST_LIVE_CLOUD").as_deref() == Some("1")
}

fn d1_client_from_env() -> D1Client {
    D1Client::new(
        required_env("LIBRA_D1_ACCOUNT_ID"),
        required_env("LIBRA_D1_API_TOKEN"),
        required_env("LIBRA_D1_DATABASE_ID"),
    )
}

fn live_env_pairs() -> Vec<(&'static str, String)> {
    let mut pairs: Vec<_> = REQUIRED_CLOUD_VARS
        .iter()
        .map(|name| (*name, required_env(name)))
        .collect();
    if let Some(region) = config_value("LIBRA_STORAGE_REGION") {
        pairs.push(("LIBRA_STORAGE_REGION", region));
    }
    pairs
}

fn r2_storage_from_env(repo_id: &str) -> RemoteStorage {
    let region = config_value("LIBRA_STORAGE_REGION").unwrap_or_else(|| "auto".to_string());
    let s3 = AmazonS3Builder::new()
        .with_bucket_name(required_env("LIBRA_STORAGE_BUCKET"))
        .with_region(region)
        .with_endpoint(required_env("LIBRA_STORAGE_ENDPOINT"))
        .with_access_key_id(required_env("LIBRA_STORAGE_ACCESS_KEY"))
        .with_secret_access_key(required_env("LIBRA_STORAGE_SECRET_KEY"))
        .with_virtual_hosted_style_request(false)
        .build()
        .expect("build R2 S3 client for publish live gate");

    RemoteStorage::new_with_prefix(Arc::new(s3), repo_id.to_string())
}

fn worker_origin_url() -> Option<Url> {
    config_value("LIBRA_PUBLISH_LIVE_WORKER_ORIGIN")
        .map(|origin| Url::parse(origin.trim_end_matches('/')).expect("parse live Worker origin"))
}

fn live_gate_clone_domain(worker_origin: Option<&Url>) -> String {
    config_value("LIBRA_PUBLISH_LIVE_CLONE_DOMAIN")
        .or_else(|| worker_origin.and_then(|url| url.host_str().map(str::to_string)))
        .unwrap_or_else(|| "live-gate.example.com".to_string())
}

fn worker_api_url(origin: &Url, path: &[&str]) -> Url {
    let mut url = origin.clone();
    url.path_segments_mut()
        .expect("live Worker origin must be a base URL")
        .extend(path);
    url
}

fn isolated_libra_command(current_dir: &Path, home: &Path) -> Command {
    let config_home = home.join(".config");
    let global_config_db = home.join(".libra-global-config.db");
    fs::create_dir_all(&config_home).expect("create isolated XDG config dir");

    let mut command = Command::new(env!("CARGO_BIN_EXE_libra"));
    command
        .current_dir(current_dir)
        .env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin".to_string()),
        )
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("USERPROFILE", home)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("LIBRA_TEST", "1")
        .env("LIBRA_TEST_ENV", "1")
        .env("LIBRA_CONFIG_GLOBAL_DB", &global_config_db);
    for (name, value) in live_env_pairs() {
        command.env(name, value);
    }
    command
}

fn run_libra(current_dir: &Path, home: &Path, args: &[&str]) -> Output {
    isolated_libra_command(current_dir, home)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run libra {}: {error}", args.join(" ")))
}

fn assert_libra_success(output: &Output, args: &[&str]) {
    assert!(
        output.status.success(),
        "libra {} failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn seed_publish_site(
    d1: &D1Client,
    repo_id: &str,
    site_id: &str,
    clone_domain: &str,
    slug: &str,
) {
    d1.ensure_publish_schema()
        .await
        .expect("ensure publish D1 schema for live gate");
    let now = chrono::Utc::now().to_rfc3339();
    d1.upsert_publish_site(&PublishSiteRow {
        site_id: site_id.to_string(),
        repo_id: repo_id.to_string(),
        clone_domain: clone_domain.to_string(),
        slug: slug.to_string(),
        display_origin: format!("https://{clone_domain}"),
        name: "Libra publish live gate".to_string(),
        visibility: "public".to_string(),
        status: "active".to_string(),
        worker_name: "libra-publish-live-gate".to_string(),
        default_ref: None,
        latest_revision_oid: None,
        refs_generation: 0,
        max_preview_bytes: 65_536,
        schema_version: 1,
        created_at: now.clone(),
        updated_at: now,
    })
    .await
    .expect("seed publish_sites row for live gate");
}

async fn smoke_all_refs_sync_and_cloud_clone(d1: &D1Client) -> LivePublishSite {
    let repo = tempdir().expect("create live publish repo tempdir");
    let home = repo.path().join(".home");
    let repo_id = format!("publish-live-repo-{}", Uuid::new_v4());
    let site_id = Uuid::new_v4().to_string();
    let slug = format!("publish-live-{}", Uuid::new_v4().simple());
    let cloud_name = format!("publish-live-name-{}", Uuid::new_v4());
    let worker_origin = worker_origin_url();
    let clone_domain = live_gate_clone_domain(worker_origin.as_ref());

    seed_publish_site(d1, &repo_id, &site_id, &clone_domain, &slug).await;

    for args in [
        vec!["init"],
        vec!["config", "--local", "user.name", "Libra Test"],
        vec!["config", "--local", "user.email", "libra@example.com"],
        vec!["config", "--local", "vault.signing", "false"],
        vec!["config", "--local", "libra.repoid", &repo_id],
        vec!["config", "--local", "cloud.name", &cloud_name],
        vec!["config", "--local", "publish.site_id", &site_id],
    ] {
        let output = run_libra(repo.path(), &home, &args);
        assert_libra_success(&output, &args);
    }

    fs::create_dir_all(repo.path().join("src")).expect("create src dir");
    fs::write(repo.path().join("README.md"), "# publish live\n").expect("write README");
    fs::write(
        repo.path().join("src/lib.rs"),
        "pub fn live_gate() -> &'static str { \"ok\" }\n",
    )
    .expect("write source file");
    for args in [vec!["add", "."], vec!["commit", "-m", "publish live gate"]] {
        let output = run_libra(repo.path(), &home, &args);
        assert_libra_success(&output, &args);
    }
    for args in [vec!["branch", "release"], vec!["tag", "v1.0.0"]] {
        let output = run_libra(repo.path(), &home, &args);
        assert_libra_success(&output, &args);
    }

    let cloud_sync_args = ["cloud", "sync"];
    let cloud_sync_output = run_libra(repo.path(), &home, &cloud_sync_args);
    assert_libra_success(&cloud_sync_output, &cloud_sync_args);

    let sync_args = ["--json", "publish", "sync"];
    let sync_output = run_libra(repo.path(), &home, &sync_args);
    assert_libra_success(&sync_output, &sync_args);
    let sync_json: Value =
        serde_json::from_slice(&sync_output.stdout).expect("decode publish sync JSON");
    assert_eq!(
        sync_json["ok"], true,
        "publish sync JSON should be ok: {sync_json}"
    );
    assert!(
        sync_json["data"]["refsCount"].as_i64().unwrap_or_default() >= 3,
        "all-refs sync should publish main, release, and v1.0.0 refs: {sync_json}"
    );
    let revision = sync_json["data"]["latestRevisionOid"]
        .as_str()
        .expect("publish sync JSON should include latestRevisionOid")
        .to_string();

    let account_id = required_env("LIBRA_D1_ACCOUNT_ID");
    let d1_database_id = required_env("LIBRA_D1_DATABASE_ID");
    let r2_bucket = required_env("LIBRA_STORAGE_BUCKET");
    let clone_config = [
        (
            format!("cloud.clone_domains.{clone_domain}.account_id"),
            account_id.as_str(),
        ),
        (
            format!("cloud.clone_domains.{clone_domain}.d1_database_id"),
            d1_database_id.as_str(),
        ),
        (
            format!("cloud.clone_domains.{clone_domain}.r2_bucket"),
            r2_bucket.as_str(),
        ),
    ];
    for (key, value) in clone_config {
        let args = ["config", "set", "--global", key.as_str(), value];
        let output = run_libra(repo.path(), &home, &args);
        assert_libra_success(&output, &args);
    }

    let clone_dest = repo.path().join("restored");
    let source = format!("libra+cloud://{clone_domain}/{slug}");
    let clone_dest_str = clone_dest.to_string_lossy().to_string();
    let clone_args = ["--json", "clone", source.as_str(), clone_dest_str.as_str()];
    let clone_output = run_libra(repo.path(), &home, &clone_args);
    assert_libra_success(&clone_output, &clone_args);
    let clone_json: Value =
        serde_json::from_slice(&clone_output.stdout).expect("decode cloud clone JSON");
    assert_eq!(
        clone_json["ok"], true,
        "cloud clone JSON should be ok: {clone_json}"
    );
    assert_eq!(clone_json["data"]["source_kind"], "cloudflare");
    assert_eq!(clone_json["data"]["cloud_site"]["site_id"], site_id);
    assert_eq!(clone_json["data"]["cloud_site"]["slug"], slug);
    assert_eq!(clone_json["data"]["cloud_site"]["revision"], revision);
    assert_eq!(
        fs::read_to_string(clone_dest.join("README.md")).expect("restored README"),
        "# publish live\n"
    );

    let tag_args = ["show-ref", "--tags", "v1.0.0"];
    let tag_output = run_libra(&clone_dest, &home, &tag_args);
    assert_libra_success(&tag_output, &tag_args);

    LivePublishSite { slug, clone_domain }
}

async fn get_json(client: &reqwest::Client, url: Url) -> Value {
    let response = client
        .get(url.clone())
        .send()
        .await
        .unwrap_or_else(|error| panic!("GET {url} failed: {error}"));
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|error| panic!("read {url} response body: {error}"));
    assert!(status.is_success(), "GET {url} returned {status}: {body}");
    serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {url} JSON: {error}: {body}"))
}

/// Scenario: when the explicit live gate is enabled, prove the configured
/// Cloudflare D1 and R2 credentials work with Libra's real clients, round-trip a
/// real cloud object sync plus all-refs publish sync through `libra+cloud://`
/// clone restore, then smoke the deployed Worker API if the deploy-smoke
/// origin/slug inputs are present.
#[tokio::test]
#[serial(cloud_live)]
async fn publish_live_gate_prerequisites_are_explicit() {
    if !publish_live_enabled() {
        eprintln!("skipped (set --features test-live-cloud and LIBRA_ENABLE_TEST_LIVE_CLOUD=1)");
        return;
    }

    let missing = missing_vars(REQUIRED_CLOUD_VARS);
    assert!(
        missing.is_empty(),
        "publish_live requires D1 and R2 credentials; missing: {}",
        missing.join(", ")
    );

    let d1 = d1_client_from_env();
    d1.execute("SELECT 1 AS publish_live_gate", None)
        .await
        .expect("D1 SELECT 1 should succeed for publish live gate");

    let storage = r2_storage_from_env(&format!("publish-live-gate-{}", Uuid::new_v4()));
    let content = format!("publish live gate {}", Uuid::new_v4());
    let blob = Blob::from_content(&content);
    storage
        .put(&blob.id, &blob.data, blob.get_type())
        .await
        .expect("write publish live gate object to R2");
    let (data, object_type) = storage
        .get(&blob.id)
        .await
        .expect("read publish live gate object from R2");
    assert_eq!(data, blob.data);
    assert_eq!(object_type, blob.get_type());

    let live_site = smoke_all_refs_sync_and_cloud_clone(&d1).await;

    let missing_deploy = missing_vars(DEPLOY_SMOKE_REQUIRED_VARS);
    if !missing_deploy.is_empty() {
        eprintln!(
            "skipped Worker deploy/API smoke (missing: {}). Set LIBRA_PUBLISH_LIVE_WORKER_ORIGIN to a deployed Worker bound to the same D1/R2 resources; LIBRA_PUBLISH_LIVE_CLONE_DOMAIN and LIBRA_PUBLISH_LIVE_SLUG are optional overrides. Full publish live gate still requires deployed Worker refs/tree/file checks.",
            missing_deploy.join(", ")
        );
        return;
    }

    smoke_deployed_worker_api(&live_site).await;
}

async fn smoke_deployed_worker_api(live_site: &LivePublishSite) {
    let origin =
        worker_origin_url().expect("LIBRA_PUBLISH_LIVE_WORKER_ORIGIN is required for deploy smoke");
    let slug = config_value("LIBRA_PUBLISH_LIVE_SLUG").unwrap_or_else(|| live_site.slug.clone());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("build Worker API smoke client");

    let refs_url = worker_api_url(&origin, &["api", "sites", &slug, "refs"]);
    let refs_body = get_json(&client, refs_url).await;
    let refs = refs_body
        .pointer("/data/refs")
        .and_then(Value::as_array)
        .expect("Worker refs response should include data.refs");
    assert!(
        !refs.is_empty(),
        "Worker refs API returned no refs for slug {slug} on clone domain {}; run a full all-refs publish sync before the live gate",
        live_site.clone_domain,
    );

    let tree_url = worker_api_url(&origin, &["api", "sites", &slug, "tree"]);
    let tree_body = get_json(&client, tree_url).await;
    let entries = tree_body
        .pointer("/data/entries")
        .and_then(Value::as_array)
        .expect("Worker tree response should include data.entries");
    assert!(
        !entries.is_empty(),
        "Worker tree API returned no root entries; publish_live requires a non-empty synced tree"
    );

    let file_path = config_value("LIBRA_PUBLISH_LIVE_FILE_PATH").or_else(|| {
        entries
            .iter()
            .find(|entry| entry.get("entryKind").and_then(Value::as_str) == Some("file"))
            .and_then(|entry| entry.get("path"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let file_path = file_path.expect(
        "Worker tree root has no file entry; set LIBRA_PUBLISH_LIVE_FILE_PATH to a published file",
    );

    let mut file_url = worker_api_url(&origin, &["api", "sites", &slug, "file"]);
    file_url.query_pairs_mut().append_pair("path", &file_path);
    let file_body = get_json(&client, file_url).await;
    assert!(
        file_body.pointer("/data/path").and_then(Value::as_str) == Some(file_path.as_str()),
        "Worker file API returned a mismatched file payload for {file_path}: {file_body}"
    );
}
