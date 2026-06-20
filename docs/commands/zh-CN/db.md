# `libra db`

检查并升级仓库 SQLite schema。

## 概要

```bash
libra db status
libra db upgrade
```

## 说明

Libra 将仓库元数据存储在 `.libra/libra.db` 中。新的 Libra 版本可能会添加较新功能需要的表、列或索引。普通仓库命令会在打开数据库前检查记录的 schema 版本。如果仓库 schema 早于正在运行的 Libra 二进制文件，命令会以 `LBR-REPO-002` 停止，并要求你运行 `libra db upgrade`。

数据库升级是显式操作。连接到仓库数据库不会自动应用待处理迁移。

## 子命令

| 子命令 | 说明 |
|------------|-------------|
| `status` | 打印当前 schema 版本，以及此 Libra 二进制文件支持的最新版本，不修改数据库。 |
| `upgrade` | 为当前 Libra 二进制文件应用内置的待处理迁移。 |

## 输出

人类可读的 `upgrade` 输出会报告是否应用了迁移：

```text
Upgraded repository database schema from 2026050801 to 2026052301 (applied: 2026052301).
```

如果没有待处理迁移：

```text
Repository database schema is up to date (version 2026052301).
```

使用 `--json` 时，`db upgrade` 输出：

```json
{
  "ok": true,
  "command": "db.upgrade",
  "data": {
    "previous_version": 2026050801,
    "current_version": 2026052301,
    "latest_version": 2026052301,
    "applied_versions": [2026052301],
    "upgraded": true
  }
}
```

## 示例

```bash
# 显示仓库 schema 版本（不写入）
libra db status

# 结构化 JSON 输出，包含当前/最新版本和状态
libra db --json status

# 应用待处理迁移，使 schema 升级到此 Libra 版本
libra db upgrade

# 结构化 JSON 输出，包含本次升级的 applied_versions[]
libra db --json upgrade
```

`libra db --help` 会渲染同一横幅，因此文档和 CLI 表面保持同步（跨命令 `--help` EXAMPLES 推出，见 `docs/development/commands/_general.md` 条目 B）。

## 安全性

- `db status` 是只读的。
- `db upgrade` 在迁移运行器的事务边界内运行每个迁移，并将已应用版本记录到 `schema_versions`。
- 如果仓库数据库由更新的 Libra 二进制文件创建，旧二进制文件会拒绝运行，并要求你安装更新的 Libra 版本，而不是尝试降级。
