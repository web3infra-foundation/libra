# Archive Testing Guide

This guide documents the focused test coverage for `libra archive`.

## Scope

The archive command resolves a committed tree and writes tracked files as
`tar`, `tar.gz`, `tar.bz2`, or `zip`. The focused integration tests live in
`tests/command/archive_test.rs` and exercise the real `libra` binary through
the shared command-test helpers.

The tests are L1 deterministic tests. They create temporary repositories,
commit fixture files, and do not require network access or external services.

## Covered Cases

| Area | Tests |
|------|-------|
| Happy path | `archive_default_produces_tar`, `archive_supports_compressed_and_zip_formats`, `archive_writes_output_file`, `archive_applies_prefix_to_tar_paths` |
| Unicode input | `archive_preserves_unicode_filenames` |
| Empty input | `archive_empty_repo_reports_invalid_target`, `archive_preserves_empty_files` |
| Special characters | `archive_preserves_spaces_in_filenames` |
| Deep paths | `archive_preserves_deeply_nested_paths` |
| Invalid arguments | `archive_rejects_invalid_format`, `archive_rejects_archive_slip_prefix` |
| Missing paths | `archive_rejects_invalid_treeish`, `archive_rejects_output_in_missing_directory` |
| Short flags | `archive_short_format_flag_writes_zip` |

## Running Archive Tests

Run only the archive integration tests:

```bash
LIBRA_SKIP_WEB_BUILD=1 CARGO_BUILD_JOBS=2 cargo test --test command_test command::archive_test
```

Run the archive unit tests inside the library:

```bash
LIBRA_SKIP_WEB_BUILD=1 CARGO_BUILD_JOBS=2 cargo test --lib archive
```

Run the complete test suite before final submission:

```bash
LIBRA_SKIP_WEB_BUILD=1 CARGO_BUILD_JOBS=2 cargo test --all
```

`CARGO_BUILD_JOBS=2` keeps compile parallelism lower for constrained WSL
environments. Remove it on machines with enough memory.

## Formatting And Lints

Format Rust code:

```bash
LIBRA_SKIP_WEB_BUILD=1 CARGO_BUILD_JOBS=2 cargo +nightly fmt
```

Check formatting:

```bash
LIBRA_SKIP_WEB_BUILD=1 CARGO_BUILD_JOBS=2 cargo +nightly fmt --check
```

Run clippy for the archive integration test target:

```bash
LIBRA_SKIP_WEB_BUILD=1 CARGO_BUILD_JOBS=2 cargo clippy --test command_test -- -D warnings
```

Run clippy for the library:

```bash
LIBRA_SKIP_WEB_BUILD=1 CARGO_BUILD_JOBS=2 cargo clippy --lib -- -D warnings
```

## Documentation Check

Build Rust documentation without dependencies:

```bash
LIBRA_SKIP_WEB_BUILD=1 CARGO_BUILD_JOBS=2 cargo doc --no-deps
```

The repository currently emits existing rustdoc warnings outside the archive
module, but the command must finish successfully.
