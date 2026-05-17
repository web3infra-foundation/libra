# C1：仓库治理基线（COMPATIBILITY.md / .gitattributes）

## 所属批次

C1（Audit P0）

## 已完成前置条件与当前代码状态

### 已确认的当前基线
- [`docs/improvement/README.md`](../README.md) 已记录第 1–8 批 CLIG 现代化命令清单与对外契约。
- [`src/cli.rs`](../../../src/cli.rs) 中 `Commands` enum 是 4-tier 矩阵的事实来源；任何新增/删除子命令都必须同步更新 `COMPATIBILITY.md`。
- 仓库根已存在 [`COMPATIBILITY.md`](../../../COMPATIBILITY.md)、[`.gitattributes`](../../../.gitattributes)；后续变更必须保持这两处与本计划同步。
- 仓库已存在二进制资源：[`docs/image/banner.png`](../../image/banner.png)、[`docs/video/demo-20260224.gif`](../../video/demo-20260224.gif)。
- [`docs/contributing.md`](../../contributing.md) 已要求 DCO 与 PGP 签名，但尚未在 GitHub 平台层把 required-checks 显式化。

### 基于当前代码的 Review 结论
- 治理文件已落地，剩余风险在于这些文件与代码、命令文档和 GitHub branch-protection UI 的持续同步；用户用 stock Git 心智来期待 Libra，差异成本最终回流到维护者。
- 二进制资源未声明 binary 属性，会让历史 diff 与跨平台 checkout 产生未知风险，但当前体量不必立刻迁 LFS。
- `.github/CODEOWNERS` 不作为本 improvement 的交付物维护；代码评审责任由平台设置和维护者流程承担。
- 原矩阵骨架未覆盖 `src/cli.rs::Commands` 中全部顶层命令，尤其遗漏 `rm` / `mv` / `grep` / `rev-parse` / `rev-list` / `lfs` / `cloud` / `code` / `code-control` / `graph` 等实际命令。C1 落地时必须以代码枚举为事实来源逐行填充，不能只列 Git 常见命令。
- LFS 需要拆开描述：`libra lfs` 是已存在命令；Git LFS 的 `.gitattributes` filter / hooks 兼容层是另一件事，不能把二者合并写成单一 `unsupported`。

## 目标与非目标

**目标：**
- 维护 `COMPATIBILITY.md`（4-tier）作为对外承诺的事实表。
- 维护仓库根 `.gitattributes` 覆盖文本归一化与已有二进制资源。
- 同步 [`docs/commands/README.md`](../../commands/README.md) 的现有命令索引，至少补上已存在但缺索引的 `code-control`；checkout / bisect 的 hidden 标记分别由 C5 / C4 处理。
- 在本文件中给出"未来若启用 LFS 的叠加规则"伪代码，但**本批不启用 LFS**。

**非目标：**
- 不在仓库内自动启用分支保护或 required-checks——平台层操作由维护者按本计划在 GitHub UI 执行。
- 不重写历史，不迁移现有二进制到 LFS。
- 不引入 commit signing 强制（DCO/PGP 维持现状由 contributing.md 规范）。

## 设计要点

### COMPATIBILITY.md 4-tier 矩阵

四档定义（英文字段，便于国际化）：

| Tier | 含义 | 用户预期 |
|------|------|--------|
| `supported` | 命令/flag 与 stock Git 行为一致或基本一致 | 可按 Git 习惯使用 |
| `partial` | 已暴露但子命令面或 flag 不全 | 常用路径可用，高级路径可能缺失 |
| `unsupported` | 不支持，无 plumbing 或 plumbing 不公开 | 请使用 stock Git 完成或寻找等价命令 |
| `intentionally-different` | 行为有意偏离 Git，文档已说明 | 阅读迁移说明再使用 |

矩阵骨架（C1 落地时 fill）。本表的 tier 表示 **Git surface 兼容层级**，不表示该命令是否已经完成 CLIG JSON / machine 现代化；后者仍由 [`docs/improvement/README.md`](../README.md) 的命令批次拥有。

