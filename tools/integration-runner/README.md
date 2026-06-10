# Libra Integration Runner

Independent black-box runner for `docs/development/integration-test-plan.md`.

This tool is intentionally outside the root `Cargo.toml` test graph. Run it explicitly:

```bash
cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan
cargo run --manifest-path tools/integration-runner/Cargo.toml -- list
cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --waves 0
cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --only cli.init-basic,cli.config-basic-kv,cli.commit-status-log
```

Per-scenario docs: `docs/development/integration-scenarios/<id>.md` (index: `integration-scenarios/README.md`). Registry: `integration-scenarios.yaml`. Plan matrix/isolation: `integration-test-plan.md`.

Implemented: R0-R5 complete (`check-plan` = yaml + per-scenario MD + §2.3 matrix + registry; `run --waves 0,1,2`; `run-live` for Wave 3). All 38 scenarios in `scenario_registry()` + `src/scenarios/*.rs`.
