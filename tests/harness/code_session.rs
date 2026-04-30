#![allow(dead_code)]

use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use reqwest::{StatusCode, blocking::Client};
use serde::Deserialize;
use serde_json::{Value, json};
use tempfile::TempDir;

#[derive(Debug, Clone)]
pub struct CodeSessionOptions {
    pub fixture: PathBuf,
    pub name: String,
    pub use_default_control_paths: bool,
}

impl CodeSessionOptions {
    pub fn new(name: impl Into<String>, fixture: impl Into<PathBuf>) -> Self {
        Self {
            fixture: fixture.into(),
            name: name.into(),
            use_default_control_paths: false,
        }
    }

    pub fn with_default_control_paths(mut self) -> Self {
        self.use_default_control_paths = true;
        self
    }
}

pub struct CodeSession {
    _temp: TempDir,
    repo_dir: PathBuf,
    fixture: PathBuf,
    logs_dir: PathBuf,
    token_path: PathBuf,
    info_path: PathBuf,
    base_url: String,
    control_token: String,
    controller_token: Option<String>,
    child: Option<Box<dyn Child + Send + Sync>>,
    writer: Option<Box<dyn Write + Send>>,
    reader_thread: Option<thread::JoinHandle<()>>,
    client: Client,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlInfo {
    base_url: String,
}

impl CodeSession {
    pub fn spawn(options: CodeSessionOptions) -> Result<Self> {
        let bin = libra_bin();
        let temp = tempfile::Builder::new()
            .prefix(&format!("libra-code-ui-{}-", options.name))
            .tempdir()
            .context("failed to create code session tempdir")?;
        let repo_dir = temp.path().join("repo");
        let logs_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("code-ui-scenarios")
            .join(&options.name);
        fs::create_dir_all(&repo_dir).context("failed to create temp repo directory")?;
        if logs_dir.exists() {
            fs::remove_dir_all(&logs_dir).with_context(|| {
                format!("failed to clear previous logs dir '{}'", logs_dir.display())
            })?;
        }
        fs::create_dir_all(&logs_dir).context("failed to create code session logs directory")?;

        let init_output = Command::new(&bin)
            .args(["init", "--vault=false", "--quiet"])
            .arg(&repo_dir)
            .output()
            .with_context(|| format!("failed to run '{} init'", bin.display()))?;
        if !init_output.status.success() {
            bail!(
                "failed to initialize temp Libra repo\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&init_output.stdout),
                String::from_utf8_lossy(&init_output.stderr)
            );
        }

        let token_path = if options.use_default_control_paths {
            repo_dir.join(".libra").join("code").join("control-token")
        } else {
            temp.path().join("control-token")
        };
        let info_path = if options.use_default_control_paths {
            repo_dir.join(".libra").join("code").join("control.json")
        } else {
            temp.path().join("control.json")
        };
        let libra_log = logs_dir.join("libra.log");
        let pty_log = logs_dir.join("pty.log");

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to open PTY")?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        let reader_thread = thread::spawn(move || {
            let Ok(mut file) = File::create(&pty_log) else {
                return;
            };
            let mut buf = [0_u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if file.write_all(&buf[..n]).is_err() {
                            break;
                        }
                        let _ = file.flush();
                    }
                    Err(_) => break,
                }
            }
        });

        let writer = pair
            .master
            .take_writer()
            .context("failed to take PTY writer")?;

        let mut cmd = CommandBuilder::new(&bin);
        cmd.args([
            "code",
            "--provider",
            "fake",
            "--fake-fixture",
            path_str(&options.fixture)?,
            "--model",
            "fake-local",
            "--control",
            "write",
            "--port",
            "0",
            "--mcp-port",
            "0",
        ]);
        if !options.use_default_control_paths {
            cmd.args([
                "--control-token-file",
                path_str(&token_path)?,
                "--control-info-file",
                path_str(&info_path)?,
            ]);
        }
        cmd.cwd(&repo_dir);
        cmd.env("TERM", "xterm-256color");
        cmd.env("LIBRA_ENABLE_TEST_PROVIDER", "1");
        cmd.env("LIBRA_LOG_FILE", path_str(&libra_log)?);
        cmd.env("LIBRA_LOG", "info,libra::internal::ai::web=debug");

        let child = pair
            .slave
            .spawn_command(cmd)
            .context("failed to spawn libra code in PTY")?;
        drop(pair.slave);

        let mut session = Self {
            _temp: temp,
            repo_dir,
            fixture: options.fixture,
            logs_dir,
            token_path,
            info_path,
            base_url: String::new(),
            control_token: String::new(),
            controller_token: None,
            child: Some(child),
            writer: Some(writer),
            reader_thread: Some(reader_thread),
            client: Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .context("failed to build test HTTP client")?,
        };
        session.wait_for_control_info(Duration::from_secs(30))?;
        Ok(session)
    }

    pub fn token_path(&self) -> &Path {
        &self.token_path
    }

    pub fn info_path(&self) -> &Path {
        &self.info_path
    }

    pub fn run_default_control_conflict(&self) -> Result<Output> {
        let mut child = Command::new(libra_bin())
            .args([
                "code",
                "--provider",
                "fake",
                "--fake-fixture",
                path_str(&self.fixture)?,
                "--model",
                "fake-local",
                "--control",
                "write",
                "--port",
                "0",
                "--mcp-port",
                "0",
            ])
            .current_dir(&self.repo_dir)
            .env("TERM", "xterm-256color")
            .env("LIBRA_ENABLE_TEST_PROVIDER", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn conflicting libra code process")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if child
                .try_wait()
                .context("failed to poll conflicting libra code process")?
                .is_some()
            {
                return child
                    .wait_with_output()
                    .context("failed to collect conflicting libra code output");
            }
            thread::sleep(Duration::from_millis(100));
        }

        let _ = child.kill();
        let _ = child.wait();
        bail!("conflicting libra code process did not exit within 10s")
    }

    pub fn snapshot(&self) -> Result<Value> {
        self.get_json("/session")
    }

    pub fn diagnostics(&self) -> Result<Value> {
        self.get_json("/diagnostics")
    }

    pub fn artifact_dir(&self) -> &Path {
        &self.logs_dir
    }

    pub fn debug_context(&self) -> String {
        let snapshot = self
            .snapshot()
            .and_then(|snapshot| {
                serde_json::to_string_pretty(&snapshot)
                    .context("failed to serialize latest snapshot")
            })
            .unwrap_or_else(|error| format!("<unavailable: {error:#}>"));
        let control_info = fs::read_to_string(&self.info_path)
            .unwrap_or_else(|error| format!("<unavailable: {error}>"));
        let pty_tail = tail_file(&self.logs_dir.join("pty.log"), 20);
        let libra_tail = tail_file(&self.logs_dir.join("libra.log"), 20);
        let context = format!(
            "artifacts: {}\ncontrol.json:\n{}\nlatest snapshot:\n{}\npty.log tail:\n{}\nlibra.log tail:\n{}",
            self.logs_dir.display(),
            control_info,
            snapshot,
            pty_tail,
            libra_tail
        );
        self.redact_known_secrets(context)
    }

    pub fn attach_automation(&mut self, client_id: &str) -> Result<String> {
        let response = self
            .client
            .post(self.url("/controller/attach"))
            .header("X-Libra-Control-Token", &self.control_token)
            .json(&json!({ "clientId": client_id, "kind": "automation" }))
            .send()
            .context("failed to send automation attach request")?;
        let status = response.status();
        let text = response
            .text()
            .context("failed to read automation attach response body")?;
        ensure_success(status, &text)?;
        let value: Value =
            serde_json::from_str(&text).context("failed to parse automation attach response")?;
        let token = value
            .get("controllerToken")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("attach response did not include controllerToken: {value}"))?
            .to_string();
        self.controller_token = Some(token.clone());
        Ok(token)
    }

    pub fn submit_message(&self, text: &str) -> Result<StatusCode> {
        let response = self
            .authorized_post("/messages")
            .json(&json!({ "text": text }))
            .send()
            .context("failed to submit automation message")?;
        let status = response.status();
        if !status.is_success() {
            bail!(
                "message submit failed with {status}: {}",
                response.text().unwrap_or_default()
            );
        }
        Ok(status)
    }

    pub fn submit_message_expect_error(&self, text: &str) -> Result<(StatusCode, Value)> {
        let response = self
            .authorized_post("/messages")
            .json(&json!({ "text": text }))
            .send()
            .context("failed to submit automation message")?;
        let status = response.status();
        let body = response.json().unwrap_or_else(|_| json!({}));
        Ok((status, body))
    }

    pub fn respond_interaction_expect_error(
        &self,
        interaction_id: &str,
    ) -> Result<(StatusCode, Value)> {
        let response = self
            .authorized_post(&format!("/interactions/{interaction_id}"))
            .json(&json!({ "approved": true }))
            .send()
            .context("failed to submit automation interaction response")?;
        let status = response.status();
        let body = response.json().unwrap_or_else(|_| json!({}));
        Ok((status, body))
    }

    pub fn submit_large_message(&self, bytes: usize) -> Result<(StatusCode, Value)> {
        let text = "x".repeat(bytes);
        let response = self
            .authorized_post("/messages")
            .json(&json!({ "text": text }))
            .send()
            .context("failed to submit large automation message")?;
        let status = response.status();
        let body = response.json().unwrap_or_else(|_| json!({}));
        Ok((status, body))
    }

    pub fn cancel_turn(&self) -> Result<StatusCode> {
        let response = self
            .authorized_post("/control/cancel")
            .send()
            .context("failed to send cancel request")?;
        let status = response.status();
        if !status.is_success() {
            bail!(
                "cancel failed with {status}: {}",
                response.text().unwrap_or_default()
            );
        }
        Ok(status)
    }

    pub fn write_tui_line(&mut self, line: &str) -> Result<()> {
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| anyhow!("PTY writer is closed"))?;
        writer
            .write_all(line.as_bytes())
            .context("failed to write line to PTY")?;
        writer
            .write_all(b"\r")
            .context("failed to write enter key to PTY")?;
        writer.flush().context("failed to flush PTY writer")
    }

    pub fn wait_for_snapshot<F>(&self, timeout: Duration, mut predicate: F) -> Result<Value>
    where
        F: FnMut(&Value) -> bool,
    {
        let deadline = Instant::now() + timeout;
        let mut last = Value::Null;
        while Instant::now() < deadline {
            match self.snapshot() {
                Ok(snapshot) => {
                    if predicate(&snapshot) {
                        return Ok(snapshot);
                    }
                    last = snapshot;
                }
                Err(error) => {
                    last = json!({ "error": error.to_string() });
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        bail!(
            "timed out waiting for snapshot condition; last snapshot: {last}\n{}",
            self.debug_context()
        )
    }

    pub fn shutdown(&mut self) -> Result<()> {
        if let Some(writer) = self.writer.as_mut() {
            let _ = writer.write_all(b"/quit\r");
            let _ = writer.flush();
        }

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.child_exited()? {
                self.join_reader();
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }

        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.join_reader();
        Ok(())
    }

    fn wait_for_control_info(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.child_exited()? {
                bail!(
                    "libra code exited before writing control info; logs: {}",
                    self.logs_dir.display()
                );
            }
            if self.info_path.exists() {
                let info_text = fs::read_to_string(&self.info_path)
                    .context("failed to read control info file")?;
                let info: ControlInfo =
                    serde_json::from_str(&info_text).context("failed to parse control info")?;
                let _ = fs::write(self.logs_dir.join("control.json"), &info_text);
                let token = fs::read_to_string(&self.token_path)
                    .context("failed to read control token file")?;
                self.base_url = info.base_url;
                self.control_token = token.trim().to_string();
                self.wait_for_snapshot(Duration::from_secs(10), |_| true)?;
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        bail!(
            "timed out waiting for control info file '{}'; logs: {}",
            self.info_path.display(),
            self.logs_dir.display()
        )
    }

    fn get_json(&self, path: &str) -> Result<Value> {
        let response = self
            .client
            .get(self.url(path))
            .send()
            .with_context(|| format!("failed to GET {path}"))?;
        let status = response.status();
        let body = response
            .text()
            .with_context(|| format!("failed to read GET {path} response body"))?;
        if !status.is_success() {
            bail!("GET {path} failed with {status}: {body}");
        }
        serde_json::from_str(&body).with_context(|| format!("failed to parse GET {path} JSON"))
    }

    fn authorized_post(&self, path: &str) -> reqwest::blocking::RequestBuilder {
        let mut request = self
            .client
            .post(self.url(path))
            .header("X-Libra-Control-Token", &self.control_token);
        if let Some(token) = self.controller_token.as_ref() {
            request = request.header("X-Code-Controller-Token", token);
        }
        request
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/code{}", self.base_url, path)
    }

    fn child_exited(&mut self) -> Result<bool> {
        let Some(child) = self.child.as_mut() else {
            return Ok(true);
        };
        match child
            .try_wait()
            .context("failed to poll libra code child")?
        {
            Some(_status) => {
                self.child = None;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn join_reader(&mut self) {
        self.writer.take();
        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
    }

    fn redact_known_secrets(&self, mut text: String) -> String {
        if !self.control_token.is_empty() {
            text = text.replace(&self.control_token, "[REDACTED_CONTROL_TOKEN]");
        }
        if let Some(token) = self.controller_token.as_ref() {
            text = text.replace(token, "[REDACTED_CONTROLLER_TOKEN]");
        }
        text
    }
}

impl Drop for CodeSession {
    fn drop(&mut self) {
        if self.child.is_some() {
            let _ = self.shutdown();
        }
    }
}

fn ensure_success(status: StatusCode, body: &str) -> Result<()> {
    if status.is_success() {
        Ok(())
    } else {
        bail!("request failed with {status}: {body}")
    }
}

fn libra_bin() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_libra")
        .map(PathBuf::from)
        .expect("CARGO_BIN_EXE_libra is set for integration tests")
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow!("path is not valid UTF-8: {}", path.display()))
}

fn tail_file(path: &Path, max_lines: usize) -> String {
    let Ok(text) = fs::read_to_string(path) else {
        return format!("<unavailable: {}>", path.display());
    };
    let mut lines: Vec<&str> = text.lines().rev().take(max_lines).collect();
    lines.reverse();
    if lines.is_empty() {
        "<empty>".to_string()
    } else {
        lines.join("\n")
    }
}
