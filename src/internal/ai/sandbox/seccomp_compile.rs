//! On-the-fly seccomp BPF compilation from the bundled
//! `template/seccomp-default.json` policy.
//!
//! `docs/improvement/sandbox.md` §G6 commits Libra to a default
//! seccomp posture: when `LIBRA_SECCOMP_POLICY` is unset and
//! `~/.libra/seccomp.bpf` is absent, the runtime should be able to
//! materialise the bundled JSON policy into BPF bytes without the
//! operator having to run `seccompiler-bin` manually. v0.17.730
//! shipped the JSON template but left the JSON → BPF step as a
//! manual `seccompiler-bin --target-arch …` invocation.
//!
//! This module closes that gap on Linux. The compiler:
//!
//! 1. Takes the bundled JSON text (embedded at compile time via
//!    `include_str!`) and the host's target arch (resolved at
//!    runtime so cross-arch builds remain correct).
//! 2. Normalises that JSON for the selected architecture, pruning
//!    denylist entries for syscall names that do not exist there.
//!    The bundled template keeps x86_64-only raw I/O denies
//!    (`iopl` / `ioperm`) so root/container x86_64 runs still
//!    block direct I/O-port access; aarch64 drops those entries
//!    before handing the policy to seccompiler.
//! 3. Calls `seccompiler::compile_from_json` to produce a
//!    `BpfThreadMap` — a map from thread name (we only use the
//!    `default` filter slot) to the assembled `BpfProgram`.
//! 4. Serialises the program to the raw `Vec<u8>` shape `bwrap
//!    --seccomp <fd>` reads from the file descriptor we hand it
//!    (each instruction is a 64-bit little-endian word — the
//!    in-kernel `sock_filter` struct ABI).
//!
//! For x86_64, the output is intentionally byte-compatible with
//! `seccompiler-bin --target-arch x86_64 --input-file
//! template/seccomp-default.json`. For aarch64, Libra first removes
//! x86_64-only syscall names from the bundled policy so the runtime
//! can still materialise the default BPF without weakening x86_64.

#![cfg(target_os = "linux")]

use std::{io::Write, path::Path};

use seccompiler::{BpfProgram, TargetArch, compile_from_json};
use serde_json::Value;
use thiserror::Error;

/// Embedded JSON policy. Lives at `template/seccomp-default.json`
/// at the repo root; v0.17.730 added it as the bundled baseline.
/// `include_str!` resolves the path relative to *this* file
/// (`src/internal/ai/sandbox/`), so the `../../../../` climb maps
/// to the workspace root.
const BUNDLED_POLICY_JSON: &str = include_str!("../../../../template/seccomp-default.json");

/// Failure modes from [`compile_bundled_seccomp_policy`].
///
/// The variants are deliberately narrow — the compiler is a pure
/// JSON-to-bytes transform; failures here are either "host arch
/// not recognised" (very rare; libc TARGET_ARCH should always
/// match one of the seccompiler variants) or "JSON did not
/// validate" (a bug in the bundled template that build-time
/// fixture tests should catch first).
#[derive(Debug, Error)]
pub enum SeccompCompileError {
    /// `std::env::consts::ARCH` returned a string seccompiler
    /// does not recognise. Carries the unrecognised label so the
    /// operator's diagnostic surface includes "is your build for
    /// arm64/x86_64/s390x?".
    #[error(
        "host arch '{arch}' is not a known seccompiler TargetArch (libra supports x86_64, aarch64)"
    )]
    UnknownArch { arch: String },

    /// `seccompiler::compile_from_json` rejected the bundled
    /// template. The underlying message is opaque (seccompiler's
    /// error type is non-exhaustive); we forward its `Display` so
    /// CI can spot a template regression at a glance.
    #[error("failed to compile bundled seccomp policy JSON: {reason}")]
    JsonInvalid { reason: String },

    /// The JSON parsed and compiled, but it did not produce a
    /// `default` thread slot. The bundled template always uses
    /// the `default` slot — a future template that uses a named
    /// thread without this slot would surface here so callers
    /// know which slot to ask for next.
    #[error(
        "compiled seccomp filter set has no `default` thread slot; \
         the bundled template must declare one"
    )]
    NoDefaultSlot,
}

