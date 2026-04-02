#![allow(dead_code)]

use super::*;

#[derive(Debug)]
pub(super) struct EmbeddedHelperDir {
    _temp_dir: TempDir,
}

const HELPER_RESUME_ENV: &str = "LIBRA_CLAUDE_HELPER_RESUME";
const HELPER_SESSION_ID_ENV: &str = "LIBRA_CLAUDE_HELPER_SESSION_ID";
const HELPER_RESUME_AT_ENV: &str = "LIBRA_CLAUDE_HELPER_RESUME_SESSION_AT";

pub(super) type BindingObjectSelector<T> = (&'static str, fn(&T) -> &str);

pub(super) trait BindingArtifactSchema {
    const SCHEMA: &'static str;

    fn schema(&self) -> &str;
}

pub(super) trait HelperResponse {
    type Output;

    fn parse_response(stdout: &str, stderr: &str) -> Result<Self::Output>;
}

pub(super) async fn current_head_sha() -> String {
    Head::current_commit()
        .await
        .map(|hash| hash.to_string())
        .unwrap_or_else(|| ZERO_COMMIT_SHA.to_string())
}

pub(super) async fn init_local_mcp_server(storage_dir: &Path) -> Result<Arc<LibraMcpServer>> {
    let objects_dir = storage_dir.join("objects");

    fs::create_dir_all(&objects_dir).await.with_context(|| {
        format!(
            "failed to create local MCP storage directory '{}'",
            objects_dir.display()
        )
    })?;

    let db_path = storage_dir.join("libra.db");
    let db_path_str = db_path
        .to_str()
        .ok_or_else(|| anyhow!("database path '{}' is not valid UTF-8", db_path.display()))?;
    #[cfg(target_os = "windows")]
    let db_path_string = db_path_str.replace("\\", "/");
    #[cfg(target_os = "windows")]
    let db_path_str = db_path_string.as_str();

    let db_conn = Arc::new(
        db::establish_connection(db_path_str)
            .await
            .with_context(|| format!("failed to connect to database '{}'", db_path.display()))?,
    );
    let storage = Arc::new(LocalStorage::new(objects_dir));
    let history_manager = Arc::new(HistoryManager::new(
        storage.clone(),
        storage_dir.to_path_buf(),
        db_conn,
    ));

    Ok(Arc::new(LibraMcpServer::new(
        Some(history_manager),
        Some(storage),
    )))
}

pub(super) async fn write_pretty_json_file<T>(path: &Path, value: &T) -> Result<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await.with_context(|| {
            format!(
                "failed to create directory for resolved artifact '{}'",
                parent.display()
            )
        })?;
    }
    let payload = serde_json::to_vec_pretty(value).context("failed to serialize JSON artifact")?;
    fs::write(path, payload)
        .await
        .with_context(|| format!("failed to write JSON artifact '{}'", path.display()))
}

pub(super) async fn read_json_artifact<T>(path: &Path, label: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    let content = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read {label} '{}'", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {label} '{}'", path.display()))
}

pub(super) async fn read_tracked_object<T>(
    storage_path: &Path,
    object_type: &str,
    object_id: &str,
    label: &str,
) -> Result<T>
where
    T: DeserializeOwned + Send + Sync,
{
    let mcp_server = init_local_mcp_server(storage_path).await?;
    let history = mcp_server
        .intent_history_manager
        .as_ref()
        .ok_or_else(|| anyhow!("local MCP history manager is unavailable"))?;
    let object_hash = history
        .get_object_hash(object_type, object_id)
        .await
        .with_context(|| format!("failed to inspect {object_type} history for '{object_id}'"))?
        .ok_or_else(|| anyhow!("{label} '{}' does not exist in AI history", object_id))?;
    let storage = LocalStorage::new(storage_path.join("objects"));
    storage
        .get_json::<T>(&object_hash)
        .await
        .with_context(|| format!("failed to read {label} '{}'", object_id))
}

pub(super) fn validate_binding_schema<T>(binding: &T, path: &Path, label: &str) -> Result<()>
where
    T: BindingArtifactSchema,
{
    if binding.schema() != T::SCHEMA {
        bail!(
            "unsupported {label} schema '{}' in '{}'",
            binding.schema(),
            path.display()
        );
    }
    Ok(())
}

