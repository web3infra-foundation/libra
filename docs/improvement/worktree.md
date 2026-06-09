# Worktree Improvement Completion

## Status

Completed on 2026-06-08.

## Delivered

- `worktree add` now supports `-b`, `-B`, `--detach`, `--no-checkout`, `--lock`, `--reason`, and optional `[commit-ish]`.
- `worktree list` now supports `--porcelain` and `--verbose`.
- `worktree remove` now supports repeatable `--force`; dirty deletion requires `--delete-dir --force`, and locked worktrees require `-f -f`.
- `worktree prune` now supports `--dry-run`, `--verbose`, and partial Git-style `--expire <time>`.
- `worktree repair` now rebuilds missing or stale linked `.libra` symlinks while skipping real `.libra` directories.
- `worktree move` now preserves registry/disk consistency on rename failure and repairs moved symlinks.
- Default and `worktree-fuse` feature command surfaces were kept in sync for standard non-FUSE worktree flags.

## Compatibility Notes

- Libra keeps `worktree remove <path>` non-destructive by default; this remains intentionally different from Git.
- Libra linked worktrees share one `.libra` storage and HEAD. `-b`, `-B`, `--detach`, and porcelain output are therefore partial Git compatibility, not per-worktree HEAD isolation.
- `worktree list --porcelain` intentionally omits `branch` and `detached` rows.
- Unsupported or out of scope: `worktree add -f/--force`, `worktree add --orphan`, explicit `--checkout`, and `worktree list --porcelain -z`.

## Verification

- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo check --lib --message-format short`
- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --test command_test -- worktree_test --test-threads=1 --nocapture`
- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --test command_test --features worktree-fuse -- worktree_fuse_test --test-threads=1 --nocapture`
