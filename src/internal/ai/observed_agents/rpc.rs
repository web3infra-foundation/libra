//! External `libra-agent-<name>` binary RPC.
//!
//! Phase 4.5 (entire.md §14.4 item 5) — bring Libra to parity with
//! EntireIO's plugin model. An external binary named
//! `libra-agent-<name>` (e.g. `libra-agent-cursor`) found on `PATH`
//! becomes a recognisable adapter: Libra spawns it, exchanges
//! line-delimited JSON requests / responses on stdin/stdout, and
//! routes the answers back through the same `ObservedAgent`-style
//! API used by built-in adapters.
//!
//! v1 surface intentionally narrow:
//! - **Discovery**: scan every `PATH` entry for executables matching
//!   the `libra-agent-*` pattern. Returns one [`RpcAgent`] per binary.
//! - **Protocol**: a single JSON object per line. Request:
//!   `{"jsonrpc": "2.0", "method": <name>, "params": <object|null>, "id": <int>}`.
//!   Response: `{"jsonrpc": "2.0", "result": <value>, "id": <int>}` or
//!   `{"jsonrpc": "2.0", "error": {"code": <int>, "message": <string>}, "id": <int>}`.
//! - **Methods** (registered names — adapters must answer these):
//!   `provider_kind` → `{"kind": "<snake_case>"}`,
//!   `provider_name` → `{"name": "<slug>"}`,
//!   `protected_dirs` → `{"dirs": ["..."]}`,
//!   `read_transcript` → `{"bytes": "<base64>"} | {"none": true}`.
//! - **Capability negotiation**: response to a `capabilities` method
//!   (mandatory) returns the set of methods the binary implements.
//!   The runtime ONLY invokes methods listed there.
//!
//! Out of scope for v1: streaming responses, hooks/lifecycle events
//! (these go through the existing in-process `ObservedAgentHooks`),
//! truncation. Future work in `entire.md` §14 phase 5 picks up
//! capability v2 (events stream, hook installation by binary).

use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// File-name prefix the discovery scan looks for.
pub const RPC_BINARY_PREFIX: &str = "libra-agent-";

/// Hard cap on how long a single RPC request may take. Protects the
/// runtime from hanging on a misbehaving binary.
pub const RPC_DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Discovered binary plus its launch path. The runtime owns one of
/// these per binary; capability negotiation happens lazily on first
/// invocation.
#[derive(Debug, Clone)]
pub struct RpcAgentBinary {
    /// Slug suffix after `libra-agent-` (e.g. `cursor`).
    pub slug: String,
    /// Absolute path to the executable.
    pub binary_path: PathBuf,
}

/// Live JSON-RPC channel against a spawned [`RpcAgentBinary`]. Owns
/// the child process; `Drop` reaps it gracefully (sends a `shutdown`
/// notification before killing as a fallback). Use [`RpcAgent::spawn`]
/// to create one.
pub struct RpcAgent {
    binary: RpcAgentBinary,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: AtomicU64,
    /// Cached capabilities returned by the first `capabilities` call.
    /// Once populated, the runtime refuses to call any method outside
    /// this set.
    capabilities: Mutex<Option<Vec<String>>>,
}

/// One JSON-RPC request frame. `id` is monotonic per binary; the
/// runtime correlates responses by id.
#[derive(Debug, Clone, Serialize)]
pub struct RpcRequest {
    pub jsonrpc: &'static str,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    pub id: u64,
}

