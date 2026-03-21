# Libra CLI 命令改进顺序计划

## Context

基于两份审计报告（CLI UX 对比研究 + CLIG 六维审计报告），结合当前代码库已实现的基础设施，制定命令级别的改进优先级。

**已完成的基础设施：**
- 全局 `--json`/`--machine`/`--quiet`/`--color`/`--no-pager`/`--progress`/`--exit-code-on-warning` 标志 (`src/cli.rs`)
- 稳定错误码体系 16 个错误码 (`src/utils/error.rs`)
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架 (`src/utils/output.rs`)
- `CommandOutput` trait 支持结构化输出
- 错误码文档 (`docs/error-codes.md`)

**已有 JSON 输出的命令：** commit, switch, status, branch, clone, config
**已用 StableErrorCode 的命令：** commit, shortlog, lfs, code（仅 4 个）

---

## 改进顺序

### 第一批：核心高频命令（P0 阻断性）

这些命令覆盖最基本的工作流（init → clone → add → status → commit → push/pull），使用频率最高，审计报告指出的问题最严重。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **1** | `init` | 无 JSON，确认消息不明确，耗时 ~6s | 确认消息 "Initialized empty repository in \<path\>"；JSON 输出；性能优化（目标 <500ms） |
| **2** | `clone` | 有 JSON，有进度 | 补齐 StableErrorCode；网络错误 hint；性能优化（目标 <1s） |
| **3** | `add` | 与 Git 一致，无 JSON | JSON 输出（变更文件列表）；--dry-run 支持；错误信息包含文件名 |
| **4** | `status` | 有 JSON + porcelain，无 hint | 添加下一步命令建议（"use libra add..."）；补齐 StableErrorCode |
| **5** | `commit` | ✅ 已完成（金标准） | 作为参考模板，无需改动 |
| **6** | `push` | 功能失败/60s 超时/无 JSON | 修复 refspec 语法；10s 超时；进度输出；JSON 输出；错误码 |
| **7** | `pull` | 级联失败/无 JSON | 修复 upstream tracking；JSON 输出；错误码 |

**理由：** 这些命令构成完整的基本工作流闭环。init/clone 是入口命令（审计指出 init 耗时 ~6s 严重违反 CLIG "100ms 内打印内容"原则）；add 是 commit 前的必经步骤；push 是审计中"最严重的三个缺陷"之一。

### 第二批：状态变更确认命令（P0 消灭"沉默"）

审计报告核心发现："成功时沉默、等待时沉默、失败时沉默"。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **8** | `switch` | 有 JSON + 确认消息 | 补齐 StableErrorCode；切换不存在分支时提示 `did you mean -c` |
| **9** | `reset` | 有确认消息，无 JSON | 输出 "HEAD is now at \<SHA\> \<msg\>"；JSON 输出；错误码 |
| **10** | `tag` | 有短标志 -l/-d/-m/-a | 补齐 JSON 输出；重复创建时 hint；退出码对齐 exit 1 |
| **11** | `branch` | 有 JSON | 补齐 StableErrorCode；退出码对齐（删除不存在分支 exit 1） |

**理由：** 这些命令改变仓库状态，必须告知用户发生了什么。

### 第三批：历史查询命令（P1 结构化输出）

使用频率高，AI Agent 场景依赖结构化输出。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **12** | `log` | 明确拒绝 --json | 实现 JSON 输出（结构化提交列表）；保持 --oneline/--graph |
| **13** | `diff` | 无 JSON | JSON 输出（hunk 级别结构化）；--numstat/--name-only |
| **14** | `show` | 有 --oneline/-s | JSON 输出；错误码 |
| **15** | `blame` | 与 Git 一致 | JSON 输出 |

**理由：** Agent 需要从历史/差异中提取结构化信息来决策。log --json 是 MCP 维度最关键的改进。

### 第四批：暂存与撤销命令（P1 一致性修复）

审计报告指出的跨命令一致性问题集中在这些命令。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **16** | `stash` | 有 -m，有子命令 | JSON 输出（stash list）；保存确认和 stash 编号 |
| **17** | `restore` | 无确认/无 JSON | 确认消息；退出码对齐 exit 1；错误码 |
| **18** | `revert` | 有确认消息，有 -n | 补齐 --no-edit；JSON 输出；错误码 |
| **19** | `cherry-pick` | 与 Git 一致 | JSON 输出；错误码 |

**理由：** 撤销操作的错误反馈尤为重要，用户需要知道操作是否成功。

