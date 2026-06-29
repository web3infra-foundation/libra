# `libra write-tree`

把当前 index 写成一个 tree 对象并打印其对象 id —— [`read-tree`](read-tree.md) 的底层配套命令，等价于 `git write-tree`。

## 用法

```
libra write-tree
```

## 说明

`write-tree` 读取 `.libra/index`，构造一个**嵌套**的 Git tree 对象（每个目录一个 tree），把所有 tree 对象写入对象库，并打印根 tree 的对象 id。文件 mode（普通/可执行/符号链接/gitlink）会被保留，对象格式（SHA-1 / SHA-256）跟随仓库的 hash kind。

空 index 产生规范空 tree（SHA-1 下为 `4b825dc642cb6eb9a060e54bf8d69288fbee4904`）。

这是只读底层命令：它写入 tree 对象，但不移动任何 ref，也不修改 index 或工作树。

## 选项

| 选项 | 说明 | 示例 |
|------|------|------|
| `--json` / `--machine` | 结构化输出：`{ tree: "<id>" }`。 | `libra --json write-tree` |

Git 的 `--prefix=<prefix>` 与 `--missing-ok` 未公开（延后）。

## 退出码

| 退出码 | 含义 |
|--------|------|
| `0` | tree 已写入，打印其 id。 |
| `128` | 不在仓库内，或无法处理 index/tree。 |

## 示例

```bash
# 写出 index 并捕获 tree id
TREE=$(libra write-tree)

# 面向 agent 的结构化输出
libra --json write-tree
```

## 与 Git 对比

| 任务 | Libra | Git |
|------|-------|-----|
| 把 index 写成 tree | `libra write-tree` | `git write-tree` |
| 把 tree 读入 index | `libra read-tree <tree>` | `git read-tree <tree>` |
