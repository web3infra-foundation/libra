## Testing

### Coverage Target: 80%+

All new code should maintain or improve test coverage. Target 80% or higher across branches, functions, and lines.

### Test-Driven Development

Follow the RED-GREEN-REFACTOR cycle:

1. **RED** -- Write a failing test that describes the expected behavior.
2. **GREEN** -- Write the minimal implementation to make the test pass.
3. **REFACTOR** -- Clean up the code while keeping tests green.

### Test Types

| Type | What to Test | Location |
|------|-------------|----------|
| **Unit** | Individual functions in isolation | `#[cfg(test)] mod tests` within source files |
| **Integration** | Module interactions, API boundaries | `tests/` directory |

### Rust Testing Patterns

- Unit tests: `#[cfg(test)]` module at the bottom of each source file.
- Async tests: `#[tokio::test]` for async function testing.
- Temp files: Use `tempfile` crate for filesystem tests -- never write to fixed paths.
- Assertions: Use specific assertions (`assert_eq!`, `assert!(x.contains(...))`) over generic `assert!`.

### Edge Cases to Cover

1. **Empty input** -- empty strings, empty vectors, None values
2. **Boundary values** -- zero, max values, off-by-one
3. **Invalid input** -- wrong types, malformed data, out-of-range values
4. **Error paths** -- IO failures, network errors, permission denied
5. **Concurrent operations** -- race conditions, deadlocks
6. **Large data** -- performance with large inputs
7. **Special characters** -- Unicode, path separators, null bytes
8. **State transitions** -- initial state, transitions, terminal states

### Test Quality

- Each test should verify one behavior. One logical assertion per test.
- Tests must be independent -- no shared mutable state between tests.
- Use descriptive test names: `test_returns_error_when_file_not_found` over `test_error`.
- Follow Arrange-Act-Assert pattern.
- Fix implementation, not tests (unless the test specification is wrong).
- No `#[ignore]` tests without a tracking issue.

### Running Tests

```bash
cargo test                    # All tests
cargo test module_name        # Specific module
cargo test -- --nocapture     # With stdout output
cargo clippy                  # Lint check
cargo fmt -- --check          # Format check
```