pub(super) async fn read_typed_json_artifact<T>(path: &Path, label: &str) -> Result<T>
where
    T: DeserializeOwned + BindingArtifactSchema,
{
    let artifact: T = read_json_artifact(path, label).await?;
    validate_binding_schema(&artifact, path, label)?;
    Ok(artifact)
}

pub(super) async fn local_object_exists(
    storage_path: &Path,
    object_type: &str,
    object_id: &str,
) -> Result<bool> {
    let mcp_server = init_local_mcp_server(storage_path).await?;
    let history = mcp_server
        .intent_history_manager
        .as_ref()
        .ok_or_else(|| anyhow!("local MCP history manager is unavailable"))?;
    Ok(history
        .get_object_hash(object_type, object_id)
        .await
        .with_context(|| format!("failed to inspect {object_type} history for '{object_id}'"))?
        .is_some())
}

pub(super) async fn read_existing_binding_if_live<T>(
    storage_path: &Path,
    binding_path: &Path,
    label: &str,
    required_objects: &[BindingObjectSelector<T>],
) -> Result<Option<T>>
where
    T: DeserializeOwned + BindingArtifactSchema,
{
    if !binding_path.exists() {
        return Ok(None);
    }

    let binding: T = read_typed_json_artifact(binding_path, label).await?;
    for (object_type, selector) in required_objects {
        if !local_object_exists(storage_path, object_type, selector(&binding)).await? {
            return Ok(None);
        }
    }

    Ok(Some(binding))
}

pub(super) fn parse_created_id(kind: &str, result: &rmcp::model::CallToolResult) -> Result<String> {
    if result.is_error.unwrap_or(false) {
        bail!("MCP create_{kind} returned an error result");
    }

    for content in &result.content {
        if let Some(text) = content.as_text().map(|value| value.text.as_str())
            && let Some(id) = text.split("ID:").nth(1)
        {
            let id = id.trim();
            if !id.is_empty() {
                return Ok(id.to_string());
            }
        }
    }

    bail!("failed to parse {kind} id from MCP response")
}

pub(super) fn managed_audit_bundle_path(storage_path: &Path, ai_session_id: &str) -> PathBuf {
    storage_path
        .join("audit-bundles")
        .join(format!("{ai_session_id}.json"))
}

pub(super) fn managed_artifact_path(storage_path: &Path, ai_session_id: &str) -> PathBuf {
    storage_path
        .join("managed-artifacts")
        .join(format!("{ai_session_id}.json"))
}

pub(super) fn default_intent_binding_path(storage_path: &Path, ai_session_id: &str) -> PathBuf {
    storage_path
        .join(INTENT_INPUTS_DIR)
        .join(format!("{ai_session_id}.json"))
}

pub(super) fn formal_run_binding_path(storage_path: &Path, ai_session_id: &str) -> PathBuf {
    storage_path
        .join(FORMAL_RUN_BINDINGS_DIR)
        .join(format!("{ai_session_id}.json"))
}

pub(super) fn evidence_binding_path(storage_path: &Path, ai_session_id: &str) -> PathBuf {
    storage_path
        .join(EVIDENCE_BINDINGS_DIR)
        .join(format!("{ai_session_id}.json"))
}

pub(super) fn decision_binding_path(storage_path: &Path, ai_session_id: &str) -> PathBuf {
    storage_path
        .join(DECISION_BINDINGS_DIR)
        .join(format!("{ai_session_id}.json"))
}

pub(super) fn patchset_binding_path(storage_path: &Path, ai_session_id: &str) -> PathBuf {
    storage_path
        .join(PATCHSET_BINDINGS_DIR)
        .join(format!("{ai_session_id}.json"))
}

pub(super) fn build_managed_evidence_input_object_id(ai_session_id: &str) -> Result<String> {
    validate_ai_session_id(ai_session_id)?;
    Ok(format!("claude_managed_evidence_input__{ai_session_id}"))
}

