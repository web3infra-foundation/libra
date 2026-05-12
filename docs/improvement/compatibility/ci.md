# C2：CI 兼容矩阵与 job 唯一化

## 所属批次

C2（Audit P1）

## 已完成前置条件与当前代码状态

### 已确认的当前基线（2026-05-11 复核）
- [`.github/workflows/base.yml`](../../../.github/workflows/base.yml) 是当前主测试 workflow，已经拆成 `compat-rustfmt` / `compat-clippy` / `compat-web-check` / `compat-redundancy` / `compat-offline-core` / `compat-network-remotes` 六个唯一 display name。
- `compat-clippy` 已运行 `cargo clippy --all-targets --all-features -- -D warnings`，并设置 `LIBRA_SKIP_WEB_BUILD=1` 避免 Rust lint job 被 Next.js export 成本污染。
- `compat-offline-core` 运行 `cargo test --all`，并在同一 job 内显式运行 `--features test-provider` 的 TUI automation 场景；需要外部网络但不需要 secret 的 `network_remotes_test` 已拆到 `compat-network-remotes`。
- [`.github/workflows/codeql.yml`](../../../.github/workflows/codeql.yml) 的 matrix job 名为 `security-codeql-actions` / `security-codeql-rust`。
- [`.github/workflows/release.yml`](../../../.github/workflows/release.yml) 继续承担发布产物构建，不属于 C2 required-checks 重命名范围。
- [`Cargo.toml`](../../../Cargo.toml) 中已存在 feature gate：`test-network` / `test-live-ai` / `test-live-cloud` / `test-provider`，分别对应网络、真实 LLM、真实云资源和本地 deterministic provider。
- [`tests/compat/`](../../../tests/compat/) 已存在，并通过 `Cargo.toml` 的 `[[test]]` 条目接入 `checkout_alias_help` / `bisect_subcommand_surface` / `worktree_delete_dir` 等跨命令兼容性回归。
- [`scripts/check_compat_matrix.sh`](../../../scripts/check_compat_matrix.sh) 与 [`tests/compat/matrix_alignment.rs`](../../../tests/compat/matrix_alignment.rs) 已把 `COMPATIBILITY.md` 顶层命令表和 `src/cli.rs::Commands` 的漂移检测接入本地测试与 CI。

### 基于当前代码的 Review 结论
- C2 的代码侧命名和 job 拆分已落地；当前 remaining action 是平台侧 branch-protection required-checks 切换，仍必须由维护者在 GitHub UI 中执行。
- live AI / live cloud 仍不应进入 required-checks；当前 workflow 只把 offline core、network-only 和 CodeQL 作为稳定 required-check 候选。
- `tests/compat/` 已成为 C2 / C4 / C5 跨命令契约回归集结点；后续若新增 compatibility surface，应继续先在 `Cargo.toml` 加 `[[test]]`，再在 `tests/compat/README.md` 登记。

## 目标与非目标

**目标：**
- 把 base.yml 现有质量门规范为全局唯一 display name：
  - `compat-rustfmt`
  - `compat-clippy`
  - `compat-redundancy`
  - `compat-offline-core`
  - `compat-network-remotes`
- 把 codeql.yml 的 matrix job 名规范为 `security-codeql-actions` / `security-codeql-rust`，避免同一 workflow 内 matrix check 名称重复。
- 维护 `tests/compat/` 目录，作为 C2 / C4 / C5 跨命令兼容性回归的集结点。
- 新增 `scripts/check_compat_matrix.sh` 和 `tests/compat/matrix_alignment.rs`，把 `COMPATIBILITY.md` 与 `src/cli.rs::Commands` 的顶层命令覆盖关系变成 fail-fast gate。
- 在 [governance.md](governance.md) 给出 required-checks 切换 checklist，由维护者在 GitHub UI 执行。

**非目标：**
- 不引入 live AI / live cloud 作为 required check（这两个保留为可选 / 手动触发，因依赖外部凭据）。
- 不重写 release.yml。
- 不引入新的 CI 提供商或缓存策略。
- 不把 `COMPATIBILITY.md` 的每个 tier/notes 语义做语义化判定；本批只自动校验顶层命令覆盖关系，tier 内容仍由人工 Review 负责。

