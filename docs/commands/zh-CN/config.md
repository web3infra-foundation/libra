# `libra config`

`libra config` 管理存储在 SQLite-backed `config_kv` 中的仓库本地和用户全局配置，包括由 vault 支撑的 secrets 和密钥管理。

**别名：** `cfg`

## 概要

```
libra config <subcommand> [options]
libra config set [--global] [--add] [--encrypt] [--plaintext] [--stdin] <key> [<value>]
libra config get [--global] [--all] [--reveal] [--regexp] [-d <default>] <key>
libra config list [--global] [--name-only] [--show-origin] [--vault] [--ssh-keys] [--gpg-keys]
libra config unset [--global] [--all] <key>
libra config import [--global]
libra config path [--global]
libra config generate-ssh-key --remote <name>
libra config generate-gpg-key [--name <name>] [--email <email>] [--usage <usage>]
```

也支持 Git 兼容的标志风格（从帮助中隐藏）：

```
libra config [--get | --get-all | --unset | --unset-all | -l | --add | --import | --get-regexp | --show-origin] [--local | --global] [key] [value] [-d <default>]
```

## 说明

`libra config` 跨两个 scope 读写配置值：**local**（仓库级，存储在 `.libra/libra.db`）和 **global**（用户级，存储在 `~/.libra/config.db`）。两个数据库都使用 SQLite 和 `config_kv` 表。

不同于 Git 的明文 INI 文件或 jj 的 TOML 文件，Libra 将配置存储在事务型数据库中，并集成 vault 加密。敏感值（API keys、tokens、SSH 私钥）会使用 AES-256-GCM 自动静态加密。

该命令支持两种调用风格：

1. **子命令风格**（推荐）：`libra config set key value`、`libra config get key`
2. **Git 兼容标志风格**（隐藏）：`libra config --get key`、`libra config key value`

使用 `get` 读取值时，Libra 会按优先级从 local 到 global 级联查找。第一个匹配项胜出。

## 选项

### 子命令

#### `set <key> [<value>]`

设置配置值。如果省略 `<value>` 且 key 是敏感 key，Libra 会交互式提示输入（隐藏回显）。在非交互上下文（CI/CD）中，使用 `--stdin` 管道传入值。

| 标志 | 说明 |
|------|------|
| `--add` | 将该 key 作为额外值添加，允许重复（类似 Git 的多值 key，如 `remote.origin.fetch`） |
| `--encrypt` | 即使 key 不匹配敏感 key 启发式，也强制 vault 加密 |
| `--plaintext` | 强制明文存储，即使看起来像敏感 key 也跳过自动加密 |
| `--stdin` | 从 stdin 读取值，而不是位置参数（适合在 CI/CD 中管道传 secrets） |

```bash
# 基本设置
libra config set user.name "Jane Doe"

# 设置全局配置
libra config set --global user.email "jane@example.com"

# 强制加密
libra config set --encrypt custom.api_token "sk-abc123"

# 从 stdin 设置（CI/CD）
echo "$SECRET" | libra config set --stdin vault.env.GEMINI_API_KEY

# 添加多值 key
libra config set --add remote.origin.fetch "+refs/heads/*:refs/remotes/origin/*"

# 省略敏感 key 的值时交互提示
libra config set vault.env.GEMINI_API_KEY
```

#### `get <key>`

获取配置值。从 local 到 global scope 级联，返回第一个匹配项。

| 标志 | 说明 |
|------|------|
| `--all` | 返回该 key 的所有值（多值 key） |
| `--reveal` | 对加密条目显示实际解密值（会阻止内部 vault 凭据，如 `vault.roottoken_enc`） |
| `--regexp` | 将 `<key>` 视为正则表达式，并返回所有匹配条目 |
| `-d`, `--default <value>` | key 未找到时返回此值（而不是报错） |

```bash
# 简单 get
libra config get user.name

# 带默认 fallback
libra config get -d "unknown" user.name

# 获取多值 key 的所有值
libra config get --all remote.origin.fetch

# 显示加密值
libra config get --reveal vault.env.GEMINI_API_KEY

# 正则搜索
libra config get --regexp "user\\..*"
```

#### `list`

列出活动 scope 中的所有配置条目。

