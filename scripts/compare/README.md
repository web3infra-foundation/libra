# Compare Script (git / jj / libra)

Run from repo root:

```bash
scripts/compare/run.sh
```

Common options:

```bash
scripts/compare/run.sh --tools git,jj,libra
scripts/compare/run.sh --report-dir /tmp/libra-compare-report
scripts/compare/run.sh --keep-sandbox
scripts/compare/run.sh --skip-github-push
```

What it covers:

- Command Surface: all git-like commands currently present in `src/command/` (excluding Libra-only command families), with `--help` and invalid-flag error paths.
- Identity Config: commit behavior with/without user identity (`name`/`email`) configured.
- Behavior Matrix: command-level success/failure comparisons across common git workflows.
- Flow Experience: mixed success/failure end-to-end scenario from init -> add -> commit -> cat-file -> tag -> blame -> remote/push/fetch/pull -> clone.

Outputs:

- Markdown report: `<sandbox>/report.md`
- Per-command stdout/stderr/rc/time: `<sandbox>/out/`

The script uses an isolated sandbox HOME and does not touch your real global git/jj configs.