### 第五批：配置与远程管理（P1 简化 + 对齐）

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **20** | `config` | 有 JSON | 简化写入语法（默认 local，省略 --add）；错误码 |
| **21** | `remote` | 有子命令，无 JSON | JSON 输出；退出码对齐（重复添加 exit 3 或 exit 1） |
| **22** | `fetch` | 与 Git 一致 | JSON 进度事件；错误码 |

**理由：** config 的冗长语法是 CLIG 违规（"默认行为应对大多数用户正确"）。

### 第六批：辅助命令（P2 增强）

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **23** | `reflog` | 子命令结构偏离 Git | 重构为 `libra reflog [-n N]`；JSON 输出 |
| **24** | `describe` | 有 --abbrev/--tags | 补齐 --always；JSON 输出 |
| **25** | `shortlog` | 已有错误码 | 补齐 revision 位置参数；JSON 输出 |
| **26** | `clean` / `checkout` / `rebase` / `merge` | 与 Git 语法一致 | JSON 输出；merge 冲突结构化输出 |

### 第七批：全局层面改进（贯穿所有命令）

这些改进不针对单个命令，而是全局性的：

| 顺序 | 改进项 | 优先级 |
|------|--------|--------|
| **A** | 退出码三级模型统一对齐（0/1/128） | 与各命令改进同步进行 |
| **B** | 每个子命令 --help 添加 EXAMPLES 段 | 与各命令改进同步进行 |
| **C** | `NO_COLOR` / `TERM=dumb` / `--no-color` 颜色控制 | 独立改进 |
| **D** | log/diff/blame/show TTY 下使用 pager | 独立改进 |
| **E** | 顶层 help 按场景分组 | 独立改进 |
| **F** | 拼写纠错建议（确认 clap suggest 已启用） | 独立改进 |
| **G** | 意外错误时输出 GitHub Issues URL | 独立改进 |

---

## Config 命令改进详细计划

同时落地 Cross-Cutting Improvements A/B/F/G。

### 设计原则

1. **所有 config 数据默认通过 vault 加密存储**，提升安全性（敏感 key 自动加密，普通 key 可选加密）
2. **子命令风格 CLI**：`libra config get/set/list/unset/generate-ssh-key/generate-gpg-key`
3. **两级 scope**：`--local`（仓库级，默认）和 `--global`（用户级），不支持 `config path`
4. **环境变量解析优先级**（从高到低）：CLI 参数 → 仓库 config → 全局 config → 系统环境变量
5. **此次改进不涉及 `src/internal/ai/` 目录下的代码**，provider 的 `from_env()` 改造留到后续批次

### 特性 1：Vault-Backed 扁平 Key/Value 存储

**背景：** 当前 config 表使用 `configuration` / `name` / `key` 三列拆分存储一个逻辑 key（如 `remote.origin.url` → configuration="remote", name="origin", key="url"）。拆分增加查询复杂度，且值以明文存储。

**方案：** 新增 `config_kv` 表，所有数据统一通过 vault 加密存储：

```sql
CREATE TABLE IF NOT EXISTS `config_kv` (
    `id` INTEGER PRIMARY KEY AUTOINCREMENT,
    `key` TEXT NOT NULL,                      -- 完整 dotted key，如 "remote.origin.url"
    `value` TEXT NOT NULL,                    -- vault 加密后的值，或明文（若 vault 未初始化）
    `encrypted` INTEGER NOT NULL DEFAULT 0,   -- 0=plaintext, 1=vault-encrypted
    UNIQUE(`key`, `value`)
);
CREATE INDEX idx_config_kv_key ON config_kv(`key`);
```

- **local scope** 存储在 `.libra/libra.db` 的 `config_kv` 表
- **global scope** 存储在 `~/.libra/config.db` 的 `config_kv` 表
- scope 由数据库文件位置决定，不在表中存储

**加密策略：**
- 敏感 key（匹配 `KEY`/`SECRET`/`TOKEN`/`PASSWORD`/`CREDENTIAL`，或 `vault.env.*` 前缀）→ 自动加密
- 普通 key → 明文存储（可通过 `--encrypt` 强制加密）
- vault 未初始化时 → 明文存储 + warning

**迁移：** 首次访问时自动从旧 `config` 表迁移，合并 `configuration.name.key` → dotted key。旧 API 标记 deprecated。

**涉及文件：**
- `sql/sqlite_20260309_init.sql` — 新增 `config_kv` 表
- `src/internal/config.rs` — 新 CRUD 接口 + 迁移逻辑
- `src/internal/model/config.rs` — 新 `ConfigKv` SeaORM model
- `src/command/config.rs` — 切换到新 API
- `src/internal/db.rs` — schema 迁移