| 标志 | 说明 |
|------|------|
| `--name-only` | 只显示 key 名，不显示值 |
| `--show-origin` | 为每个条目加上 scope 前缀（`local` 或 `global`） |
| `--vault` | 只显示 `vault.env.*` 条目 |
| `--ssh-keys` | 显示 SSH key 条目 |
| `--gpg-keys` | 显示 GPG key 条目 |

```bash
# 列出所有本地条目
libra config list

# 列出并显示 scope 标签
libra config list --show-origin

# 只列出 vault 环境条目
libra config list --vault

# 只列出 key 名
libra config list --name-only

# 列出 SSH keys
libra config list --ssh-keys
```

#### `unset <key>`

移除配置条目。

| 标志 | 说明 |
|------|------|
| `--all` | 移除该 key 的所有值（用于多值 key） |

```bash
# 移除一个 key
libra config unset user.signingkey

# 移除多值 key 的所有值
libra config unset --all remote.origin.fetch
```

#### `import`

从用户的 Git config（`.gitconfig`）导入配置。将相关条目复制到 Libra 的配置数据库。

```bash
# 从 Git 全局配置导入到 Libra 全局配置
libra config import --global

# 导入到本地配置
libra config import
```

#### `path`

打印活动 scope 的配置数据库文件系统路径。

```bash
# 显示本地配置路径
libra config path
# Output: /path/to/repo/.libra/libra.db

# 显示全局配置路径
libra config path --global
# Output: /home/user/.libra/config.db
```

#### `edit`

