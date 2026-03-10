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
  - Per-category result tables
  - Final summary table
  - Case-level summary table (`Libra vs git/jj`)
  - Raw command output section (per case, per tool, stdout/stderr snippets)
- Per-command stdout/stderr/rc/time: `<sandbox>/out/`

You can tune raw output snippet size in the Markdown report via:

```bash
RAW_OUTPUT_MAX_BYTES=4000 scripts/compare/run.sh
```

The script uses an isolated sandbox HOME and does not touch your real global git/jj configs.
