# C2：CI 兼容矩阵与 job 唯一化

## 所属批次

C2（Audit P1）

## 已完成前置条件与当前代码状态

### 已确认的当前基线
- [`.github/workflows/base.yml`](../../../.github/workflows/base.yml) 是当前主测试 workflow，已经拆成 `format` / `clippy` / `redundancy` / `test` 四个 job；对应显示名为 `Rustfmt Check` / `Clippy Check` / `Redundancy Check` / `Run Tests`，尚未采用兼容矩阵命名。
- 当前 `clippy` job 运行的是 `cargo clippy -- -D warnings`，缺少 `--all-targets --all-features`，与 AGENTS.md 的 lint 要求不完全一致；C2 规范化时需同步修正命令参数。
- 当前 `test` job 同时承担普通 `cargo test --all`、带 live secret 的测试环境变量、以及 TUI automation 场景；required-checks 与 live/secret 测试边界不够清晰。
- [`.github/workflows/codeql.yml`](../../../.github/workflows/codeql.yml) 承担安全扫描，当前 matrix 生成 `Analyze (actions)` / `Analyze (rust)`；命名没有与 `compat-*` required-checks 体系统一。
- [`.github/workflows/release.yml`](../../../.github/workflows/release.yml) 承担发布产物构建。
- [`Cargo.toml`](../../../Cargo.toml) 中已存在 feature gate：`test-network` / `test-live-ai` / `test-live-cloud`，分别对应"需要网络"/"需要 LLM 凭据"/"需要云存储凭据"的测试集。
- 仓库**缺失** `tests/compat/` 目录与跨命令兼容性回归汇编。

### 基于当前代码的 Review 结论
- 当前 workflow 的显示名虽然不是单 job，但仍未形成稳定的 `compat-*` / `security-*` 命名约定；required-checks 迁移时容易混入旧名称或 matrix 自动生成名。
- 三层 feature gate 已存在但没有被清晰映射到 workflow job 维度；live 矩阵无法稳定通过 PR required-checks，因为依赖外部凭据。
- C4 / C5 的回归测试需要一个公共集结点，避免散落在各命令的 `tests/command/*_test.rs`。

## 目标与非目标

**目标：**
- 把 base.yml 现有质量门规范为全局唯一 display name：
  - `compat-rustfmt`
  - `compat-clippy`
  - `compat-redundancy`
  - `compat-offline-core`
  - `compat-network-remotes`
- 把 codeql.yml 的 matrix job 名规范为 `security-codeql-actions` / `security-codeql-rust`，避免同一 workflow 内 matrix check 名称重复。
- 新建 `tests/compat/` 占位目录，作为 C4 / C5 跨命令兼容性回归的集结点。
- 在 [governance.md](governance.md) 给出 required-checks 切换 checklist，由维护者在 GitHub UI 执行。

**非目标：**
- 不引入 live AI / live cloud 作为 required check（这两个保留为可选 / 手动触发，因依赖外部凭据）。
- 不重写 release.yml。
- 不引入新的 CI 提供商或缓存策略。
- 不在本批落地"COMPATIBILITY.md 与 src/cli.rs 一致性"自动校验脚本的实现，仅在 C2 文档中规划占位（实际脚本由 C1 落地后的迭代补足）。

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
      - run: cargo test --all --features test-network
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
└── matrix_alignment.rs          # 占位：未来 COMPATIBILITY.md 与 src/cli.rs 一致性校验
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

### required-checks 切换 checklist（由维护者在 GitHub UI 执行）

写进 [governance.md](governance.md)：

- [ ] Settings → Branches → main 的 "Require status checks" 列表，删除旧 `Rustfmt Check` / `Clippy Check` / `Redundancy Check` / `Run Tests` / `Analyze (...)` 名称。
- [ ] 添加 `compat-rustfmt` / `compat-clippy` / `compat-redundancy` / `compat-offline-core` / `compat-network-remotes` / `security-codeql-actions` / `security-codeql-rust`。
- [ ] 不要把 `compat-live-*` 加入 required（凭据敏感）。
- [ ] 验证：故意提交一个会失败的 fmt 改动，PR 显示 `compat-rustfmt` 失败并阻塞合并。

## 关键文件与改动

| 文件 | 操作 | 说明 |
|-----|-----|-----|
| [`.github/workflows/base.yml`](../../../.github/workflows/base.yml) | 修改 | 现有质量门重命名为 `compat-*`；普通测试与 network/live 测试拆清 |
| [`.github/workflows/codeql.yml`](../../../.github/workflows/codeql.yml) | 修改 | matrix job 重命名为 `security-codeql-actions` / `security-codeql-rust` |
| [`tests/compat/README.md`](../../../tests/compat/README.md) | 新建 | 目录用途说明 + 文件命名约定 |
| [`docs/improvement/compatibility/governance.md`](governance.md) | 追加 | required-checks 切换 checklist 小节 |

## 测试与验收

- [ ] PR 触发后，GitHub Actions 页面出现 `compat-rustfmt` / `compat-clippy` / `compat-redundancy` / `compat-offline-core` / `compat-network-remotes` / `security-codeql-actions` / `security-codeql-rust` 七项 required job。
- [ ] `gh pr checks <PR>` 输出中无重名 job。
- [ ] `compat-rustfmt` / `compat-clippy` / `compat-offline-core` 任一失败时阻塞 PR。
- [ ] `tests/compat/README.md` 已说明该目录被 `compat-offline-core` 选中，并列举 C4 / C5 待填充文件名。
- [ ] [governance.md](governance.md) 已更新 required-checks 切换 checklist。

## 风险与缓解

1. **重命名 job 后旧 required-checks 不再匹配，PR 永久卡住** → 缓解：governance.md 中明确写出 GitHub UI 切换步骤；维护者切换完成前不要 merge C2，可在临时分支验证后一次性应用。
2. **`compat-network-remotes` 不稳定（公共网络抖动）** → 缓解：仅放真正只依赖网络无凭据的测试；任何依赖私有 endpoint 的测试归 `compat-live-*`。
3. **`tests/compat/` 与 `tests/command/` 职责模糊** → 缓解：`tests/compat/` 仅放"跨命令对外契约一致性"用例；`tests/command/` 仍然是单命令 happy path / error path 的主战场。
