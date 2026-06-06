# `libra open`

Resolve a remote URL into a web URL and optionally launch the system browser.

## Synopsis

```
libra open [<remote>] [-b <branch> | -c <commit> | --issue[=<id>] | --pr[=<id>]]
```

## Description

`libra open` determines the web-browsable URL for a repository and, in human-output
mode, opens it in the default system browser. The command accepts an optional
positional argument that can be either a configured remote name (e.g. `origin`) or a
direct URL.

When no argument is given, the command tries the following in order:
1. The current branch's configured upstream remote.
2. A remote named `origin`.
3. The first configured remote (alphabetically).

If the resolved URL uses SSH or SCP syntax (`git@host:path` or `ssh://...`), it is
automatically transformed to an HTTPS URL. The final URL is validated to ensure it
uses `http://` or `https://` before being passed to the OS browser launcher. This
prevents local file access, `javascript:`, or other injection vectors.

On macOS the command uses `open`, on Linux `xdg-open`, and on Windows `cmd /C start`.

## Options

| Flag / Argument | Description | Example |
|-----------------|-------------|---------|
| `<remote>` | Remote name or direct URL. When omitted, auto-detects from tracking config or `origin`. | `libra open origin` |
| `-b`, `--branch <NAME>` | Open the branch page (`/tree/<name>`). | `libra open -b main origin` |
| `-c`, `--commit <HASH>` | Open the commit page. Accepts a full or short hash (`[0-9a-fA-F]`, 4–64 chars); not required to exist locally so remote-only commits can be opened. | `libra open -c a1b2c3d origin` |
| `-i`, `--issue[=<ID>]` | Open the issues list, or a specific issue with `--issue=<ID>`. Must use the `=` form for the ID. | `libra open --issue=42 origin` |
| `-p`, `--pr[=<ID>]` | Open the pull-request list (GitHub `/pulls`, GitLab `/merge_requests`), or a specific PR/MR with `--pr=<ID>`. | `libra open --pr=7 origin` |
| `--json` | Emit structured JSON envelope to stdout instead of opening a browser (global flag). | `libra open --json` |
| `--machine` | Compact single-line JSON without launching a browser (global flag). | `libra open --machine` |
| `--quiet` | Suppress the "Opening ..." message on stdout. | `libra open --quiet` |
| `--print-only` | Print the resolved URL to stdout without opening the browser. | `libra open --print-only` |

The four target flags (`--branch`, `--commit`, `--issue`, `--pr`) are **mutually
exclusive**; supplying more than one is a usage error (exit `129`,
`LBR-CLI-002`). With no target flag the repository root is opened, regardless of
the current branch, so the default behaviour is unchanged. `--issue` / `--pr`
require the `=` form for an ID (`--issue=42`); a bare `--issue` opens the list,
and `libra open --issue origin` keeps `origin` as the remote.

### Deep-link targets (Libra extension)

`libra open` can jump straight to a branch, commit, issue, or pull-request page.
Per-component inputs are whitelist-validated **before** any URL is assembled:
branch names allow only `[A-Za-z0-9._/-]` (and reject path-traversal shapes such
as `..`, leading/trailing `/`, and `//`); commit hashes allow only hex digits
(length 4–64); issue/PR IDs allow only digits (a leading `#` is stripped). Any
input outside these sets is rejected with `LBR-CLI-003` (exit `129`) before the
browser is launched — shell metacharacters never reach the OS launcher.

### Platform adaptation & URL templates (Libra extension)

The page path differs per host (GitHub `/commit/`, GitLab `/-/commit/`,
Bitbucket `/commits/`, …). The platform is auto-detected from the host name and
can be overridden with the local config key `open.platform`
(`github`/`gitlab`/`gitea`/`bitbucket`/`custom`, case-insensitive). An
unrecognised value warns and falls back to host detection.

When `open.platform = custom`, the per-kind templates `open.template.branch`,
`open.template.commit`, `open.template.issue`, and `open.template.pull_request`
are consulted. Each value may contain the placeholders `{base_url}`, `{branch}`,
`{commit}`, `{issue}`, and `{pr}`. A template missing its value placeholder (or
otherwise malformed) is ignored and GitHub-style assembly is used instead — the
command never fails because of a bad template. Configuration is read only from
the **current local repository** (no global cascade in this release); a direct
URL run outside a repository reads no config and detects the platform from the
host alone.

```bash
libra config open.platform custom
libra config open.template.commit "{base_url}/commit-detail/{commit}"
libra open -c deadbeef origin   # -> https://<host>/<repo>/commit-detail/deadbeef
```

## Common Commands

```bash
libra open
libra open origin
libra open https://github.com/web3infra-foundation/libra
libra open -b main origin
libra open -c a1b2c3d origin
libra open --issue=42 origin
libra open --pr=7 origin
libra open --json
libra open --print-only
libra open origin --print-only
```

## Human Output

```text
Opening https://github.com/web3infra-foundation/libra
```

`--quiet` suppresses `stdout`.

## Structured Output (JSON examples)

```json
{
  "ok": true,
  "command": "open",
  "data": {
    "remote": "origin",
    "remote_url": "git@github.com:web3infra-foundation/libra.git",
    "web_url": "https://github.com/web3infra-foundation/libra",
    "launched": false,
    "target_type": "repo",
    "platform": "github"
  }
}
```

With a deep-link target flag (e.g. `-b dev`), `web_url` carries the assembled
sub-page and `target_type` reflects the kind:

```json
{
  "ok": true,
  "command": "open",
  "data": {
    "remote": "origin",
    "remote_url": "git@github.com:web3infra-foundation/libra.git",
    "web_url": "https://github.com/web3infra-foundation/libra/tree/dev",
    "launched": false,
    "target_type": "branch",
    "platform": "github"
  }
}
```

