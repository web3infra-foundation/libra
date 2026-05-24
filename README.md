![Libra](docs/image/banner.png)

Libra is a partial implementation of a **Git** client, developed in **Rust**. The goal is **not** to build a perfect, 100% feature-complete reimplementation of Git (if you want that, take a look at [gitoxide](https://github.com/Byron/gitoxide)). Instead, Libra is evolving into an **AI agent–native version control system**.

The `libra code` command starts an interactive TUI (with a background web server and an MCP stdio surface) that is designed to be driven collaboratively by AI agents and humans. Libra also ships AI-native subcommands not found in Git: `code-control`, `automation`, `agent`, `usage`, `graph`, `sandbox`, and `publish`.

---

# AI Features

The AI surface is what makes Libra different from a vanilla Git client. The sections below cover where AI data lives, how to drive the AI runtime (`libra code`), which providers are supported, and the Libra-only subcommands that orchestrate the agent.

## AI Data Storage

Libra persists AI threads, runs, tasks, decisions, validation reports, tool-invocation events, patchset snapshots, automation history, captured external-agent sessions, and the live context window into the same repository storage directory that holds Git objects. Everything an AI agent does inside a Libra repository is durable, queryable, and replayable — no out-of-band state.

### Repository Layout (`.libra/`)

```
.libra/
├── libra.db              # SQLite — Git core + AI threads + AI runtime contracts
├── vault.db              # libvault — encrypted secrets (signing keys, provider creds)
├── objects/              # Local object store (loose + pack) — used when no remote backend is configured
├── sessions/             # JSONL session store for AI conversations and file history
└── ai/                   # Working files written by the AI runtime
```

If `--separate-libra-dir <dir>` is passed to `libra init`, the entire storage directory is relocated; the working tree only keeps a pointer file.

### SQLite Schema Groups

The single `libra.db` carries three logical schema groups (canonical bootstrap files: `sql/sqlite_20260309_init.sql` and `sql/sqlite_20260415_ai_runtime_contract.sql`; versioned forward + `_down.sql` migrations live in `sql/migrations/`):

| Group | Tables |
|-------|--------|
| Git core | `config`, `config_kv`, `reference`, `reflog`, `rebase_state`, `object_index`, `schema_version` |
| AI threads & scheduling | `ai_thread`, `ai_thread_participant`, `ai_thread_intent`, `ai_thread_provider_metadata`, `ai_scheduler_state`, `ai_scheduler_plan_head`, `ai_scheduler_selected_plan`, `ai_live_context_window` |
| AI runtime contracts | `ai_index_intent_plan`, `ai_index_intent_task`, `ai_index_intent_context_frame`, `ai_index_plan_step_task`, `ai_index_run_event`, `ai_index_run_patchset`, `ai_index_task_run`, `ai_decision_proposal`, `ai_risk_score_breakdown`, `ai_validation_report` |

The publish Worker uses its own D1 schema in `sql/publish/` (independent from `libra.db`).

### Inspecting AI Data

Every AI record is addressable via `cat-file`'s AI selectors. Pair with `--json=pretty` for machine-readable output:

```bash
libra cat-file --ai-list ai_session                       # List captured AI sessions
libra cat-file --ai-list run                              # Runs (one per agent invocation)
libra cat-file --ai-list task                             # Tasks within a plan
libra cat-file --ai-list tool_invocation_event            # Every tool call the agent issued
libra cat-file --ai-list patchset_snapshot                # Patchset diffs proposed by the agent
libra --json=pretty cat-file --ai ai_session:<ai_session_id>

libra graph <thread_id> [--repo /path/to/repo]            # TUI: navigate an AI thread version graph
libra usage                                               # Token + cost summary per provider/model
libra --json=pretty usage
libra db status                                           # Schema version and migration status
```

### Tiered Object Storage (S3 / R2 / MinIO)

Libra can offload large objects (commits, blobs, packs, AI patchset snapshots) to S3-compatible object storage while keeping a local LRU cache. Tiering rules:

- **Small objects** (`< LIBRA_STORAGE_THRESHOLD`) — stored in both local and remote storage.
- **Large objects** (`≥ LIBRA_STORAGE_THRESHOLD`) — stored remotely with a local LRU cache.

If `LIBRA_STORAGE_TYPE` is not set, Libra uses local-only storage under `.libra/objects`.

| Variable                     | Description                                                   | Required (for S3/R2) | Default              |
|-----------------------------|---------------------------------------------------------------|----------------------|----------------------|
| `LIBRA_STORAGE_TYPE`        | Storage backend type: `s3` or `r2`                            | Yes                  | –                    |
| `LIBRA_STORAGE_BUCKET`      | Bucket name                                                   | Yes                  | `libra`              |
| `LIBRA_STORAGE_ENDPOINT`    | S3-compatible endpoint URL (required for R2)                  | Yes (for R2)         | AWS S3 default       |
| `LIBRA_STORAGE_REGION`      | Region for bucket                                             | No                   | `auto`               |
| `LIBRA_STORAGE_ACCESS_KEY`  | Access key ID                                                 | Yes                  | –                    |
| `LIBRA_STORAGE_SECRET_KEY`  | Secret access key                                             | Yes                  | –                    |
| `LIBRA_STORAGE_THRESHOLD`   | Size threshold in bytes for tiering                           | No                   | `1048576` (1 MB)     |
| `LIBRA_STORAGE_CACHE_SIZE`  | Local cache size limit in bytes                               | No                   | `209715200` (200 MB) |
| `LIBRA_STORAGE_ALLOW_HTTP`  | Allow HTTP (non-TLS) endpoints for testing (not for prod)     | No                   | `false`              |

> If any mandatory variable is invalid or empty, Libra automatically falls back to local storage and logs an error.

### Cloud Backup & Restore (Cloudflare D1 + R2)

`libra cloud` backs up the full repository state — Git objects, refs, **and** all AI tables — to Cloudflare D1 (metadata) plus R2 (objects). This is the canonical way to move a Libra repository (including its AI history) between machines.

| Variable | Description | Required |
|----------|-------------|----------|
| `LIBRA_D1_ACCOUNT_ID` | Cloudflare Account ID | Yes |
| `LIBRA_D1_API_TOKEN` | Cloudflare API Token | Yes |
| `LIBRA_D1_DATABASE_ID` | Cloudflare D1 Database ID | Yes |

```bash
libra cloud sync                       # Sync local repository (incl. AI data) to D1/R2
libra cloud restore --name <NAME>      # Restore by project name
libra cloud restore --repo-id <ID>     # Restore by repo ID
libra cloud status                     # Show synchronization status

libra config cloud.name <my-unique-project-name>   # Override the default (directory name) project name
```

### Vault-Backed Secrets

`vault.db` (powered by [`libvault`](https://crates.io/crates/libvault)) stores AI provider keys, commit-signing GPG/SSH material, and arbitrary secrets used by the `automation` runtime. The unseal key is held outside the repository at `~/.libra/vault-keys/<repoid>`; the encrypted root token is recorded in repository config (`vault.roottoken_enc`).

Vault is enabled by default for every `libra init`; see [Vault-Backed Signing](#vault-backed-signing) for details.

---

## Libra Code Modes

Libra Code supports three operation modes, each designed for different use cases.

### 1. TUI Mode (Default)

Starts an interactive Terminal User Interface along with a background web server.
This is the standard mode for developers who want to work directly in the terminal with AI assistance.

```bash
libra code
```

- **Storage**: Uses the local project directory (`.libra/`) to isolate history and context per project.

### 2. Web Mode

Runs only the web server without the TUI.
Useful for remote development or when you prefer using the browser interface exclusively.

```bash
libra code --web
```

- **Storage**: Uses the local project directory (`.libra/`).

### 3. Stdio Mode (MCP)

Runs the Model Context Protocol (MCP) server over standard input/output.
This mode is designed for integration with AI clients like **Claude Desktop**.

```bash
libra code --stdio
```

- **Storage**: Uses the local project directory (`.libra/`) for history persistence (same as TUI/Web modes).
  The directory must be writable by the calling process (including sandboxed desktop AI apps).

#### Claude Desktop Configuration

To use Libra with Claude Desktop, you must configure the MCP server to run within a valid Libra repository.
Update your `claude_desktop_config.json` as follows:

```json
{
  "mcpServers": {
    "libra": {
      "command": "/path/to/libra",
      "args": ["code", "--stdio"],
      "cwd": "/path/to/your/libra/repo"
    }
  }
}
```

> **Note**: The `cwd` (current working directory) must be set to the root of a valid Libra repository.
> If `libra code` is launched outside of a repository, it will exit with an error.

#### Managed Runtime Migration

The legacy `claudecode` provider was removed. Use `libra code --provider codex`
for Libra's managed agent runtime, or `libra code --provider anthropic` for
direct Anthropic chat completions. Claude provider-session flags such as
`--resume-session`, `--fork-session`, `--session-id`, and `--resume-at` are no
longer accepted; use Libra's canonical `--resume <thread_id>` flow for persisted
sessions.

## AI Provider Selection

Libra Code supports multiple AI provider backends. Use the `--provider` and `--model` flags to choose which LLM to use:

```bash
# Gemini (default)
libra code --provider gemini
libra code --provider gemini --model gemini-2.5-flash

# OpenAI
libra code --provider openai --model gpt-4o

# Anthropic (direct chat completions)
libra code --provider anthropic --model claude-sonnet-4-6

# DeepSeek
libra code --provider deepseek
libra code --provider deepseek --model deepseek-v4-pro --deepseek-thinking enabled --deepseek-reasoning-effort high
libra code --provider deepseek --model deepseek-v4-pro --deepseek-thinking enabled --deepseek-reasoning-effort high --deepseek-stream true
libra code --env-file .env.test --provider deepseek --model deepseek-v4-pro --deepseek-thinking enabled --deepseek-reasoning-effort high --deepseek-stream true

# Kimi (Moonshot AI)
libra code --provider kimi
libra code --provider kimi --model kimi-k2.6
libra code --provider kimi --model kimi-k2.6 --kimi-thinking disabled
libra code --provider kimi --model moonshot-v1-128k

# Zhipu (GLM)
libra code --provider zhipu --model glm-5

# Ollama (local inference, no API key required, --model is required)
libra code --provider ollama --model llama3.2
libra code --provider ollama --model codellama

# Ollama with a remote instance
libra code --provider ollama --model llama3.2 --api-base http://remote-host:11434/v1
libra code --provider ollama --model minimax-m2.7:cloud --api-base http://remote-host:11434/v1 --ollama-compact-tools

# Ollama thinking control for reasoning models
OLLAMA_THINK=false libra code --provider ollama --model qwen3.6
libra code --provider ollama --model qwen3.6 --ollama-thinking high
```

> **Note**: The `--api-base` CLI flag is only honored for the `ollama` provider. Other providers accept custom base URLs through their respective environment variables (e.g. `OPENAI_BASE_URL`). Use `--env-file .env.test` to load provider keys from a dotenv-style file and override stale shell environment variables. DeepSeek reasoning fields are opt-in with `--deepseek-thinking enabled|disabled` and `--deepseek-reasoning-effort low|medium|high|max`; `xhigh` is accepted as an alias for `max`. DeepSeek streaming is opt-in with `--deepseek-stream true`; `--stream` is accepted as a DeepSeek-only alias. Kimi thinking can be overridden with `--kimi-thinking enabled|disabled`; omit it to use the selected model's default. Ollama requests stream `/api/chat` responses by default, include a per-request `request_id` in debug logs, and default to `think:false` to keep tool calls responsive; use `--ollama-thinking auto|off|on|low|medium|high` for one run, or set `OLLAMA_THINK=true`, `low`, `medium`, `high`, or `auto` as the environment default. `auto` omits the `think` field and lets Ollama decide. Use `--ollama-compact-tools` or `OLLAMA_COMPACT_TOOLS=true` for remote/cloud Ollama endpoints that return 503s when receiving Libra's full tool schemas.

| Provider | Default Model | Auth Env Variable | Base URL Override | Provider-specific Tuning |
|----------|--------------|-------------------|-------------------|-------------------------|
| `gemini` | `gemini-2.5-flash` | `GEMINI_API_KEY` | — | — |
| `openai` | `gpt-4o-mini` | `OPENAI_API_KEY` | `OPENAI_BASE_URL` | — |
| `anthropic` | `claude-sonnet-4-6` | `ANTHROPIC_API_KEY` | `ANTHROPIC_BASE_URL` | — |
| `deepseek` | `deepseek-chat` | `DEEPSEEK_API_KEY` | `--api-base` only (no env var) | `--deepseek-thinking`, `--deepseek-reasoning-effort`, `--deepseek-stream` |
| `kimi` | `kimi-k2.6` | `MOONSHOT_API_KEY` | `MOONSHOT_BASE_URL` | `--kimi-thinking` |
| `zhipu` | `glm-5` | `ZHIPU_API_KEY` | `ZHIPU_BASE_URL` | — |
| `ollama` | *(requires `--model`)* | `OLLAMA_API_KEY` for direct Cloud API | `OLLAMA_BASE_URL`, `--api-base` | `OLLAMA_THINK`, `OLLAMA_COMPACT_TOOLS`, `--ollama-thinking`, `--ollama-compact-tools` |

`libra code` tries the Brave Search API for the `web_search` tool when `BRAVE_SEARCH_API_KEY` is set in the process environment or stored as `vault.env.BRAVE_SEARCH_API_KEY`; if Brave is not configured or the request fails, it falls back to DuckDuckGo HTML search. The session network policy must still allow outbound access.

---

## AI-Native Extensions

These subcommands are Libra-only (not present in Git) and form the AI-agent surface around the Git core.

### `libra automation` — Rule-Based Automation

Run scheduled (cron-driven) or ad-hoc automation rules. Rules live in repository config; the runner enforces a command safety preflight before any live shell action is spawned. History is persisted into the AI tables so a previous run can be replayed.

```bash
libra automation list                                 # List configured rules
libra automation run                                  # Dry-run all due cron rules
libra automation run --rule my-rule                   # Force-run a single rule
libra automation run --now 2026-05-23T12:00:00Z       # Simulate "now" when evaluating cron triggers
libra automation run --live                           # Actually spawn shell actions (subject to preflight)
libra automation history --limit 50                   # Recent automation history
libra --json=pretty automation list                   # Structured JSON output for agents
```

### `libra agent` — External-Agent Capture

Capture sessions and checkpoints from external coding agents (Claude Code, Gemini, ...) into `refs/libra/agent-traces`. Useful for replaying agent transcripts and pushing traces to a shared remote so the team can audit what an external agent actually did.

```bash
libra agent status                                    # Captured-session counts and recent checkpoints
libra agent enable --agent claude                     # Install hooks for one agent
libra agent enable                                    # Enable every stable external agent
libra agent disable --agent claude
libra agent session list
libra agent checkpoint list
libra agent checkpoint show <id>
libra agent checkpoint rewind <id>                    # Replay as a JSONL transcript
libra agent clean [--all]                             # Drop temporary checkpoints from stopped sessions
libra agent doctor                                    # Diagnose hook installation and capture state
libra agent push [--remote origin]                    # Push refs/libra/agent-traces
libra agent rpc list                                  # Discover libra-agent-<name> RPC binaries on PATH
libra agent rpc invoke <slug> <method> --params '{}'
```

### `libra publish` — Read-Only Cloudflare Worker Publishing

Publish a snapshot of one or more refs to Cloudflare D1 (metadata) + R2 (objects) and serve them through a thin read-only Worker. Designed for sharing AI-generated artifacts or read-only mirrors of a repository.

```bash
libra publish init --slug <slug> --clone-domain <domain>   # Materialise the local Worker scaffold
libra publish status                                       # Inspect local template / D1 ref drift
libra publish status --site-id <uuid>
libra publish sync                                         # Sync default refs to D1/R2
libra publish sync --dry-run
libra publish sync --ref refs/heads/main
libra publish sync --force                                 # Re-upload everything, ignoring CAS
libra publish sync --allow-sensitive-path <path>           # Override the deny list for a private site
libra publish deploy                                       # Build and deploy the Worker
libra publish deploy --skip-deploy                         # Build only
libra publish unpublish --site-id <uuid> --yes
```

### `libra sandbox` — AI Sandbox Diagnostics

Inspect the command-safety sandbox used by AI tool execution: enforcement mode, network policy, seccomp/seatbelt status, and writable-root tmpdir layout. Every shell tool invoked by the AI runtime passes through this sandbox before it runs.

```bash
libra sandbox status
libra sandbox inspect --command "<cmd>"               # Dry-run the safety classifier on a command
```

Relevant environment toggles:

- `LIBRA_SANDBOX_ENFORCEMENT` — `disabled` / `warn` / `enforce`
- `LIBRA_SANDBOX_NETWORK_DISABLED` — block outbound network for sandboxed tools
- `LIBRA_LINUX_SANDBOX_EXE`, `LIBRA_USE_LINUX_SANDBOX_BWRAP` — Linux bwrap integration
- `LIBRA_SECCOMP_POLICY` — override the bundled `template/seccomp-default.json` allow-list

### `libra code-control` and `libra graph`

`code-control` drives an existing local `libra code` TUI from another process via a lease-based automation API — useful for AI-agent-in-the-loop scripts. `graph` opens a TUI for inspecting an AI thread's version graph.

```bash
libra code-control --help
libra graph <thread_id> [--repo /path/to/repo]
```

### `libra usage` — AI Provider/Model Usage

Report token usage and cost across providers and models, persisted by the AI runtime.

```bash
libra usage                                           # Summary across all providers
libra --json=pretty usage                             # Structured JSON output
```

---

## Optional FUSE Backend

Libra Code can use a FUSE overlay backend for temporary Agent task worktrees on
Unix platforms. This backend is optional: if FUSE is unavailable or fails its
health check, Libra logs a warning and falls back to the copy backend.

### macOS

Install [macFUSE](https://macfuse.github.io/) before using the FUSE backend.
The upstream project recommends downloading the latest installer from the
[macFUSE website or GitHub releases](https://github.com/macfuse/macfuse/wiki/Getting-Started).

Homebrew can also install the cask for development machines:

```bash
brew install --cask macfuse
```

Follow any macOS System Settings prompts to allow macFUSE if required by the
selected backend. After installation, verify that the mount helper expected by
Libra's FUSE library exists:

```bash
test -x /Library/Filesystems/macfuse.fs/Contents/Resources/mount_macfuse
```

If Libra logs `macfuse mount binary not found`, macFUSE is not installed or the
mount helper is not present at the expected path. Install or repair macFUSE, then
start `libra code` again.

### Linux

Install FUSE 3 with your distribution package manager. Common package names are:

```bash
# Debian / Ubuntu
sudo apt-get update
sudo apt-get install -y fuse3

# Fedora / RHEL
sudo dnf install fuse3

# Arch Linux
sudo pacman -S fuse3
```

Verify that `fusermount3` is available:

```bash
command -v fusermount3
```

If the command is missing, Libra cannot use the unprivileged FUSE mount path and
will use the copy backend instead.

---

# Git-Compatible Features

Libra's Git surface stays compatible enough to fetch from / push to standard Git servers (GitHub, Gitea, …). The per-command compatibility status (`supported` / `partial` / `unsupported` / `intentionally-different`) is tracked in [`COMPATIBILITY.md`](COMPATIBILITY.md).

## Features

### Clean Code

The codebase is designed to be clean and easy to read, making it maintainable and approachable for developers of all skill levels.

### Cross-Platform

- [x] Windows
- [x] Linux
- [x] macOS

### Compatibility with Git

Libra's core implementation is essentially compatible with **Git** (developed with reference to Git's own documentation), including support for on-disk formats such as:

- `objects`
- `index`
- `pack`
- `pack-index`

This allows Libra to interact seamlessly with Git servers (for example, `push` and `pull` work with standard Git remotes).

### Differences from Git

While maintaining compatibility with Git, Libra intentionally diverges in some areas:

- Uses an **SQLite** database to manage loosely structured files such as `config`, `HEAD`, and `refs`, providing unified and transactional management instead of plain-text files.
- Records AI threads, runs, decisions, and patchset snapshots in the same SQLite database (see [AI Data Storage](#ai-data-storage)).
- Object storage can be tiered into S3/R2; backups go to Cloudflare D1/R2.

## grep

`grep` searches tracked working-tree files, the index (`--cached`), or committed trees (`--tree <revision>`) using regular expressions by default. It also supports fixed-string mode, multiple explicit patterns, pattern files, and requiring all patterns to match within the same file.

```bash
# Search tracked working-tree files with a regex
libra grep "foo.*bar"

# Search with multiple explicit patterns
libra grep -e alpha -e beta

# Require every pattern to appear in the same file
libra grep --all-match -e alpha -e beta

# Search the staged/index version of tracked files
libra grep --cached "needle"

# Search a specific revision or branch
libra grep --tree HEAD "needle"
libra grep --tree main "needle"

# Read patterns from a file
libra grep -f patterns.txt
```

## Bisect — Binary Search for Bugs

Libra implements a `bisect` subcommand that uses binary search to find the commit that introduced a bug. It is broadly compatible with `git bisect`.

### Basic Usage

```bash
# Start a bisect session
libra bisect start

# Mark the current commit as bad (contains the bug)
libra bisect bad

# Mark a known-good commit
libra bisect good <commit>

# After marking, bisect will checkout commits for you to test
# Continue marking commits as good/bad until the culprit is found

# End the session and restore your original HEAD
libra bisect reset
```

### Quick Start with Known Bounds

```bash
# Start with both bad and good commits specified
libra bisect start HEAD~10 HEAD~20  # HEAD~10 is bad, HEAD~20 is good
```

### Subcommands

- `libra bisect start [<bad> [<good>]]` – start a new bisect session
- `libra bisect bad [<rev>]` – mark a commit as bad (contains the bug)
- `libra bisect good [<rev>]` – mark a commit as good (bug-free)
- `libra bisect skip [<rev>]` – skip the current commit (untestable)
- `libra bisect reset [<rev>]` – end the session and restore original HEAD
- `libra bisect log` – show the current bisect state

### Safety Features

Libra's bisect implementation includes several safety guards:

- **Clean working tree required**: Bisect will not start if you have uncommitted changes (including ignored files like `.env`)
- **Bare repository protection**: Bisect is blocked in bare repositories (no working tree)
- **State preserved until reset**: After finding the culprit, bisect state is preserved so you can run `bisect reset` to restore your original branch
- **Branch restoration**: `bisect reset` restores you to your original branch, not a detached HEAD

## Worktree Management

Libra implements a `worktree` subcommand that is broadly compatible with `git worktree`, allowing you to manage multiple working directories attached to the same repository storage.

Unlike `git worktree remove`, Libra does **not** delete worktree directories on disk by default.

Supported subcommands:

- `libra worktree add <path>` – create a new linked working tree at `<path>`
- `libra worktree list` – list all registered working trees (including the main worktree)
- `libra worktree lock <path> [--reason <msg>]` – mark a worktree as locked with an optional reason
- `libra worktree unlock <path>` – unlock a previously locked worktree
- `libra worktree move <src> <dest>` – move a worktree directory to a new location
- `libra worktree prune` – prune missing or non-existent worktrees from the registry
- `libra worktree remove <path>` – remove a worktree from the registry without deleting its directory on disk (the main worktree cannot be removed)
- `libra worktree umount <path> [--cleanup]` – unmount a FUSE worktree or stale Agent task worktree mountpoint
- `libra worktree repair` – repair inconsistent worktree state if the registry and directories get out of sync

## Vault-Backed Signing

Libra supports repository-local vault initialization for commit signing:

```bash
libra init [--separate-libra-dir <dir>] [<repo_directory>]
```

Vault is enabled by default for all `libra init` invocations — no extra flag is needed.

When vault is enabled:

- A vault database (`vault.db`) is created in the repository storage directory (`.libra/` or the directory passed via `--separate-libra-dir`).
- Libra generates a signing key and enables `vault.signing=true`.
- The vault unseal key is stored outside the repository at `~/.libra/vault-keys/<repoid>`.
- The encrypted root token is stored in repository config (`vault.roottoken_enc`).

Security note:

- Libra no longer falls back to storing the unseal key inside repository config.
- If the home directory is not writable/usable, `libra init` fails with a fatal error.

Troubleshooting:

- Ensure `HOME` (or `USERPROFILE` on Windows) points to a writable directory.
- In container/CI environments, explicitly set `HOME` to a writable path before running `libra init`.

Key management commands:

```bash
# Print current signing GPG public key (for GitHub GPG key settings)
libra config get vault.gpg.pubkey

# Generate a repo-local SSH key for a configured remote
libra config generate-ssh-key --remote origin

# Print the SSH public key for a configured remote
libra config get vault.ssh.origin.pubkey

# Generate (or rotate) vault GPG signing key and print public key
libra config generate-gpg-key [--name <user>] [--email <mail>]
```

See `docs/commands/config.md` for the full `libra config` command reference and migration notes.

### GitHub End-to-End Verification (libvault + Git conversion)

The following flow validates:

- `libvault` integration with Libra storage (`.libra/vault.db` + config metadata in SQLite)
- Conversion from Git repository format to Libra repository format
- Vault-backed GPG signing on commit
- SSH push from Libra to GitHub

```bash
# 1) Clone an existing GitHub repository locally with Git (SSH).
#    (This step can use your existing SSH credential.)
git clone git@github.com:<owner>/<repo>.git /tmp/<repo>-git

# 2) Convert the cloned Git repository into a Libra repository and
#    initialize vault in the same command.
mkdir -p /tmp/<repo>-libra
cd /tmp/<repo>-libra
libra init --from-git-repository /tmp/<repo>-git

# 3) Export vault public keys and register them in GitHub settings:
#    - GPG key: GitHub -> Settings -> SSH and GPG keys -> New GPG key
#    - SSH key: GitHub -> Settings -> SSH and GPG keys -> New SSH key
libra config get vault.gpg.pubkey
libra config generate-ssh-key --remote origin
libra config get vault.ssh.origin.pubkey

# 4) Make sure origin points to GitHub SSH URL in Libra config.
libra remote set-url origin git@github.com:<owner>/<repo>.git

# 5) Create a signed commit and push through SSH.
echo "vault e2e" > vault-e2e.txt
libra add vault-e2e.txt
libra commit -m "feat(vault): verify signed commit to GitHub"
libra push origin master
```

Verification points:

- `libra commit` should produce a commit object containing `gpgsig`.
- `libra push` should succeed over SSH (`git@github.com:...`).
- The commit should appear in GitHub with signature metadata.

Note:

- For the very first `git clone` in step 1, Git may still use your existing SSH credentials.
  After step 3, Libra fetch/push uses the vault-generated key for this repository.

## 🚧 Pending Git commands (not yet supported)

The following Git top-level commands are currently **not implemented** in Libra (excluding `submodule` and `subtree`, which are intentionally omitted):

- `gc` – garbage-collect unreachable objects and pack files
- `prune` – remove loose objects that are no longer reachable
- `maintenance` – periodic maintenance tasks
- `pack-objects` / `unpack-objects` – pack and unpack object collections
- `remote-show` – show detailed remote info
- `fetch-pack` / `push-pack` – low-level fetch/push operations
- `filter-branch` (or `git filter-repo`) – rewrite history
- `notes` – attach arbitrary metadata to objects
- `archive` – create tar/zip archives of tree snapshots
- `rebase --autosquash` / `rebase --reapply-cherry-picks` – advanced rebase options

These commands are slated for future implementation according to the project roadmap. The full per-command compatibility status (`supported` / `partial` / `unsupported` / `intentionally-different`) is tracked in [`COMPATIBILITY.md`](COMPATIBILITY.md).

## Note on Submodule and Subtree

Libra does **not** provide the `submodule` or `subtree` commands. Because Libra stores objects in an S3-compatible backend and is designed around a **Monorepo** layout with **Trunk-based Development**, the use-cases that `git submodule`/`git subtree` address (embedding separate repositories) are handled differently – large external data lives in S3 and all code lives in a single repository.

This design choice simplifies dependency management and aligns with Libra's goal of supporting ultra-large repositories while keeping a single source of truth.

---

## Error Reporting

CLI failures use stable exit codes and stable error codes. When `stderr` is not a TTY, Libra also appends a JSON stderr report for agents and wrappers. Set `LIBRA_ERROR_JSON=1` to force that structured report in interactive terminals.
Run `libra help error-codes` for the built-in CLI reference.
See [docs/error-codes.md](docs/error-codes.md).

---

## Contributing & Development

Before submitting a Pull Request, please ensure your code passes the following checks:

```bash
# Run clippy with all warnings treated as errors
cargo clippy --all-targets --all-features -- -D warnings

# Check code formatting (requires nightly toolchain)
cargo +nightly fmt --all --check
```

Both commands must complete without any warnings. The clippy check treats all warnings as errors, and the formatter check ensures code follows the project style guide.

If the formatting check fails, you can automatically fix formatting issues by running:

```bash
cargo +nightly fmt --all
```

## Run on Windows

If you are building Libra on Windows for the first time, install OpenSSL before running
`cargo build` or `cargo test`. The easiest setup is to use a precompiled OpenSSL package:
<https://slproweb.com/products/Win32OpenSSL.html>

Recommended setup:

1. Install a 64-bit OpenSSL build that matches the default Rust Windows target
   `x86_64-pc-windows-msvc`.
2. Note the installation directory, for example `D:\OpenSSL-Win64`.
3. Create `.cargo/config.toml` in the project root if it does not already exist.
4. Add OpenSSL environment overrides so Cargo and dependent build scripts can find the
   headers and libraries.

Project layout:

```text
.cargo/
  config.toml
```

Example `.cargo/config.toml`:

```toml
[env]
OPENSSL_DIR = "D:\\OpenSSL-Win64"
OPENSSL_LIB_DIR = "D:\\OpenSSL-Win64\\lib\\VC\\static"
OPENSSL_INCLUDE_DIR = "D:\\OpenSSL-Win64\\include"
OPENSSL_NO_VENDOR = "1"
```

Notes:

- Update the paths if OpenSSL is installed in a different directory.
- If `.cargo/config.toml` already exists, merge these entries instead of replacing the file.
- Some OpenSSL installers place libraries in a different subdirectory. If `VC\\static` does
  not exist in your installation, point `OPENSSL_LIB_DIR` at the directory that contains the
  `.lib` files for your installation.
- After updating the config, open a new terminal and verify the setup with:

```bash
cargo build
```
