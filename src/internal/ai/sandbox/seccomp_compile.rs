//! On-the-fly seccomp BPF compilation from the bundled
//! `template/seccomp-default.json` policy.
//!
//! 从捆绑的 `template/seccomp-default.json` 策略动态编译 seccomp BPF。
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
//! 2. Calls `seccompiler::compile_from_json` to produce a
//!    `BpfThreadMap` — a map from thread name (we only use the
//!    `default` filter slot) to the assembled `BpfProgram`.
//! 3. Serialises the program to the raw `Vec<u8>` shape `bwrap
//!    --seccomp <fd>` reads from the file descriptor we hand it
//!    (each instruction is a 64-bit little-endian word — the
//!    in-kernel `sock_filter` struct ABI).
//!
//! The output is intentionally byte-compatible with
//! `seccompiler-bin --target-arch <arch> --input-file
//! template/seccomp-default.json` so an operator who pre-compiled
//! the policy with the standalone tool sees the same bytes the
//! runtime would compile on its own.

#![cfg(target_os = "linux")]

use std::path::Path;

use seccompiler::{BpfProgram, TargetArch, compile_from_json};
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
/// `bwrap --seccomp <fd>` expects. The byte layout matches
/// `seccompiler-bin --output bpf-bin --input-file
/// template/seccomp-default.json` so a pre-compiled file and an
/// on-the-fly compiled blob are interchangeable.
pub fn compile_bundled_seccomp_policy() -> Result<Vec<u8>, SeccompCompileError> {
    let arch = host_target_arch()?;
    let mut filters = compile_from_json(BUNDLED_POLICY_JSON.as_bytes(), arch).map_err(|err| {
        SeccompCompileError::JsonInvalid {
            reason: err.to_string(),
        }
    })?;
    let program: BpfProgram = filters
        .remove("default")
        .ok_or(SeccompCompileError::NoDefaultSlot)?;
    Ok(bpf_program_to_bytes(&program))
}

/// Write the compiled policy to `path`, creating the parent
/// directory if necessary. Used by the runtime's first-launch
/// fallback path to materialise `~/.libra/seccomp.bpf` so the
/// `--seccomp <fd>` wiring (v0.17.725) can pick it up on
/// subsequent invocations.
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
    std::fs::write(path, bytes)
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
}