## 设计要点

### Workflow job 命名规范

仓库内每个 workflow job 必须满足：

1. `name:` 字段全局唯一（跨 workflow file）。
2. 以 kebab-case 命名，前缀按职责分类：
   - `compat-*`：兼容性测试，会被 GitHub required-checks 引用。
   - `security-*`：安全扫描。
   - `release-*`：发布相关。
   - `live-*`：依赖外部凭据，不进入 required-checks。
3. 任何修改 job `name:` 的 PR 必须在 [governance.md](governance.md) 的 required-checks checklist 中标注"GitHub UI 同步切换"。

### compat-* 矩阵设计

`base.yml` 规范化后。实现可以保留当前 `format` / `clippy` / `redundancy` / `test` 的 job id；关键是 `name:` 字段全局唯一且能被 branch protection 稳定引用。下面是命名结构示意，具体 setup steps 复用现有 workflow。

```yaml
jobs:
  format:
    name: compat-rustfmt
    runs-on: [self-hosted]
    steps:
      - uses: actions/checkout@v5
      - run: cargo +nightly fmt --all --check

  clippy:
    name: compat-clippy
    runs-on: [self-hosted]
    steps:
      - uses: actions/checkout@v5
      - run: cargo clippy --all-targets --all-features -- -D warnings

  redundancy:
    name: compat-redundancy
    runs-on: [self-hosted]
    # keep existing redundancy check steps

  compat-offline-core:
    name: compat-offline-core
    runs-on: [self-hosted]
    steps:
      - uses: actions/checkout@v5
      - run: cargo test --all  # 不开 test-network/live-* feature

  compat-network-remotes:
    name: compat-network-remotes
    runs-on: [self-hosted]
    steps:
      - uses: actions/checkout@v5
      - run: cargo test --features test-network --test network_remotes_test -- --test-threads=1
```

`codeql.yml` matrix 命名：

```yaml
jobs:
  analyze:
    name: security-codeql-${{ matrix.language }}
```

可选 / 手动触发的 live 矩阵（保留现有 feature gate，新文件 `compat-live.yml` 可后续追加；本批仅占位）：

```yaml
# Future: compat-live-ai (workflow_dispatch + scheduled)
# Future: compat-live-cloud (workflow_dispatch + scheduled)
```

### `tests/compat/` 目录约定

```
tests/compat/
├── README.md                    # 目录用途说明
├── stash_subcommand_surface.rs  # C4 填充：show / branch / clear 跨场景断言
├── bisect_subcommand_surface.rs # C4 填充：run / view + 退出码语义
├── worktree_delete_dir.rs       # C5 填充：--delete-dir on/off 行为差异
├── checkout_alias_help.rs       # C5 填充：取消 hide 后顶层 help 可见性
└── matrix_alignment.rs          # C2：COMPATIBILITY.md 与 src/cli.rs 一致性校验
```

约定：

- `tests/compat/` 下的测试通过 `compat-offline-core` job 选中执行（依赖 `compat` 子模块声明，见下）。
- 每个文件聚焦"对外契约对齐"，不重复 `tests/command/*_test.rs` 已覆盖的 happy path。
- 顶层 `tests/compat/mod.rs` 或单独 `tests/compat_main.rs` 通过 Cargo `[[test]]` 入口暴露。

### 与 Cargo.toml feature 的映射

| Job | Feature gate | required check | 触发 |
|-----|------------|--------------|-----|
| `compat-rustfmt` | 无 | ✅ | PR / push |
| `compat-clippy` | all targets / all features | ✅ | PR / push |
| `compat-redundancy` | 无 | ✅ | PR / push |
| `compat-offline-core` | 默认 | ✅ | PR / push |
| `compat-network-remotes` | `test-network` | ✅ | PR / push |
| `security-codeql-actions` | 无 | ✅ | PR / push / scheduled |
| `security-codeql-rust` | 无 | ✅ | PR / push / scheduled |
| `compat-live-ai`（占位） | `test-live-ai` | ❌ | workflow_dispatch / scheduled |
| `compat-live-cloud`（占位） | `test-live-cloud` | ❌ | workflow_dispatch / scheduled |

