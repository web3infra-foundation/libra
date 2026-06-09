# Rev-Parse 命令改进详细计划

## 所属批次

对象与历史读取命令（plumbing / script bridge）

## 落地状态（2026-06-08）

`rev-parse` 的高频脚本兼容子集已落地：单对象校验、路径/状态查询、shell 引用、范围展开、多 spec JSON 输出和符号全名解析均已支持。`COMPATIBILITY.md` 维持 `rev-parse | supported`，并在备注中列出 `.libra` 存储目录、129 invalid-target、`--verify` 单对象模式、`--sq-quote` 前导 `--` 和 `--symbolic*` partial 子集等差异。

### 已完成能力

- `RevParseArgs` 支持 `[SPEC]...`、`--verify`、`-q`/`--quiet`、`--default`、`--git-dir`、`--show-prefix`、`--show-cdup`、`--is-inside-git-dir`、`--is-inside-work-tree`、`--is-bare-repository`、`--sq`、`--sq-quote`、`--symbolic`、`--symbolic-full-name`。
- `--verify` 成功时输出单个对象；失败时普通模式退出 128，`--quiet` 下静默退出 1；`--verify --short` 输出非歧义短哈希。
- 路径输出统一 `/` 分隔，`--git-dir` 返回 `.libra`，`.libra` 内部 `--is-inside-work-tree` 返回 `false`。
- `--sq` 对解析后的 revision 输出 shell-safe 单引号结果；`--sq-quote` 对字面参数执行转义，且可在仓库外运行。
- `A..B` 输出 `B` 与 `^A`，`A...B` 输出两端点和全部 best merge-base 排除项，`^A` 单独输出排除项。
- JSON envelope 对 range / 多 spec 输出保留换行拼接的 `value`，并追加有序 `values[]`。
- `--symbolic-full-name` 覆盖本地分支、远程跟踪分支和 tag。
- `docs/commands/rev-parse.md`、`docs/commands/zh-CN/rev-parse.md`、`docs/development/integration-scenarios.yaml`、`docs/development/integration-scenarios/cli.object-readback.md` 和 `tools/integration-runner/src/scenarios/object_readback.rs` 已同步。

### 仍 deferred / partial

- Git 的自由 flag-stream 组合语义未完全复刻；`--verify` 在 Libra 中是单对象模式，和路径/状态参数互斥。
- `--symbolic*` 不承诺覆盖 Git 所有特殊 symbolic 形式，当前覆盖分支、远程跟踪分支和 tag。
- `--sq-quote -- -x` 的前导 `--` 会被 clap 作为选项终止符消费，不作为字面 token 输出。

## 已验证命令

- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo check`
- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --lib rev_parse -- --test-threads=1 --nocapture`
- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --test command_test -- rev_parse --test-threads=1 --nocapture`
- `source .env.test && cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan`
- `source .env.test && cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --only cli.object-readback`
- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --test compat_help_flag_descriptions --test compat_help_examples_banner --test compat_command_docs_examples_section --test compat_matrix_alignment --test compat_all_production_unwrap_guard --test compat_extra_production_unwrap_guard -- --nocapture`
- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo clippy --all-targets --all-features -- -D warnings`
- `cargo +nightly fmt --all --check`
- `rg -n "\\.expect\\(|\\.unwrap\\(" src/command/rev_parse.rs` 仅命中 `#[cfg(test)]` 模块，生产路径无新增 `unwrap()` / 非-INVARIANT `expect()`。