不支持。Libra 使用 SQLite 存储，无法安全地通过文本编辑器 round-trip。详情见[设计动机](#设计动机为何不同于-gitjj)。

#### `generate-ssh-key --remote <name>`

为命名远程生成 SSH 密钥对。私钥加密存储在 vault（`vault.ssh.<remote>.privkey`）中；公钥存储在 `vault.ssh.<remote>.pubkey`。

```bash
libra config generate-ssh-key --remote origin
libra config get vault.ssh.origin.pubkey
```

#### `generate-gpg-key`

生成用于提交签名或加密的 GPG 密钥对。

| 标志 | 说明 |
|------|------|
| `--name <name>` | key 使用的用户名（默认使用 `user.name` 配置） |
| `--email <email>` | key 使用的用户邮箱（默认使用 `user.email` 配置） |
| `--usage <usage>` | Key 用途：`signing`（默认）或 `encrypt` |

```bash
# 生成签名 key
libra config generate-gpg-key

# 使用显式身份生成加密 key
libra config generate-gpg-key --name "Jane Doe" --email "jane@example.com" --usage encrypt

# 获取公钥
libra config get vault.gpg.pubkey
```

### Scope 标志

这些标志是全局的（适用于任意子命令）：

| 标志 | 说明 |
|------|------|
| `--local` | 使用仓库配置（`.libra/libra.db`）。这是写入的默认值。 |
| `--global` | 使用全局用户配置（`~/.libra/config.db`）。 |
| `--system` | **已移除。** 始终产生错误。见设计动机。 |

### 隐藏的 Git 兼容标志

这些标志为 `git config` 调用模式提供向后兼容。它们从 `--help` 中隐藏，并在内部翻译为等价子命令。

| 标志 | 等价子命令 |
|------|------------|
| `--get` | `get <key>` |
| `--get-all` | `get --all <key>` |
| `--unset` | `unset <key>` |
| `--unset-all` | `unset --all <key>` |
| `-l`, `--list` | `list` |
| `--add` | `set --add <key> <value>` |
| `--import` | `import` |
| `--get-regexp` | `get --regexp <key>` |
| `--show-origin` | `list --show-origin` |

### 其他标志

| 标志 | 说明 |
|------|------|
| `-d`, `--default <value>` | key 未找到时使用的默认值（Git 兼容位置模式） |
| `--json` | 输出结构化 JSON |
| `--quiet` | 抑制人类可读输出 |

## 常用命令

```bash
libra config set user.name "Jane Doe"
libra config get user.name
libra config list
libra config list --show-origin
libra config unset user.signingkey
libra config import
libra config path
```

## 人工输出

**`get`** 在单行打印值：

```
Jane Doe
```

**`list`** 打印 key-value 对：

```
user.name=Jane Doe
user.email=jane@example.com
core.editor=vim
```

带 `--show-origin`：

```
local   user.name=Jane Doe
global  user.email=jane@example.com
```

带 `--name-only`：

```
user.name
user.email
core.editor
```

**`set`** 成功时不打印任何内容（退出码 0）。

**`path`** 打印数据库路径：

```
/home/user/repo/.libra/libra.db
```

## 结构化输出（JSON 示例）

**`get`：**

```json
{
  "command": "config",
  "data": {
    "key": "user.name",
    "value": "Jane Doe",
    "origin": "local"
  }
}
```

**`list`：**

```json
{
  "command": "config",
  "data": {
    "entries": [
      { "key": "user.name", "value": "Jane Doe", "origin": "local" },
      { "key": "user.email", "value": "jane@example.com", "origin": "global", "encrypted": false }
    ]
  }
}
```

## Secrets 与 Vault 条目

当 key 匹配 Libra 的敏感 key 规则时，敏感 key 会加密存储，包括：

- `vault.env.*`
- `*.privkey`
- API keys、tokens、passwords 以及类似 secret 的 key

示例：

```bash
libra config set vault.env.GEMINI_API_KEY
echo "$SECRET" | libra config set --stdin vault.env.GEMINI_API_KEY
libra config set --encrypt custom.api_token "secret"
libra config get vault.env.GEMINI_API_KEY
libra config get --reveal vault.env.GEMINI_API_KEY
libra config list --vault
```

`--reveal` 对内部 vault 凭据（如 `vault.roottoken_enc` 和 `vault.ssh.<remote>.privkey`）会被阻止。

## 密钥管理

SSH keys 按远程生成并存储在 config 中：

```bash
libra config generate-ssh-key --remote origin
libra config get vault.ssh.origin.pubkey
libra config list --ssh-keys
```

GPG 公钥通过 config 暴露，而私有签名材料保留在 `vault.db` 内：

```bash
libra config generate-gpg-key
libra config generate-gpg-key --usage encrypt
libra config get vault.gpg.pubkey
libra config list --gpg-keys
```

支持的 `--usage` 值是 `signing` 和 `encrypt`。

## Scope

- 默认 scope 是 local（`.libra/libra.db`）
- `--global` 使用 `~/.libra/config.db`
- `--system` 已移除（见设计动机）；将旧用法迁移到 `--global`

运行时由配置支撑的环境变量解析顺序是：

1. CLI 参数
2. 本地配置（`vault.env.<NAME>`）
3. 全局配置（`vault.env.<NAME>`）
4. 进程环境变量

如果必需 API key 没有由 Vault 条目或进程环境变量提供，Libra 会报告缺失 key，并要求你设置 `vault.env.<NAME>` 或导出 `<NAME>`。

## 设计动机（为何不同于 Git/jj）

### 为什么使用 SQLite 而不是文本文件？

Git 使用 INI 格式文本文件；jj 使用 TOML。Libra 使用 SQLite，因为：

1. **事务写入。** SQLite 提供 ACID 保证。不同于写到一半的文本文件，写入中崩溃不会损坏配置。当多个 AI agent 可能并发写配置时，这很关键。
2. **结构化查询。** 多值 key、前缀搜索和正则匹配都是 SQL 查询，而不是文本解析。这消除了一整类转义和解析 bug。
3. **集成加密。** Vault 加密值以加密 blob 形式与明文值一起存储在同一张表中。文本文件格式需要独立加密层或内联编码方案。

### 为什么使用 vault 加密？

Git 将配置存储在明文 INI 文件中，用来保存 API keys、access tokens 和 SSH/GPG 私钥本质上不安全。Libra 原生集成 Vault-backed 加密存储。敏感 key（如 `vault.env.*`、`*.privkey`，或包含 `secret`/`token` 等子串的 key）会在 local 和 global scope 中使用 AES-256-GCM 自动静态加密。这消除了“CLI 中已脱敏但磁盘上是明文”的虚假安全感，让开发者可以安全地把环境覆盖值直接存储在配置中。

### 为什么没有 `--system` scope？

系统级配置（`--system`）被有意移除。在多用户 OS 环境中，在系统级共享加密 vault 会引入严重的权限隔离问题。例如，只有 `root` 可读的 unseal key 会导致级联 config 读取对普通用户失败，并使其命令崩溃。操作复杂度和安全风险远超收益。系统范围默认值应在 OS/环境层处理，而 Libra 使用 `--global` 处理用户级默认值。

### 为什么没有 `config edit`？

Libra 使用 SQLite 数据库（`config_kv` 表），而不是明文文件。将数据库行导出到文本编辑器，再把 unified diff 解析回 SQL `UPDATE`/`DELETE` 语句是危险的。具体而言，对于多值 key（如 `remote.origin.fetch`），明文表示缺少行级主键。重新排序、部分修改或删除行会阻止 Libra 准确地将文本更改映射回数据库行，最终不可避免地导致数据丢失或损坏。为保证数据一致性，必须使用稳健的 `set`、`--add`、`unset` 和 `list` 命令。

### 为什么内置 SSH/GPG 密钥管理？

Libra 不把 SSH 私钥作为明文文件分散到文件系统，而是将它们加密存储在 config vault 中（`vault.ssh.<remote>.privkey`）。调用 SSH 传输时，key 会动态解密到临时文件（`chmod 600`），传给 SSH client，然后立即删除。GPG 私钥完全由 vault 内部 PKI engine 管理，绝不会导出到文件系统。

### 为什么将子命令风格作为主接口？

Git 使用 `git config key value`（隐式 set）和 `git config key`（隐式 get），这存在歧义：`git config foo` 可能是 get，也可能是不完整的 set。Libra 参考 jj，要求显式子命令（`set`、`get`、`list`、`unset`）。Git 兼容标志风格（`--get`、`-l` 等）作为迁移用隐藏别名保留，但文档化接口是子命令风格，因为它无歧义、可通过 `--help` 发现，也更容易让 AI agent 正确生成。

### 为什么使用 `--default` 而不是区分退出码？

Git 在 key 未找到时以代码 1 退出，这在脚本中与其他错误难以区分。Libra 的 `--default` 标志提供显式 fallback 值，让脚本和 agent 无需解析退出码就能处理缺失 key。

## Git Config 兼容矩阵

本矩阵记录 `libra config` 与 `git config`（Git 2.54.0 基线）的对齐情况。目标是**高价值脚本兼容**，
而非逐字节克隆：与 Libra 的 SQLite/vault 模型冲突的能力被记录为*有意差异*或*延后*，而不是静默模拟。

状态图例：**已实现** · **部分** · **延后**（识别但快速失败并给出清晰信息） · **有意差异**（刻意不模拟） · **不适用**。

| Git 能力 | Libra 状态 | 说明 |
|---|---|---|
| `--get`、`--get-all`、`get`、`get --all` | 已实现 | local→global 级联；单 get 为 last-one-wins |
| `set`、旧式位置参数 set | 已实现 | 明文、自动加密和 `--stdin` 值 |
| `--list` / `-l`、`list` | 已实现 | 支持 `--name-only` |
| `--unset`、`--unset-all` | 已实现 | 多值保护（歧义退出 5） |
| `--local`、`--global` | 已实现 | local 默认；global 位于 `~/.libra/config.db` |
| `--null` / `-z` | 已实现 | Git 记录格式 `key\nvalue\0`（仅文本输出） |
| `--show-origin` | 已实现 | 输出 `file:<path>` SQLite 来源（不是 scope 标签） |
| `--show-scope` | 已实现 | 输出 `local`/`global` scope 标签 |
| `--name-only`（list / 旧式 `--get-regexp`） | 已实现 | 单 key 现代 `get --name-only` → 退出 129 |
| `--get-regexp` | 已实现 | key 正则；文本输出为 Git 风格 `key value` |
| `--append` / 旧式 `--add` / `set --add` | 已实现 | 追加值，不替换 |
| `--replace-all` / `set --all` | 已实现 | 事务内替换 key 的所有匹配值 |
| `--value=<pattern>` | 已实现 | get/set/unset 的正则（或 `--fixed-value` 字面量）value 过滤 |
| `--fixed-value` | 已实现 | `--value` 字面量匹配；关闭正则/取反解析 |
| `--ignore-case` / `-i` | 已实现 | 大小写不敏感 key/value 正则匹配 |
| `rename-section`、`--rename-section` | 已实现 | 通用 dotted 前缀移动（无 remote 专用级联） |
| `remove-section`、`--remove-section` | 已实现 | 通用 dotted 前缀删除 |
| `--type=bool\|int\|path`（+ `--bool`/`--int`/`--path`） | 已实现 | 输出规范化 + 写入校验 |
| `--default` | 部分 | 仅 get / get-all / get-regexp；变更型组合 → 退出 129 |
| `--import`（Libra 扩展） | 已实现 | 读取 `git config --list -z`；非 Git 能力 |
| vault 加密 secret、SSH/GPG 密钥生成 | Libra-only | 非 Git 能力 |
| `--type=bool-or-int\|expiry-date\|color` | 延后 | 退出 129；语义对 Libra 核心流程价值较低 |
| `--type` + `--list` | 延后 | 退出 129；Git 会静默丢弃无法规范化的条目 |
| `--no-type`、`--no-value`、`--show-names`/`--no-show-names` | 延后 | 退出 129；多 flag 清除/显示状态机未实现 |
| `--get-color`、`--get-colorbool` | 延后 | 退出 129；旧式 color helper |
| `--url`、`--get-urlmatch` | 延后 | 退出 129；高级 URL 匹配 |
| `--system` | 有意差异 | 退出 129；不进行特权系统写入（见设计动机） |
| `--worktree` | 有意差异 | 退出 129；worktree scoped config 模型未设计 |
| `--file <path>` / `-f` | 有意差异 | 退出 129；用 `libra config --import` 摄取 Git 文件 |
| `--blob <blob>` | 有意差异 | 退出 129；blob-backed config 非权威存储 |
| `--includes` / `--no-includes` / `includeIf` | 有意差异 | 退出 129；include 图与 SQLite-backed config 冲突 |
| `edit` | 有意差异 | SQLite 存储无法安全文本编辑 |

> **本路线图无需 SQL schema 迁移。** 所有新行为（多值过滤、section 操作、类型化值、输出 flag）都使用
> 现有 `config_kv(id, key, value, encrypted)` 表；复杂变更在应用层通过显式 sea-orm 事务实现原子性，不新增 DDL。

## 决策账本

本账本记录每个 Git config 能力的约束性决策。脚本可见行为（stdout / stderr / 退出码）一旦标记*支持*即为契约稳定。

| 能力 | 决策 | 行为 |
|---|---|---|
| `--null` / `-z` | 支持 | 仅文本输出。value 以 NUL 结尾，key/value 用 `\n` 分隔（Git 风格 `key\nvalue\0`），**不是** `key=value\0`。与 JSON 输出组合 → 退出 129。与 `--stdin` 组合 → 退出 129（不解析 stdin）。 |
| `--show-origin` | 支持 | 增加 `file:<path>` 来源（local → 仓库 `.libra/libra.db`，global → 解析后的全局 DB）。绝不把 `local`/`global` scope 标签当作来源。 |
| `--show-scope` | 支持 | 增加 `local`/`global` scope 标签。级联 get 报告胜出的 scope。 |
| `--name-only` | 支持（list + 旧式 regexp） | 记录只含 key。旧式单 key `--get <key> --name-only` → 退出 129；现代 `get --name-only` 延后。 |
| `--append` / `--add` | 支持 | 追加值；绝不替换已有值。`set --add` 保留为 Libra 子命令 UX。 |
| `--replace-all` / `set --all` | 支持 | 事务内替换 key 的所有匹配值。无匹配 → 插入并退出 0。 |
| `--value=<pattern>` | 支持 | get/set/unset 的正则 value 过滤。无效正则 → 退出 6。前导 `!` 取反（除非 `--fixed-value`）。遵守下方敏感/加密交互契约。 |
| `--ignore-case` / `-i` | 支持 | key 正则和 `--value` 正则大小写不敏感匹配（不影响 `--fixed-value` 字面匹配）。 |
| `--fixed-value` | 支持 | `--value` 按字面量匹配；关闭 value 正则/取反解析。key 正则长度限制仍生效。 |
| `rename-section` / `remove-section` | 支持 | 现代子命令 + 旧式 `--rename-section` / `--remove-section`。section 映射为 dotted 前缀（`remote.origin` → `remote.origin.`）。 |
| `--type=bool\|int\|path`（+ `--bool`/`--int`/`--path`） | 支持 | 输出规范化；bool/int 写入校验。整数支持大小写不敏感 `k`/`m`/`g` 后缀（1024 进制）并输出规范十进制。 |
| `--no-value` / `--show-names` / `--no-show-names` / `--no-type` | 延后 | 退出 129 并给出说明。 |
| `--type=bool-or-int\|expiry-date\|color` | 延后 | 退出 129（类型尚未支持）。 |
| `--type` + `--list` | 延后 | 退出 129（避免静默丢弃无法规范化的条目）。 |
| `--system` / `--worktree` | 拒绝 | 退出 129；不进行 system/worktree config 写入。 |
| `--file` / `-f` | 拒绝 | 退出 129；提示 `libra config --import`。绝不读取任意明文文件。 |
| `--blob` | 拒绝 | 退出 129；不支持 blob-backed config。 |
| `--includes` / `--no-includes` | 拒绝 | 退出 129；include 图不是 SQLite-backed 权威配置。 |
| `--url` / `--get-urlmatch` / `--get-color` / `--get-colorbool` | 延后 | 退出 129；高级/旧式 helper。 |
| `--default` 组合 | 当前 scope | 仅 get / get-all / get-regexp；变更型组合 → 退出 129。 |
| 缺失 key 退出码 | Git-like 退出 1 | `--get missing.key`（无 `--default`）退出 1，stdout 为空。 |
| JSON 交互 | 结构化 JSON | JSON 拒绝 `--null` 和 `--name-only`（退出 129）。既有 `origin` 字段保持 scope 含义；`scope`/`origin_type`/`origin_path` 承载精确数据。 |

### `--show-origin` 与 `--show-scope`

二者**不同**，绝不可混用：

- `--show-origin` 输出 Git 风格的**来源路径**：`file:<绝对路径>`，路径是承载该值的 SQLite 数据库文件
  （local 为 `.libra/libra.db`，global 为解析后的全局 DB）。Windows 上 `file:<path>` 形式由文档和测试固定；
  若未实现 URI 规范化，则记录该平台差异而非任由其变化。
- `--show-scope` 输出**scope 标签**：`local` 或 `global`。

文本模式下前缀字段位于 `key`/`value` 之前，tab 分隔（`--null` 下为 NUL 分隔）。JSON 模式下它们表现为
`origin_type`/`origin_path` 与 `scope` 字段。

### JSON 向后兼容

既有 JSON 字段**不改变含义**。特别地，`origin` 字段继续承载**scope 标签**（`local`/`global`）。Git 风格
来源路径仅通过新增的 `origin_type`（`"file"`）和 `origin_path` 字段暴露，scope 标签也经新增的 `scope` 字段
呈现。新字段仅在请求 `--show-origin`/`--show-scope` 时出现（`skip_serializing_if`）。任何既有字段都不被
重命名、删除或改变语义。

## Value 过滤与敏感/加密值

`--value` / `--fixed-value` 面向公共多值配置（如 `remote.*.fetch` refspec）。对于**敏感 key**
（`is_sensitive_key(key)` 为真，或行以 `encrypted != 0` 存储）：

- 未带 `--reveal` 时，过滤在**存储的 ciphertext**（或 redacted 形式）上匹配；输出条目仍 redacted
  （`<REDACTED>`），除非带 `--reveal` 且该 key 不是 vault 内部。
- 带 `--reveal`（且允许解密）时，先解密再做 pattern 匹配；解密/规范化错误消息绝不包含 secret 或 ciphertext。
- section rename/remove 可移动普通自动加密的敏感 key（ciphertext 与 `encrypted` 标志随 key 名迁移），但任何
  `vault.*` section 一律拒绝，只能由专用 vault/config 命令管理。

> **Shell history 警告：** 命令行上键入的 `--value` pattern 可能被 shell history 记录。敏感 key 上请避免
> `--value`；优先用精确 key 或专用 vault 命令。在敏感 key 上使用 `--value` 时 Libra 会输出一次性 stderr 警告。

所有用户提供的正则（key 与 value）受 **4 KiB** 上限约束，由 Rust 线性时间 `regex` 引擎求值（无灾难性回溯）；
超长或无效正则退出 6。

## 事务边界与 Helper 拆分

复杂变更（`--value` 过滤的 replace/unset、`rename-section`、`remove-section`）在 command 层的**显式事务**内
执行（`conn.begin()` → `*_with_conn(&txn, …)` → `txn.commit()`），因此校验失败、冲突或写错误**不产生局部变更**
（事务被丢弃而不提交）。value matcher、section mutator 和类型规范化器位于 `src/internal/config.rs`，作为可测试的
`_with_conn` helper（自身绝不 `begin`/`commit`）；command 层只负责解析、编排、渲染和错误映射。SQLite 写失败——
包括 `database is locked`/`SQLITE_BUSY` 和 `SQLITE_FULL`（磁盘满）——映射为退出 4，给出可操作信息且无已提交变更。
`establish_connection_with_busy_timeout` 配置的 30 秒 busy-timeout 使锁竞争罕见。

## 参数对比：Libra vs Git vs jj

| 功能 | Git | jj | Libra |
|------|-----|----|-------|
| 隐式 set | `git config key val` | 无（要求 `set`） | `libra config set key val` 加兼容的 `libra config key val` |
| 子命令风格 | 无 | 有（`set/get/list/edit/path`） | 有（`set/get/list/unset/import/path`） |
| 获取值 | `git config key` | `jj config get key` | `libra config get key` |
| 列表 | `git config -l` | `jj config list` | `libra config list` |
| 在编辑器中编辑 | `git config -e` | `jj config edit` | 不支持（SQLite 存储） |
| 正则搜索 | `git config --get-regexp` | 无 | `libra config get --regexp` |
| 显示来源 | `git config --show-origin` | 无 | `libra config list --show-origin` |
| 类型转换 | `--type=bool\|int\|path` | 无（TOML 类型） | **`--type=bool\|int\|path`**（+ `--bool`/`--int`/`--path`） |
| 默认 fallback | `--default value` | 无 | `--default value` |
| Null 分隔 | `-z` | 无 | **`-z` / `--null`**（`key\nvalue\0`） |
| 重命名/移除 section | 有 | 无 | **`rename-section` / `remove-section`**（通用 dotted 前缀） |
| JSON 输出 | 无 | 无 | **`--json`** |
| Secret 脱敏 | 无 | 无 | **自动检测** |
| 从 Git 导入 | N/A | N/A | **`libra config import`** |
| Vault 加密 | 无 | 无 | **AES-256-GCM（所有 scopes）** |
| Env var vault | 无 | 无 | **`vault.env.*`** |
| 每个远程 SSH key | 无 | 无 | **`generate-ssh-key --remote`** |
| GPG key 生成 | 无 | 无 | **`generate-gpg-key`** |
| Env var 解析 | 无 fallback | 无 fallback | **CLI -> env -> repo -> global** |
| Config 文件路径 | N/A | `jj config path` | **`libra config path`** |
| 条件配置 | `includeIf` | `[[when]]` blocks | 不支持 |
| Worktree scope | `--worktree` | `--workspace` | 不支持 |
| 任意文件 | `--file <path>` | 无 | 不支持 |
| 存储格式 | INI 文本文件 | TOML 文本文件 | **SQLite + vault** |
| Scopes | system/global/local/worktree | user/repo/workspace | **global/local**（system 已移除） |
| 只列 key 名 | `--name-only` | 无 | **`--name-only`** |
| 多值 add | `--add` | 无 | **`set --add`** |
| Stdin 输入 | 无 | 无 | **`set --stdin`** |
| 强制加密 | 无 | 无 | **`set --encrypt`** |
| 强制明文 | 无 | 无 | **`set --plaintext`** |

## 错误处理

| 代码 | 条件 | 提示 |
|------|------|------|
| `LBR-REPO-001` | 不在 libra 仓库内（local scope） | 使用 `libra init` 初始化，或使用 `--global` |
| `LBR-CLI-002` | 使用了 `--system` scope（已移除） | 使用 `--global` 设置用户级默认值 |
| `LBR-CLI-003` | key 未找到且未提供 `--default` | 用 `libra config list` 检查 key 名称 |
| `LBR-CLI-002` | 使用了 `edit` 子命令（不支持） | 使用 `set`、`get`、`unset`、`list` 子命令 |
| `LBR-IO-001` | 读取配置数据库失败 | 检查 `.libra/libra.db` 的文件权限 |
| `LBR-IO-002` | 写入配置数据库失败 | 检查文件权限和磁盘空间 |

## 兼容性说明

- `libra vault` 已移除。请改用 `libra config generate-ssh-key`、`libra config generate-gpg-key` 和 `libra config get vault.*`。
- 不支持 `libra config edit`（见上方设计动机）。
- 旧仓库可能仍包含遗留的 `vault.gpg_pubkey` 条目；新写入使用 `vault.gpg.pubkey`。