### required-checks 平台切换 runbook（由维护者在 GitHub UI 执行）

写进 [governance.md](governance.md)：

- Settings → Branches → main 的 "Require status checks" 列表，删除旧 `Rustfmt Check` / `Clippy Check` / `Redundancy Check` / `Run Tests` / `Analyze (...)` 名称。
- 添加 `compat-rustfmt` / `compat-clippy` / `compat-redundancy` / `compat-offline-core` / `compat-network-remotes` / `security-codeql-actions` / `security-codeql-rust`。
- 不要把 `compat-live-*` 加入 required（凭据敏感）。
- 验证：故意提交一个会失败的 fmt 改动，PR 显示 `compat-rustfmt` 失败并阻塞合并。

## 关键文件与改动

| 文件 | 操作 | 说明 |
|-----|-----|-----|
| [`.github/workflows/base.yml`](../../../.github/workflows/base.yml) | 修改 | 现有质量门重命名为 `compat-*`；普通测试与 network/live 测试拆清 |
| [`.github/workflows/codeql.yml`](../../../.github/workflows/codeql.yml) | 修改 | matrix job 重命名为 `security-codeql-actions` / `security-codeql-rust` |
| [`scripts/check_compat_matrix.sh`](../../../scripts/check_compat_matrix.sh) | 新建 | `src/cli.rs::Commands` ↔ `COMPATIBILITY.md` 顶层命令漂移检测 |
| [`tests/compat/README.md`](../../../tests/compat/README.md) | 新建 | 目录用途说明 + 文件命名约定 |
| [`tests/compat/matrix_alignment.rs`](../../../tests/compat/matrix_alignment.rs) | 新建 | 通过 Cargo test 运行矩阵漂移检测脚本 |
| [`Cargo.toml`](../../../Cargo.toml) | 修改 | 注册 `compat_matrix_alignment` 集成测试 |
| [`docs/improvement/compatibility/governance.md`](governance.md) | 追加 | required-checks 切换 checklist 小节 |

## 测试与验收

- [x] [`.github/workflows/base.yml`](../../../.github/workflows/base.yml) 出现 `compat-rustfmt` / `compat-clippy` / `compat-web-check` / `compat-redundancy` / `compat-offline-core` / `compat-network-remotes` 唯一 display name。
- [x] [`.github/workflows/codeql.yml`](../../../.github/workflows/codeql.yml) 的 CodeQL matrix 使用 `security-codeql-${{ matrix.language }}`，即 `security-codeql-actions` / `security-codeql-rust`。
- [x] `compat-clippy` 使用 `cargo clippy --all-targets --all-features -- -D warnings`。
- [x] [`tests/compat/README.md`](../../../tests/compat/README.md) 已说明该目录由 `compat-offline-core` 覆盖，并列出现有兼容性测试文件。
- [x] [`scripts/check_compat_matrix.sh`](../../../scripts/check_compat_matrix.sh) 能在 `COMPATIBILITY.md` 顶层命令表遗漏或多列 `src/cli.rs::Commands` 变体时 fail-fast。
- [x] [`compat_matrix_alignment`](../../../tests/compat/matrix_alignment.rs) 已通过 Cargo `[[test]]` 接入 `cargo test --all`。
- [x] [governance.md](governance.md) 已更新 required-checks 切换 checklist。
- 外部平台确认：GitHub UI branch protection 完成 required-checks 切换后，经 PR 页面验证新名称会阻塞失败检查。该项不能由代码提交自动完成，不计入本仓库代码验收 checkbox。

## 风险与缓解

1. **重命名 job 后旧 required-checks 不再匹配，PR 永久卡住** → 缓解：governance.md 中明确写出 GitHub UI 切换步骤；维护者切换完成前不要 merge C2，可在临时分支验证后一次性应用。
2. **`compat-network-remotes` 不稳定（公共网络抖动）** → 缓解：仅放真正只依赖网络无凭据的测试；任何依赖私有 endpoint 的测试归 `compat-live-*`。
3. **`tests/compat/` 与 `tests/command/` 职责模糊** → 缓解：`tests/compat/` 仅放"跨命令对外契约一致性"用例；`tests/command/` 仍然是单命令 happy path / error path 的主战场。