### 特性 2：子命令风格 CLI + Git 兼容

**语法设计：子命令为主，同时保留 Git 兼容的 flag 风格：**

| 操作 | 子命令风格 | Git 兼容风格 | Git 对应 |
|------|----------|------------|---------|
| 设置值 | `libra config set key value` | `libra config key value` | `git config key value` |
| 获取值 | `libra config get key` | `libra config --get key` | `git config --get key` |
| 获取所有值 | `libra config get --all key` | `libra config --get-all key` | `git config --get-all key` |
| 列表 | `libra config list` | `libra config -l` / `--list` | `git config -l` |
| 列表+来源 | `libra config list --show-origin` | `libra config -l --show-origin` | `git config -l --show-origin` |
| 删除 | `libra config unset key` | `libra config --unset key` | `git config --unset key` |
| 删除所有 | `libra config unset --all key` | `libra config --unset-all key` | `git config --unset-all key` |
| 添加重复 | `libra config set --add key value` | `libra config --add key value` | `git config --add key value` |
| 正则搜索 | `libra config get --regexp pattern` | `libra config --get-regexp pattern` | `git config --get-regexp` |
| 编辑器打开 | `libra config edit` | `libra config -e` | `git config -e` |
| 导入 Git | `libra config import` | `libra config --import` | N/A |
| 生成 SSH Key | `libra config generate-ssh-key` | N/A | N/A |
| 生成 GPG Key | `libra config generate-gpg-key` | N/A | N/A |

> **原则：** 所有 Git 用户熟悉的 flag（`-l`、`-e`、`--get`、`--get-all`、`--get-regexp`、`--unset`、`--unset-all`、`--add`、`--show-origin`）均保留兼容。子命令风格是推荐用法，flag 风格确保 Git 用户无学习成本。

**scope 标志：**
- `--local` — 仓库级（默认）
- `--global` — 用户级（`~/.libra/config.db`）
- 不支持 `--system` 和 `config path`

### 特性 3：环境变量 Vault 存储

**背景：** AI provider API key 等只能通过系统环境变量提供，不友好且不安全。

**方案：** `vault.env.*` 命名空间存储环境变量，值自动加密。

**环境变量解析优先级（从高到低）：**

```
1. CLI 参数（如 --api-key, --api-base）                ← 最高，显式传入
2. 仓库 config（vault.env.* in .libra/libra.db）       ← 项目级覆盖
3. 全局 config（vault.env.* in ~/.libra/config.db）    ← 用户级默认
4. 系统环境变量（std::env::var）                        ← 最低，传统方式
```

**统一入口函数：**
```rust
/// 解析环境变量，按优先级查找。
/// CLI 参数由调用方在调用前自行处理，若有则不调用此函数。
pub async fn resolve_env(name: &str) -> Option<String> {
    // 1. 仓库 config (local vault)
    if let Some(val) = config_vault_get_local(name).await { return Some(val); }
    // 2. 全局 config (global vault)
    if let Some(val) = config_vault_get_global(name).await { return Some(val); }
    // 3. 系统环境变量
    if let Ok(val) = std::env::var(name) { return Some(val); }
    None
}
```

**涉及文件（此次改进范围）：**
- `src/internal/config.rs` — `resolve_env()` / `config_vault_get()` / `config_vault_set()`
- `src/internal/vault.rs` — 适配 global scope unseal key
- `src/utils/client_storage.rs` — env var 读取 → `resolve_env()`
- `src/utils/d1_client.rs` — 同上

> **注：** `src/internal/ai/providers/*/client.rs` 的 `from_env()` → `resolve_env()` 改造**不在此次改进范围内**，留到后续批次。

### 特性 4：SSH Key 与 GPG Key 管理

**背景：** 当前 SSH/GPG key 生成在 `libra vault` 子命令中。将 key 管理集成到 `libra config` 更符合用户心智模型（config 管理仓库配置，key 是配置的一部分）。

#### SSH Key 管理

```bash
# 为指定 remote 生成 SSH Key（已有 vault 基础设施支持，RSA 3072）
libra config generate-ssh-key --remote origin
libra config generate-ssh-key --remote upstream
libra config generate-ssh-key --remote origin --global   # 全局级别

# 查看 SSH 公钥
libra config get vault.ssh.origin.pubkey
libra config get vault.ssh.upstream.pubkey

# 列出所有 SSH keys
libra config list --ssh-keys
```

**存储：**
- 公钥：`vault.ssh.<remote>.pubkey` in config_kv
- 私钥：`~/.libra/ssh-keys/<repo-id>/<remote>/id_rsa`（文件权限 `0o600`）
- 每个 remote 独立一组 key，支持向不同服务器同时 push