pub(super) fn managed_evidence_input_artifact_path(
    storage_path: &Path,
    object_id: &str,
) -> PathBuf {
    storage_path
        .join(MANAGED_EVIDENCE_INPUTS_DIR)
        .join(format!("{object_id}.json"))
}

pub(super) fn build_decision_input_object_id(ai_session_id: &str) -> Result<String> {
    validate_ai_session_id(ai_session_id)?;
    Ok(format!("claude_decision_input__{ai_session_id}"))
}

pub(super) fn decision_input_artifact_path(storage_path: &Path, object_id: &str) -> PathBuf {
    storage_path
        .join(DECISION_INPUTS_DIR)
        .join(format!("{object_id}.json"))
}

pub(super) fn validate_provider_session_id(provider_session_id: &str) -> Result<()> {
    if provider_session_id.len() > 128 {
        bail!("invalid provider session id: exceeds 128 characters");
    }
    if !provider_session_id
        .chars()
        .all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-'))
    {
        bail!("invalid provider session id: only [A-Za-z0-9._-] is allowed");
    }
    Ok(())
}

pub(super) fn validate_ai_session_id(ai_session_id: &str) -> Result<()> {
    if ai_session_id.len() > 128 {
        bail!("invalid ai session id: exceeds 128 characters");
    }
    if !ai_session_id
        .chars()
        .all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-'))
    {
        bail!("invalid ai session id: only [A-Za-z0-9._-] is allowed");
    }
    Ok(())
}

pub(super) async fn load_managed_audit_bundle_for_ai_session(
    storage_path: &Path,
    ai_session_id: &str,
) -> Result<(PathBuf, ManagedAuditBundle)> {
    let audit_bundle_path = managed_audit_bundle_path(storage_path, ai_session_id);
    let audit_bundle: ManagedAuditBundle =
        read_json_artifact(&audit_bundle_path, "managed audit bundle")
            .await
            .with_context(|| {
                format!(
                    "failed to load managed audit bundle at '{}'; run a managed Claude Code turn or import a managed artifact for ai_session_id '{}' first",
                    audit_bundle_path.display(),
                    ai_session_id
                )
            })?;
    if audit_bundle.schema != "libra.claude_managed_audit_bundle.v1" {
        bail!(
            "unsupported managed audit bundle schema '{}' in '{}'",
            audit_bundle.schema,
            audit_bundle_path.display()
        );
    }
    if audit_bundle.ai_session_id != ai_session_id {
        bail!(
            "managed audit bundle '{}' belongs to ai session '{}', not '{}'",
            audit_bundle_path.display(),
            audit_bundle.ai_session_id,
            ai_session_id
        );
    }
    Ok((audit_bundle_path, audit_bundle))
}

pub(super) async fn materialize_helper(
    helper_path: Option<&Path>,
) -> Result<(Option<EmbeddedHelperDir>, PathBuf)> {
    if let Some(path) = helper_path {
        return Ok((None, path.to_path_buf()));
    }

    let temp_dir = tempfile::Builder::new()
        .prefix("libra-claudecode-helper-")
        .tempdir()
        .context("failed to create temporary helper directory")?;
    let temp_dir_path = temp_dir.path().to_path_buf();
    let helper_path = temp_dir_path.join("libra-claude-managed-helper.py");
    let mut helper_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&helper_path)
        .await
        .with_context(|| format!("failed to create helper '{}'", helper_path.display()))?;
    helper_file
        .write_all(EMBEDDED_PYTHON_HELPER_SOURCE.as_bytes())
        .await
        .with_context(|| format!("failed to write helper '{}'", helper_path.display()))?;
    Ok((
        Some(EmbeddedHelperDir {
            _temp_dir: temp_dir,
        }),
        helper_path,
    ))
}

pub(super) fn resolve_helper_python_binary(cwd: &Path, requested_python_binary: &str) -> String {
    if requested_python_binary.trim() != DEFAULT_PYTHON_BINARY {
        return requested_python_binary.to_string();
    }

    let candidates = [
        cwd.join(".venv").join("bin").join("python"),
        cwd.join(".venv").join("bin").join("python3"),
        cwd.join(".venv").join("Scripts").join("python.exe"),
        cwd.join(".venv").join("Scripts").join("python"),
    ];

    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().to_string())
        .unwrap_or_else(|| requested_python_binary.to_string())
}

