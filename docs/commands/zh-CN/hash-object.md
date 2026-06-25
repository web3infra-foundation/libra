# `libra hash-object`

为原始文件内容或标准输入计算与 Git 兼容的对象 ID。

```bash
libra hash-object [OPTIONS] <PATH>...
libra hash-object --stdin [OPTIONS]
libra hash-object --stdin-paths [OPTIONS]
```

支持 `blob`（默认）、`commit`、`tree`、`tag` 四种 Git 对象类型；对象 id 由 `<type> <size>\0<content>` 头部计算，与 `git hash-object -t <type>` 逐字节一致。默认会校验内容是否为良构对象（blob 接受任意字节），`--literally` 跳过校验。它不会应用 clean 过滤器、attributes 或 LFS 指针转换。`--path` 作为 Git 兼容路径上下文和 stdin JSON source label 接受；在实现路径过滤前，它不会改变被哈希的字节。

只读哈希不需要 Libra 仓库，并且在没有可用仓库对象格式时默认为 SHA-1。`-w` / `--write` 需要仓库，因为它会将对象存入仓库对象数据库。

## 选项

| 选项 | 短选项 | 说明 |
|--------|-------|-------------|
| `<PATH>...` | | 要哈希的文件路径 |
| `--stdin` | | 从标准输入读取字节，而不是读取文件路径 |
| `--stdin-paths` | | 从标准输入读取文件路径（每行一个）并逐个哈希 |
| `--write` | `-w` | 将计算出的对象存入仓库对象数据库 |
| `--type <TYPE>` | `-t` | 要哈希的对象类型：`blob`（默认）、`commit`、`tree`、`tag` |
| `--literally` | | 按给定类型哈希字节，但不校验内容是否为该类型的良构对象 |
| `--path <PATH>` | | Git hash-object 兼容路径上下文标签 |
| `--no-filters` | | 显式按原始字节哈希，不使用路径过滤器 |
| `--json` | | 输出结构化 JSON 信封 |
| `--machine` | | 以一行紧凑 JSON 输出同一信封 |

## 示例

只哈希文件，不写入对象：

```bash
libra hash-object README.md
```

将文件作为 blob 对象哈希并写入：

```bash
libra hash-object -w src/main.rs
```

将文件内容作为类型化对象哈希（id 与 `git hash-object -t <type>` 一致）；除非加 `--literally`，否则会校验内容是否为该类型的良构对象：

```bash
libra hash-object -t commit commit-payload
libra hash-object -t tag --literally arbitrary-bytes
```

从标准输入哈希字节：

```bash
printf 'hello' | libra hash-object --stdin
```

使用 Git 兼容路径上下文标签哈希 stdin：

```bash
printf 'hello' | libra hash-object --stdin --path README.md
```

## 输出

人类可读输出会为每个输入打印一个对象 ID：

```text
b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0
```

结构化输出：

```json
{
  "ok": true,
  "command": "hash-object",
  "data": {
    "object_type": "blob",
    "write": false,
    "objects": [
      {
        "source": "-",
        "oid": "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0",
        "size": 5,
        "written": false
      }
    ]
  }
}
```

## 兼容性

| 功能 | Libra | Git | Jujutsu |
|---------|-------|-----|---------|
| 将文件作为 blob 哈希 | `libra hash-object <path>` | `git hash-object <path>` | N/A |
| 从 stdin 读取 | `--stdin` | `--stdin` | N/A |
| 从 stdin 读取路径 | `--stdin-paths` | `--stdin-paths` | N/A |
| 写入对象 | `-w` / `--write` | `-w` | N/A |
| 选择对象类型 | `-t blob/commit/tree/tag` | `-t <type>` | N/A |
| 路径上下文 | 接受 `--path <path>`，不应用 filters | `--path <path>` | N/A |
| 禁用 filters | 接受 `--no-filters` | `--no-filters` | N/A |
| 路径过滤器 / attributes | 不支持 | filters / attributes | N/A |
| 按字面哈希无效对象 | `--literally`（仅限已知类型） | `--literally`（任意类型字符串） | N/A |

## 错误

| 条件 | 稳定代码 | 退出码 | 提示 |
|-----------|-------------|------|------|
| 对象类型不在 blob/commit/tree/tag 之内 | `LBR-CLI-002` | 129 | `hash-object supports blob, commit, tree, and tag` |
| 内容不是 `-t <type>` 的良构对象（且未加 `--literally`） | `LBR-CLI-002` | 129 | `pass --literally to hash malformed content without validation` |
| 无法读取输入文件 | `LBR-IO-001` | 128 | 确认路径存在且可读 |
| 无法写入对象 | `LBR-IO-002` | 128 | 检查对象存储权限和磁盘空间 |
