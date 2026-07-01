# `libra completions`

Generate a shell completion script for the `libra` CLI. This mirrors the
ergonomics of Lore's `completions` command and the `git completion` contrib
scripts, so shells can tab-complete Libra subcommands and flags.

## Synopsis

```
libra completions <shell>
```

`<shell>` is one of `bash`, `zsh`, `fish`, `powershell`, or `elvish`.

## Description

`completions` prints a completion script for the requested shell to stdout. The
script is generated from Libra's live clap command tree, so it always reflects
the current subcommand and flag surface without a hand-maintained table.

The command reads no repository state and works outside a repository.

Install by redirecting the script to the location your shell loads completions
from, or `eval` it into the current shell for a one-off session.

## Options

| Option | Description | Example |
|--------|-------------|---------|
| `<shell>` | Target shell: `bash`, `zsh`, `fish`, `powershell`, or `elvish`. | `libra completions zsh` |
| `--json` / `--machine` | Structured output: `{ shell, script }`. | `libra --json completions bash` |

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Completion script was written. |
| `129` | Unknown or missing shell argument (Git-style clap usage error). |

## Examples

```bash
# bash: install system-wide
libra completions bash | sudo tee /etc/bash_completion.d/libra >/dev/null

# zsh: drop into a directory on $fpath
libra completions zsh > ~/.zsh/completions/_libra

# fish
libra completions fish > ~/.config/fish/completions/libra.fish

# Load into the current shell without installing
eval "$(libra completions bash)"

# Structured output for tooling
libra --json completions bash
```

## Comparison with Git

Git does not ship a `git completions` subcommand; completion scripts live in the
`contrib/completion` directory and are sourced manually. Libra exposes the
generator directly as `libra completions <shell>`, which is why this command is
classified `intentionally-different` in [`COMPATIBILITY.md`](../../COMPATIBILITY.md).
