# `libra tag`

创建、列出或删除标签。

## 概要

```
libra tag [<name>] [-m <message>] [-f]
libra tag -l [-n <lines>]
libra tag -d <name>
```

## 说明

`libra tag` 管理轻量标签和附注标签。轻量标签只是指向提交的具名指针，而附注标签会存储带有消息、打标签者身份和时间戳的完整标签对象。

不带参数（或带 `-l`）时，命令列出所有标签。给出名称时，它会在 HEAD 处创建新标签。添加 `-m <message>` 会创建附注标签，而不是轻量标签。`-f` 标志允许覆盖同名已有标签。

标签引用与分支引用一起存储在 SQLite 数据库中，提供相同的事务保证。

## 选项

| 标志 | 长选项 | 值 | 说明 |
|------|------|-------|-------------|
| | `<name>` | 位置参数（可选） | 要创建、显示或删除的标签名 |
| `-l` | `--list` | | 列出所有标签 |
| `-d` | `--delete` | | 删除具名标签 |
| `-m` | `--message` | `<msg>` | 使用给定消息创建附注标签 |
| `-f` | `--force` | | 覆盖已有标签 |
| `-n` | `--n-lines` | `<lines>` | 列出时显示的附注行数（0 = 只显示名称） |

### 标志示例

```bash
# 在 HEAD 创建轻量标签
libra tag v1.0

# 创建带消息的附注标签
libra tag -m "Release v1.1" v1.1

# 强制覆盖已有标签
libra tag -f v1.0

# 列出所有标签
libra tag -l

# 列出标签并预览附注（2 行）
libra tag -l -n 2

# 删除标签
libra tag -d v1.0

# 面向代理的 JSON 输出
libra tag --json v1.0
```

## 常用命令

```bash
libra tag v1.0                        # 在 HEAD 创建轻量标签
libra tag -m "Release v1.1" v1.1      # 创建附注标签
libra tag -l -n 2                     # 列出标签，最多显示 2 行附注
libra tag -d v1.0                     # 删除标签
libra tag --json v1.0                 # 面向代理的结构化 JSON 输出
```

## 人类可读输出

- `libra tag -l`：打印标签列表，每行一个；使用 `-n` 时缩进显示附注行
- `libra tag v1.0`：`Created lightweight tag 'v1.0' at abc1234`
- `libra tag -m "msg" v1.0`：`Created annotated tag 'v1.0' at abc1234`
- `libra tag -d v1.0`：`Deleted tag 'v1.0' (was abc1234)`
- 默认创建路径保留当前人类可读输出

## 结构化输出（JSON 示例）

`--json` / `--machine` 使用 `action` 区分操作：

创建标签：

```json
{
  "ok": true,
  "command": "tag",
  "data": {
    "action": "create",
    "name": "v1.0",
    "hash": "abc123...",
    "tag_type": "lightweight",
    "message": null
  }
}
```

创建附注标签：

```json
{
  "ok": true,
  "command": "tag",
  "data": {
    "action": "create",
    "name": "v1.1",
    "hash": "abc123...",
    "tag_type": "annotated",
    "message": "Release v1.1"
  }
}
```

列出标签：

```json
{
  "ok": true,
  "command": "tag",
  "data": {
    "action": "list",
    "tags": [
      { "name": "v1.0", "hash": "abc123...", "tag_type": "lightweight", "message": null },
      { "name": "v1.1", "hash": "def456...", "tag_type": "annotated", "message": "Release v1.1" }
    ]
  }
}
```

删除标签：

```json
{
  "ok": true,
  "command": "tag",
  "data": {
    "action": "delete",
    "name": "v1.0",
    "hash": "abc123..."
  }
}
```

`action=list` 返回 `tags` 数组；`action=delete` 返回 `name` 和 `hash`。对于格式异常标签引用的恢复性删除，当存储目标缺失时，`hash` 可以为 `null`。

## 设计理由

### 为什么没有 --sign/-s？

Git 的 `--sign` 标志使用 GPG 生成嵌入在标签对象中的内联 PGP 签名。Libra 省略此功能有几个原因：

- **GPG 密钥管理脆弱**：开发者经常丢失密钥、让密钥过期，或误配置 gpg-agent，导致签名工作流损坏。在 CI/CD 环境中，安全管理 GPG keyring 是运维负担。
- **基于 Vault 的签名是预期路径**：Libra 架构围绕基于 vault 的签名模型设计（见 `libra init` 上的 `--vault`），加密操作委托给安全密钥存储，而不是要求每个开发者维护本地 GPG 密钥。这种方式集中信任并简化密钥轮换。
- **通过 SQLite 保证标签完整性**：因为标签引用位于事务数据库而不是 loose 文件中，GPG 签名原本要缓解的篡改表面已经降低。未经授权的引用修改需要数据库访问，而不只是文件系统写入。

### 为什么没有 --verify？

没有 `--sign` 时，就没有可验证的内联签名。未来验证将在 vault/trust 层处理，而不是通过逐标签 GPG 检查。这避免了 Git 中 `git tag -v` 因签名者公钥不在本地 keyring 而令人困惑地失败的情况。

### 为什么区分轻量标签和附注标签？

Libra 保留 Git 的两层标签模型，以保持磁盘格式兼容。轻量标签是简单 ref 指针（适合临时标记），而附注标签存储对发布有用的元数据。`-m` 标志是开关：存在时创建附注标签，不存在时创建轻量标签。这与 Git 行为完全匹配，让从 Git 迁移的用户保持一致心智模型。

## 参数对比：Libra vs Git vs jj

| 功能 | Git | Libra | jj |
|---------|-----|-------|----|
| 创建轻量标签 | `git tag <name>` | `libra tag <name>` | `jj tag create <name>` |
| 创建附注标签 | `git tag -a -m "msg" <name>` | `libra tag -m "msg" <name>` | 不支持（仅轻量） |
| 列出标签 | `git tag -l` | `libra tag -l` | `jj tag list` |
| 带消息列出 | `git tag -l -n3` | `libra tag -l -n 3` | N/A |
| 删除 | `git tag -d <name>` | `libra tag -d <name>` | `jj tag delete <name>` |
| 强制覆盖 | `git tag -f <name>` | `libra tag -f <name>` | `jj tag create <name>`（总是覆盖） |
| 签名标签 | `git tag -s <name>` | 不支持（计划基于 vault） | N/A |
| 验证标签 | `git tag -v <name>` | 不支持（计划基于 vault） | N/A |
| 结构化输出 | 无 | `--json` / `--machine` | `--template` |

## 错误处理

| 场景 | 错误码 | 提示 |
|----------|-----------|------|
| 标签已存在 | `LBR-CONFLICT-002` | "delete it first with 'libra tag -d <name>'." |
| HEAD 没有可打标签的提交 | `LBR-REPO-003` | "create a commit first before tagging HEAD." |
| 标签未找到（delete/show） | `LBR-CLI-003` | "use 'libra tag -l' to list available tags." |
| --delete/--message/--force 缺少标签名 | `LBR-CLI-002` | "use 'libra tag <name>' to create or update a tag" |
| 无法解析 HEAD | `LBR-IO-001` 或 `LBR-REPO-002` | -- |
| 无法序列化附注标签 | `LBR-REPO-005` | -- |
| 无法存储对象 | `LBR-IO-002` | -- |
| 无法持久化引用 | `LBR-IO-002` | -- |
| 无法删除标签 | `LBR-IO-002` | -- |
| 无法列出标签（DB 错误） | `LBR-IO-001` | -- |
| 无法列出标签（对象损坏） | `LBR-REPO-002` | -- |
