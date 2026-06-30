# `libra bundle`

创建与检查 Git **v2 bundle** 文件 —— 一个把仓库历史装进单个文件的归档，可被 Git（或另一个 Libra）读取。`git bundle` 的一个聚焦子集。

## 用法

```
libra bundle create <file> <rev>...
libra bundle verify <file>
libra bundle list-heads <file>
```

## 说明

bundle 是一个小文本头加一个 pack：

```text
# v2 git bundle
<tip-oid> <ref-name>      （每个包含的 ref 一行）
                          （空行）
PACK……                    （所有可达对象的 v2 pack）
```

- **`create <file> <rev>...`** —— 把每个 `<rev>` 解析为一个 tip，收集这些 tip 可达的全部对象，写为一个完整（非 thin）bundle。每个 `<rev>` 成为一行 head（`<oid> refs/heads/<name>`）。文件先写到临时路径再 rename 到目标，失败绝不留下半成品。
- **`verify <file>`** —— 检查头是合法的 `# v2 git bundle`、pack 存在（`PACK` v2）、且任何 prerequisite 对象本地已有。打印 `<file> is okay` 与 heads。
- **`list-heads <file>`** —— 打印 bundle 携带的 `<oid> <ref>` head 行。

pack 用仓库的 hash kind 编码，因此 SHA-1 与 SHA-256 仓库都会产生长度正确的对象 id。

## 退出码

| 退出码 | 含义 |
|--------|------|
| `0` | 成功（已写 bundle / 有效 / 已列 heads）。 |
| `1` | `verify` / `list-heads`：bundle 无效或不可读，或缺少 prerequisite（与 `git bundle verify` 一致）。 |
| `128` | 不在仓库内，或 `create` 遇到非法修订或写入错误。 |

## 示例

```bash
libra bundle create repo.bundle main          # 打包 main 分支
libra bundle create snapshot.bundle HEAD       # 打包当前分支
git clone repo.bundle restored                 # 系统 Git 可读
libra bundle verify repo.bundle
libra bundle list-heads repo.bundle
```

## 与 Git 对比

| 任务 | Libra | Git |
|------|-------|-----|
| 创建 | `libra bundle create <f> <rev>` | `git bundle create <f> <rev>` |
| 校验 | `libra bundle verify <f>` | `git bundle verify <f>` |
| 列 heads | `libra bundle list-heads <f>` | `git bundle list-heads <f>` |

差异与延后项：仅写完整 bundle（暂无 prerequisite/thin/增量 `<rev>..<rev>`）；`unbundle` 与通过 `libra` 从 bundle 克隆未实现（用 `git clone <file>`）；`verify` 校验头与 pack 魔数而非完整 pack 校验和（完整校验用 `libra index-pack` / `libra fsck`）。
