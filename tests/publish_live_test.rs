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
//! sync/clone use. A deployed Worker API smoke can be layered on by setting
//! `LIBRA_PUBLISH_LIVE_WORKER_ORIGIN`, `LIBRA_PUBLISH_LIVE_SLUG`, and
//! `LIBRA_PUBLISH_LIVE_CLONE_DOMAIN`; until those are present, the test reports the
//! missing deploy-smoke inputs and exits after the D1/R2 credential checks.

use std::{
    collections::HashMap,
    fs,
    sync::{Arc, OnceLock},
    time::Duration,
};

use git_internal::internal::object::{ObjectTrait, blob::Blob};
use libra::utils::{
    d1_client::D1Client,
    storage::{Storage, remote::RemoteStorage},
};
use object_store::aws::AmazonS3Builder;
use serde_json::Value;
use serial_test::serial;
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

const DEPLOY_SMOKE_VARS: &[&str] = &[
    "LIBRA_PUBLISH_LIVE_WORKER_ORIGIN",
    "LIBRA_PUBLISH_LIVE_SLUG",
    "LIBRA_PUBLISH_LIVE_CLONE_DOMAIN",
];

static DOT_ENV_TEST: OnceLock<HashMap<String, String>> = OnceLock::new();

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

fn worker_api_url(path: &[&str]) -> Url {
    let origin = required_env("LIBRA_PUBLISH_LIVE_WORKER_ORIGIN");
    let mut url = Url::parse(origin.trim_end_matches('/')).expect("parse live Worker origin");
    url.path_segments_mut()
        .expect("live Worker origin must be a base URL")
        .extend(path);
    url
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
/// Cloudflare D1 and R2 credentials work with Libra's real clients, then smoke the
/// deployed Worker API if the deploy-smoke origin/slug inputs are present.
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

    let missing_deploy = missing_vars(DEPLOY_SMOKE_VARS);
    if !missing_deploy.is_empty() {
        eprintln!(
            "skipped Worker deploy/API smoke (missing: {}). Full publish live gate still requires all-refs sync, libra+cloud clone restore, and deployed Worker refs/tree/file checks.",
            missing_deploy.join(", ")
        );
        return;
    }

    smoke_deployed_worker_api().await;
}

async fn smoke_deployed_worker_api() {
    let slug = required_env("LIBRA_PUBLISH_LIVE_SLUG");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("build Worker API smoke client");

    let refs_url = worker_api_url(&["api", "sites", &slug, "refs"]);
    let refs_body = get_json(&client, refs_url).await;
    let refs = refs_body
        .pointer("/data/refs")
        .and_then(Value::as_array)
        .expect("Worker refs response should include data.refs");
    assert!(
        !refs.is_empty(),
        "Worker refs API returned no refs; run a full all-refs publish sync before the live gate"
    );

    let tree_url = worker_api_url(&["api", "sites", &slug, "tree"]);
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

    let mut file_url = worker_api_url(&["api", "sites", &slug, "file"]);
    file_url.query_pairs_mut().append_pair("path", &file_path);
    let file_body = get_json(&client, file_url).await;
    assert!(
        file_body.pointer("/data/path").and_then(Value::as_str) == Some(file_path.as_str()),
        "Worker file API returned a mismatched file payload for {file_path}: {file_body}"
    );
}