pub(super) async fn ensure_helper_python_environment(
    custom_helper: bool,
    python_binary: &str,
    cwd: &Path,
) -> Result<()> {
    if custom_helper {
        return Ok(());
    }

    let output = Command::new(python_binary)
        .arg("-c")
        .arg("import claude_agent_sdk")
        .current_dir(cwd)
        .output()
        .await
        .with_context(|| {
            format!(
                "failed to launch Python helper runtime '{}' from '{}'; \
if this Libra repo uses a uv-managed virtualenv, run 'uv venv' and \
'uv pip install claude-agent-sdk', or pass --python-binary explicitly",
                python_binary,
                cwd.display()
            )
        })?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let detail = if stderr.is_empty() {
        "claude_agent_sdk import failed".to_string()
    } else {
        stderr
    };

    bail!(
        "selected Python runtime '{}' is missing claude_agent_sdk ({detail}); \
if this Libra project uses uv, run 'uv venv' in '{}' and \
'uv pip install claude-agent-sdk', or pass --python-binary explicitly",
        python_binary,
        cwd.display()
    );
}

pub(super) fn build_helper_command(
    custom_helper: bool,
    python_binary: &str,
    helper_path: &Path,
) -> Command {
    let mut command = if custom_helper {
        Command::new(helper_path)
    } else {
        let mut python = Command::new(python_binary);
        python.arg(helper_path);
        python
    };
    command.kill_on_drop(true);
    command
}

/// Apply credential-bearing environment overrides directly on the child
/// process `Command` rather than serialising them into the JSON request body.
/// This keeps secret values out of any serialised/logged data path.
pub(super) fn apply_provider_env_to_command(
    command: &mut Command,
    overrides: &BTreeMap<String, String>,
    unset: &[String],
) {
    for (key, value) in overrides {
        command.env(key, value);
    }
    for key in unset {
        command.env_remove(key);
    }
}

fn set_optional_command_env(command: &mut Command, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        command.env(key, value);
    } else {
        command.env_remove(key);
    }
}

pub(super) fn apply_helper_session_controls_to_command(
    command: &mut Command,
    request: &ManagedHelperRequest,
) {
    set_optional_command_env(command, HELPER_RESUME_ENV, request.resume.as_deref());
    set_optional_command_env(
        command,
        HELPER_SESSION_ID_ENV,
        request.session_id.as_deref(),
    );
    set_optional_command_env(
        command,
        HELPER_RESUME_AT_ENV,
        request.resume_session_at.as_deref(),
    );
}

pub(super) fn redact_helper_request_session_controls(
    request: &ManagedHelperRequest,
) -> ManagedHelperRequest {
    let mut redacted = request.clone();
    redacted.resume = None;
    redacted.session_id = None;
    redacted.resume_session_at = None;
    redacted
}

pub(super) async fn upsert_tracked_json_object<T>(
    storage_path: &Path,
    object_type: &str,
    object_id: &str,
    value: &T,
) -> Result<()>
where
    T: Serialize + Send + Sync,
{
    let storage = LocalStorage::new(storage_path.join("objects"));
    let object_hash = storage
        .put_json(value)
        .await
        .with_context(|| format!("failed to persist {object_type} '{object_id}' JSON blob"))?;
    let mcp_server = init_local_mcp_server(storage_path).await?;
    let history = mcp_server
        .intent_history_manager
        .as_ref()
        .ok_or_else(|| anyhow!("local MCP history manager is unavailable"))?;
    let existing_hash = history
        .get_object_hash(object_type, object_id)
        .await
        .with_context(|| format!("failed to inspect {object_type} history for '{object_id}'"))?;
    if existing_hash != Some(object_hash) {
        history
            .append(object_type, object_id, object_hash)
            .await
            .with_context(|| format!("failed to append {object_type} '{object_id}' to history"))?;
    }
    Ok(())
}

