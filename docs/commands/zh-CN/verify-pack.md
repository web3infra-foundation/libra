# `libra verify-pack`

根据匹配的 pack 归档（`.pack`）验证 Git pack 索引（`.idx`）。

## 概要

```bash
libra verify-pack [OPTIONS] <IDX_FILE>...
```

## 说明

`libra verify-pack` 是一个只读 plumbing 命令。它解析 pack 索引，解码对应的 pack 文件，并验证两个文件在以下方面一致：

- 索引版本和结构布局
- fanout 表单调性和对象名排序
- 索引校验和
- 存储在索引 trailer 中的 pack 校验和
- 对象数量、对象 ID 和偏移量
- version 2 索引的 CRC32 值

默认情况下，pack 路径通过将索引文件扩展名替换为 `.pack` 得出。当 pack 归档位于其他位置时，使用 `--pack <PACK_FILE>`。该命令不需要 Libra 仓库。在仓库内运行时，它使用该仓库的对象格式。在仓库外运行时，version 2 索引文件会从索引布局推断 SHA-1 或 SHA-256；version 1 索引仅支持 SHA-1。

兼容性说明：`--pack <PACK_FILE>` 是 Libra 扩展，只能在验证单个 `<IDX_FILE>` 时使用。

## 选项

| 标志 | 短选项 | 说明 | 默认值 |
|------|-------|-------------|---------|
| `<IDX_FILE>...` | | 要验证的 pack 索引文件 | 必需 |
| `--pack <PATH>` | | 要对照验证的 pack 归档 | 扩展名替换为 `.pack` 的 `<IDX_FILE>` |
| `--verbose` | `-v` | 使用 Git 兼容的 verbose 字段打印每个索引对象 | 关闭 |
| `--stat-only` | `-s` | 仅打印 pack 统计信息 | 关闭 |
| `--json` | | 输出结构化 JSON 信封 | 关闭 |
| `--machine` | | 以一行紧凑 JSON 输出同一信封 | 关闭 |

## 示例

```bash
libra verify-pack objects/pack/pack-abc123.idx
libra verify-pack pack-a.idx pack-b.idx
libra verify-pack --pack /tmp/pack-abc123.pack /tmp/pack-abc123.idx
libra verify-pack -v pack-abc123.idx
libra verify-pack -s pack-abc123.idx
libra verify-pack pack-abc123.idx --json
```

## 人类可读输出

成功的非 verbose 验证会打印一行摘要：

```text
objects/pack/pack-abc123.idx: ok
```

Verbose 模式会先使用 Git 的基础字段布局打印索引对象，然后打印摘要行：

```text
3b18e512dba79e4c8300dd08aeb37f8e728b8dad blob 12 21 48
objects/pack/pack-abc123.idx: ok
```

字段为 `<oid> <type> <size> <size-in-pack> <offset>`。version 2 索引的 CRC32 值会被验证，并且仍可在结构化输出中使用，但不会在人类可读 verbose 模式下打印。

> **与 Git 的有意差异。** 对于 deltified 对象，`git verify-pack -v` 会在每行末尾追加两列 —— `<chain-depth> <base-oid>`。Libra 不打印它们：`git-internal` 解码器吐出重构后的对象流和每个对象的 `chain_len`，但不保留原始 delta 的 base 引用，因此在回调处无法获得 base OID。链深仍可通过 `--stat-only` 的直方图观察。

Stat-only 模式输出 Git 兼容的聚合统计：

```text
non delta: 19 objects
chain length = 1: 4 objects
```

## 结构化输出

```json
{
  "ok": true,
  "command": "verify-pack",
  "data": {
    "idx_file": "objects/pack/pack-abc123.idx",
    "pack_file": "objects/pack/pack-abc123.pack",
    "index_version": 2,
    "object_count": 42,
    "pack_hash": "0123456789abcdef0123456789abcdef01234567",
    "index_hash": "89abcdef0123456789abcdef0123456789abcdef",
    "verified": true
  }
}
```

当多个索引文件与 `--json` 一起验证时，`data.packs[]` 包含每个输入索引的结果对象。当 `--verbose` 与 `--json` 组合使用时，每个结果的 `objects[]` 包含 `oid`、`object_type`、`size`、`size_in_pack`、`offset` 和可选的 `crc32`。

## 兼容性

| 功能 | Libra | Git | jj |
|---------|-------|-----|----|
| 验证 pack 索引 | `libra verify-pack <idx>...` | `git verify-pack <idx>...` | N/A |
| Verbose 对象 | `-v` / `--verbose` | `-v` | N/A |
| Stat-only 模式 | `-s` / `--stat-only` | `-s` / `--stat-only` | N/A |
| 显式 pack 路径 | `--pack <path>` | N/A | N/A |
| JSON 输出 | `--json` / `--machine` | N/A | N/A |
| Version 1 索引 | SHA-1 仓库支持 | 支持 | N/A |
| Version 2 索引 | 支持 | 支持 | N/A |

## 错误处理

| 场景 | StableErrorCode | 退出码 |
|----------|-----------------|------|
| 无法打开索引文件 | `LBR-IO-001` | 128 |
| 无法打开 pack 文件 | `LBR-IO-001` | 128 |
| 索引格式错误 | `LBR-REPO-002` | 128 |
| Pack 格式错误 | `LBR-REPO-002` | 128 |
| 索引和 pack 不一致 | `LBR-REPO-002` | 128 |

## 被 `fsck` 复用

`verify-pack` 的核心校验逻辑被 [`libra fsck`](../fsck.md) 在进程内复用，用于体检 `objects/pack/` 下的每个 packfile。fsck 不 fork 子进程，而是直接调用同一校验逻辑：报告任何受损或不可读的 pack，并以退出码 `1`（与 `git fsck` 一致）结束，且不会因单个坏 pack 而中断对其余 pack 的检查。
