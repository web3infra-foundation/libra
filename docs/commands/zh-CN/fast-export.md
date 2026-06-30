# `libra fast-export`

把某修订可达的历史导出为 `git fast-import` 流 —— `git fast-export` 的一个聚焦子集。只读：绝不写对象或 refs。

## 用法

```
libra fast-export [<rev>]
```

## 说明

`fast-export` 按从旧到新遍历从 `<rev>`（默认 `HEAD`）可达的提交，向 stdout 写出 fast-import 流：

- 每个 blob 仅发出一次并带 `mark`；
- 每个提交发出其 `author`/`committer`/`data`（消息）、到父提交的 `from`/`merge`，随后 `deleteall` + 每个文件一行 `M` —— 用**整树重建**而非对父做 diff。流比 Git 的 diff 形式更大，但逐字节等价。

提交在 `<rev>` 解析到的分支 ref 下发出（`HEAD`/分支名 → `refs/heads/<branch>`）。

## 选项

| 选项 | 说明 | 示例 |
|------|------|------|
| `<rev>` | 要导出其可达提交的修订（默认 `HEAD`）。 | `libra fast-export main` |

## 退出码

| 退出码 | 含义 |
|--------|------|
| `0` | 已写出流。 |
| `128` | 不在仓库内、修订无法解析，或对象/IO 错误。 |

## 示例

```bash
# 把当前分支保存为流
libra fast-export > repo.fastimport

# 管道给另一个导入器
libra fast-export main | git fast-import --quiet   # 在另一个仓库中
```

## 与 Git 对比

| 任务 | Libra | Git |
|------|-------|-----|
| 导出历史 | `libra fast-export <rev>` | `git fast-export <rev>` |

差异与延后项：输出用整树重建（`deleteall` + `M` 列表）而非父 diff，故更大；一次导出多个 ref、附注/签名 tag、`--export-marks` / `--import-marks`、blob/path 过滤暂不支持。
