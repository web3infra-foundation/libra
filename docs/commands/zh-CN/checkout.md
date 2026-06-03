# `libra checkout`

显示当前分支、切换到已有分支、创建并切换到新分支，或通过显式 `--` 兼容形式恢复路径。
为常见分支操作与显式路径恢复提供 `git checkout` 兼容面。

## 概要

```
libra checkout [-f] [<branch>]
libra checkout -b <name>
libra checkout -B <name> [<start-point>]
libra checkout --detach [<commit-ish>]
libra checkout --orphan <name> [<start-point>]
libra checkout (--ours | --theirs) -- <pathspec>...
libra checkout [<tree-ish>] -- <pathspec>...
```

## 说明

`libra checkout` 是一个 Git 兼容面，内部委托给 `switch` 和 `restore`。它支持最常见的 `git checkout` 用法：显示当前分支、切换到已有分支、用 `-b` 创建新分支、用 `-B` 强制创建或重置分支、用 `--detach` 分离 HEAD、用 `--orphan` 开启无历史分支、用 `-f` 在工作区有改动时强制切换、用 `--ours`/`--theirs` 把冲突路径恢复到某一侧、自动跟踪远程分支，以及在出现显式 `--` 分隔符时恢复路径。

这个命令的存在是为了让从 Git 迁移的开发者复用熟悉的肌肉记忆。对于新的工作流，请优先使用 `libra switch`（分支操作）和 `libra restore`（文件操作），它们提供更丰富的错误信息、结构化 JSON 输出与更清晰的语义。

当检出的分支名在本地不存在但匹配某个远程跟踪分支（如 `origin/feature`）时，Libra 会自动创建本地跟踪分支、设置 upstream 并 pull —— 比 Git 的自动跟踪更进一步，会立即同步内容。

路径恢复只有在显式 `--` 分隔符出现时才启用。没有 `--` 时，`libra checkout <name>` 始终是分支模式，即使存在同名文件也是如此。

内部的 `intent` 与 `agent-traces` 分支受保护：`-b`/`-B`/`--orphan`/`--detach` 都会拒绝创建、重置或检出这些 AI 托管引用（位置参数 commit-ish 采用 revision 感知检查，所以 `agent-traces~1` 也会被拒绝）。`main` 始终允许。

### 分支控制模式

- **`-B <name> [<start-point>]`** —— 分支不存在则创建，已存在则重置到 start point（默认当前 HEAD），然后切换。会记录一条 `checkout` reflog 条目。
- **`--detach [<commit-ish>]`** —— 把 HEAD 移动到解析出的提交（默认当前 HEAD）并进入分离态，不检出任何分支。
- **`--orphan <name> [<start-point>]`** —— 把 HEAD 指向一个新的、尚未诞生（unborn）的分支（在首次提交前不存在 `reference` 行）。索引/工作区会对齐到 start point，与 Git 的 “如同执行了 `checkout <start-point>`” 一致。与 Git 一致，`--orphan` **不**写 HEAD reflog 条目（目标还没有提交 OID）。孤儿分支的首个提交没有父提交。
- **`-f` / `--force`** —— 跳过工作区脏检查，让目标覆盖未提交改动（对普通切换、`-B`、`--detach`、`--orphan` 均生效）。

### 冲突路径检出（`--ours` / `--theirs`）

`--ours` / `--theirs` 仅作用于 `--` 之后的路径，且只在这些路径处于合并冲突态时生效：

- **`--ours`** 把合并 stage #2（我方）恢复到工作区。
- **`--theirs`** 把合并 stage #3（对方）恢复到工作区。

无论哪一侧，路径都会被收敛为干净的 stage #0 索引条目（保留其 mode），并丢弃剩余的冲突 stage。对一个非冲突态路径执行 `--ours`/`--theirs` 是错误——文件绝不会被静默改写。`--ours` 与 `--theirs` 互斥，且都需要在 `--` 之后给出 pathspec。

## 选项

| 标志 | 短选项 | 长选项 | 说明 |
|------|-------|------|-------------|
| 目标分支 | | `<branch>` | 要切换到的目标分支（可选）。省略则显示当前分支。 |
| 新建分支 | `-b` | | 从当前 HEAD 创建新分支并切换。 |
| 创建/重置分支 | `-B` | | 创建分支，或将其重置到 start point（或当前 HEAD），然后切换。 |
| 分离 HEAD | | `--detach` | 在给定 commit-ish（或当前 HEAD）处分离 HEAD，而不是切换到分支。 |
| 孤儿分支 | | `--orphan <name>` | 创建一个新的 unborn 分支，其首个提交没有父提交。 |
| 取我方 | | `--ours` | 在冲突路径上检出我方（stage #2）；需要 `-- <path>`。 |
| 取对方 | | `--theirs` | 在冲突路径上检出对方（stage #3）；需要 `-- <path>`。 |
| 强制 | `-f` | `--force` | 强制检出：即使工作区有会被覆盖的改动也继续。 |
| 路径恢复 | | `[<tree-ish>] -- <pathspec>...` | 恢复路径。无 `<tree-ish>` 时从索引恢复工作区；有 `<tree-ish>` 时从该来源恢复索引与工作区。 |

## 示例