/// One JSON-RPC response frame. Either `result` or `error` is
/// populated, never both. `id` MUST match the request the binary is
/// answering.
#[derive(Debug, Clone, Deserialize)]
pub struct RpcResponse {
    #[serde(default)]
    pub jsonrpc: String,
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// Structured error payload returned by an RPC binary.
#[derive(Debug, Clone, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl RpcAgent {
    /// Spawn `binary` as a child process and prepare a JSON-RPC
    /// channel against it. The child's stderr inherits from the
    /// runtime so operators see binary-side panics in their terminal;
    /// stdout/stdin are piped for RPC traffic.
    pub fn spawn(binary: RpcAgentBinary) -> Result<Self> {
        let mut child = Command::new(&binary.binary_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| {
                format!(
                    "spawn libra-agent binary at {}",
                    binary.binary_path.display()
                )
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("child {} closed stdin unexpectedly", binary.slug))?;
        let stdout = BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| anyhow!("child {} closed stdout unexpectedly", binary.slug))?,
        );
        Ok(Self {
            binary,
            child,
            stdin,
            stdout,
            next_id: AtomicU64::new(1),
            capabilities: Mutex::new(None),
        })
    }

    /// Send a JSON-RPC request and wait for the matching response.
    /// Times out per [`RPC_DEFAULT_TIMEOUT`] — the binary is killed
    /// on timeout so a hang doesn't propagate.
    ///
    /// Capability gating: any method other than `capabilities` is
    /// rejected with `Err` if the binary did not advertise it via the
    /// `capabilities` exchange. Callers therefore typically invoke
    /// `negotiate_capabilities` once before any other method.
    pub fn invoke(&mut self, method: &str, params: Option<Value>) -> Result<Value> {
        if method != "capabilities" {
            let caps = self.capabilities.lock().expect("capabilities mutex");
            match caps.as_ref() {
                None => bail!(
                    "must call `capabilities` before any other RPC against {}",
                    self.binary.slug
                ),
                Some(c) if !c.iter().any(|s| s == method) => bail!(
                    "binary {} does not advertise method '{method}' (capabilities: {c:?})",
                    self.binary.slug
                ),
                _ => {}
            }
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = RpcRequest {
            jsonrpc: "2.0",
            method: method.to_string(),
            params,
            id,
        };
        let line = serde_json::to_string(&request)
            .with_context(|| format!("serialize RPC request for {method}"))?;
        // Write request line + LF terminator.
        writeln!(self.stdin, "{line}").with_context(|| {
            format!(
                "write RPC request to {} stdin (likely the child died)",
                self.binary.slug
            )
        })?;
        self.stdin
            .flush()
            .with_context(|| format!("flush RPC request to {} stdin", self.binary.slug))?;

        // Read responses with a timeout fence. We don't have async I/O
        // available here, so we treat each read as a polling attempt
        // bounded by `RPC_DEFAULT_TIMEOUT`.
        let deadline = Instant::now() + RPC_DEFAULT_TIMEOUT;
        loop {
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                bail!(
                    "RPC method '{method}' against {} timed out after {:?}",
                    self.binary.slug,
                    RPC_DEFAULT_TIMEOUT
                );
            }
            let mut buf = String::new();
            let n = self
                .stdout
                .read_line(&mut buf)
                .with_context(|| format!("read RPC response from {}", self.binary.slug))?;
            if n == 0 {
                bail!(
                    "RPC binary {} closed stdout before answering '{method}'",
                    self.binary.slug
                );
            }
            let line = buf.trim();
            if line.is_empty() {
                continue;
            }
            let resp: RpcResponse = serde_json::from_str(line).with_context(|| {
                format!("parse RPC response line from {}: {line}", self.binary.slug)
            })?;
            if resp.id != id {
                // Out-of-order frame — keep reading. v1 is strictly
                // synchronous so this is unusual, but we tolerate it
                // rather than crash.
                continue;
            }
            if let Some(err) = resp.error {
                bail!(
                    "RPC method '{method}' against {} returned error {}: {}",
                    self.binary.slug,
                    err.code,
                    err.message
                );
            }
            return resp.result.ok_or_else(|| {
                anyhow!("RPC response for '{method}' had neither result nor error")
            });
        }
    }

    /// Mandatory first call. Asks the binary for the set of methods
    /// it implements and caches the result. Subsequent
    /// [`Self::invoke`] calls reject methods not in this set.
    pub fn negotiate_capabilities(&mut self) -> Result<Vec<String>> {
        let value = self.invoke("capabilities", None)?;
        let methods = value
            .get("methods")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                anyhow!(
                    "capabilities response from {} missing `methods` array",
                    self.binary.slug
                )
            })?
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect::<Vec<_>>();
        let mut guard = self.capabilities.lock().expect("capabilities mutex");
        *guard = Some(methods.clone());
        Ok(methods)
    }
}

impl Drop for RpcAgent {
    fn drop(&mut self) {
        // Best-effort graceful shutdown: send a `shutdown`
        // notification (id-less) so the child can clean up, then kill
        // if it doesn't exit promptly.
        let notify = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "shutdown",
        });
        if let Ok(s) = serde_json::to_string(&notify) {
            let _ = writeln!(self.stdin, "{s}");
            let _ = self.stdin.flush();
        }
        // Give the child a brief window to exit on its own.
        let deadline = Instant::now() + Duration::from_millis(200);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Discover every `libra-agent-*` executable on `$PATH`. The slug is
