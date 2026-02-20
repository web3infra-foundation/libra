---
name: code-review
description: Perform a comprehensive code review for quality and security.
agent: code_reviewer
---

## /code-review $ARGUMENTS

Perform a thorough code review on the specified code or recent changes.

**Target:** $ARGUMENTS

If no specific target is provided, review the most recent uncommitted changes (use `git diff` output if available).

### Review Checklist

**Security Issues (CRITICAL)**
- Hardcoded credentials, API keys, tokens
- SQL/command injection vulnerabilities
- Missing input validation
- Path traversal risks
- Unsafe code blocks without justification

**Code Quality (HIGH)**
- Functions exceeding 50 lines
- Files exceeding 800 lines
- Nesting depth > 4 levels
- Missing error handling (unwrap/expect without justification)
- TODO/FIXME comments without tracking
- Missing documentation for public APIs

**Rust-Specific (HIGH)**
- Unnecessary cloning (prefer borrowing)
- Missing `#[must_use]` on fallible functions
- Improper error type design (should use thiserror)
- Blocking calls in async context
- Missing Send/Sync bounds where needed

**Best Practices (MEDIUM)**
- Missing tests for new code
- Inconsistent naming conventions
- Overly complex type signatures
- Dead code or unused imports

### Output Format

For each finding, report:
```
[SEVERITY] file:line — Description
  Suggestion: How to fix
```

### Summary

End with a summary:
- Total findings by severity
- Overall assessment: APPROVE / REQUEST CHANGES / BLOCK
- Top 3 most important items to address
