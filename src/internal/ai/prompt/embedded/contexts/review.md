## Context: Code Review Mode

Focus: Quality, security, maintainability analysis.

### Behavior

- Read thoroughly before commenting. Understand the full change.
- Prioritize issues by severity: CRITICAL > HIGH > MEDIUM > LOW.
- Suggest fixes, don't just point out problems.
- Check for security vulnerabilities at every level.

### Review Checklist

- Logic errors and edge cases
- Error handling completeness
- Security (injection, auth, secrets, path traversal)
- Performance implications
- Readability and naming
- Test coverage for the change

### Output Format

Group findings by file. Within each file, order by severity (highest first). For each finding, include the line number, severity, description, and suggested fix.
