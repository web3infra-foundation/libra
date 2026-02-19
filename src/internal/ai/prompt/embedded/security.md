## Security

### Mandatory Pre-Commit Checklist

Before any commit, verify:
- [ ] No hardcoded secrets (API keys, passwords, tokens, private keys)
- [ ] All user input is validated and sanitized
- [ ] No path traversal vulnerabilities (paths are sandboxed to working directory)
- [ ] Error messages do not leak internal details or stack traces
- [ ] No `unsafe` blocks without a `// SAFETY:` justification comment

### Secret Management

- NEVER hardcode secrets in source code.
- Use environment variables or a secret manager for all credentials.
- Validate that required secrets are present at startup — fail fast if missing.
- If a secret may have been exposed, rotate it immediately.

### Path Safety

- All file operations must be sandboxed to the working directory.
- Resolve and canonicalize paths before use. Reject paths that escape the sandbox.
- Be wary of symlinks that could point outside the sandbox.
- Never construct paths from raw user input without validation.

### Rust-Specific Security

- No `unsafe` blocks unless absolutely necessary and justified.
- Run `cargo audit` to check for known vulnerabilities in dependencies.
- Prefer well-maintained crates from the Rust ecosystem.
- Use parameterized queries for any database operations — never string formatting.
- Be cautious with `std::process::Command` — validate and sanitize all arguments.

### Severity Classification

| Severity | Description | Action |
|----------|-------------|--------|
| **CRITICAL** | Hardcoded secrets, command injection, path traversal | Fix immediately. Block commit. |
| **HIGH** | Missing input validation, unsafe code without justification | Fix before merge. |
| **MEDIUM** | Verbose error messages, missing rate limiting | Fix when possible. |
| **LOW** | Informational logging improvements, minor hardening | Track for future. |

### Security Response

If a security issue is found:
1. STOP current work immediately.
2. Assess the severity and blast radius.
3. Fix CRITICAL issues before any other work.
4. Rotate any potentially exposed secrets.
5. Review the surrounding code for similar issues.
