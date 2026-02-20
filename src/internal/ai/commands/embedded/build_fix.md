---
name: build-fix
description: Diagnose and fix build or compilation errors with minimal changes.
agent: build_error_resolver
---

## /build-fix $ARGUMENTS

Diagnose and fix the build or compilation error described below.

**Error / Context:** $ARGUMENTS

If no specific error is provided, run `cargo build` or `cargo test` to identify the current failures.

### Resolution Process

1. **Parse the Error** — Identify the file, line number, and error type from compiler output.
2. **Read the Source** — Use read_file to see full context around the error.
3. **Understand the Intent** — What was the code trying to do? What went wrong?
4. **Apply Minimal Fix** — Use apply_patch to fix ONLY the error. Do not refactor or improve surrounding code.
5. **Verify** — Confirm the fix resolves the error without introducing new issues.

### Constraints

- **Minimal diff**: Changes should be <5% of the file.
- **No refactoring**: Fix the error, nothing else.
- **No feature changes**: Preserve the original intent.
- **One at a time**: Fix errors in the order the compiler reports them.