pub(super) async fn invoke_helper(
    custom_helper: bool,
    python_binary: &str,
    helper_path: &Path,
    request: &ManagedHelperRequest,
    provider_env_overrides: &BTreeMap<String, String>,
    provider_env_unset: &[String],
) -> Result<ClaudeManagedArtifact> {
    let redacted_request = redact_helper_request_session_controls(request);
    let serialized_request = serde_json::to_vec(&redacted_request)
        .context("failed to serialize Claude Code helper request")?;
    let executable = if custom_helper {
        helper_path.display().to_string()
    } else {
        python_binary.to_string()
    };
    let mut command = build_helper_command(custom_helper, python_binary, helper_path);
    apply_helper_session_controls_to_command(&mut command, request);
    apply_provider_env_to_command(&mut command, provider_env_overrides, provider_env_unset);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "failed to start Claude Code helper with '{}' '{}'",
                executable,
                helper_path.display()
            )
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&serialized_request)
            .await
            .context("failed to send request to Claude Code helper")?;
    }

    let output = child
        .wait_with_output()
        .await
        .context("failed to wait for Claude Code helper process")?;
    let stdout = String::from_utf8(output.stdout).context("helper stdout is not valid UTF-8")?;
    let stderr = String::from_utf8(output.stderr).context("helper stderr is not valid UTF-8")?;

    if !output.status.success() {
        bail!(
            "Claude Code helper exited with status {}: {}",
            output.status,
            stderr.trim()
        );
    }

    <ManagedHelperRequest as HelperResponse>::parse_response(&stdout, &stderr)
}

pub(super) async fn invoke_helper_json<T>(
    custom_helper: bool,
    python_binary: &str,
    helper_path: &Path,
    request: &T,
) -> Result<T::Output>
where
    T: Serialize + HelperResponse,
{
    invoke_helper_json_with_env(
        custom_helper,
        python_binary,
        helper_path,
        request,
        &BTreeMap::new(),
        &[],
    )
    .await
}

async fn invoke_helper_json_with_env<T>(
    custom_helper: bool,
    python_binary: &str,
    helper_path: &Path,
    request: &T,
    provider_env_overrides: &BTreeMap<String, String>,
    provider_env_unset: &[String],
) -> Result<T::Output>
where
    T: Serialize + HelperResponse,
{
    let serialized_request =
        serde_json::to_vec(request).context("failed to serialize Claude Code helper request")?;
    let executable = if custom_helper {
        helper_path.display().to_string()
    } else {
        python_binary.to_string()
    };
    let mut command = build_helper_command(custom_helper, python_binary, helper_path);
    apply_provider_env_to_command(&mut command, provider_env_overrides, provider_env_unset);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "failed to start Claude Code helper with '{}' '{}'",
                executable,
                helper_path.display()
            )
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&serialized_request)
            .await
            .context("failed to send request to Claude Code helper")?;
    }

    let output = child
        .wait_with_output()
        .await
        .context("failed to wait for Claude Code helper process")?;
    let stdout = String::from_utf8(output.stdout).context("helper stdout is not valid UTF-8")?;
    let stderr = String::from_utf8(output.stderr).context("helper stderr is not valid UTF-8")?;

    if !output.status.success() {
        let detail = if stderr.trim().is_empty() {
            "helper exited with a non-zero status".to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(anyhow!("Claude Code helper failed: {detail}"));
    }

    T::parse_response(stdout.trim(), stderr.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_helper_python_binary_prefers_project_venv() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let venv_python = temp_dir.path().join(".venv").join("bin").join("python");
        std::fs::create_dir_all(venv_python.parent().expect("venv parent"))
            .expect("create venv bin dir");
        std::fs::write(&venv_python, b"#!/usr/bin/env python3\n").expect("write fake python");

        let resolved = resolve_helper_python_binary(temp_dir.path(), DEFAULT_PYTHON_BINARY);
        assert_eq!(resolved, venv_python.to_string_lossy());
    }

    #[test]
    fn resolve_helper_python_binary_keeps_explicit_override() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let resolved = resolve_helper_python_binary(temp_dir.path(), "/custom/python");
        assert_eq!(resolved, "/custom/python");
    }
}