```bash
# 显示当前分支
libra checkout

# 切换到已有本地分支
libra checkout main

# 创建并切换到新分支
libra checkout -b feature-x

# 创建或将分支重置到当前 HEAD，然后切换
libra checkout -B feature-x

# 在某个历史提交处分离 HEAD
libra checkout --detach HEAD~1

# 开启一个无历史的新分支
libra checkout --orphan fresh-start

# 强制切换，丢弃未提交的本地改动
libra checkout -f main

# 把冲突路径恢复到我方 / 对方
libra checkout --ours -- src/conflicted.rs
libra checkout --theirs -- src/conflicted.rs

# 从索引恢复路径到工作区
libra checkout -- src/main.rs

# 从 HEAD 恢复路径到索引与工作区
libra checkout HEAD -- src/main.rs
```

## 常见命令

```bash
libra checkout                         # 显示当前分支
libra checkout main                    # 切换到已有本地分支
libra checkout -b feature-x            # 创建并切换到新分支
libra checkout -B feature-x            # 创建或将分支重置到 HEAD，然后切换
libra checkout --detach HEAD~1         # 在某提交处分离 HEAD
libra checkout --orphan fresh          # 开启无历史的新分支
libra checkout -f main                 # 强制切换，丢弃本地改动
libra checkout --ours -- file.txt      # 取冲突路径的我方
libra checkout --theirs -- file.txt    # 取冲突路径的对方
libra checkout -- file.txt             # 从索引恢复文件到工作区
libra checkout HEAD -- file.txt        # 从 HEAD 恢复文件到索引 + 工作区
libra --json checkout main             # 结构化兼容输出
```

## 人类可读输出

默认人类模式将结果写到 `stdout`。

显示当前分支：

```text
Current branch is main.
```

显示分离 HEAD：

```text
HEAD detached at abc1234d
```

切换到已有分支：

```text
Switched to branch 'main'
```

创建并切换到新分支：

```text
Switched to a new branch 'feature-x'
```

`-B` 重置已有分支：

```text
Reset branch 'feature-x'
Switched to branch 'feature-x'
```

`--quiet` 抑制所有 `stdout` 输出。

## 结构化输出（JSON）

`checkout` 在兼容面上支持 `--json` 与 `--machine`。`--json` 输出常规命令信封；`--machine` 输出同样的信封，但压缩为单行 NDJSON。嵌套的 `restore`、分支 upstream 与 pull 输出会被抑制，使得 stdout 只包含 checkout 结果。

| Action | 触发场景 |
|--------|----------|
| `show-current` | 无分支参数的 `libra checkout` |
| `already-on` | 目标分支已检出 |
| `switch` | 切换到已有本地分支 |
| `create` | `checkout -b <branch>`，或 `-B`/`--orphan` 创建新分支 |
| `reset` | `checkout -B <branch>` 重置已存在分支（设置 `reset: true`） |
| `detach` | `checkout --detach [<commit-ish>]`（设置 `detached: true`） |
| `track` | 由 `origin/<branch>` 创建本地分支并尝试 pull |
| `restore-paths` | 显式 `checkout [<tree-ish>] -- <pathspec>...` 路径恢复，包括 `--ours`/`--theirs` |

`--orphan` 输出 `action: "create"`、`orphan: true`，且 `commit` 为 null（分支尚未诞生）。

## 错误处理

`checkout` 拥有专属的 typed `CheckoutError`，并在委托路径恢复给 `restore` 时保留稳定错误码。

退出码遵循 Libra 的粗粒度分类契约：`Cli` 类稳定码（`LBR-CLI-002` / `LBR-CLI-003`）退出 **129**；`Repo` / `Conflict` / `Io` / `RepoCorrupt` 类退出 **128**；clap 解析失败退出 **2**，但已存在子命令的 clap 参数冲突（如 `--detach -b`、`--ours --theirs`）会被 Libra 重映射为 `command_usage` → **129**。

| 场景 | 稳定码 | 退出码 |
|------|--------|--------|
| 工作区脏（未暂存/未提交改动）且无 `-f` | `LBR-REPO-003` | 128 |
| 未跟踪文件会被覆盖 | `LBR-CONFLICT-002` | 128 |
| 检出内部（`intent`/`agent-traces`）分支被拦截 | `LBR-CLI-003` | 129 |
| 创建/重置内部分支被拦截 | `LBR-CLI-003` | 129 |
| 分支不存在（且无远程匹配） | `LBR-CLI-003` | 129 |
| `-b` 与路径模式混用 | `LBR-CLI-002` | 129 |
| `--ours`/`--theirs` 缺少 pathspec | `LBR-CLI-002` | 129 |
| `--ours`/`--theirs` 作用于非冲突路径 | `LBR-CONFLICT-002` | 128 |
| `--ours --theirs` 同用（clap 冲突） | `LBR-CLI-002` | 129 |
| 冲突 stage 检出时索引/对象读失败 | `LBR-IO-001` | 128 |
| 工作区/索引写失败 | `LBR-IO-002` | 128 |
| HEAD/分支引用写失败 | `LBR-IO-002` | 128 |
| 当前分支（no-op） | 无 | 0 |

## 设计动机

`checkout` 作为兼容命令保留，是为了降低 Git 用户的迁移门槛；推荐的日常用法仍是 `switch`（分支导航）与 `restore`（文件恢复）。本批次将分支切换、`-B` 强制重置、分离 HEAD、孤儿分支、`--ours`/`--theirs` 冲突路径恢复与 `-f` 强制覆盖统一纳入同一条安全控制流：先防止破坏内部保留分支与未提交数据，再对齐 Git 的 CLI、stdout/stderr、退出码（128/129/2 体系）与状态机语义。
