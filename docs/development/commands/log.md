# `libra log` 开发设计

## 命令实现目标

`libra log` 的目标是展示提交历史，并支持数量、时间、作者、graph、format、颜色、revision range（通过 `--range`）、`--all`、`--reverse`、`--follow`、`-L` 和 `--parents`/`--children` 等常用查看能力。实现需要在 Git 兼容历史查看和 Libra 结构化输出之间保持一致，并把尚未覆盖的复杂格式能力列为后续工作。

## 对比 Git 与兼容性

- 兼容级别：`partial`。

- 当前矩阵承诺常用 Git log 子集已支持；`--range`（revision ranges）、`--all`、`--reverse`、`--follow`、`-L`、`--parents`/`--children`、`-i`/`--regexp-ignore-case`、`--invert-grep` 已补齐，但 Git 位置性 revision range、复杂 rename follow 和精确行级归属仍为 partial。新增语义必须同步矩阵、用户文档和测试。


## 设计方案

- 入口与分发：已公开接入 `src/cli.rs::Commands`；已由 `src/command/mod.rs` 导出。CLI 层在 `src/cli.rs` 把解析后的参数交给命令模块，命令模块负责把领域错误转换为 `CliError` / `CliResult`。
- 源码分层：主要实现文件为 `src/command/log.rs`。参数/子命令类型包括：`LogArgs`；输出、错误或状态类型包括：`LogCommitEntry`、`LogOutput`；主要执行函数包括：`execute`、`execute_safe`。
- 执行路径：`execute_safe` 负责 CLI 安全包装、错误映射和输出配置；对象路径会解析 revision 并读写 blob/tree/commit/tag 等对象；引用路径会读取或更新 SQLite refs、HEAD 与 reflog。

- 流程图：以下流程图按当前源码分层展示主路径和底层对象边界，便于维护者把代码入口、执行函数和副作用范围对应起来。

```mermaid
flowchart TD
    A["入口与分发<br/>src/cli.rs::Commands"] --> B["源码分层<br/>src/command/log.rs"]
    B --> C["参数模型<br/>LogArgs"]
    C --> D["执行路径<br/>execute / execute_safe"]
    D --> E["底层对象<br/>Blob / Commit / Tree / Branch"]
    D --> F["输出与错误<br/>LogCommitEntry / LogOutput"]
    E --> G["副作用边界<br/>写入分支需先预检"]
```

- 底层操作对象：`Blob`（文件内容或 LFS pointer 写入对象库后的 blob 对象）；`Commit`（提交对象、父提交关系和提交消息载荷）；`Tree`（由索引或对象遍历生成的目录树对象）；`Branch` / branch store（SQLite refs 上的分支读写、过滤和上游关系）；`Head`（SQLite 中的 HEAD 指向、当前分支和 detached 状态）；`ObjectHash`（SHA-1/SHA-256 对象 ID 和 revision 解析结果）；`ConfigKv`（配置键值持久化行）
- 输出与错误契约：人类输出、`--json` / `--machine` 输出和 quiet/verbose 分支必须继续走现有 `OutputConfig` / `emit_json_data` / `CliError` 路径；新增失败模式要补稳定错误码、用户提示和回归测试。
- 副作用边界：凡是写入索引、对象库、refs/HEAD、reflog、SQLite/D1、工作树或远端的路径，都必须先完成参数校验和 dry-run/预检分支，再执行持久化，避免部分写入后静默成功。

## 实现历史

