## Error Handling

### Core Rules

- NEVER use `.unwrap()` or `.expect()` in production code. Propagate errors with `?`.
- Use `thiserror` for library/domain error enums. Use `anyhow` only in binary crates or tests.
- Never silently swallow errors. Every error path must be handled explicitly.
- Error messages must be user-friendly in UI-facing code. Log detailed context server-side.

### Error Enum Pattern

Define a domain error enum per module:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("validation failed: {0}")]
    Validation(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}
```

### Error Propagation

- Use `?` operator for error propagation throughout the call chain.
- Map errors at module boundaries with `.map_err()` when crossing domain boundaries.
- Use `#[from]` derives in `thiserror` for automatic conversion from underlying errors.
- Add context with `.context()` or `.with_context()` from `anyhow` when the original error lacks context.

### Logging

- Use `tracing` for structured logging. Never `println!` or `eprintln!` in library code.
- Log at appropriate levels: `error!` for failures, `warn!` for recoverable issues, `info!` for significant events, `debug!`/`trace!` for development.
- Never log secrets, credentials, or personally identifiable information.
- Include structured fields: `tracing::error!(?err, path = %file_path, "failed to read file");`

### Input Validation

- Validate at system boundaries: user input, external API responses, file content.
- Fail fast with clear error messages.
- Use type-driven validation where possible (newtypes, enums).
- Never trust external data.