```markdown
# Libra Compatibility Matrix

> 4 tiers: `supported` / `partial` / `unsupported` / `intentionally-different`
> Source of truth: top-level `Commands` variants in `src/cli.rs`.

## Top-level commands (from `src/cli.rs`)

| Command | Tier | Notes |
|---------|------|-------|
| init | supported | |
| clone | partial | --depth and --single-branch supported; --sparse unsupported; --recurse-submodules unsupported |
| code | intentionally-different | Libra AI extension, not a Git command |
| code-control | intentionally-different | Libra AI automation extension, not a Git command |
| graph | intentionally-different | Libra AI graph inspection extension, not a Git command |
| add | partial | sparse-checkout flag unsupported |
| rm | partial | --force / --dry-run / --quiet not exposed |
| mv | partial | sparse-checkout flag unsupported; --skip-errors not exposed |
| restore | supported | |
| status | supported | |
| clean | supported | |
| stash | partial | push / pop / list / apply / drop / show / branch / clear supported; create / store unsupported (see [declined.md#d8-stash-create](../../improvement/compatibility/declined.md#d8) / [#d9-stash-store](../../improvement/compatibility/declined.md#d9)) |
| lfs | partial | built-in Libra LFS command; uses `.libraattributes`, not Git LFS filters/hooks |
| log | supported | |
| shortlog | supported | |
| show | supported | |
| show-ref | supported | |
| branch | supported | |
| tag | supported | |
| commit | supported | |
| switch | supported | |
| rebase | partial | --autosquash / --reapply-cherry-picks not supported |
| merge | partial | fast-forward only; other strategies unsupported |
| reset | supported | |
| rev-parse | supported | |
| rev-list | supported | |
| describe | supported | |
| cherry-pick | supported | |
| push | partial | local file remote rejected (intentional, see push.md) |
| fetch | supported | --depth public flag |
| pull | partial | --ff-only / --rebase / --squash subset |
| diff | supported | |
| grep | supported | |
| blame | supported | |
| revert | supported | |
| remote | supported | |
| open | supported | |
| config | supported | vault-backed |
| reflog | supported | |
| worktree | intentionally-different | remove keeps disk dir by default |
| cloud | intentionally-different | Libra cloud backup/restore extension, not a Git command |
| cat-file | supported | -e does not support JSON |
| index-pack | supported | hidden plumbing command |
| checkout | partial | visible branch compatibility surface; use `restore` for file restoration |
| bisect | partial | start / bad / good / reset / skip / log / run / view supported; replay / terms deferred |

## Git commands intentionally absent from `src/cli.rs`

| Command | Tier | Notes |
|---------|------|-------|
| submodule | unsupported | intentional product boundary (see compatibility/declined.md#d1-submodule-子命令族) |
| sparse-checkout | unsupported | no public sparse checkout command (see compatibility/declined.md#d10-clone---sparse-与顶层-sparse-checkout-命令) |

## Hooks
- Stock Git hooks at `.git/hooks` / `core.hooksPath`: `unsupported`
- AI provider hooks: `intentionally-different` (see agent.md)

## LFS compatibility notes
- `libra lfs`: `partial` command compatibility. Libra uses built-in pointer / lock management and `.libraattributes`.
- Git LFS filter bridge (`.gitattributes` smudge/clean filters + `git-lfs` hook install): `intentionally-different` (see compatibility/declined.md#d5-git-lfs-gitattributes-filter--hooks-bridge).
- Repository asset storage policy: current committed binaries remain inline; optional future Git LFS rules are tracked below as a repository governance decision, not as the `libra lfs` command status.
```

### COMPATIBILITY.md 更新路线图（C4/C5 已部分落地）

以下 roadmap 仅供维护者跟踪，**不应写入 C1 创建的 `COMPATIBILITY.md`**。各批次落地时按各自子文档的“COMPATIBILITY.md 行更新”指令修改事实表。2026-05-11 复核：C4 的 `bisect run/view` surface、C5 的 checkout 可见性和 worktree `--delete-dir` 已落地，表中对应行保留为事实索引。