- 本节依据本地 main 分支提交历史重写，筛选与该命令实现、测试或文档路径直接相关的提交；以下是归纳后的实现脉络。
- 2025-12-19 `d45bec8c`（`feat(log): add --abbrev-commit/--abbrev/--no-abbrev-commit for commit… (#93)`）：基础实现节点：add --abbrev-commit/--abbrev/--no-abbrev-commit for commit… (#93)；当前实现的主要轮廓可追溯到该提交。
- 2026-06-06 `f95b80df`（`feat(log): colorize graph columns and align compatibility matrix`）：功能演进：colorize graph columns and align compatibility matrix；该节点扩展了当前命令可用的参数或行为。
- 2026-06-06 `89045f35`（`feat(log): support revision ranges (A..B, A...B, ^A B)`）：当前 HEAD 通过 `--range <SPEC>` 保留 revision range 入口，但未复刻 Git 的位置性 `git log A..B` 语法；该差异继续列在“还未实现的功能”。
- 2026-06-07 `155a430a`（`fix(log): close compatibility plan gaps`）：实现修正：close compatibility plan gaps；该节点把边界行为、错误处理或兼容差异纳入当前实现约束。
- 历史结论：当前文档应以这些提交之后的代码、测试和兼容矩阵为准；更早的迁移式文档只保留为背景，不再作为事实来源。

## 当前状态

- 公开状态：已公开；模块状态：已导出。
- 用户文档：`docs/commands/log.md`。
- Synopsis：`libra log [OPTIONS] [PATHS]...`。
- 公开参数/子命令包括：`-n, --number <NUMBER>`（Git 别名 `--max-count`）、`--oneline`、`--abbrev-commit`、`--abbrev <N>`、`--no-abbrev-commit`、`-p, --patch`、`--name-only`、`--name-status`、`--author <PATTERN>`、`--committer <PATTERN>`、`--since <DATE>`、`--until <DATE>`、`--merges`、`--no-merges`、`--min-parents <N>`、`--max-parents <N>`、`--first-parent`、`-S <STRING>`、`-G <REGEX>`、`--skip <N>`、`--pretty <FORMAT>`、`--format <FORMAT>`（`--pretty` 的 Git 别名）、`--date <FORMAT>`、`--decorate[=<MODE>]`、`--no-decorate`、`--graph`、`--stat`、`--shortstat`（仅 diffstat 摘要行）、`--grep <PATTERN>`、`-i, --regexp-ignore-case`、`--invert-grep`、`--reverse`、`--author-date-order`（按作者日期而非提交者日期排序，newest-first；经 `sort_commits_newest_first` 仅按时间戳排序，无 Git 的拓扑约束）、`--date-order`（接受式 no-op，显式选择默认的提交者日期顺序，与 `--author-date-order` 互斥）、`--all`、`--follow <FILE>`、`-L <RANGE:FILE>`、`--parents`、`--children`、`--range <SPEC>`、位置参数 `[PATHS]...`（限定 diff 输出范围，无需 `--` 分隔符）等。`-i`/`--regexp-ignore-case` 让 `--grep` 大小写不敏感（author/committer 在 Libra 中本就大小写不敏感，故 `-i` 仅作用于 `--grep`）；`--invert-grep` 保留消息**不**匹配 `--grep` 的提交（在 `CommitFilter::with_grep_options` 中按 `matches == invert_grep` 排除）。`--parents`/`--children`（互斥）在每个提交哈希后追加缩写后的父/子提交 id：父来自 `commit.parent_commit_ids`，子在所展示提交（已渲染集合）范围内反向计算（与 rev-list 的子映射同算法，但作用于 log 的渲染集，不含范围外的子提交），经 `FormatContext.extra_hashes` 进入 full / oneline 格式。
- `--committer <PATTERN>`：按 committer name/email 的大小写不敏感子串过滤（对照 `--author`）。`--merges`/`--no-merges` 和 `--min-parents`/`--max-parents <N>`：按父提交数过滤（merges=≥2，no-merges=≤1，显式 min/max 优先）。`--first-parent`：遍历时只跟随合并提交的第一个父提交，折叠被并入的侧分支历史。`-S <STRING>`：pickaxe，仅显示改变了 STRING 出现次数的提交（对每个被改动文件比较其在该提交与第一父提交中的内容出现次数，总数变化即匹配；大小写敏感字面匹配）。`-G <REGEX>`：pickaxe，仅显示 diff 的新增/删除行中存在匹配该正则的提交（基于 `compute_diff`；与 `-S` 互斥）。`--skip <N>`：在输出前跳过前 N 个匹配提交（在过滤之后、`-n` 限制之前，对人类与 JSON 两条输出路径一致）。`--date=<mode>`：作者/提交日期渲染模式（`short`/`iso`/`iso-strict`/`rfc`/`unix`/`raw`，其它值回退默认形式），作用于人类输出（Full 与 `--pretty` 的 `%ad`/`%cd`）；时间以 UTC 渲染（时区 `+0000`），JSON 输出仍用规范日期。`relative`/`human`/`local` 暂未实现。
- `--pretty=<value>`：识别 `oneline` 预设与 `format:<tmpl>`/`tformat:<tmpl>` 前缀（携带自定义模板），`medium`（及空值）映射为默认完整格式；其它值按原始自定义模板处理。`oneline` 之外的命名预设（short/full/fuller/raw）暂未单独渲染。


## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| 兼容矩阵说明 | common Git log surface plus `--range` revision expressions, `--all`, `--reverse`, `--follow`, `-L`, and `--parents`/`--children` supported | 按当前兼容矩阵保留；实现状态变化时同步 `_compatibility.md` 和测试证据。 |
| 功能缺口 | Git 原生位置性 revision range 语法（`A..B`、`A...B`、位置参数）未完全复刻；当前通过 `--range` 标志提供 | 后续实现时需要同步源码、测试和兼容矩阵。 |
| 功能缺口 | `--follow` 重命名跟踪基于 best-effort blob 匹配，不保证复杂重命名场景 | 后续实现时需要同步源码、测试和兼容矩阵。 |
| 功能缺口 | `-L` 行级历史跟踪为 best-effort，尚未实现精确 blame 级行归属 | 后续实现时需要同步源码、测试和兼容矩阵。 |

## 维护要求

- 改进本命令前，必须先阅读并遵循 [docs/development/commands/_general.md](_general.md)；这是命令设计、实现、测试和文档同步的强制要求。
- 任何行为变更都要先核对实现源码，再同步 `COMPATIBILITY.md`、`docs/commands/<cmd>.md` 和相关测试。
- 新增 Git 兼容参数时必须明确 tier、错误码、JSON/机器输出契约和回归测试。
