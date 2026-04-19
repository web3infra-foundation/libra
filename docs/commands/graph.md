# `libra graph`

Inspect a Libra Code thread as an interactive version graph.

## Synopsis

```bash
libra graph <THREAD_ID> [--repo <PATH>]
```

## Description

`libra graph` opens a dedicated TUI for a canonical Libra Code thread. It reads the AI projection tables and formal AI history under `.libra/`, then renders the thread's version chain as a graph of:

- Intent revisions
- Execution plans
- Tasks
- Runs
- PatchSets

The graph highlights current/latest intent heads, selected plan heads, active tasks/runs, latest runs, and latest patchsets when that projection data is available.

The Details pane shows both the projection links for the selected graph node and the persisted AI object content loaded from history, including a bounded pretty-printed JSON view of the corresponding `intent`, `plan`, `task`, `run`, or `patchset` object.

After `libra code` exits, it prints a follow-up command in this form:

```bash
libra graph 11111111-1111-4111-8111-111111111111
```

## Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `<THREAD_ID>` | yes | Canonical Libra thread UUID to inspect. |

## Options

| Option | Description |
|--------|-------------|
| `--repo <PATH>` | Inspect a specific Libra repository instead of discovering one from the current directory. |

## TUI Controls

| Key | Action |
|-----|--------|
| Up / Down | Select previous or next graph node. |
| PageUp / PageDown | Scroll the Details pane by one visible page. |
| Home / End | Jump to the first or last graph node. |
| `[` / `]` | Scroll the Details pane by one line. |
| `q`, Esc, Ctrl-C | Exit the graph TUI. |

## Common Commands

```bash
# Open the version graph for a thread
libra graph 11111111-1111-4111-8111-111111111111

# Open the graph for a thread in another working tree
libra graph 11111111-1111-4111-8111-111111111111 --repo /path/to/repo

# Resume the same thread in Code after inspection
libra code --resume 11111111-1111-4111-8111-111111111111
```

## Output

`libra graph` is an interactive TUI command and does not produce line-oriented stdout. Run it from an interactive terminal. The command exits with a usage error if the thread ID is not a UUID, and with a repository/projection error if neither the current directory nor `--repo` resolves to a Libra repository, or if the requested thread cannot be found.

## Design Notes

The graph uses Libra's projection read model instead of parsing TUI session JSON directly. That keeps the view provider-neutral: generic LLM sessions and managed Codex sessions can both be inspected as long as they have formal AI history.

The command accepts the canonical Libra thread ID, not a provider-specific session ID. `libra code` prints the canonical command after exit when it can derive one from session metadata, the Code UI projection, or the latest formal AI history in the repository.
