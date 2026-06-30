# fast-import 命令开发设计

## 命令实现目标

`libra fast-import` —— 导入 `git fast-import` 流（对象 + refs）。GGT-13 互操作池的最后一个、最重的命令；fast-export 的反向。事务性写入 + 资源边界。

## 对比 Git 与兼容性

- 兼容级别：`partial`。
- 已支持指令：`blob`(mark/data)、`commit <ref>`(mark/author?/committer/data/from/merge/`M <mode> <dataref> <path>`/`D <path>`/deleteall)、`reset <ref>`(from?)、`checkpoint`、`done`；宽松前导 `feature`/`option`/`progress`（忽略）。data 支持 counted `data <n>` 与 here-doc `data <<DELIM`。
- **延后/拒绝**：`tag`/`cat-blob`/`ls`/`get-mark`/note(`N`)/复制重命名(`C`/`R`)（拒绝）；仅持久化 `refs/heads/*`（其他命名空间解析但不写）；marks 文件 import/export；多 GiB 真流式（当前逐行+定量读，但全量计数）。

## 设计方案

- 入口与分发：`src/cli.rs::Commands::FastImport` → `command::fast_import::execute_safe`（require_repo；stdin 或 `--input`）。
- 源码分层：`src/command/fast_import.rs`：`FastImportArgs`(input/max_count/quiet)、`Importer<R: BufRead>`（状态机）、`configured_max_input`（读 `fastimport.maxInputSize`）。
- 复用：`tree_plumbing::write_tree_from_leaves`（M/D 应用后 path→(mode,id) → tree）、`command::load_object`（from 的父树 flatten）/`save_object`（blob/commit 写入）、`Signature::from_data`（解析 `<kind> <name> <email> <ts> <tz>`）、`Commit::new`、`Branch::update_branch`、`util::is_valid_refname`、`get_hash_kind().hex_len()`。
- 解析：`next_line`（read_until \n + 单行 pushback `pending_line` 供 commit 体前瞻）；`read_data_payload`（counted：account+read_exact+消费可选尾 LF via fill_buf/consume；here-doc：逐行到 DELIM）。
- commit 树构建：from → flatten 父树到 `HashMap<path,(TreeItemMode,oid)>`；M 插入、D 删（含子树前缀）、deleteall 清空；`write_tree_from_leaves`。
- **事务**：对象立即写；ref 更新缓冲在 `pending_refs`，仅在 `checkpoint`/`done`/干净 EOF 提交（`flush_refs`）。截断流在 flush 前报错→refs 不更新（孤立对象由 gc 回收）。
- **资源/校验**：`account` 累计字节（含命令行、定量 data、以及 data 后的可选尾 LF）> max_input(默认 1GiB / config) → 128；`save` 计数（仅 blob + commit；tree 经 tree_plumbing 内部写，不计入）> max_objects(默认 10^6 / --max-count) → 128；ref 必须 `refs/…` + is_valid_refname（否则仓库外/非法 → 128）；字面 oid 必须 hex_len 匹配（hash 格式 → 128）；重复 mark → 128。
- 底层操作对象：写对象库（blob/tree/commit）+ 更新 `refs/heads/*`（SQLite）。

## 实现历史

- 2026-06-30（GGT-13 / 5 最后一个，`grit-gap.md` 阶段 6）：互操作池收官命令；独立增量。

## 当前状态

- 公开状态：已公开（`Commands::FastImport`）。
- 测试：`tests/command/fast_import_test.rs`（手工流建分支+对象 [rev-parse/log 验证]、**fast-export 流往返**导入、`--input` 文件、拒绝仓库外 ref 128、拒绝重复 mark 128、输入超限 128（config maxInputSize=10）、非仓库 128）+ `fast_import.rs` 单测（trim_newline/split_first/unquote_path/apply_delete 子树）。
- 用户文档：`docs/commands/fast-import.md`（EN + zh-CN）。

## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| 指令 | tag/cat-blob/ls/get-mark/N/C/R | 拒绝（128）；文档列明。 |
| refs | 非 `refs/heads/*` 命名空间持久化 | 解析但不写；记录延后。 |
| marks | `--import-marks`/`--export-marks` | 延后。 |
| 流式 | 多 GiB 真流式（当前全量字节计数） | 逐行+定量读已避免全量缓冲；计数为近似（树由 write_tree 内部写，不计入对象上限）。 |

## 维护要求

- 改进本命令前先阅读 [docs/development/commands/_general.md](_general.md)。
- ⚠️ 事务边界（checkpoint/done/clean-EOF flush）是崩溃一致性的核心——改动 flush 时机必须保持「截断流不更新 refs」。
- 树写入必须复用 `tree_plumbing`（与 write-tree/read-tree 一致），不得另写 tree 序列化。