| Command | 当前 Tier | 批次 | 落地后 Tier | 落地后 Notes |
|---------|-----------|------|-------------|--------------|
| fetch | supported | C3 ✅ | supported | `--depth` public flag |
| clone | partial | C3 | partial | `--depth` / `--single-branch` supported; `--sparse` unsupported; `--recurse-submodules` unsupported |
| stash | partial | C4 | partial | `show` / `branch` / `clear` added; `create` / `store` deferred |
| bisect | partial | C4 ✅ | partial | `run` / `view` added; `replay` / `terms` deferred |
| checkout | partial | C5 ✅ | partial | visible branch compatibility surface; use `restore` for file restoration |
| worktree | intentionally-different | C5 ✅ | intentionally-different | `remove` keeps disk dir by default; `--delete-dir` for Git-style behavior |
| submodule | — | C6 | unsupported | intentional product boundary (see compatibility/declined.md) |
| sparse-checkout | — | C6 | unsupported | no public sparse checkout command |

### 填充策略

C1 初次创建 `COMPATIBILITY.md` 时必须记录**当前代码状态**。上表 roadmap 信息不得出现在 C1 创建的 `COMPATIBILITY.md` 正文中；每批实际落地后再在自己的子文档里更新 `COMPATIBILITY.md` 对应行。

### .gitattributes 最小集

```gitattributes
# Text normalization
* text=auto eol=lf

# Shell / scripts
*.sh   text eol=lf
*.bash text eol=lf
*.ps1  text eol=crlf
*.bat  text eol=crlf
*.cmd  text eol=crlf

# Source code
*.rs   text eol=lf
*.toml text eol=lf
*.yml  text eol=lf
*.yaml text eol=lf
*.md   text

# Binary assets currently in repo
*.png  binary
*.gif  binary
*.jpg  binary
*.jpeg binary
*.ico  binary
*.webp binary
*.mp4  binary
*.pdf  binary

# Test fixtures (preserve byte-for-byte)
tests/data/** -text
```

### 未来 LFS 叠加伪代码（不在本批启用）

当总二进制体量超过 50MB 或单文件超过 5MB 时，按以下规则叠加（不重写历史，仅对新增文件生效）：

```gitattributes
# LFS opt-in (DEFERRED — do not enable in C1)
# *.gif filter=lfs diff=lfs merge=lfs -text
# *.mp4 filter=lfs diff=lfs merge=lfs -text
# docs/video/** filter=lfs diff=lfs merge=lfs -text
```

启用前提：仓库维护者评估存储/带宽成本并在独立 RFC 中决策；本计划仅占位。

### Code Owners 立场

`.github/CODEOWNERS` 不作为 C1 的 improvement 产物维护。若未来需要 Code Owners review，维护者应在独立治理流程中确认团队 handle、仓库权限和 branch protection，再单独引入对应配置；本计划不保存半可执行的 CODEOWNERS 占位。

### 分支保护平台 Runbook（不计入代码 improvement 验收）

以下内容是维护者在 GitHub UI 中执行的外部平台 runbook，不是仓库代码提交可以自动完成的 improvement 交付物，也不作为本批代码验收的未完成 checkbox：

- 要求 PR 才能合并；禁止直接 push 到 `main`。
- 至少 1 个 reviewer 批准。
- 如平台治理另行要求，启用 Code Owners review；本 improvement 不维护 `.github/CODEOWNERS`。
- 要求 status checks 通过——配合 C2 落地后的唯一 job 名：`compat-rustfmt` / `compat-clippy` / `compat-redundancy` / `compat-offline-core` / `compat-network-remotes` / `security-codeql-actions` / `security-codeql-rust`（live 矩阵不作为 required，因受网络/凭据影响）。
- 启用 linear history。
- 禁止 force push。
- 视团队执行情况启用 signed commits（与 contributing.md 中 PGP 要求对齐）。

