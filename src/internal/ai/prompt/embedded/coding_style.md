## Coding Style

### Rust Conventions

- Use `&T` and `&str` in function parameters; return owned types (`String`, `Vec<T>`) when ownership transfers.
- Prefer borrowing over cloning. Clone only when ownership is genuinely needed by multiple owners.
- Use `clippy` and fix all warnings. Treat warnings as errors in CI.
- Derive `Debug` on all public types. Derive `Clone`, `PartialEq`, `Eq` only when needed.
- No `unsafe` blocks unless justified with a `// SAFETY:` comment explaining the invariant.

### Naming Conventions

- Types: `PascalCase` (structs, enums, traits)
- Functions and variables: `snake_case`
- Constants: `UPPER_SNAKE_CASE`
- Modules: `snake_case`, one file per module
- Use descriptive names. `user_count` over `n`. `is_valid` over `flag`.

### Import Organization

Group imports in this order, separated by blank lines:
1. `std` library
2. External crates
3. `crate` / `super` imports

### File Organization

- Many small files over few large files.
- High cohesion, low coupling.
- 200-400 lines typical, 800 lines maximum per file.
- Extract utilities when a module grows beyond 400 lines.
- Organize by feature/domain, not by type.
- `mod.rs` files should contain only re-exports and minimal glue code.

### Functions

- Keep functions small: under 50 lines.
- Prefer early returns over deep nesting. Maximum 4 levels of indentation.
- Use the builder pattern for complex configuration.
- Prefer `impl Into<String>` or `impl AsRef<str>` for flexible string parameters.

### Code Quality

- Code is read more than written. Optimize for readability.
- Self-documenting code preferred over comments.
- Comments should explain WHY, not WHAT.
- No hardcoded values — use constants or configuration.
- No magic numbers — name them as constants.