**与 init 的关系（备忘，待 init 改进时添加）：**
- `libra init` 默认为 origin 生成 SSH Key
- 复用 `vault::generate_ssh_key()` 现有逻辑（RSA 3072, OpenSSH 兼容）

#### GPG Key 管理

```bash
# 生成新 GPG Key（用于签名以外的用途，如代码加密/解密）
libra config generate-gpg-key --name "Alice" --email "alice@example.com"
libra config generate-gpg-key --usage encrypt   # 标记用途为加密

# 查看 GPG 公钥
libra config get vault.gpg.pubkey

# 列出所有 GPG keys
libra config list --gpg-keys
```

**存储：**
- 签名用 GPG key（init 时生成）：`vault.gpg.pubkey`、`vault.signing=true`
- 额外 GPG keys（config 生成）：`vault.gpg.<usage>.pubkey`（如 `vault.gpg.encrypt.pubkey`）

**与 init 的关系（备忘，待 init 改进时添加）：**
- `libra init` 默认生成一组 GPG Key 用于 commit 签名
- 复用 `vault::generate_pgp_key()` 现有逻辑（PGP 2048-bit, 10 年有效期）

**涉及文件：**
- `src/command/config.rs` — 新增 `generate-ssh-key` 和 `generate-gpg-key` 子命令
- `src/internal/vault.rs` — 适配 per-remote SSH key 生成（当前只支持单一 key）
- `src/command/vault.rs` — 保留旧命令作为兼容，内部转发到 config

### 特性 5：Cross-Cutting Improvements 落地

在 config 中首先实施，作为其他命令的参考实现：

| ID | 改进 | config 中的具体落地 |
|----|------|---------------------|
| **A** | 退出码 0/1/128 | key not found → exit 1；invalid key → exit 1；not a repo → exit 128；DB 错误 → exit 128 |
| **B** | `--help` EXAMPLES | `after_help` 包含 10+ 常用示例 |
| **F** | 拼写纠错 | get 找不到 key 时 Levenshtein 匹配："did you mean 'user.name'?" |
| **G** | Issues URL | `LBR-INTERNAL-001` 时输出 GitHub Issues URL |

**退出码映射：**

| 场景 | 当前 | Git | 目标 |
|------|------|-----|------|
| Key not found | 128 | 1 | **1** |
| Invalid key format | 128 | 1 | **1** |
| Multiple values (set) | 128 | 2 | **1** |
| Not a repository | 128 | 128 | 128 |
| DB failure | 128 | 128 | 128 |
| Permission denied | 128 | 128 | 128 |

> **注：** 退出码映射变更后，必须同步更新 `docs/error-codes.md` 中的退出码表和稳定错误码参考表。

### 全部子命令 Human Output Mockup

#### `libra config set user.name "John Doe"`
```
Set local: user.name = John Doe
```

#### `libra config set --global user.email "john@example.com"`
```
Set global: user.email = john@example.com
```

#### `libra config set --add remote.origin.push "+refs/heads/*"`
```
Added local: remote.origin.push = +refs/heads/*
```

#### `libra config get user.name`
```
John Doe
```

#### `libra config get user.name --default "Unknown"`（key 不存在时）
```
Unknown
```

#### `libra config get --all remote.origin.fetch`
```
+refs/heads/*:refs/remotes/origin/*
+refs/tags/*:refs/tags/*
```

#### `libra config get --regexp "remote\..*\.url"`
```
remote.origin.url = git@github.com:user/repo.git
remote.upstream.url = git@github.com:org/repo.git
```

#### `libra config list`
```
Config (local, /Users/eli/projects/my-repo/.libra):
  user.name = John Doe
  user.email = john@example.com
  remote.origin.url = git@github.com:user/repo.git
  remote.origin.fetch = +refs/heads/*:refs/remotes/origin/*
  branch.main.remote = origin
  branch.main.merge = refs/heads/main

3 sections, 6 entries

Tip: use --show-origin to see which scope each value comes from
```

#### `libra config list --show-origin`
```
Config (all scopes, cascade):
  local    user.name = John Doe
  local    user.email = john@example.com
  local    remote.origin.url = git@github.com:user/repo.git
  global   core.editor = vim
  global   user.signingkey = <REDACTED>
  global   push.default = current

2 scopes, 6 entries
```

#### `libra config list --global`
```
Config (global, ~/.libra/config.db):
  core.editor = vim
  user.signingkey = <REDACTED>
  push.default = current

1 section, 3 entries
```

