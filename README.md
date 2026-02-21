# Libra

Libra is a partial implementation of a **Git** client, developed in **Rust**.

The goal is **not** to build a perfect, 100% feature-complete reimplementation of Git (if you want that, take a look at [gitoxide](https://github.com/Byron/gitoxide)). Instead, Libra is evolving into an **AI agent–native version control system**.

The `libra code` command starts an interactive TUI (with a background web server) that is designed to be driven collaboratively by AI agents and humans.

---

## Example

```bash
$ libra --help
Simulates git commands

Usage: libra <COMMAND>

Commands:
  init     Initialize a new repository
  clone    Clone a repository into a new directory
  code     Start Libra Code interactive TUI (with a background web server)
  add      Add file contents to the index
  rm       Remove files from the working tree and from the index
  restore  Restore working tree files
  status   Show the working tree status
  log      Show commit logs
  diff     Show changes between commits, commit and working tree, etc
  branch   List, create, or delete branches
  commit   Record changes to the repository
  switch   Switch branches
  merge    Merge changes
  push     Update remote refs along with associated objects
  fetch    Download objects and refs from another repository
  pull     Fetch from and integrate with another repository or a local branch
  remote   Manage set of tracked repositories
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

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

To use Libra with Claude Desktop, add the following to your `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "libra": {
      "command": "/path/to/libra",
      "args": ["code", "--stdio"]
    }
  }
}
```

---

## Features

### Clean Code

The codebase is designed to be clean and easy to read, making it maintainable and approachable for developers of all skill levels.

### Cross-Platform

- [x] Windows  
- [x] Linux  
- [x] macOS

### Compatibility with Git

Libra’s core implementation is essentially compatible with **Git** (developed with reference to Git’s own documentation), including support for on-disk formats such as:

- `objects`
- `index`
- `pack`
- `pack-index`

This allows Libra to interact seamlessly with Git servers (for example, `push` and `pull` work with standard Git remotes).

### Differences from Git

While maintaining compatibility with Git, Libra intentionally diverges in some areas:

- Uses an **SQLite** database to manage loosely structured files such as `config`, `HEAD`, and `refs`, providing unified and transactional management instead of plain-text files.

---

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
- `libra worktree repair` – repair inconsistent worktree state if the registry and directories get out of sync

---

## Object Storage Configuration

Libra supports using S3-compatible object storage (AWS S3, Cloudflare R2, MinIO, etc.) as an alternative or supplement to local storage.  
This feature implements a **tiered storage architecture**:

- **Small objects** (< threshold) – stored in both local and remote storage
- **Large objects** (≥ threshold) – stored in remote storage with a local LRU cache

If `LIBRA_STORAGE_TYPE` is **not** set, Libra falls back to local-only storage under `.libra/objects`.

### Environment Variables

Configure object storage by setting these environment variables:

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

> Note: If any mandatory variable is invalid or empty (for example, empty bucket or credentials), Libra automatically falls back to local storage and logs an error message.

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

### Buck2 Build Requirements

This project builds with Buck2. Please install both Buck2 and `cargo-buckal` before development:

```bash
# Install buck2: download the latest release tarball from
# https://github.com/facebook/buck2/releases, extract the binary,
# and place it in ~/.cargo/bin (ensure ~/.cargo/bin is on PATH).
# Example (replace <tag> and <platform> with the latest for your OS):
wget https://github.com/facebook/buck2/releases/download/<tag>/buck2-<platform>.tar.gz
tar -xzf buck2-<platform>.tar.gz
mv buck2 ~/.cargo/bin/

# Install cargo-buckal (requires Rust toolchain)
cargo install --git https://github.com/buck2hub/cargo-buckal.git
```

Pull Requests must also pass the Buck2 build:

```bash
cargo buckal build
```

When you update dependencies in `Cargo.toml`, regenerate Buck metadata and third-party lockfiles:

```bash
cargo buckal migrate
```
