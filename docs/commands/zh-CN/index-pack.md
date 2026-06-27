# `libra index-pack`

为已有 `.pack` 归档构建 `.idx` 索引文件。

## 概要

```
libra index-pack [OPTIONS] [<PACK_FILE>]
```

## 说明

`libra index-pack` 读取 Git pack 文件并生成对应的 pack 索引（`.idx`）文件。索引文件通过将对象哈希映射到字节偏移，为 pack 内对象提供 O(1) 随机访问。

没有 `-o` 时，输出文件名通过将 `.pack` 扩展名替换为 `.idx` 得出。默认索引格式为 version 1（SHA-1 fan-out 表加 offset/hash 对）。可以用 `--index-version 2` 请求 version 2（带 CRC32 校验和，并支持大偏移）。

使用 `--stdin` 时，Libra 从标准输入读取 pack 字节。该模式要求 `-o <PATH>`，因为没有输入文件名可用于推导索引路径。Libra 会把 stdin pack 持久化到索引旁边，将输出路径扩展名替换为 `.pack`，然后从该 pack 生成目标 `.idx`。

使用 `--keep` 时，Libra 还会在 pack 旁边写入 `.keep` 文件。裸 `--keep` 创建空 keep 文件；`--keep=<MSG>` 会写入消息并追加换行，行为与 Git 的 keep-file 约定一致。

为了兼容脚本，Git 风格的 `--progress` 和 `--no-progress` 也会被接收。它们映射到 Libra 现有的全局进度模式，不会为 `index-pack` 增加单独的进度流。

`--fix-thin` 为兼容 Git 而接收，且是 **no-op**。*thin* pack 携带 `REF_DELTA` 对象、其 base 不在 pack 内；补全它意味着从仓库解析这些 base 并追加。Libra 的 pack decoder 要求自包含 pack（无外部 delta-base 解析器）、且从不产出 thin pack，故任何能成功建索引的 pack 都没有需要追加的外部 base——这正是 Git 的 `--fix-thin` 也什么都不做的场景。真正的 thin-pack 补全（解析外部 delta base）不支持。

这是一个低层 plumbing 命令。它由 `libra fetch` 和 `libra clone` 在通过网络接收 pack 数据后内部使用，也可以手动调用来重建缺失或损坏的索引文件。

## 选项

| 标志 | 短选项 | 说明 | 默认值 |
|------|-------|-------------|---------|
| `<PACK_FILE>` | | 要索引的 `.pack` 文件路径。除非使用 `--stdin`，否则必需。除非给出 `-o`，否则必须以 `.pack` 结尾。 | |
| `--stdin` | | 从标准输入读取 pack 字节。要求 `-o`；会在索引旁边写出同 stem 的 `.pack` 文件。 | 关闭 |
| `-o <PATH>` | `-o` | 生成的索引文件输出路径。 | 将 `<PACK_FILE>` 的 `.pack` 替换为 `.idx` |
| `--keep[=<MSG>]` | | 创建将 `<PACK_FILE>` 扩展名替换为 `.keep` 的 keep 文件；如果提供 `MSG`，写入消息并追加换行。 | 不创建 |
| `--index-version <N>` | | 强制索引格式版本（1 或 2）。 | `1` |
| `--progress` | | 接收 Git 风格进度请求；映射到 Libra 全局 text 进度模式。 | 全局进度模式 |
| `--no-progress` | | 接收 Git 风格进度抑制；映射到 Libra 全局 no-progress 模式。 | 全局进度模式 |
| `--fix-thin` | | 接收 Git 的 thin-pack 补全标志。no-op：Libra 要求自包含 pack（无外部 delta-base 解析器）、从不产出 thin pack，故对它建索引的 pack 无需补全。 | 关闭 |

### 示例

```bash
# 使用默认设置构建索引（version 1，自动命名）
libra index-pack objects/pack/pack-abc123.pack

# 指定自定义输出路径
libra index-pack pack-abc123.pack -o /tmp/pack-abc123.idx

# 从 stdin 读取 pack stream 并生成 /tmp/incoming.idx
cat incoming.pack | libra index-pack --stdin -o /tmp/incoming.idx

# 强制 version 2 索引格式
libra index-pack pack-abc123.pack --index-version 2

# 重建索引后保留 pack，避免被清理
libra index-pack --keep="manual recovery" pack-abc123.pack

# 接收脚本常用的 Git 风格进度标志
libra index-pack --progress pack-abc123.pack
libra index-pack --no-progress pack-abc123.pack

# 接收 Git 的 thin-pack 补全标志（对 Libra 的自包含 pack 为 no-op）
libra index-pack --fix-thin pack-abc123.pack

# 面向脚本的 JSON 输出
libra index-pack pack-abc123.pack --json
```

## 常用命令

```bash
libra index-pack pack-123.pack
libra index-pack pack-123.pack -o pack-123.idx
libra index-pack --stdin -o pack-123.idx
libra index-pack --keep pack-123.pack
libra index-pack --progress pack-123.pack
libra index-pack --no-progress pack-123.pack
libra index-pack --fix-thin pack-123.pack
libra index-pack pack-123.pack --index-version 2
libra index-pack pack-123.pack --json
```

## 人类可读输出

成功时，人类模式会打印生成的索引路径：

```text
/tmp/pack-123.idx
```

`--quiet` 会抑制 `stdout`。

## 结构化输出（JSON 示例）

```json
{
  "ok": true,
  "command": "index-pack",
  "data": {
    "pack_file": "/tmp/pack-123.pack",
    "index_file": "/tmp/pack-123.idx",
    "index_version": 1,
    "keep_file": null
  }
}
```