When the argument is a direct URL instead of a remote name, `remote` is `null` and `resolved_from_remote` is `false`:

```json
{
  "ok": true,
  "command": "open",
  "data": {
    "remote": null,
    "remote_url": "https://github.com/web3infra-foundation/libra",
    "web_url": "https://github.com/web3infra-foundation/libra",
    "launched": false,
    "resolved_from_remote": false
    "target_type": "repo",
    "platform": "github"
  }
}
```

### Schema Notes

- `remote` is the logical remote name, or `null` when a direct URL was provided
- `remote_url` is the raw URL from config (or the direct URL argument)
- `web_url` is the transformed browsable HTTPS URL (including any deep-link sub-page)
- `launched` is `true` when the browser was successfully spawned in human mode
- `launched` is `false` for `--json` / `--machine`, where browser launch is intentionally skipped
- `target_type` is one of `repo` / `branch` / `commit` / `issue` / `pull_request`
- `platform` is the resolved platform name (`github` / `gitlab` / `gitea` / `bitbucket` / `custom`)

> **Compatibility:** `target_type` and `platform` are **appended** to the
> existing field set (additive schema). The original four fields keep their
> names and order, so existing consumers that read known keys are unaffected.

### URL Transformation Rules

| Input Format | Transformed Output |
|-------------|-------------------|
| `https://github.com/user/repo.git` | `https://github.com/user/repo` |
| `http://github.com/user/repo.git` | `http://github.com/user/repo` |
| `git@github.com:user/repo.git` (SCP) | `https://github.com/user/repo` |
| `ssh://git@github.com/user/repo.git` | `https://github.com/user/repo` |
| `ssh://user@host.com:2222/repo.git` | `https://host.com/repo` |

## Design Rationale

### Why support direct URLs?

The primary use case for `libra open` is quickly jumping to a repository's web interface.
Sometimes a developer or agent has a URL from a chat message, issue tracker, or log output
and wants to open it without first configuring a remote. Accepting direct URLs alongside
remote names makes the command a universal "open this repo in the browser" tool. If the
argument matches a configured remote name, that takes precedence; otherwise it is treated
as a literal URL. This dual-mode behavior eliminates a common friction point without
adding complexity.

### Why not just use `git web--browse`?

`git web--browse` is an internal Git helper that launches a browser but has several
limitations: it does not transform SSH/SCP URLs to HTTPS, it does not validate URL
safety, and it requires the `instaweb` or `browse` helpers to be configured. Libra's
`open` command handles the full URL transformation pipeline (SCP to HTTPS, SSH to HTTPS,
`.git` suffix stripping) and validates that the final URL uses a safe scheme before
passing it to the OS launcher. This makes it work out-of-the-box for all common remote
URL formats without additional configuration.

### Why URL safety validation?

When a remote URL is transformed and passed to an OS command (`open`, `xdg-open`,
`cmd /C start`), there is a risk of command injection or unintended file access if the
URL uses a scheme like `file://`, `javascript:`, or contains shell metacharacters. Libra
validates that the final URL uses only `http://` or `https://` before launching the
browser. On Windows, the URL is additionally quoted to prevent `cmd.exe` metacharacter
expansion. This defense-in-depth approach protects against both accidental misconfiguration
and deliberate attacks via crafted remote URLs.

## Parameter Comparison: Libra vs Git vs jj

| Feature | Libra | Git | jj |
|---------|-------|-----|----|
| Open repo in browser | `libra open` | `git web--browse` (manual) | N/A |
| Open specific remote | `libra open origin` | N/A | N/A |
| Open direct URL | `libra open <url>` | N/A | N/A |
| SSH-to-HTTPS transform | Automatic | N/A | N/A |
| SCP-to-HTTPS transform | Automatic | N/A | N/A |
| URL safety validation | http/https only | N/A | N/A |
| Branch / commit deep link | `-b` / `-c` | N/A | N/A |
| Issue / PR deep link | `--issue` / `--pr` | N/A | N/A |
| Multi-platform paths | github/gitlab/gitea/bitbucket + `open.platform` | N/A | N/A |
| Custom URL templates | `open.template.<kind>` | N/A | N/A |
| Structured output | `--json` / `--machine` | No | No |
| Print-only mode | `--print-only` | No | No |
| Auto-detect remote | Tracking -> origin -> first | N/A | N/A |

## Error Handling

| Scenario | StableErrorCode | Exit | Hint |
|----------|-----------------|------|------|
| Not in a repo and no explicit URL | `LBR-REPO-001` | 128 | "run this command inside a libra repository, or pass a URL" |
| No remote configured | `LBR-REPO-003` | 128 | "add a remote first: 'libra remote add origin \<url>'" |
| Remote configured but has no URL | `LBR-REPO-003` | 128 | "configure the URL: 'libra config set remote.\<name>.url \<url>'" |
| Resolved URL is unsafe or invalid | `LBR-CLI-003` | 129 | "pass an explicit https:// URL or configure a supported remote URL" |
| Malicious / invalid branch, commit, or issue/PR id | `LBR-CLI-003` | 129 | "pass an explicit https:// URL or configure a supported remote URL" |
| More than one target flag (`-b`/`-c`/`--issue`/`--pr`) | `LBR-CLI-002` | 129 | -- |
| Failed to read remote config | `LBR-IO-001` | 128 | -- |
| Failed to launch browser (IO error other than missing launcher) | `LBR-IO-002` | 128 | "check that a default browser is configured" |

> When the browser launcher binary itself is missing (e.g. no `xdg-open` on a
> headless host), `libra open` does **not** fail: it prints the URL to stderr
> for manual copy and exits `0`.
