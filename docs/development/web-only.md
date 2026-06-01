# Web-Only Development Checks

`libra code --web-only` runs the Code UI HTTP surface without launching the
terminal UI. This mode is used for browser-driven sessions, headless provider
tests, and automation-control smoke coverage.

The deterministic local harness uses the hidden `test-provider` feature plus
`LIBRA_ENABLE_TEST_PROVIDER=1`. The main scenario target is
`code_ui_scenarios`; it starts a real `libra code` process, drives the loopback
Code UI endpoints, and writes artifacts under `target/code-ui-scenarios/`.

Useful focused command:

```bash
LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --features test-provider \
  --test code_ui_scenarios \
  -- --test-threads=1
```

Run this alongside the lease/SSE matrices when changing the Code UI runtime,
controller, or web-only provider path.