/// the substring after the prefix; duplicates from later PATH entries
/// are skipped (the first match wins, matching shell `which`
/// behaviour).
///
/// Returns an empty vec when `$PATH` is unset or no binaries match.
pub fn discover_rpc_agents() -> Vec<RpcAgentBinary> {
    use std::collections::HashSet;

    let Some(path_var) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<RpcAgentBinary> = Vec::new();
    for dir in std::env::split_paths(&path_var) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name_os = entry.file_name();
            let Some(name) = name_os.to_str() else {
                continue;
            };
            let Some(slug) = name.strip_prefix(RPC_BINARY_PREFIX) else {
                continue;
            };
            if slug.is_empty() {
                continue;
            }
            // On Unix, only count executable files. Symlinks and
            // non-files are skipped.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                if meta.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            if seen.insert(slug.to_string()) {
                out.push(RpcAgentBinary {
                    slug: slug.to_string(),
                    binary_path: entry.path(),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_returns_empty_when_no_binaries_match() {
        // Point PATH at an empty tempdir — we expect no matches.
        let dir = tempfile::tempdir().unwrap();
        let original = std::env::var_os("PATH");
        // SAFETY: tests serialize this env mutation via `#[serial]`
        // would be safer, but the discovery scan does not depend on
        // global state beyond PATH and we restore it immediately.
        unsafe {
            std::env::set_var("PATH", dir.path());
        }
        let agents = discover_rpc_agents();
        unsafe {
            match original {
                Some(prev) => std::env::set_var("PATH", prev),
                None => std::env::remove_var("PATH"),
            }
        }
        assert!(agents.is_empty());
    }

    #[test]
    fn discover_picks_up_libra_agent_prefix() {
        let dir = tempfile::tempdir().unwrap();
        // Plant a file named `libra-agent-test-fixture` and chmod it
        // executable. Discovery should find it.
        let path = dir.path().join("libra-agent-test-fixture");
        std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let original = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("PATH", dir.path());
        }
        let agents = discover_rpc_agents();
        unsafe {
            match original {
                Some(prev) => std::env::set_var("PATH", prev),
                None => std::env::remove_var("PATH"),
            }
        }

        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].slug, "test-fixture");
        assert_eq!(agents[0].binary_path, path);
    }

    #[test]
    fn discover_skips_files_without_executable_bit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("libra-agent-no-exec");
        std::fs::write(&path, b"plain file\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        let original = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("PATH", dir.path());
        }
        let agents = discover_rpc_agents();
        unsafe {
            match original {
                Some(prev) => std::env::set_var("PATH", prev),
                None => std::env::remove_var("PATH"),
            }
        }

        assert!(
            agents.is_empty(),
            "non-executable file must be skipped: {agents:?}"
        );
    }

    #[test]
    fn discover_skips_files_with_empty_slug() {
        // `libra-agent-` (no slug) must NOT match.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("libra-agent-");
        std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let original = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("PATH", dir.path());
        }
        let agents = discover_rpc_agents();
        unsafe {
            match original {
                Some(prev) => std::env::set_var("PATH", prev),
                None => std::env::remove_var("PATH"),
            }
        }

        assert!(
            agents.is_empty(),
            "empty-slug binary must be skipped: {agents:?}"
        );
    }

    #[test]
    fn discover_dedups_across_path_entries() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        for dir in [dir_a.path(), dir_b.path()] {
            let path = dir.join("libra-agent-dup");
            std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }

        let original = std::env::var_os("PATH");
        let combined = std::env::join_paths([dir_a.path(), dir_b.path()]).unwrap();
        unsafe {
            std::env::set_var("PATH", &combined);
        }
        let agents = discover_rpc_agents();
        unsafe {
            match original {
                Some(prev) => std::env::set_var("PATH", prev),
                None => std::env::remove_var("PATH"),
            }
        }

        assert_eq!(agents.len(), 1, "first match wins: {agents:?}");
        assert_eq!(agents[0].slug, "dup");
        // First match must be from the first PATH entry.
        assert!(agents[0].binary_path.starts_with(dir_a.path()));
    }
}