#### `libra config list --name-only`
```
user.name
user.email
remote.origin.url
remote.origin.fetch
branch.main.remote
branch.main.merge
```

#### `libra config list --vault`
```
Vault environment variables (cascade):
  local    vault.env.GEMINI_API_KEY = <REDACTED>  (encrypted)
  global   vault.env.OPENAI_API_KEY = <REDACTED>  (encrypted)
  global   vault.env.OPENAI_BASE_URL = https://api.openai.com/v1

2 encrypted, 1 plaintext

Next steps:
  - add:     libra config set vault.env.<ENV_VAR_NAME>
  - remove:  libra config unset vault.env.<name>

Tip: repo config takes precedence over global; both override system env vars
```

#### `libra config set vault.env.GEMINI_API_KEY`（交互式，无 value 参数）
```
Enter value for vault.env.GEMINI_API_KEY: ****
Stored (local, encrypted): vault.env.GEMINI_API_KEY

Tip: this value takes precedence over the GEMINI_API_KEY environment variable
```

#### `libra config unset user.signingkey`
```
Unset local: user.signingkey
```

#### `libra config unset --all remote.origin.fetch`
```
Unset local: remote.origin.fetch (removed 2 values)
```

#### `libra config import`
```
Imported 12 entries from Git global config → libra global config
  skipped: 2 duplicates, 0 invalid keys

Tip: use libra config list --show-origin to review imported values
```

#### `libra config edit`
```
# 导出到临时文件，打开 $EDITOR，退出后 diff 并应用变更
Editing local config...
Applied 2 changes (1 added, 1 modified, 0 removed)
```

#### `libra config edit --global`
```
Editing global config...
Applied 1 change (0 added, 1 modified, 0 removed)
```

#### `libra config generate-ssh-key --remote origin`
```
Generated SSH key for remote 'origin':
  Type:      RSA 3072
  Key ID:    libra-john
  Public key: ssh-rsa AAAA...xxxx libra-john

Stored:
  public key:  vault.ssh.origin.pubkey (in config)
  private key: ~/.libra/ssh-keys/<repo-id>/origin/id_rsa

Next steps:
  - add to GitHub:  copy the public key above to your GitHub SSH settings
  - push:           libra push origin main
```

#### `libra config generate-ssh-key --remote upstream`
```
Generated SSH key for remote 'upstream':
  Type:      RSA 3072
  Key ID:    libra-john
  Public key: ssh-rsa AAAA...yyyy libra-john

Stored:
  public key:  vault.ssh.upstream.pubkey (in config)
  private key: ~/.libra/ssh-keys/<repo-id>/upstream/id_rsa
```

#### `libra config list --ssh-keys`
```
SSH keys:
  origin     ssh-rsa AAAA...xxxx libra-john
  upstream   ssh-rsa AAAA...yyyy libra-john

2 keys configured

Tip: use libra config generate-ssh-key --remote <name> to add more
```

#### `libra config generate-gpg-key --name "Alice" --email "alice@example.com"`
```
Generated GPG key:
  Type:    PGP 2048-bit
  User:    Alice <alice@example.com>
  Valid:   10 years
  Fingerprint: ABCD 1234 5678 EFGH

Stored:
  public key: vault.gpg.pubkey (in config)

Tip: commit signing is now enabled (vault.signing = true)
```

#### `libra config generate-gpg-key --usage encrypt`
```
Generated GPG key (usage: encrypt):
  Type:    PGP 2048-bit
  Valid:   10 years
  Fingerprint: IJKL 9012 3456 MNOP

Stored:
  public key: vault.gpg.encrypt.pubkey (in config)
```

#### `libra config list --gpg-keys`
```
GPG keys:
  signing    PGP 2048-bit  ABCD 1234 5678 EFGH  (vault.signing = true)
  encrypt    PGP 2048-bit  IJKL 9012 3456 MNOP

2 keys configured
```

#### 错误输出

> **错误输出风格规则：** error/warning 行与 hint 行之间始终保持一个空行；hint 顶头显示，无缩进。

**key not found（exit 1）：**
```
error: key 'username' not found in any scope

hint: did you mean 'user.name'?
hint: use libra config list to see all configured keys
```

**ambiguous set（exit 1）：**
```
error: cannot set 'user.name': 3 values exist for this key

hint: use libra config unset --all user.name first, or libra config set --add
```

**not a repository（exit 128）：**
```
error: not a libra repository (or any parent up to /)

hint: use --global to read/write user-level config without a repository
hint: use libra init to create a repository here
```

**vault not initialized（warning）：**
```
warning: vault not initialized, storing value in plaintext

hint: run libra init (with --vault) to enable encrypted storage
```