/// Resolve the host's target architecture for seccompiler. Public
/// so the runtime can include the resolved label in error
/// messages without compiling the whole policy first.
pub fn host_target_arch() -> Result<TargetArch, SeccompCompileError> {
    match std::env::consts::ARCH {
        "x86_64" => Ok(TargetArch::x86_64),
        "aarch64" => Ok(TargetArch::aarch64),
        other => Err(SeccompCompileError::UnknownArch {
            arch: other.to_string(),
        }),
    }
}

/// Compile the bundled JSON policy into the raw BPF bytes
/// `bwrap --seccomp <fd>` expects. The policy is normalised for
/// the target architecture before seccompiler sees it.
pub fn compile_bundled_seccomp_policy() -> Result<Vec<u8>, SeccompCompileError> {
    let arch = host_target_arch()?;
    compile_bundled_seccomp_policy_for_arch(arch)
}

fn compile_bundled_seccomp_policy_for_arch(
    arch: TargetArch,
) -> Result<Vec<u8>, SeccompCompileError> {
    let policy_json = policy_json_for_arch(arch)?;
    let mut filters = compile_from_json(policy_json.as_bytes(), arch).map_err(|err| {
        SeccompCompileError::JsonInvalid {
            reason: err.to_string(),
        }
    })?;
    let program: BpfProgram = filters
        .remove("default")
        .ok_or(SeccompCompileError::NoDefaultSlot)?;
    Ok(bpf_program_to_bytes(&program))
}

fn policy_json_for_arch(arch: TargetArch) -> Result<String, SeccompCompileError> {
    let mut policy: Value = serde_json::from_str(BUNDLED_POLICY_JSON).map_err(|err| {
        SeccompCompileError::JsonInvalid {
            reason: err.to_string(),
        }
    })?;
    prune_policy_for_arch(&mut policy, arch);
    serde_json::to_string(&policy).map_err(|err| SeccompCompileError::JsonInvalid {
        reason: err.to_string(),
    })
}

