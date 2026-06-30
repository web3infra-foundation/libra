# `libra fast-import`

把 `git fast-import` 流导入仓库 —— `git fast-import` 的一个聚焦子集。是 [`fast-export`](fast-export.md) 的自然反向。

## 用法

```
libra fast-import [--input <file>] [--max-count <n>] [--quiet]
```

## 说明

`fast-import` 从 stdin（或 `--input <file>`）读取 fast-import 流，写入其描述的对象与 refs。支持的指令：

- `blob`（含 `mark` / `data`）；
- `commit <ref>`（含 `mark`、`author`、`committer`、`data`（消息）、`from`、`merge`，以及文件操作 `M <mode> <dataref> <path>`、`D <path>`、`deleteall`）；
- `reset <ref>`（含可选 `from`）；
- `checkpoint`、`done`；
- 宽松的前导 `feature` / `option` / `progress`（接受并忽略）。

`tag`、`cat-blob`、`ls`、`get-mark`、note（`N`）、复制/重命名（`C` / `R`）暂不支持，会被拒绝。

### 事务模型

对象在解析时即写入，但 **ref 更新被缓冲**，仅在 `checkpoint`、`done` 或干净的流结束时提交。中途被截断的流会在提交前失败，故分支绝不会半更新；孤立对象未被引用，由后续 `libra gc` 回收。中断导入后恢复：先 `libra fsck` 再 `libra gc`。

### 安全与资源上限

- 输入总量有上限（默认 **1 GiB**，可由 `fastimport.maxInputSize` 配置）。
- 创建的 **blob 与 commit** 数有上限（默认 **1,000,000**）；用 `--max-count <n>` 提升。（tree 为派生对象，经共享 `write-tree` 路径写入，不单独计数。）
- refs 必须在 `refs/…` 下、合法、且绝不逃出仓库。
- 字面引用的对象 id 必须匹配仓库 hash 长度（SHA-1 / SHA-256）；重复 mark 被拒绝。

## 选项

| 选项 | 说明 |
|------|------|
| `--input <file>` | 从文件而非 stdin 读取流。 |
| `--max-count <n>` | 提升本次导入的 blob+commit 数上限。 |
| `--quiet` | 抑制末尾汇总行。 |

## 退出码

| 退出码 | 含义 |
|--------|------|
| `0` | 流导入成功。 |
| `128` | 不在仓库内、流损坏、重复 mark、非法/仓库外 ref、hash 格式不匹配、资源超限，或 IO 错误。 |

## 示例

```bash
# 通过流往返历史
libra fast-export main | libra fast-import

# 导入已保存的流
libra fast-import < repo.fastimport
libra fast-import --input repo.fastimport
```

## 与 Git 对比

| 任务 | Libra | Git |
|------|-------|-----|
| 导入流 | `libra fast-import` | `git fast-import` |

差异与延后项：仅持久化分支 ref（`refs/heads/*`）（其他命名空间会被解析但暂不写入）；`tag`、`cat-blob`、`ls`、`get-mark`、notes、复制/重命名、marks 文件导入/导出（`--import-marks` / `--export-marks`）、以及对多 GiB 输入的真正流式处理暂未实现。
