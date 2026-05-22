# Opting into seccomp BPF policy filtering for `libra code`

Libra's Linux sandbox supports seccomp-bpf filtering via the bwrap
`--seccomp <fd>` argument. The wiring landed in v0.17.725 (see
`docs/improvement/sandbox.md` G6 row), and an environment-variable
convenience landed in v0.17.729 so users can opt in without
editing `SandboxRuntimeConfig` in-process.

> **Status**: opt-in. The default `LIBRA_SECCOMP_POLICY` is unset,
> which keeps the pre-Phase-7 behaviour (no seccomp filter). Libra
> does not ship a default restrictive policy because the
> "correct" policy depends on the host distribution, the workloads
> the agent is expected to run, and architecture-specific syscall
> numbers. The recommendation below is a pragmatic baseline; tune
> it for your environment.

## Quick start

1. Install one of:
   * [`seccompiler`](https://crates.io/crates/seccompiler) (Rust;
     used by Firecracker)
   * `libseccomp-tools` (`apt install libseccomp-tools` or
     `dnf install libseccomp-devel`)

2. Save the policy below as `~/.libra/seccomp.json` (seccompiler
   form) or `~/.libra/seccomp.cil` (libseccomp form).

3. Compile to a BPF binary:
   ```sh
   # seccompiler — recommended for portability
   seccompiler-bin --target-arch "$(uname -m)" \
       --input-file ~/.libra/seccomp.json \
       --output-file ~/.libra/seccomp.bpf

   # libseccomp-tools — alternative
   libseccomp-tools --binary --output ~/.libra/seccomp.bpf ~/.libra/seccomp.cil
   ```

4. Export the env var (e.g. in your shell rc):
   ```sh
   export LIBRA_SECCOMP_POLICY="$HOME/.libra/seccomp.bpf"
   ```

5. Verify the wiring picked it up:
   ```sh
   LIBRA_LOG=info libra code --goal 'noop' --network deny
   # → look for `sandbox.evidence ...` lines and `--seccomp 200`
   #   in the bwrap arg vector.
   ```

## Recommended baseline policy (seccompiler form)

```json
{
  "default_action": "allow",
  "filter": [
    { "action": "kill_process", "syscalls": [
      "mount", "umount", "umount2", "swapon", "swapoff",
      "init_module", "finit_module", "delete_module",
      "kexec_load", "kexec_file_load",
      "ptrace", "process_vm_writev", "process_vm_readv",
      "syslog", "setdomainname", "sethostname",
      "reboot", "iopl", "ioperm",
      "perf_event_open", "bpf",
      "userfaultfd", "clone3", "io_uring_setup",
      "pivot_root", "open_tree", "move_mount",
      "fsopen", "fsmount", "fsconfig", "fspick",
      "setns", "unshare"
    ]}
  ]
}
```

This denies the most common sandbox-escape vectors (kernel module
load, ptrace ATTACH, reboot, kexec) and namespace manipulation
syscalls that could circumvent the bwrap mount namespace. Tools
like `cargo`, `pytest`, `npm`, and standard shell commands stay
fully functional under this filter.

### Architecture caveat

`seccompiler-bin --target-arch` must match the runner's `uname
-m`. A policy compiled for `x86_64` will be rejected at load time
on `aarch64` because the syscall numbers differ. If you need both
architectures, compile two policies and pick at startup:

```sh
case "$(uname -m)" in
  x86_64)  export LIBRA_SECCOMP_POLICY="$HOME/.libra/seccomp-x86_64.bpf" ;;
  aarch64) export LIBRA_SECCOMP_POLICY="$HOME/.libra/seccomp-aarch64.bpf" ;;
esac
```

## Precedence

When both an in-process `SandboxRuntimeConfig::seccomp_policy_path`
AND `LIBRA_SECCOMP_POLICY` are set, the in-process value wins.
This lets a test or a CI harness pin a specific policy without
the developer's local env override interfering.

The runtime falls back to the env var only when the in-process
field is `None` (which it is for every default code path).

## Observability

When the policy file fails to open inside the child, the
`pre_exec` hook returns the underlying `std::io::Error` to bwrap,
which then exits with a non-zero status and stderr like:

```
bwrap: fcntl: Bad file descriptor
# or
bwrap: open seccomp policy: No such file or directory
```

These surface to the AI agent through the normal sandbox error
channel; the higher-level
`crate::internal::ai::sandbox::evidence::SandboxEvidenceEvent`
sink will eventually grow a `SeccompLoadFailed` variant once the
proxy-backend / failure-paths sweep lands.

## Disabling

`unset LIBRA_SECCOMP_POLICY` (or `export LIBRA_SECCOMP_POLICY=""`)
reverts to the no-seccomp behaviour. The bwrap arg vector drops
`--seccomp 200` and the `pre_exec` hook is not installed.