**internal error（exit 128）：**
```
error: database corruption detected in config store [LBR-INTERNAL-001]

hint: please report this issue at: https://github.com/web3infra-foundation/libra/issues
```

### 全部子命令 JSON Output 设计（`--json`）

所有 JSON 输出遵循统一信封格式，通过 `emit_json_data()` 输出到 stdout。错误 JSON 通过 `CliError` 输出到 stderr。

#### 信封格式

成功：
```json
{
  "ok": true,
  "command": "config",
  "action": "<子命令名>",
  "data": { ... }
}
```

失败：
```json
{
  "ok": false,
  "error_code": "LBR-XXX-NNN",
  "category": "<类别>",
  "exit_code": 1,
  "message": "<错误描述>",
  "hints": ["<建议1>", "<建议2>"]
}
```

---

#### `libra config set user.name "John Doe" --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "set",
  "data": {
    "scope": "local",
    "key": "user.name",
    "value": "John Doe",
    "encrypted": false
  }
}
```

#### `libra config set --add remote.origin.push "+refs/heads/*" --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "add",
  "data": {
    "scope": "local",
    "key": "remote.origin.push",
    "value": "+refs/heads/*",
    "encrypted": false
  }
}
```

#### `libra config get user.name --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "get",
  "data": {
    "key": "user.name",
    "value": "John Doe",
    "origin": "local",
    "default_applied": false
  }
}
```

#### `libra config get user.name --default "Unknown" --json`（key 不存在时）
```json
{
  "ok": true,
  "command": "config",
  "action": "get",
  "data": {
    "key": "user.name",
    "value": "Unknown",
    "origin": null,
    "default_applied": true
  }
}
```

#### `libra config get --all remote.origin.fetch --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "get-all",
  "data": {
    "key": "remote.origin.fetch",
    "values": [
      "+refs/heads/*:refs/remotes/origin/*",
      "+refs/tags/*:refs/tags/*"
    ],
    "origin": "local"
  }
}
```

#### `libra config get --regexp "remote\..*\.url" --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "get-regexp",
  "data": {
    "pattern": "remote\\..*\\.url",
    "entries": [
      { "key": "remote.origin.url", "value": "git@github.com:user/repo.git", "origin": "local" },
      { "key": "remote.upstream.url", "value": "git@github.com:org/repo.git", "origin": "local" }
    ]
  }
}
```

#### `libra config list --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "list",
  "data": {
    "scope": "local",
    "entries": [
      { "key": "user.name", "value": "John Doe", "encrypted": false },
      { "key": "user.email", "value": "john@example.com", "encrypted": false },
      { "key": "remote.origin.url", "value": "git@github.com:user/repo.git", "encrypted": false },
      { "key": "remote.origin.fetch", "value": "+refs/heads/*:refs/remotes/origin/*", "encrypted": false },
      { "key": "branch.main.remote", "value": "origin", "encrypted": false },
      { "key": "branch.main.merge", "value": "refs/heads/main", "encrypted": false }
    ],
    "count": 6
  }
}
```

#### `libra config list --show-origin --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "list",
  "data": {
    "scope": "all",
    "cascade": true,
    "entries": [
      { "key": "user.name", "value": "John Doe", "origin": "local", "encrypted": false },
      { "key": "user.email", "value": "john@example.com", "origin": "local", "encrypted": false },
      { "key": "remote.origin.url", "value": "git@github.com:user/repo.git", "origin": "local", "encrypted": false },
      { "key": "core.editor", "value": "vim", "origin": "global", "encrypted": false },
      { "key": "user.signingkey", "value": "<REDACTED>", "origin": "global", "encrypted": true },
      { "key": "push.default", "value": "current", "origin": "global", "encrypted": false }
    ],
    "count": 6
  }
}
```

#### `libra config list --vault --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "list-vault",
  "data": {
    "entries": [
      { "key": "vault.env.GEMINI_API_KEY", "value": "<REDACTED>", "origin": "local", "encrypted": true },
      { "key": "vault.env.OPENAI_API_KEY", "value": "<REDACTED>", "origin": "global", "encrypted": true },
      { "key": "vault.env.OPENAI_BASE_URL", "value": "https://api.openai.com/v1", "origin": "global", "encrypted": false }
    ],
    "encrypted_count": 2,
    "plaintext_count": 1
  }
}
```

#### `libra config set vault.env.GEMINI_API_KEY --json`（交互式）
```json
{
  "ok": true,
  "command": "config",
  "action": "set",
  "data": {
    "scope": "local",
    "key": "vault.env.GEMINI_API_KEY",
    "value": "<REDACTED>",
    "encrypted": true
  }
}
```

#### `libra config unset user.signingkey --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "unset",
  "data": {
    "scope": "local",
    "key": "user.signingkey",
    "removed_count": 1
  }
}
```

#### `libra config unset --all remote.origin.fetch --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "unset-all",
  "data": {
    "scope": "local",
    "key": "remote.origin.fetch",
    "removed_count": 2
  }
}
```

#### `libra config import --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "import",
  "data": {
    "source": "git-global",
    "target_scope": "global",
    "imported": 12,
    "skipped_duplicates": 2,
    "ignored_invalid": 0
  }
}
```

#### `libra config edit --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "edit",
  "data": {
    "scope": "local",
    "added": 1,
    "modified": 1,
    "removed": 0,
    "total_changes": 2
  }
}
```

#### `libra config generate-ssh-key --remote origin --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "generate-ssh-key",
  "data": {
    "remote": "origin",
    "type": "RSA",
    "bits": 3072,
    "key_id": "libra-john",
    "public_key": "ssh-rsa AAAA...xxxx libra-john",
    "pubkey_config_key": "vault.ssh.origin.pubkey",
    "private_key_path": "~/.libra/ssh-keys/<repo-id>/origin/id_rsa"
  }
}
```

#### `libra config list --ssh-keys --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "list-ssh-keys",
  "data": {
    "keys": [
      { "remote": "origin", "type": "RSA 3072", "public_key": "ssh-rsa AAAA...xxxx libra-john", "key_id": "libra-john" },
      { "remote": "upstream", "type": "RSA 3072", "public_key": "ssh-rsa AAAA...yyyy libra-john", "key_id": "libra-john" }
    ],
    "count": 2
  }
}
```

#### `libra config generate-gpg-key --name "Alice" --email "alice@example.com" --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "generate-gpg-key",
  "data": {
    "usage": "signing",
    "type": "PGP",
    "bits": 2048,
    "user": "Alice <alice@example.com>",
    "valid_days": 3650,
    "fingerprint": "ABCD 1234 5678 EFGH",
    "pubkey_config_key": "vault.gpg.pubkey",
    "signing_enabled": true
  }
}
```

#### `libra config generate-gpg-key --usage encrypt --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "generate-gpg-key",
  "data": {
    "usage": "encrypt",
    "type": "PGP",
    "bits": 2048,
    "valid_days": 3650,
    "fingerprint": "IJKL 9012 3456 MNOP",
    "pubkey_config_key": "vault.gpg.encrypt.pubkey",
    "signing_enabled": false
  }
}
```

#### `libra config list --gpg-keys --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "list-gpg-keys",
  "data": {
    "keys": [
      { "usage": "signing", "type": "PGP 2048", "fingerprint": "ABCD 1234 5678 EFGH", "signing_enabled": true },
      { "usage": "encrypt", "type": "PGP 2048", "fingerprint": "IJKL 9012 3456 MNOP", "signing_enabled": false }
    ],
    "count": 2
  }
}
```

#### 错误 JSON（输出到 stderr）

**key not found（exit 1）：**
```json
{
  "ok": false,
  "error_code": "LBR-CLI-002",
  "category": "cli",
  "exit_code": 1,
  "message": "key 'username' not found in any scope",
  "hints": ["did you mean 'user.name'?", "use libra config list to see all configured keys"]
}
```

**not a repository（exit 128）：**
```json
{
  "ok": false,
  "error_code": "LBR-REPO-001",
  "category": "repo",
  "exit_code": 128,
  "message": "not a libra repository (or any parent up to /)",
  "hints": ["use --global to read/write user-level config without a repository", "use libra init to create a repository here"]
}
```

**internal error（exit 128）：**
```json
{
  "ok": false,
  "error_code": "LBR-INTERNAL-001",
  "category": "internal",
  "exit_code": 128,
  "message": "database corruption detected in config store",
  "hints": ["please report this issue at: https://github.com/web3infra-foundation/libra/issues"]
}
```

---

#### `--help` EXAMPLES 段

```
EXAMPLES:
    libra config set user.name "John Doe"              Set local config value
    libra config get user.name                         Get value (cascade lookup)
    libra config list                                  List all local entries
    libra config list --show-origin                    List with scope labels
    libra config set --global user.email "j@x.com"     Set global config
    libra config unset user.signingkey                 Remove a key
    libra config import                                Import from Git config
    libra config set vault.env.GEMINI_API_KEY          Store API key (interactive)
    libra config list --vault                          List vault entries
    libra config generate-ssh-key --remote origin      Generate SSH key for remote
    libra config generate-gpg-key                      Generate GPG signing key
    libra config edit                                  Open config in $EDITOR