### required-checks 平台切换 runbook（C2 落地后由维护者在 GitHub UI 执行）

C2 把 `.github/workflows/base.yml` 与 `.github/workflows/codeql.yml` 的 `name:` 字段
统一到 `compat-*` / `security-*` 命名后，旧的 required-checks 标识（`Rustfmt Check`
等）已经不会再出现。维护者必须按以下顺序在 GitHub UI 切换：

- Settings → Branches → main → "Require status checks" 列表，移除旧名称：`Rustfmt Check` / `Clippy Check` / `Redundancy Check` / `Run Tests` / `Analyze (...)`。
- 同一列表中加入新名称：`compat-rustfmt` / `compat-clippy` / `compat-redundancy` / `compat-offline-core` / `compat-network-remotes` / `security-codeql-actions` / `security-codeql-rust`。
- **不要**把 `compat-live-ai` / `compat-live-cloud`（占位 / 未来 workflow_dispatch）加入 required-checks——这些依赖外部凭据，会让 PR 阻塞在配置外因素上。
- 在临时分支故意提交一个会失败的 fmt 改动，确认 PR 显示 `compat-rustfmt` 失败并阻塞合并。
- 在临时分支故意 lint 失败，确认 `compat-clippy` 阻塞合并。

切换完成前不要 merge C2 自身——若 main 已经在跑新 job 名而 branch protection 还
锁着旧名，所有 PR 会出现"required check 永远 pending"。

## 关键文件与改动

| 文件 | 操作 | 说明 |
|-----|-----|-----|
| [`/COMPATIBILITY.md`](../../../COMPATIBILITY.md) | 维护 | 仓库根，4-tier 矩阵 |
| [`/.gitattributes`](../../../.gitattributes) | 维护 | 仓库根，文本归一化 + binary 声明 |
| [`docs/improvement/README.md`](../README.md) | 修改 | "全局层面改进" 表追加 I 行 |
| [`docs/commands/README.md`](../../commands/README.md) | 修改 | 同步现有命令索引，补 `code-control` |

## 测试与验收

- [x] (v0.17.11) [`.gitattributes`](../../../.gitattributes) 中 `*.rs` 规则声明 `text eol=lf`，覆盖 `src/cli.rs` 这类 Rust 源文件。
- [x] (v0.17.11) [`.gitattributes`](../../../.gitattributes) 中 `*.png` 规则声明 `binary`，覆盖 `docs/image/banner.png` 这类 PNG 资产。
- [x] (v0.17.13) `COMPATIBILITY.md` 的 "Top-level commands" 表逐一覆盖 `src/cli.rs::Commands` 变体（含 hidden 命令）；`submodule` / `sparse-checkout` 等不存在于 CLI 的 Git 命令放在 "intentionally absent" 表，未混入顶层命令表。
- [x] (v0.17.11) `COMPATIBILITY.md` 中 `lfs` 命令行与 Git LFS filter / hooks 兼容说明分开描述，不再出现笼统的 "LFS unsupported"。
- [x] (v0.17.11) [`docs/improvement/README.md`](../README.md) 中 "全局层面改进" 表新增一行指向 [`compatibility/README.md`](README.md)。
- [x] (v0.17.11) [`docs/commands/README.md`](../../commands/README.md) 中现有顶层命令索引覆盖 `code-control`；checkout / bisect hidden 标记在对应 C5 / C4 批次处理。

## 风险与缓解

1. **`.gitattributes` 影响历史 diff 显示** → 缓解：text=auto eol=lf 对已有 LF 文件无效；仅在新平台 checkout 时归一化。
2. **`COMPATIBILITY.md` 与代码不同步** → 缓解：C2 已在 `compat-offline-core` job 中加入 `scripts/check_compat_matrix.sh`，并通过 `tests/compat/matrix_alignment.rs` 接入 `cargo test --all`，扫描 `src/cli.rs` Commands 变体并对比矩阵行。
