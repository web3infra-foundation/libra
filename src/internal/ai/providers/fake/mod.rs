//! Test-only deterministic provider used by local TUI automation harnesses.
//!
//! This module is compiled only with the `test-provider` feature and remains
//! hidden behind `LIBRA_ENABLE_TEST_PROVIDER=1` in the CLI. It returns normal
//! provider-neutral completion responses so the real TUI, tool loop, approval,
//! and control paths stay in production shape during cross-process tests.

mod completion;
mod fixture;

pub use completion::{Client, CompletionModel, FakeRawResponse};
pub use fixture::{FakeFixture, FakeFixtureError};

pub const FAKE_DEFAULT_MODEL: &str = "fake-local";