Version 2 示例：

```json
{
  "ok": true,
  "command": "index-pack",
  "data": {
    "pack_file": "/tmp/pack-123.pack",
    "index_file": "/tmp/pack-123.idx",
    "index_version": 2,
    "keep_file": null
  }
}
```

Keep 文件示例：

```json
{
  "ok": true,
  "command": "index-pack",
  "data": {
    "pack_file": "/tmp/pack-123.pack",
    "index_file": "/tmp/pack-123.idx",
    "index_version": 1,
    "keep_file": "/tmp/pack-123.keep"
  }
}
```

Stdin 示例：

```json
{
  "ok": true,
  "command": "index-pack",
  "data": {
    "pack_file": "/tmp/stdin-pack.pack",
    "index_file": "/tmp/stdin-pack.idx",
    "index_version": 1,
    "keep_file": null
  }
}
```

## 设计理由

### 为什么暴露这个低层命令？

Pack 索引是大多数用户不会直接调用的 plumbing 操作。Libra 暴露它有三个原因：

1. **可调试性。** 当 fetch 或 clone 中途失败时，用户可能有一个有效 `.pack` 文件但没有 `.idx`。暴露 `index-pack` 能让他们无需重新下载即可恢复。
2. **代理工作流。** 管理 pack 文件的 AI 代理（例如用于 S3/R2 分层云存储）需要一种可编程方式生成索引。`--json` 输出让它适合脚本化。
3. **Git 兼容性。** Git 生态中的工具和脚本期望 `index-pack` 存在。提供它意味着 Libra 可以在调用 plumbing 命令的 CI 流水线中作为替代品使用。

### 为什么使用单独的 `verify-pack` 命令？

Git 通过 `verify-pack` 和部分 `index-pack` 工作流暴露验证。Libra 将索引生成和验证分开：`index-pack` 写入索引，而 [`verify-pack`](verify-pack.md) 在已有 `.idx` 和其 `.pack` 之间执行只读一致性检查。

### 为什么索引版本受限？

Libra 支持 version 1 和 version 2，覆盖 Git pack-index 规范定义的两种格式。Version 1 紧凑，并且足以支持 2 GB 以下的 pack（offset 为 32 位）。Version 2 为每个对象添加 CRC32 校验和，并为大 pack 添加 64 位 offset 表。Git 规范中没有 version 3，因此 Libra 不会发明一个。默认使用 version 1 是为了简单，也因为大多数 Libra 管理的 pack 远低于 2 GB 阈值。Version 1 还避免依赖 CRC32 计算，使快速路径更轻。

### 为什么 version 1 需要 SHA-1？

Version 1 索引格式早于 Git 的 SHA-256 迁移，并硬编码 20 字节哈希槽。Libra 在运行时强制此约束：如果仓库配置了非 SHA-1 哈希，version 1 索引生成会失败并给出明确错误。Version 2 是替代哈希算法的前进路径。

## 参数对比：Libra vs Git vs jj

| 功能 | Libra | Git | jj |
|---------|-------|-----|----|
| 从 pack 构建索引 | `libra index-pack <file>` | `git index-pack <file>` | N/A（jj 使用自己的存储） |
| 自定义输出路径 | `-o <path>` | `-o <path>` | N/A |
| 索引版本 | `--index-version 1\|2`（默认 1） | `--index-version <N>[,<offset>]`（默认 2） | N/A |
| 验证已有索引 | `libra verify-pack <idx>` | `verify-pack` / `index-pack --verify` | N/A |
| `--stdin`（从 stdin 读取 pack） | `--stdin -o <idx>`；在 idx 旁边保存同 stem `.pack` | 是 | N/A |
| `--fix-thin`（为 thin pack 添加 base） | 接受式 no-op（仅自包含 pack；无外部-base 解析器） | 是 | N/A |
| `--keep`（创建 .keep 文件） | `--keep[=<MSG>]` | 是 | N/A |
| `--threads`（并行解压） | 内部使用（8 线程） | `--threads=<N>` | N/A |
| 进度标志 | 接收 `--progress` / `--no-progress`；无专属进度流 | `--progress` / `--no-progress` | N/A |
| JSON 输出 | `--json` | 无 | N/A |
| 最大 pack 大小（v1） | 约 2 GB（32 位 offset） | 约 2 GB（32 位 offset） | N/A |
| CRC32 校验和 | 仅 version 2 | Version 2+ | N/A |
| 默认哈希 | SHA-1 | SHA-1（SHA-256 实验性） | Blake2b（内部） |

## 错误处理

| 场景 | StableErrorCode | 退出码 |
|----------|-----------------|------|
| Pack 路径不以 `.pack` 结尾（且没有 `-o`） | `LBR-CLI-002` | 129 |
| `--stdin` 未提供 `-o <PATH>` | `LBR-CLI-002` | 129 |
| `--stdin` 与 `<PACK_FILE>` 同时使用 | `LBR-CLI-002` | 129 |
| Pack 路径和索引路径相同 | `LBR-CLI-002` | 129 |
| Keep 路径和索引路径相同 | `LBR-CLI-002` | 129 |
| 无法创建 stdin 派生 pack 文件 | `LBR-IO-002` | 128 |
| 无法打开 pack 文件 | `LBR-IO-001` | 128 |
| 不支持的索引版本 | `LBR-CLI-002` | 129 |
| Pack 内容无效或损坏 | `LBR-REPO-002` | 128 |
| 索引写入失败 | `LBR-IO-002` | 128 |
| Keep 文件写入失败 | `LBR-IO-002` | 128 |