```

### 文档输出

创建 `docs/commands/config/README.md`，Markdown 格式的完整 config 文档，包含：
- 概述与设计理念
- 子命令参考（set/get/list/unset/import/edit/generate-ssh-key/generate-gpg-key）
- Scope 与优先级说明
- Vault 加密存储说明
- 环境变量解析优先级说明
- SSH/GPG Key 管理说明
- JSON 输出 schema
- 完整 Libra vs Git vs jj 功能对比表（见下方）

#### Libra vs Git vs jj 功能对比（文档末尾附录）

| Feature | Git | jj | Libra |
|---------|-----|-----|-------|
| Implicit set | `git config key val` | No (requires `set`) | `libra config set key val` + 兼容 `libra config key val` |
| Subcommand style | No | Yes (`set/get/list/edit/path`) | Yes (`set/get/list/unset/import/edit`) |
| Get value | `git config key` | `jj config get key` | `libra config get key` |
| List | `git config -l` | `jj config list` | `libra config list` |
| Edit in editor | `git config -e` | `jj config edit` | `libra config edit` |
| Regex search | `git config --get-regexp` | No | `libra config get --regexp` |
| Show origin | `git config --show-origin` | No | `libra config list --show-origin` |
| Type coercion | `--type=bool\|int\|path` | No (TOML types) | `--type=bool\|int\|path` |
| Default fallback | `--default value` | No | `--default value` |
| Null-delimited | `-z` | No | `-z` |
| Rename/remove section | Yes | No | `--rename-section` / `--remove-section` |
| JSON output | No | No | **`--json`** ✓ |
| Secret redaction | No | No | **Auto-detect** ✓ |
| Import from Git | N/A | N/A | **`libra config import`** ✓ |
| Vault encryption | No | No | **AES-256-GCM** ✓ |
| Env var vault | No | No | **`vault.env.*`** ✓ |
| SSH key per remote | No | No | **`generate-ssh-key --remote`** ✓ |
| GPG key generation | No | No | **`generate-gpg-key`** ✓ |
| Env var resolution | No fallback | No fallback | **CLI → repo → global → env** ✓ |
| Conditional config | `includeIf` | `[[when]]` blocks | Not supported |
| Worktree scope | `--worktree` | `--workspace` | Not supported |
| Arbitrary file | `--file <path>` | No | Not supported |
| Storage format | INI text files | TOML text files | **SQLite + vault** |
| Scopes | system/global/local/worktree | user/repo/workspace | **local/global** |

### Init 改进备忘（待 init 详细分析时添加）

以下 init 相关改动暂记于此，不在 config 改进中实施：

- [ ] `libra init` 使用 `--global` 时默认生成 SSH Key for origin（复用 `vault::generate_ssh_key()`，存储到 global config `vault.ssh.origin.pubkey`）
- [ ] `libra init` 使用 `--global` 时默认生成一组 GPG Key 用于 commit 签名（存储到 global config，设置 `vault.signing=true`）
- [ ] `libra init` 不使用 `--global` 时（local init），不自动生成 SSH/GPG key，用户可后续通过 `libra config generate-ssh-key/generate-gpg-key` 手动生成
- [ ] init 完成后输出 SSH 公钥并提示用户添加到 GitHub/GitLab

### Vault 命令废弃计划

当所有命令改进批次完成后，`libra vault` 命令将被废弃并删除：

- [ ] config 改进完成后，`libra vault generate-gpg-key` → 转发到 `libra config generate-gpg-key` + deprecation warning
- [ ] config 改进完成后，`libra vault generate-ssh-key` → 转发到 `libra config generate-ssh-key` + deprecation warning
- [ ] config 改进完成后，`libra vault gpg-public-key` → 转发到 `libra config get vault.gpg.pubkey` + deprecation warning
- [ ] config 改进完成后，`libra vault ssh-public-key` → 转发到 `libra config get vault.ssh.origin.pubkey` + deprecation warning
- [ ] 所有命令改进批次完成后，删除 `src/command/vault.rs`，从 `src/cli.rs` 中移除 vault 子命令
- [ ] `src/internal/vault.rs` 保留，作为加密基础设施继续被 config 使用

---

## 验证方式

每个命令改进完成后：
1. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
2. `cargo test --all` 全部通过
3. 新增/更新对应的集成测试（`tests/command/`）
4. 人工验证：正常路径确认消息 + 错误路径 hint 提示 + `--json` 输出格式正确
