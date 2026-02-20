---
name: verify
description: Run comprehensive verification checks before commit or PR.
agent:
---

## /verify $ARGUMENTS

Run a comprehensive verification pipeline on the project.

**Mode:** $ARGUMENTS (default: `full`)

### Verification Modes

- `quick` — Build + clippy only
- `full` — All checks (default)
- `pre-commit` — Build + clippy + tests
- `pre-pr` — Full + documentation check

### Verification Pipeline

Execute checks in this exact order. Stop on first critical failure.

1. **Build Check**
   ```
   cargo build
   ```
   Report: OK or FAIL with error summary

2. **Clippy Check**
   ```
   cargo clippy -- -D warnings
   ```
   Report: OK or number of warnings/errors

3. **Test Suite**
   ```
   cargo test
   ```
   Report: pass/fail count

4. **Format Check**
   ```
   cargo fmt --check
   ```
   Report: OK or list of unformatted files

5. **Unsafe Audit**
   Search for `unsafe` blocks in source files.
   Report: OK or count with file locations

6. **Git Status**
   Report uncommitted changes.

### Output Format

```
VERIFICATION: [PASS/FAIL]
Build:    [OK/FAIL]
Clippy:   [OK/X warnings]
Tests:    [X/Y passed]
Format:   [OK/X files]
Unsafe:   [OK/X blocks]
Ready:    [YES/NO]
```