fn prune_policy_for_arch(policy: &mut Value, arch: TargetArch) {
    let Some(rules) = policy
        .get_mut("default")
        .and_then(|default| default.get_mut("filter"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };

    rules.retain(|rule| {
        let syscall = rule.get("syscall").and_then(Value::as_str);
        match syscall {
            Some(name) => bundled_policy_syscall_supported_on_arch(name, arch),
            None => true,
        }
    });
}

fn bundled_policy_syscall_supported_on_arch(syscall: &str, arch: TargetArch) -> bool {
    match arch {
        TargetArch::x86_64 => true,
        TargetArch::aarch64 | TargetArch::riscv64 => !matches!(syscall, "iopl" | "ioperm"),
    }
}

/// Write the compiled policy to `path`, creating the parent
/// directory if necessary. Used by the runtime's first-launch
/// fallback path to materialise `~/.libra/seccomp.bpf` so the
/// `--seccomp <fd>` wiring (v0.17.725) can pick it up on
/// subsequent invocations.
///
/// The write is atomic from readers' perspective: a fresh process
/// either sees no policy and compiles its own, or sees a complete
/// policy after the temp-file rename. It must never observe the
/// target path while BPF bytes are still being written.
pub fn ensure_compiled_seccomp_policy_at(path: &Path) -> Result<(), std::io::Error> {
    if path.is_file() {
        return Ok(());
    }
    let bytes = compile_bundled_seccomp_policy().map_err(|err| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to compile bundled seccomp policy: {err}"),
        )
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_policy_file_atomically(path, &bytes)
}

fn write_policy_file_atomically(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("seccomp.bpf");
    let mut temp = tempfile::Builder::new()
        .prefix(&format!(".{file_name}."))
        .suffix(".tmp")
        .tempfile_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file_mut().sync_all()?;
    temp.persist(path).map(|_| ()).map_err(|err| err.error)
}

/// Serialise a `BpfProgram` (a `Vec<sock_filter>`) into the raw
/// byte buffer the kernel reads from the `--seccomp <fd>` file
/// descriptor. Each `sock_filter` is 8 bytes:
///   - u16 code (LE)
///   - u8  jt   (jump-true offset)
///   - u8  jf   (jump-false offset)
///   - u32 k    (LE constant)
///
/// `BpfProgram` is a `Vec<sock_filter>` ABI-stable struct, but the
/// crate does not expose a `to_bytes()`; reaching into the field
/// names directly keeps us pinned to seccompiler's public type
/// and lets us flag a breaking version bump at compile time.
fn bpf_program_to_bytes(program: &BpfProgram) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(program.len() * 8);
    for instruction in program {
        bytes.extend_from_slice(&instruction.code.to_le_bytes());
        bytes.push(instruction.jt);
        bytes.push(instruction.jf);
        bytes.extend_from_slice(&instruction.k.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: the bundled JSON template compiles cleanly on the
    /// host. A regression in the JSON shape (missing required
    /// field, unknown syscall name, malformed action) fails here.
    #[test]
    fn bundled_policy_compiles_on_host_arch() {
        let bytes = compile_bundled_seccomp_policy()
            .expect("bundled seccomp JSON must compile on the host arch");
        assert!(
            bytes.len() >= 8,
            "compiled BPF must contain at least one sock_filter (8 bytes); got {} bytes",
            bytes.len(),
        );
        // sock_filter sizes must be exact multiples of 8 (the
        // ABI guarantees this; any non-multiple is a serialisation
        // bug).
        assert_eq!(
            bytes.len() % 8,
            0,
            "compiled BPF byte length must be a multiple of 8; got {} bytes",
            bytes.len(),
        );
    }

    #[test]
    fn bundled_policy_compiles_for_supported_arches() {
        for arch in [TargetArch::x86_64, TargetArch::aarch64] {
            let bytes = compile_bundled_seccomp_policy_for_arch(arch).unwrap_or_else(|err| {
                panic!("bundled seccomp JSON must compile for {arch:?}: {err}")
            });
            assert!(
                bytes.len() >= 8,
                "compiled BPF for {arch:?} must contain at least one sock_filter; got {} bytes",
                bytes.len(),
            );
            assert_eq!(
                bytes.len() % 8,
                0,
                "compiled BPF byte length for {arch:?} must be a multiple of 8; got {} bytes",
                bytes.len(),
            );
        }
    }

    #[test]
    fn policy_normalization_keeps_x86_raw_io_denies_and_filters_aarch64() {
        let x86_policy =
            policy_json_for_arch(TargetArch::x86_64).expect("x86 policy JSON normalizes");
        assert!(
            x86_policy.contains("\"syscall\":\"iopl\""),
            "x86 policy must retain iopl raw I/O deny"
        );
        assert!(
            x86_policy.contains("\"syscall\":\"ioperm\""),
            "x86 policy must retain ioperm raw I/O deny"
        );

        let aarch64_policy =
            policy_json_for_arch(TargetArch::aarch64).expect("aarch64 policy JSON normalizes");
        assert!(
            !aarch64_policy.contains("\"syscall\":\"iopl\""),
            "aarch64 policy must drop x86-only iopl before seccompiler validation"
        );
        assert!(
            !aarch64_policy.contains("\"syscall\":\"ioperm\""),
            "aarch64 policy must drop x86-only ioperm before seccompiler validation"
        );
    }

    /// Scenario: `ensure_compiled_seccomp_policy_at` produces a
    /// file at the target path when none exists, and is a no-op
    /// when a file is already there (no re-compile, no overwrite).
    #[test]
    fn ensure_compiled_seccomp_policy_at_is_idempotent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("nested").join("seccomp.bpf");
        ensure_compiled_seccomp_policy_at(&path).expect("first call must compile + write");
        assert!(path.is_file(), "first call must create the file");
        let bytes_before = std::fs::read(&path).expect("read after first call");

        // Truncate to a recognisable sentinel; the second call
        // must NOT overwrite it (idempotent), proving the
        // is_file() early return fires.
        std::fs::write(&path, b"sentinel").expect("write sentinel");
        ensure_compiled_seccomp_policy_at(&path).expect("second call must be a no-op");
        let bytes_after = std::fs::read(&path).expect("read after second call");
        assert_eq!(bytes_after, b"sentinel", "second call must not overwrite");
        assert_ne!(
            bytes_before, bytes_after,
            "sentinel should differ from compiled BPF; sanity check failed",
        );
    }

    #[test]
    fn write_policy_file_atomically_writes_complete_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("nested").join("seccomp.bpf");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");

        write_policy_file_atomically(&path, b"complete policy").expect("atomic write succeeds");

        assert_eq!(
            std::fs::read(&path).expect("read final policy"),
            b"complete policy"
        );
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().expect("parent"))
            .expect("read parent")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() != "seccomp.bpf")
            .collect();
        assert!(
            leftovers.is_empty(),
            "atomic write should not leave temp files behind: {leftovers:?}"
        );
    }
}
