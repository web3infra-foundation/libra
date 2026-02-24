## Git Workflow

### Commit Message Format

```
<type>: <description>

<optional body>
```

Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `perf`, `ci`

- Keep the subject line under 72 characters.
- Use imperative mood: "add feature" not "added feature".
- The body should explain WHY the change was made, not WHAT changed (the diff shows that).

### Implementation Workflow

1. **Analyze & Plan (AI Object Workflow)**
   - **Simple Task (Single File)**: You may directly create a Task (`create_task`) and execute it.
   - **Complex Task (Multi-file / Multi-step)**: You **MUST** follow the `Intent -> Plan -> Task` workflow:
     1. Create an **Intent** (`create_intent`) capturing the user's high-level goal.
     2. Create a **Plan** (`create_plan`) that breaks the Intent into sequential Steps.
     3. For each step in the Plan, create a **Task** (`create_task` with `intent_id`) and execute it.
   - Do not skip the Planning phase for complex features. It ensures context is preserved and steps are logical.

2. **Implement** -- Write the code. Follow existing patterns in the codebase.
3. **Test** -- Verify the change compiles (`cargo build`) and passes tests (`cargo test`).
4. **Lint** -- Run `cargo clippy` and `cargo fmt --check`. Fix all warnings.
5. **Commit** -- Write a clear commit message following the format above.

### Pull Request Guidelines

- Analyze the full commit history, not just the latest commit.
- Use `git diff <base-branch>...HEAD` to see all changes.
- Keep PRs focused: one logical change per PR.
- Include a test plan describing how the change was verified.
- Small, focused commits within a PR are preferred over monolithic ones.

### Branch Strategy

- Feature branches from `main`.
- Never commit directly to `main`.
- Delete branches after merge.
- Rebase onto `main` before merge to maintain linear history when possible.
