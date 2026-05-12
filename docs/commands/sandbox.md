# `libra sandbox`

Inspect AI sandbox diagnostics for the current machine.

## Synopsis

```bash
libra sandbox status
libra --json sandbox status
```

## Description

`libra sandbox status` reports the sandbox backend Libra would use for AI shell
execution diagnostics. It does not require a repository, so it can be used while
debugging provider or CI hosts before running `libra code`.

The default runtime is best-effort: Linux uses the external helper configured by
`LIBRA_LINUX_SANDBOX_EXE`, macOS uses Seatbelt when `/usr/bin/sandbox-exec` is
available, and unsupported or unconfigured hosts report warnings instead of
claiming isolation. Set `LIBRA_SANDBOX_ENFORCEMENT=required` to fail commands
that request Libra's internal sandbox when no supported backend can be applied.

## Human Output

```text
Sandbox status
  platform: linux
  sandbox_type: none
  enforcement: best_effort
  network: denied
  proxy_backend: none
  bwrap_available: false
  bwrap_requested: false
  seatbelt_available: false
  helper_path: (not configured)
  helper_path_exists: false
  writable_roots:
    - /path/to/workspace
  warnings:
    - linux sandbox helper is not configured; AI shell commands currently fall back to no OS sandbox
```

## JSON Output

```json
{
  "ok": true,
  "command": "sandbox.status",
  "data": {
    "platform": "linux",
    "sandbox_type": "none",
    "enforcement": "best_effort",
    "writable_roots": ["/path/to/workspace"],
    "network": {
      "mode": "denied",
      "allowlist": []
    },
    "proxy_backend": "none",
    "bwrap_available": false,
    "bwrap_requested": false,
    "seatbelt_available": false,
    "helper_path": {
      "path": null,
      "exists": false
    },
    "warnings": []
  }
}
```

## Fields

| Field | Description |
|-------|-------------|
| `platform` | Rust target OS for the running Libra binary |
| `sandbox_type` | Effective OS sandbox backend, or `none` when no backend is currently usable |
| `enforcement` | Current enforcement policy from `LIBRA_SANDBOX_ENFORCEMENT`; `required` rejects missing internal sandboxes, while `best_effort` reports downgrade risk without failing commands |
| `writable_roots` | Default workspace-write roots after resolving the current directory and temporary directories |
| `network.mode` | Current network policy summary; the default policy is `denied` |
| `network.allowlist` | Reserved for the planned network allowlist model; currently empty |
| `proxy_backend` | Reserved for the planned network proxy; currently `none` |
| `bwrap_available` | Whether `bwrap` is executable on `PATH` |
| `bwrap_requested` | Whether `LIBRA_USE_LINUX_SANDBOX_BWRAP` is enabled |
| `seatbelt_available` | Whether `/usr/bin/sandbox-exec` is executable |
| `helper_path` | `LIBRA_LINUX_SANDBOX_EXE` path and executable probe result |
| `warnings` | Downgrade or unsupported-platform diagnostics |
