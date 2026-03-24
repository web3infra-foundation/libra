## Config 命令改进详细计划

> 最后编写时间：2026-03-24

同时落地 Cross-Cutting Improvements A/B/F/G。

### 设计原则

1. **local 和 global scope 敏感 key 通过 vault 加密存储**
2. **子命令风格 CLI**：`libra config get/set/list/unset/generate-ssh-key/generate-gpg-key`
3. **两级 scope**：`--local`（仓库级，默认）、`--global`（用户级）。**⚠️ Breaking change：`--system` scope 移除**——system 级配置在多用户环境下的权限隔离（unseal key 仅 root 可读，普通用户级联读取时解密失败导致命令崩溃）会造成比收益更大的安全风险和运维复杂度。现有 `ConfigScope::System` 实现标记 `#[deprecated]` 并移除。使用 `--system` 时报错并提示迁移到 `--global`。此变更需记入 CHANGELOG
4. **环境变量解析优先级**（从高到低）：CLI 参数 → 系统环境变量 → 仓库 config → 全局 config。环境变量优先于配置文件，符合 12-Factor App 原则和主流 CLI 工具惯例（Git、Docker、AWS CLI、kubectl），确保 `GEMINI_API_KEY=B libra push` 这类 per-process override 始终生效
5. **AI provider 的 `from_env()` 改造留到后续批次**（`src/internal/ai/providers/*/client.rs`），但 `src/internal/ai/hooks/runtime.rs` 等直接读取配置的模块**纳入本批迁移**
6. **依赖命令同步迁移**：本批将所有依赖 `Config` 旧 API 的命令和模块**直接迁移**到新 `config_kv` 后端，不做 shim/代理层。**旧 `config` 表不在本批处理范围内**（不读、不写、不迁移、不导入），`libra config import` 仅从 Git config 导入
7. **SSH 私钥改为 vault-backed 存储 + 按需临时落盘；GPG 私钥由 vault PKI 引擎托管，不导出**：
    - **SSH 私钥**：存入 vault（`config_kv` 表，`vault.ssh.<remote>.privkey`，加密存储），不再持久化写入 `~/.libra/ssh-keys/`。SSH transport 调用时，从 vault 解密私钥 → 写入临时文件 → 传递给 SSH client → 操作完成后删除
    - **GPG 私钥**：由 vault.db PKI 引擎管理（现有 `vault::pgp_sign()` 机制），签名操作在 vault 内部执行，私钥**不导出**到文件系统
    - **临时文件安全加固**（仅适用于 SSH 私钥）：
    - 使用专用目录 `~/.libra/tmp/`（`0o700`），不使用系统 `/tmp`（避免其他用户读取）
    - `tempfile::NamedTempFile` 权限 `0o600`
    - **不做无差别启动时清理**（避免并发进程互相破坏）。主要依赖 `tempfile::NamedTempFile` 的 Drop 自动删除。**过期 GC**：`fetch`/`push`/`pull` 启动时，仅清理 `~/.libra/tmp/` 下**修改时间超过 24 小时**的 `.tmp` 文件（正常操作不可能持续 24 小时，超过即为残留）。这既不影响并发进程，又保证长期整洁
    - **⚠️ 已知限制**：SIGKILL/OOM 时 Drop 不执行，私钥会残留在 `~/.libra/tmp/`。完全消除此风险需要 in-process SSH 库（见第七批改进项 **H**）
    - **⚠️ Agent blocker（部分解决）**：临时文件方案解决了无状态容器的私钥丢失问题，但仍依赖文件系统写入。改进项 **H**（in-process SSH）将完全消除此依赖
8. **local 和 global scope 均支持加密**：为消除"前端脱敏但后端明文"的虚假安全感，此批为每个 scope 提供独立的 vault unseal key：
    - **local scope**：per-repo unseal key，存储在 `~/.libra/vault-keys/<repo-id>`（现有机制）
    - **global scope**：per-user unseal key，存储在 `~/.libra/vault-unseal-key`（新增，`0o600`），首次写入敏感 key 时 lazy init
    - 各 scope 的 encrypted root token 存储在对应 scope 的 `config_kv` 表中（key = `vault.roottoken_enc`）
    - 复用现有 `encrypt_token()`/`decrypt_token()` 原语（AES-256-GCM + HKDF-SHA256）
    - `--encrypt` 在 local 和 global scope 上均可用
9. **vault 命令在此批直接删除**：vault 命令的全部功能由 config 命令吸收后，此批直接删除 `src/command/vault.rs`、从 `src/cli.rs` 移除 vault 子命令注册、删除 `tests/command/vault_test.rs` 和 `tests/command/vault_cli_test.rs`。vault 测试用例中的功能覆盖迁移到 config 对应的测试中。`src/internal/vault.rs` 保留，作为加密基础设施继续被 config 使用
10. **不支持 `config edit`**：Libra 使用 SQLite（`config_kv` 表）存储配置，而非 Git 的纯文本 INI 文件。`config edit` 需要将 SQLite 数据导出为纯文本 → 用户编辑 → diff 回写，但多值 key（如 `remote.origin.fetch`）在纯文本中没有行级 UUID 主键，乱序/部分修改/删除时无法准确推断对应的 UPDATE/DELETE 操作，极易导致数据丢失或更新混乱。因此 Libra 不兼容 `config edit`（`git config -e`/`jj config edit`），使用时报错：`error: config edit is not supported (SQLite storage does not support text-based editing)`，并提示使用 `config set`/`config unset`/`config list` 组合操作
11. **统一敏感 key 分类规则**（`is_sensitive_key()`）：以下规则是唯一判定标准，大小写无关。**`list --vault` 有独立过滤规则，不共用此规则**（见特性 3）。此规则控制三个行为：
    - **加密**（local 和 global scope）：`is_sensitive_key()` 为 true 的 key 自动加密存储
    - **脱敏显示**：`encrypted == true` 的 key 在 `list`/`list --vault`/`list --show-origin` 输出中 value 显示为 `<REDACTED>`（见下方脱敏判定逻辑）。**`get` 命令例外**：`get` 对 `encrypted=1` 的 key 默认脱敏输出 `<REDACTED>`，支持 `--reveal` 标志输出真实明文。**`--reveal` 限制**（仅凭 key namespace 判断，无需元数据）：vault 内部凭据一律拒绝（以 `.privkey` 结尾或等于 `vault.unsealkey`/`vault.roottoken`/`vault.roottoken_enc`）。对于精确查询（`get <key> --reveal`），命中内部凭据直接报错 `error: key '<key>' is a vault internal credential and cannot be revealed`，exit 1；对于批量查询（`get --all` 或 `get --regexp` 配合 `--reveal`），命中内部凭据时**不报错中断**，而是优雅降级继续输出 `<REDACTED>`，其他合法敏感 key 正常明文输出。其余所有 `encrypted=1` 的 key（含 `vault.env.*` 和用户 `--encrypt` 的业务 key）均允许 `--reveal`
    - **脱敏判定逻辑**（渲染时）：`encrypted == true` → 脱敏。即**只看数据库中的 `encrypted` 字段**，不再用 `is_sensitive_key()` 做运行时推断。这确保：
      - 正常敏感 key（自动加密，`encrypted=1`）→ 脱敏 ✓
      - `--encrypt` 强制加密的普通 key（`encrypted=1`）→ 脱敏 ✓
      - `--plaintext` 逃生舱写入的 key（`encrypted=0`，但 `is_sensitive_key(key) == true`）→ **不脱敏**（尊重用户明文意图），在 `list` 中显示 `[PLAINTEXT]` 警告后缀
      - 普通明文 key（`encrypted=0`，`is_sensitive_key(key) == false`）→ 不脱敏，无警告 ✓
      - **`[PLAINTEXT]` 警告条件**：`is_sensitive_key(key) == true && encrypted == false`（两者同时满足才显示，避免普通 key 误报）

    分类规则：
    - key 以 `vault.env.` 开头
    - key 以 `.privkey` 结尾（匹配 `vault.ssh.*.privkey`、`vault.gpg.privkey`、`vault.gpg.*.privkey`）
    - key 等于 `vault.unsealkey` 或 `vault.roottoken` 或 `vault.roottoken_enc`
    - key 名（最后一个 `.` 之后的部分）先**归一化**（去除 `_` 和 `-`，转小写），然后检查是否**包含**以下任一子串：`secret`、`token`、`password`、`credential`、`privatekey`、`accesskey`、`apikey`、`secretkey`。这确保 `api_key`、`access-key`、`private_key`、`API_KEY` 等常见变体均被匹配
    - **注意**：`signingkey` **不在**敏感列表中——`user.signingkey` 通常是 key ID 或 fingerprint（公开信息），不是私钥材料
    - **显式排除**：归一化后以 `pubkey` 或 `publickey` 结尾的 key **不是**敏感 key（公钥应明文存储和显示）

### 特性 1：Vault-Backed 扁平 Key/Value 存储

**背景：** 当前 config 表使用 `configuration` / `name` / `key` 三列拆分存储一个逻辑 key（如 `remote.origin.url` → configuration="remote", name="origin", key="url"）。拆分增加查询复杂度，且值以明文存储。

**方案：** 新增 `config_kv` 表作为统一存储后端（local 和 global scope 敏感 key 均加密，普通 key 明文）：

```sql
CREATE TABLE IF NOT EXISTS `config_kv` (
    `id` INTEGER PRIMARY KEY AUTOINCREMENT,
    `key` TEXT NOT NULL,                      -- 完整 dotted key，如 "remote.origin.url"
    `value` TEXT NOT NULL,                    -- 敏感 key 为 vault 加密值（local/global scope），其余为明文
    `encrypted` INTEGER NOT NULL DEFAULT 0    -- 0=plaintext, 1=vault-encrypted
    -- 注意：不添加 UNIQUE 约束。--add 允许同一 key 存在完全相同的 value（Git 兼容）。
);
CREATE INDEX idx_config_kv_key ON config_kv(`key`);
```

- **local scope** 存储在 `.libra/libra.db` 的 `config_kv` 表
- **global scope** 存储在 `~/.libra/config.db` 的 `config_kv` 表
- scope 由数据库文件位置决定，不在表中存储
- **不支持 system scope**（已移除，见设计原则 #3）

**加密策略（所有 scope 统一）：**
- **所有 scope**：
  - `is_sensitive_key()` 匹配的 key → 自动加密；**vault 未初始化时自动 lazy init**（首次写入敏感 key 触发 vault 初始化，生成对应 scope 的 unseal key）
  - 普通 key → 明文存储（可通过 `--encrypt` 强制加密）
  - `--encrypt` 显式标志 → 强制加密；vault 未初始化时同样 lazy init
  - `--plaintext` 显式标志 → 强制明文存储，跳过自动加密和脱敏。适用于 `is_sensitive_key()` 规则误伤场景（如 `http.proxyPasswordPrompt = false`、`alias.token = "!echo ..."`）。**限制**：`--plaintext` 不允许用于 vault 内部凭据和真正的 secret 命名空间——以下 key 一律拒绝 `--plaintext`：`vault.env.*`、`*.privkey`、`vault.unsealkey`、`vault.roottoken`、`vault.roottoken_enc`（报错 `error: --plaintext cannot be used with vault internal/secret keys`，exit 1）。`--plaintext` 与 `--encrypt` 互斥，同时使用报错 exit 2
  - **加密状态继承**（UPDATE 和 `--add` INSERT 均适用）：操作已存在的 key 时，如果数据库中该 key 已有 `encrypted = 1` 的条目，即使本次未提供 `--encrypt`，系统也自动继承加密属性。这包括：UPDATE 覆盖时继承加密；`--add` 追加新行时，检查同名 key 是否存在 `encrypted=1` 条目，存在则新行也强制加密。要将已加密 key 降级为明文，使用 `--plaintext` 显式覆盖
  - **同键同态约束**：不允许同一个 key 混合存在明文和加密的多行记录。如果在 `--add` 时显式指定的 `--plaintext` / `--encrypt` 与数据库中该 key 已有记录的加密状态冲突，直接报错拒绝插入（`error: cannot mix encrypted and plaintext values for the same key`，exit 1）
  - **多值覆盖保护**：如果对已存在多个值的 key 使用 `set` 覆盖（而非 `--add` 或 `unset`），即使携带了 `--plaintext` 或 `--encrypt` 标志，也必须抛出多值冲突错误（`exit 5`），防止一瞬间意外清空并覆盖整个多值数组。必须先 `unset --all` 才能改变其整体加密状态
- **vault lazy init 行为**：首次在某个 scope 写入敏感/加密 key 时，自动初始化该 scope 的 vault（生成 unseal key + root token），无需用户显式操作。初始化成功后输出提示："Initialized vault for <scope> scope"
- **vault init 失败处理**：如果 lazy init 失败（如文件系统权限不足），直接报错，不做明文降级

**类型辅助方法（此批不实现 `--type` CLI flag，但提供内部 API）：**
- `config_kv_get_bool(key)` — 归一化 `true/yes/on/1` → `true`，`false/no/off/0` → `false`，其他值报错
- `config_kv_get_int(key)` — 解析整数（支持 `k`/`m`/`g` 后缀），无效值报错
- `config_kv_set` 对已知路径做基本合法性校验：`vault.signing` 只接受 `true`/`false`，`core.autocrlf` 只接受 `true`/`false`/`input`
- 校验失败 → `error: invalid value '<val>' for key '<key>': expected bool (true/false)`，exit 1。**安全**：如果 `is_sensitive_key(key) == true` 或 `encrypted == true`，错误消息中的 `<val>` 替换为 `<REDACTED>`（防止敏感值通过校验错误消息泄漏到终端或 CI 日志）

**依赖命令同步迁移（不做 shim）：**
- 本批直接将所有调用旧 `Config` API 的命令和模块迁移到新 `config_kv` 后端
- 旧 `config` 表不在本批处理范围内：不读、不写、不迁移、不导入
- 旧 `Config` API（`get`/`insert`/`update`/`remove` 等）标记 `#[deprecated]`，保留编译兼容但不再被任何运行时路径调用

**必须一起迁移的命令：**
`config`、`remote`、`fetch`、`pull`、`push`、`open`、`branch`、`clone`、`init`、`commit`、`status`、`log`、`reflog`、`cloud`（`vault` 不在此列——本批直接删除，见 Vault 命令删除计划）

**必须一起迁移的内部/支撑模块：**
`src/internal/vault.rs`、`src/internal/tag.rs`、`src/internal/protocol/local_client.rs`、`src/internal/protocol/lfs_client.rs`、`src/internal/ai/hooks/runtime.rs`、`src/utils/util.rs`、`src/utils/client_storage.rs`

**跨模块联动要求：**
- **`remote rename` 级联更新 SSH key**：`libra remote rename <old> <new>` 必须同步更新 config_kv 中所有 `vault.ssh.<old>.*` key 为 `vault.ssh.<new>.*`（包括 `.pubkey` 和 `.privkey`）。实现方式：执行前检查目标 remote 的 SSH key 是否已存在（避免覆写），然后执行 `UPDATE config_kv SET key = REPLACE(key, 'vault.ssh.<old>.', 'vault.ssh.<new>.') WHERE key LIKE 'vault.ssh.<old>.%'`。对应修改 `src/command/remote.rs` 的 rename 逻辑
- **`remote remove` 清理 SSH key**：`libra remote remove <name>` 必须同步删除 `vault.ssh.<name>.*` 条目

**涉及文件：**
- `sql/sqlite_20260309_init.sql` — 新增 `config_kv` 表
- `src/internal/config.rs` — 新 CRUD 接口（基于 config_kv 表）+ 类型辅助方法（`get_bool`/`get_int`）。**API 签名要求**：级联查询函数（如 `get_all_config_cascaded`）必须返回 `Vec<(String, ConfigScope)>` 而非 `Vec<String>`，以支持 JSON 输出中的 `origin` 字段
- `src/internal/model/config_kv.rs` — **新增** `ConfigKv` SeaORM entity（旧 `src/internal/model/config.rs` 保留但标记 `#[deprecated]`）
- `src/command/config.rs` — 切换到新 API
- `src/internal/db.rs` — schema 迁移

### 特性 2：子命令风格 CLI + Git 兼容

**语法设计：子命令为主，同时保留 Git 兼容的 flag 风格：**

| 操作 | 子命令风格 | Git 兼容风格 | Git 对应 |
|------|----------|------------|---------|
| 设置值 | `libra config set key [value]` | `libra config key [value]` | `git config key value` |
| 获取值 | `libra config get key` | `libra config --get key` | `git config --get key` |
| 获取敏感值（明文） | `libra config get --reveal key` | N/A | N/A |
| 获取所有值 | `libra config get --all [--reveal] key` | `libra config --get-all [--reveal] key` | `git config --get-all key` |
| 列表 | `libra config list` | `libra config -l` / `--list` | `git config -l` |
| 列表+来源 | `libra config list --show-origin` | `libra config -l --show-origin` | `git config -l --show-origin` |
| 删除 | `libra config unset key` | `libra config --unset key` | `git config --unset key` |
| 删除所有 | `libra config unset --all key` | `libra config --unset-all key` | `git config --unset-all key` |
| 添加重复 | `libra config set --add key value` | `libra config --add key value` | `git config --add key value` |
| 强制加密 | `libra config set --encrypt key value` | N/A | N/A（local 和 global scope 均可用） |
| 强制明文 | `libra config set --plaintext key value` | N/A | N/A（跳过自动加密和脱敏，规则误伤逃生舱） |
| 从 stdin 读值 | `libra config set --stdin key` | N/A | N/A（CI/CD 安全传值） |
| 正则搜索 | `libra config get --regexp [--reveal] pattern` | `libra config --get-regexp [--reveal] pattern` | `git config --get-regexp` |
| 编辑器打开 | N/A（不支持，SQLite 存储） | N/A | `git config -e` |
| 导入 Git | `libra config import [--scope]` | `libra config --import` | N/A |
| 配置文件路径 | `libra config path [--scope]` | N/A | `jj config path` |
| 生成 SSH Key | `libra config generate-ssh-key` | N/A | N/A |
| 生成 GPG Key | `libra config generate-gpg-key` | N/A | N/A |
| 查看 GPG 公钥 | `libra config get vault.gpg.pubkey` | N/A | N/A（原 `libra vault gpg-public-key`） |
| 查看 SSH 公钥 | `libra config get vault.ssh.<remote>.pubkey` | N/A | N/A（原 `libra vault ssh-public-key`） |

> **原则：** 所有 Git 用户熟悉的 flag（`-l`、`--get`、`--get-all`、`--get-regexp`、`--unset`、`--unset-all`、`--add`、`--show-origin`）均保留兼容。子命令风格是推荐用法，flag 风格确保 Git 用户无学习成本。`-e`（edit）不保留——SQLite 存储不支持文本编辑，使用 `-e` 时报错并提示。

**`config set` value 参数规则：** `value` 为可选参数 `[value]`。缺省时的行为由 key 类型决定：
- **受保护 key**（`is_sensitive_key(key) == true` 或使用了 `--encrypt` 或**数据库中该 key 已有 `encrypted=1` 的条目**）且**未使用 `--plaintext`**：触发安全交互式输入（`Enter value for <key>: ****`），要求交互环境（`stdin.is_terminal() == true && !output.is_json()`）；非交互环境下报错 `LBR-CLI-002`。**`--json`/`--machine` 视为非交互**——即使处于 TTY，也不启动交互输入（避免 Agent 通过 PTY 调用时死锁）
- **`--plaintext` + value 缺省**：直接报参数缺失错误（`error: missing value for key '<key>'`），不触发交互输入（`--plaintext` 的语义是"我知道我在做什么"，不需要安全输入保护）
- **`--stdin` 标志**：从标准输入读取 value（**读取全部内容直到 EOF**，仅去除最末尾的一个换行符，原生支持 JSON/PEM 等多行凭证）。适用于 CI/CD 和 Agent 管道场景，避免明文参数暴露在 shell history 和进程列表中。示例：`echo "$MY_CI_SECRET" | libra config set --stdin vault.env.API_KEY`。`--stdin` 不要求 TTY，可与 `--encrypt` 或 `--plaintext` 组合使用。如果同时提供了 `[value]` 参数和 `--stdin`，报错 `error: cannot use both value argument and --stdin`，exit 2
- **普通 key**（`is_sensitive_key() == false` 且无 `--encrypt` 且数据库中无 `encrypted=1` 条目）：直接报参数缺失错误（`error: missing value for key '<key>'`），exit 2

**scope 标志：**
- `--local` — 仓库级（默认），存储在 `.libra/libra.db`
- `--global` — 用户级，存储在 `~/.libra/config.db`
- `--system` — **不支持**（已移除，使用 `--system` 时报错并提示使用 `--global`）

**config path 子命令：**
- `libra config path` — 打印当前 scope 的数据库文件路径
- `libra config path --local` — 打印仓库级配置路径
- `libra config path --global` — 打印用户级配置路径
- `libra config path --system` — 报错（system scope 已移除）

**config import scope 映射规则：**

| Libra scope | Git 来源 | 示例命令 |
|-------------|---------|---------|
| `--local`（默认） | `.git/config`（仓库目录下的 Git local config） | `libra config import` |
| `--global` | `~/.gitconfig`（`git config --global --list`） | `libra config import --global` |
| `--system` | N/A（不支持） | `libra config import --system` → 报错 |

- import 通过 `git config --<scope> --list -z` 获取对应 scope 的配置。**关键**：必须显式传递 `--<scope>` 给 git，确保 Git 只读取该 scope 的配置文件：
  - `import --local` → `git config --local --list -z`（只读 `.git/config`，`[includeIf]` 基于当前目录求值——对 local 来说这是正确的）
  - `import --global` → `git config --global --no-includes --list -z`（只读 `~/.gitconfig` 本体，`--no-includes` 物理阻断 `[include]`/`[includeIf]` 求值，避免当前目录上下文触发条件配置被错误固化为全局默认）
- 默认 scope 遵循全局规则（`--local`），不做特殊例外
- **仓库前置条件**：`import --local` **要求当前目录是已初始化的 Libra 仓库**（存在 `.libra/`）。不支持"纯 Git 仓库下导入并自动创建 `.libra/`"——用户需先 `libra init`，再 `libra config import`
- **敏感 key 处理**：import 复用 `config set` 的敏感判定与加密策略。导入时对每个 key 调用 `is_sensitive_key()`，所有 scope 的敏感 key 均自动加密（vault lazy init）。vault init 失败则拒绝该条目并计入 skipped。导入完成后输出统计（imported / auto_encrypted / skipped_duplicate）。**导入语义（单值 vs 多值）**：Git 配置中大部分 key 是单值的（如 `user.name`），少数是多值的（如 `remote.origin.fetch`）。导入时区分处理：
  - **隐式布尔值**：如果在 `-z` 输出的 chunk 中找不到 `\n` 分隔符（如 Git 的 `[core] bare` 配置），则视其为隐式布尔值，自动解析 value 并存为 `"true"`
  - **已知多值 key**（`remote.*.fetch`、`remote.*.push`、`remote.*.pushurl`、`branch.*.merge`、`url.*.insteadOf`、`url.*.pushInsteadOf`、`http.*.extraHeader`、`credential.helper`）：按 `--add` 语义追加，`(key, value)` 完全一致才算 duplicate 并跳过
  - **其余 key**：last-one-wins 语义——同 key 多次出现时仅保留最后一个值。**如果检测到未知 key 有多个不同值被压扁，输出 warning**：`warning: key '<key>' has N values in Git config, only last value kept (not in known multi-value list)`，计入导入统计的 `collapsed_multivalue_warnings` 计数
- `import --system` → 报错 `error: --system scope is not supported`，exit 2
- 错误处理：
  - `import --local` 在非 Libra 仓库目录 → exit 128，提示 "not a libra repository (use libra init first)"
  - `import --local` 在没有 `.git/` 的 Libra 仓库 → exit 1，提示 "no Git config found (.git/config does not exist)"
  - Git config 来源为空（文件不存在或无配置项）→ exit 1，提示 "no Git config entries found for scope <scope>"
  - `git` 命令执行失败（权限不足等）→ exit 128，附带 git 的 stderr 输出

### 特性 3：环境变量 Vault 存储

**背景：** AI provider API key 等只能通过系统环境变量提供，不友好且不安全。

**方案：** `vault.env.*` 命名空间存储环境变量。local 和 global scope 敏感值均自动加密。

**`list --vault` 过滤规则（独立于 `is_sensitive_key()`）：** `list --vault` 仅列出 `vault.env.*` 前缀的条目，不列出其他敏感 key（如 `vault.unsealkey`、`*.privkey`）。该命令的语义是"列出通过 vault.env 存储的环境变量"，而非"列出所有敏感项"。
**命名建议**：为与系统 Shell 环境对齐，`vault.env.` 后的部分建议使用大写与下划线格式（如 `vault.env.GEMINI_API_KEY`）。

**环境变量解析优先级（从高到低）：**

```
1. CLI 参数（如 --api-key, --api-base）                ← 最高，显式传入
2. 系统环境变量（std::env::var）                        ← per-process override（12-Factor）
3. 仓库 config（vault.env.* in .libra/libra.db）       ← 项目级配置
4. 全局 config（vault.env.* in ~/.libra/config.db）    ← 最低，用户级默认
```

**统一入口函数：**
```rust
/// 解析环境变量，按优先级查找。
/// 返回 Result 而非 Option：vault 解密/DB 失败是错误，不应静默 fallthrough。
/// - Ok(Some(val)) — 找到值
/// - Ok(None) — 所有来源均未配置
/// - Err(e) — vault/DB 查询失败（调用方应向用户报告，而非静默降级）
///
/// CLI 参数由调用方在调用前自行处理，若有则不调用此函数。
///
/// `name` 是原始环境变量名（如 "GEMINI_API_KEY"），函数内部自动加 `vault.env.` 前缀
/// 查询 config，最后用原始 `name` 查询系统环境变量。
pub async fn resolve_env(name: &str) -> Result<Option<String>> {
    let vault_key = format!("vault.env.{}", name);

    // 1. 系统环境变量 — per-process override（12-Factor App 原则）
    if let Ok(val) = std::env::var(name) { return Ok(Some(val)); }

    // 2. 仓库 config (local scope, vault 加密)
    match config_kv_get_local(&vault_key).await {
        Ok(Some(val)) => return Ok(Some(val)),
        Ok(None) => {}                          // 未配置，继续查找
        Err(e) => return Err(e.context(format!(
            "failed to read '{name}' from local config"
        ))),
    }
    // 3. 全局 config (global scope) — 最低优先级
    match config_kv_get_global(&vault_key).await {
        Ok(Some(val)) => return Ok(Some(val)),
        Ok(None) => {}
        Err(e) => return Err(e.context(format!(
            "failed to read '{name}' from global config"
        ))),
    }
    Ok(None)
}
```

> **设计决策：** 返回 `Result<Option<String>>` 而非 `Option<String>`。vault 解密失败与"未配置"是完全不同的语义——如果 vault 损坏时静默 fallthrough 到系统环境变量，可能导致使用错误凭据发送请求，这是安全隐患。调用方应在 `Err` 时向用户报告可行动的错误信息。

**涉及文件（此次改进范围）：**
- `src/internal/config.rs` — `resolve_env()` / `config_kv_get()` / `config_kv_set()`（local 和 global scope 均调用 vault 解密）
- `src/utils/client_storage.rs` — env var 读取 → `resolve_env()`
- `src/utils/d1_client.rs` — 同上

> **注：** `src/internal/ai/providers/*/client.rs` 的 `from_env()` → `resolve_env()` 改造**不在此次改进范围内**，留到后续批次。

### 特性 4：SSH Key 与 GPG Key 管理

**背景：** 当前 SSH/GPG key 生成在 `libra vault` 子命令中。将 key 管理集成到 `libra config` 更符合用户心智模型（config 管理仓库配置，key 是配置的一部分）。

#### SSH Key 管理

```bash
# 为指定 remote 生成 SSH Key（已有 vault 基础设施支持，RSA 3072）
# 此批仅支持 local scope（依赖 repo-id 定位私钥存储路径和 vault unseal key）
libra config generate-ssh-key --remote origin
libra config generate-ssh-key --remote upstream

# 查看 SSH 公钥
libra config get vault.ssh.origin.pubkey
libra config get vault.ssh.upstream.pubkey

# 列出所有 SSH keys
libra config list --ssh-keys
```

> **注：** `--global` key generation 需要先设计非 repo 作用域下的 key 存储布局和 unseal key 定位方案，留到后续批次。

**remote 名校验规则：** `--remote <name>` 的 `<name>` 必须满足以下约束（同时用于 config key 和文件路径，必须安全）：
- 只允许 `[a-zA-Z0-9_-]` 字符，长度 1-64
- 禁止 `.`（会造成 config key `vault.ssh.<remote>.pubkey` 歧义）、`/`、`\`、`..`（路径注入风险）
- **必须是已配置的 remote**（用户直接调用时）：`generate-ssh-key --remote <name>` 前先检查 `remote.<name>.url` 是否存在于 config 中；不存在则报错 `error: remote '<name>' not found, add it first with libra remote add`，exit 1。**豁免**：`libra init` bootstrap 内部调用 `generate-ssh-key` 时不做此校验（init 时 origin 尚未配置是正常流程）
- 校验失败 → `error: invalid remote name '<name>': only [a-zA-Z0-9_-] allowed`，exit 2

**存储：**
- 公钥：`vault.ssh.<remote>.pubkey` in config_kv（明文）
- 私钥：`vault.ssh.<remote>.privkey` in config_kv（vault 加密存储，`is_sensitive_key()` 自动匹配 `.privkey` 后缀）。SSH transport 调用时按需解密 → 写入临时文件 → 传递路径给 SSH client → 操作完成后删除
- 每个 remote 独立一组 key，支持向不同服务器同时 push

**SSH Key 查找 fallback 链**（SSH transport 调用时按优先级查找）：
1. **local config**：`vault.ssh.<remote>.privkey`（repo 级，vault 加密）。**注意**：如果该配置项存在但解密/读取发生错误，必须**直接报错中断**并向用户暴露异常，严禁静默 fallback 到第 2 步（避免掩盖 Vault 异常）。只有当该条目在数据库中**完全不存在（Not Found）**时，才允许 fallback 到第 2 步
2. **系统默认 SSH 行为**：若无 local config 覆盖，**不传递任何私钥路径参数**（不再显式传递 `-i`），直接调用系统 SSH client，让 OpenSSH 自身去完美处理 `~/.ssh/config`、默认密钥路径和 SSH agent。

> **注：** global scope 不存储 `vault.ssh.<remote>.privkey`——`<remote>` 是仓库级概念，存入 global 会导致 `remote rename` 级联更新破坏其他仓库。后续批次如需全局 SSH key，应基于 Host（域名）映射（如 `vault.ssh.host.github.com.privkey`），而非 remote 别名。

> 这确保绝大多数用户无需在 Libra 内生成 SSH key——系统已有的 `~/.ssh/id_ed25519` 自动生效，与原生 Git 行为一致。`generate-ssh-key` 是可选的增强（per-remote 隔离），不是必须步骤。

**与 init 的关系（详见下方 Init 命令改进详细计划设计原则 #3 及特性 4）：**
- `libra init` **不再**默认生成 SSH Key（系统标准 key 已通过 fallback 自动生效）
- init 完成后检测系统 SSH key 并输出 tip 提示
- 仅在用户显式调用 `libra config generate-ssh-key --remote origin` 时生成 repo 级 key

#### GPG Key 管理

> **注：** `generate-gpg-key` 此批仅支持 local scope（GPG 私钥由仓库内 `.libra/vault.db` PKI 引擎托管，全局没有对应的 vault.db）。`--global` 报错，与 SSH key 一致。

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
- 签名用 GPG key（init 时生成，local scope）：公钥 `vault.gpg.pubkey` in config_kv，私钥由 vault.db PKI 引擎管理（现有机制），`vault.signing=true`
- 额外 GPG keys（config 生成）：`vault.gpg.<usage>.pubkey` in config_kv，私钥由 vault.db 管理
- GPG 签名通过 vault PKI 引擎执行（`vault::pgp_sign()`），私钥不导出

**与 init 的关系（详见下方 Init 命令改进详细计划设计原则 #4 及特性 1）：**
- `libra init` 默认生成一组 GPG Key 用于 commit 签名（`--vault true`，默认），通过实时进度输出展示生成过程
- `--vault false` 跳过 vault 初始化；用户后续可通过 `libra config generate-gpg-key` 手动补充
- 复用 `vault::generate_pgp_key()` 现有逻辑（PGP 2048-bit, 10 年有效期）

**涉及文件：**
- `src/command/config.rs` — 新增 `generate-ssh-key` 和 `generate-gpg-key` 子命令
- `src/internal/vault.rs` — 适配 per-remote SSH key 生成（当前只支持单一 key）；私钥存入 config_kv（vault 加密），SSH transport 调用时按需解密到临时文件
- `src/command/fetch.rs` — SSH transport 改为从 vault 解密私钥 → 临时文件 → 传递路径 → 操作完成删除
- `src/command/vault.rs` — 此批直接删除（功能已由 config 吸收）

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
| Multiple values (set/unset) | 128 | 5 | **5**（Git 对 multiple values 冲突使用 exit 5） |
| Not a repository | 128 | 128 | 128 |
| DB failure | 128 | 128 | 128 |
| Permission denied | 128 | 128 | 128 |

> **注：** 退出码映射变更后，必须同步更新 `docs/error-codes.md` 中的退出码表和稳定错误码参考表。

**多值 key 的 `get` 行为（Git 兼容）：** `libra config get key`（不带 `--all`）对多值 key **不报错**，返回最后一个值（last-one-wins），与 `git config key` 行为一致。exit 5 仅适用于 `set`（无 `--add`）和 `unset`（无 `--all`）尝试操作多值 key 时。

**get 家族对加密值的统一脱敏规则：** `get`、`get --all`、`get --regexp` 对 `encrypted=1` 的条目均默认脱敏（value 显示为 `<REDACTED>`）。`--reveal` 标志在三者上均可用（受同一 namespace 限制：vault 内部凭据拒绝）。JSON 输出中 `encrypted=1` 的条目始终携带 `"redacted": true`（无 `--reveal` 时）或 `"redacted": false`（有 `--reveal` 时）。

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

#### `libra config set --encrypt custom.data "sensitive-value"`
```
Set local (encrypted): custom.data
```

#### `libra config set --global --encrypt custom.data "value"`
```
Initialized vault for global scope
Set global (encrypted): custom.data
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

#### `libra config get vault.env.GEMINI_API_KEY`（默认脱敏）
```
<REDACTED>
```

#### `libra config get --reveal vault.env.GEMINI_API_KEY`（明文输出，支持管道）
```
AIzaSyD...xxxxx
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
  global   user.signingkey = ABCD1234EFGH5678
  global   push.default = current

2 scopes, 6 entries
```

#### `libra config list --global`
```
Config (global, ~/.libra/config.db):
  core.editor = vim
  user.signingkey = ABCD1234EFGH5678
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
  global   vault.env.OPENAI_BASE_URL = <REDACTED>  (encrypted)

3 encrypted entries (1 local, 2 global)

Next steps:
  - add:     libra config set vault.env.<ENV_VAR_NAME>
  - remove:  libra config unset vault.env.<name>

Tip: env vars override config; repo config takes precedence over global
```

#### `libra config set vault.env.GEMINI_API_KEY`（交互式，无 value 参数）
```
Enter value for vault.env.GEMINI_API_KEY: ****
Stored (local, encrypted): vault.env.GEMINI_API_KEY

Tip: the GEMINI_API_KEY environment variable takes precedence over this config value (12-Factor)
Note: local encrypted keys are bound to this machine — they will not be accessible if the repository is moved to another computer
```

#### `echo "$CI_SECRET" | libra config set --stdin vault.env.GEMINI_API_KEY`（CI/CD 管道）
```
Stored (local, encrypted): vault.env.GEMINI_API_KEY
```

#### `libra config unset user.signingkey`
```
Unset local: user.signingkey
```

#### `libra config unset --all remote.origin.fetch`
```
Unset local: remote.origin.fetch (removed 2 values)
```

#### `libra config import`（默认 local）
```
Imported 8 entries from Git local config (.git/config) → libra local config
  skipped: 1 duplicate, 0 invalid keys
  warnings: 0 multi-value keys collapsed

Tip: use libra config list --show-origin to review imported values
Note: conditional Git configs (e.g. [includeIf]) are imported as static values based on their current evaluation
```

#### `libra config import --global`
```
Imported 12 entries from Git global config (~/.gitconfig) → libra global config
  skipped: 2 duplicates, 0 invalid keys
  encrypted: 1 sensitive key auto-encrypted
  warnings: 0 multi-value keys collapsed

Tip: use libra config list --show-origin to review imported values
Note: [include]/[includeIf] entries are excluded from global import (--no-includes)
```

#### `libra config edit`（错误）
```
error: config edit is not supported (SQLite storage does not support text-based editing)

hint: use libra config set/unset/list to manage configuration
hint: use libra config list --name-only to see all keys
```

#### `libra config generate-ssh-key --remote origin`
```
Generated SSH key for remote 'origin':
  Type:       RSA 3072
  Key ID:     libra-john
  Public key: ssh-rsa AAAA...xxxx libra-john

Stored:
  public key:  vault.ssh.origin.pubkey (in config)
  private key: vault.ssh.origin.privkey (vault-encrypted, temp file on use)

Next steps:
  - add to GitHub:  copy the public key above to your GitHub SSH settings
  - push:           libra push origin main
```

#### `libra config generate-ssh-key --remote upstream`
```
Generated SSH key for remote 'upstream':
  Type:       RSA 3072
  Key ID:     libra-john
  Public key: ssh-rsa AAAA...yyyy libra-john

Stored:
  public key:  vault.ssh.upstream.pubkey (in config)
  private key: vault.ssh.upstream.privkey (vault-encrypted, temp file on use)
```

#### `libra config path`
```
/Users/eli/projects/my-repo/.libra/libra.db
```

#### `libra config path --global`
```
/Users/eli/.libra/config.db
```

#### `libra config path --system`（错误）
```
error: --system scope is not supported

hint: use --local or --global
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

**ambiguous set（exit 5）：**
```
error: cannot set 'user.name': 3 values exist for this key

hint: use libra config unset --all user.name first, or libra config set --add
```

**ambiguous unset（exit 5）：**
```
error: cannot unset 'remote.origin.fetch': 2 values exist for this key

hint: use libra config unset --all remote.origin.fetch to remove all values
```

**not a repository（exit 128）：**
```
error: not a libra repository (or any parent up to /)

hint: use --global to read/write user-level config without a repository
hint: use libra init to create a repository here
```

**vault init 失败（exit 128）：**
```
error: failed to initialize vault for global scope: permission denied writing ~/.libra/vault-unseal-key

hint: check file permissions on ~/.libra/
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

#### `libra config set --encrypt custom.data "sensitive-value" --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "set",
  "data": {
    "scope": "local",
    "key": "custom.data",
    "value": "<REDACTED>",
    "encrypted": true,
    "redacted": true
  }
}
```

#### `libra config set --global --encrypt custom.data "value" --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "set",
  "data": {
    "scope": "global",
    "key": "custom.data",
    "value": "<REDACTED>",
    "encrypted": true,
    "redacted": true,
    "vault_initialized": true
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

#### `libra config get vault.env.GEMINI_API_KEY --json`（默认脱敏）
```json
{
  "ok": true,
  "command": "config",
  "action": "get",
  "data": {
    "key": "vault.env.GEMINI_API_KEY",
    "value": "<REDACTED>",
    "origin": "local",
    "redacted": true
  }
}
```

#### `libra config get --reveal vault.env.GEMINI_API_KEY --json`（明文输出）
```json
{
  "ok": true,
  "command": "config",
  "action": "get",
  "data": {
    "key": "vault.env.GEMINI_API_KEY",
    "value": "AIzaSyD...xxxxx",
    "origin": "local",
    "redacted": false
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
    "entries": [
      { "value": "+refs/heads/*:refs/remotes/origin/*", "origin": "local" },
      { "value": "+refs/tags/*:refs/tags/*", "origin": "local" }
    ]
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
      { "key": "user.signingkey", "value": "ABCD1234EFGH5678", "origin": "global", "encrypted": false, "sensitive": false },
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
      { "key": "vault.env.GEMINI_API_KEY", "value": "<REDACTED>", "origin": "local", "encrypted": true, "sensitive": true },
      { "key": "vault.env.OPENAI_API_KEY", "value": "<REDACTED>", "origin": "global", "encrypted": true, "sensitive": true },
      { "key": "vault.env.OPENAI_BASE_URL", "value": "<REDACTED>", "origin": "global", "encrypted": true, "sensitive": true }
    ],
    "encrypted_count": 3
  }
}
```

#### `libra config set vault.env.GEMINI_API_KEY --json`（错误——`--json` 视为非交互，value 缺省时报错）
```json
{
  "ok": false,
  "error_code": "LBR-CLI-002",
  "category": "cli",
  "exit_code": 2,
  "message": "missing value for protected key 'vault.env.GEMINI_API_KEY' (--json disables interactive input)",
  "hints": ["provide value as argument: libra config set vault.env.GEMINI_API_KEY <value> --json", "or use --stdin: echo $SECRET | libra config set --stdin vault.env.GEMINI_API_KEY --json"]
}
```

#### `echo "$CI_SECRET" | libra config set --stdin vault.env.GEMINI_API_KEY --json`（CI/CD 管道）
```json
{
  "ok": true,
  "command": "config",
  "action": "set",
  "data": {
    "scope": "local",
    "key": "vault.env.GEMINI_API_KEY",
    "value": "<REDACTED>",
    "encrypted": true,
    "source": "stdin"
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

#### `libra config import --global --json`
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
    "collapsed_multivalue_warnings": 0,
    "auto_encrypted": 1,
    "ignored_invalid": 0
  }
}
```

#### `libra config edit --json`（错误）
```json
{
  "ok": false,
  "error_code": "LBR-CLI-002",
  "category": "cli",
  "exit_code": 2,
  "message": "config edit is not supported (SQLite storage does not support text-based editing)",
  "hints": ["use libra config set/unset/list to manage configuration"]
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
    "privkey_config_key": "vault.ssh.origin.privkey",
    "storage": "vault-encrypted"
  }
}
```

#### `libra config path --json`
```json
{
  "ok": true,
  "command": "config",
  "action": "path",
  "data": {
    "scope": "local",
    "path": "/Users/eli/projects/my-repo/.libra/libra.db",
    "exists": true
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
    libra config import --global                       Import from Git global config
    libra config set vault.env.GEMINI_API_KEY          Store API key (interactive)
    echo "$SECRET" | libra config set --stdin vault.env.KEY  Set secret from stdin (CI/CD)
    libra config set --encrypt custom.key "value"      Force-encrypt a local value
    libra config list --vault                          List vault entries
    libra config generate-ssh-key --remote origin      Generate SSH key for remote
    libra config generate-gpg-key                      Generate GPG signing key
    libra config list --name-only                      List all key names
```

### 文档输出

创建 `docs/commands/config/README.md`，Markdown 格式的完整 config 文档，包含：
- 概述与设计理念
- 子命令参考（set/get/list/unset/import/generate-ssh-key/generate-gpg-key/path）
- Scope 与优先级说明
- Vault 加密存储说明
- 环境变量解析优先级说明
- SSH/GPG Key 管理说明
- JSON 输出 schema
- 完整 Libra vs Git vs jj 功能对比表（见下方）

创建 `CHANGELOG.md`（项目根目录），记录此批次的 breaking changes：

```markdown
# Changelog

## [Unreleased]

### Breaking Changes

- **`libra vault` 子命令已删除**：vault 功能已整合到 `libra config`。迁移指南：
  | 旧命令 | 新命令 |
  |--------|--------|
  | `libra vault generate-ssh-key` | `libra config generate-ssh-key --remote <remote-name>` |
  | `libra vault generate-gpg-key` | `libra config generate-gpg-key` |
  | `libra vault gpg-public-key` | `libra config get vault.gpg.pubkey` |
  | `libra vault ssh-public-key` | `libra config get vault.ssh.<remote-name>.pubkey` |

  说明：`<remote-name>` 需替换为仓库中实际的 remote 名称；若仓库只有一个默认 remote，通常为 `origin`

- **`--system` scope 已移除**：system 级配置因多用户环境下的权限隔离安全问题被移除。原有 `--system` 配置请迁移到 `--global`：
  | 旧用法 | 新用法 |
  |--------|--------|
  | `libra config set --system key value` | `libra config set --global key value` |
  | `libra config --get --system key` | `libra config get --global key` |
  | `libra config --list --system` | `libra config list --global` |
  | `libra config import --system` | 不再支持 |
  | `libra config path --system` | 不再支持 |

- **`libra config edit` 不再支持**：Libra 使用 SQLite 存储配置，多值 key 的文本 diff 回写无法保证数据一致性。请使用 `libra config set`/`unset`/`list` 组合操作。

- **移除 `--separate-git-dir` / `--separate-libra-dir` 功能**：系统全局取消了将存储目录与工作树分离的特性。所有组件（包括 config 定位逻辑）不再需要支持 `.libra` 作为 `gitdir:` 文本文件的情况。所有 non-bare 仓库均统一在工作树根目录的 `.libra/` 目录下读取数据。

- **Config 存储后端迁移**：配置存储从三列拆分表（`config`）迁移到扁平 key/value 表（`config_kv`），支持 vault 加密。旧 `Config` API 已标记 `#[deprecated]`。
```

#### Libra vs Git vs jj 功能对比（文档末尾附录）

| Feature | Git | jj | Libra |
|---------|-----|-----|-------|
| Implicit set | `git config key val` | No (requires `set`) | `libra config set key val` + 兼容 `libra config key val` |
| Subcommand style | No | Yes (`set/get/list/edit/path`) | Yes (`set/get/list/unset/import/path`) |
| Get value | `git config key` | `jj config get key` | `libra config get key` |
| List | `git config -l` | `jj config list` | `libra config list` |
| Edit in editor | `git config -e` | `jj config edit` | Not supported (SQLite storage) |
| Regex search | `git config --get-regexp` | No | `libra config get --regexp` |
| Show origin | `git config --show-origin` | No | `libra config list --show-origin` |
| Type coercion | `--type=bool\|int\|path` | No (TOML types) | Not supported (this batch) |
| Default fallback | `--default value` | No | `--default value` |
| Null-delimited | `-z` | No | Not supported (this batch) |
| Rename/remove section | Yes | No | Not supported (this batch) |
| JSON output | No | No | **`--json`** ✓ |
| Secret redaction | No | No | **Auto-detect** ✓ |
| Import from Git | N/A | N/A | **`libra config import`** ✓ |
| Vault encryption | No | No | **AES-256-GCM (all scopes)** ✓ |
| Env var vault | No | No | **`vault.env.*`** ✓ |
| SSH key per remote | No | No | **`generate-ssh-key --remote`** ✓ |
| GPG key generation | No | No | **`generate-gpg-key`** ✓ |
| Env var resolution | No fallback | No fallback | **CLI → env → repo → global** ✓ |
| Config file path | N/A | `jj config path` | **`libra config path`** ✓ |
| Conditional config | `includeIf` | `[[when]]` blocks | Not supported |
| Worktree scope | `--worktree` | `--workspace` | Not supported |
| Arbitrary file | `--file <path>` | No | Not supported |
| Storage format | INI text files | TOML text files | **SQLite + vault** |
| Scopes | system/global/local/worktree | user/repo/workspace | **global/local** (system removed) |


### Vault 命令删除计划

**⚠️ Breaking change**：`libra vault` 子命令在此批直接删除，不提供 deprecation 过渡期。

**迁移说明**（需在 CHANGELOG / release notes 中告知用户）：

| 旧命令 | 新命令 |
|--------|--------|
| `libra vault generate-ssh-key` | `libra config generate-ssh-key --remote <remote-name>` |
| `libra vault generate-gpg-key` | `libra config generate-gpg-key` |
| `libra vault gpg-public-key` | `libra config get vault.gpg.pubkey` |
| `libra vault ssh-public-key` | `libra config get vault.ssh.<remote-name>.pubkey` |

说明：`<remote-name>` 需替换为仓库中实际的 remote 名称；若仓库只有一个默认 remote，通常为 `origin`

**实施清单：**
- [ ] 删除 `src/command/vault.rs`
- [ ] 从 `src/cli.rs` 中移除 `vault` 子命令注册
- [ ] 删除 `tests/command/vault_test.rs` 和 `tests/command/vault_cli_test.rs`
- [ ] `src/internal/vault.rs` **保留**，作为加密基础设施继续被 config 使用
- [ ] 将 vault 测试用例中的功能覆盖迁移到 config 测试中：
  - `vault generate-ssh-key` → `config generate-ssh-key --remote origin` 测试
  - `vault generate-gpg-key` → `config generate-gpg-key` 测试
  - `vault gpg-public-key` → `config get vault.gpg.pubkey` 测试
  - `vault ssh-public-key` → `config get vault.ssh.origin.pubkey` 测试
  - vault init / unseal / encrypt / decrypt → config set 敏感 key 的加密存储测试

---

## 验证方式

> 每次提交前必须先通过 [README.md 统一质量验收](README.md#每次改进质量验收)（fmt / clippy / test / 测试覆盖规则），再执行以下 config 专项验收。

### 架构验收
- 运行时主路径不再依赖旧 `Config` 公共 API；旧 `config` 表不在本批处理范围内

### 静态验收
- `src/internal/config.rs` 中旧 `Config` 的全部公共 API（`get`/`get_all`/`insert`/`update`/`remove`/`remove_config`/`list_all`/`remote_config`/`all_remote_configs`/`get_remote`/`get_remote_url`/`branch_config` 等）标记 `#[deprecated]`
- `src/internal/model/config.rs` 中旧 SeaORM entity（`Model`/`Entity`/`Column`/`ActiveModel`）标记 `#[deprecated]`，防止通过旧 entity 或原始 SQL 绕过新 API 访问旧 `config` 表
- `cargo clippy -- -D warnings` 会将所有旧 API 和旧 entity 引用（包括别名如 `use Config as UserConfig`）报为编译错误
- **原始 SQL 检查**：`rg -i '(FROM|INTO|UPDATE|DELETE\s+FROM)\s+["\x60]?config["\x60]?\b' src/ --type rust` 不得在运行时代码中出现（仅允许在 deprecated 定义文件和 schema 迁移脚本中）
- 验收标准：`cargo +nightly fmt --all --check` 无格式差异 + `cargo clippy --all-targets --all-features -- -D warnings` 通过 + 原始 SQL 检查通过，即证明无活跃旧 API/entity/SQL 调用

### 行为验收
- `libra config set` 写入的新数据，所有迁移范围内的命令在同一次运行中都能立即读到

### 安全验收
- 敏感项默认加密存储（`encrypted=1`）并脱敏显示；`--plaintext` 逃生舱写入的条目（`encrypted=0`）不脱敏但显示 `[PLAINTEXT]` 警告
- `config edit` 不支持（SQLite 存储，使用时报错 `LBR-CLI-002`）
- `config set`（value 缺省触发交互式输入）在非 TTY 环境下直接报错；指定 `--json`/`--machine` 时也视为非交互并直接报错（`LBR-CLI-002`）
- `generate-ssh-key --remote <name>` 的 remote 名通过 `[a-zA-Z0-9_-]` 校验且必须是已配置的 remote

### 兼容验收
- `libra vault` 子命令已删除；`libra vault` 应输出 `error: unknown subcommand 'vault'`（clap 默认行为）
- vault 测试覆盖的功能已迁移到 config 测试中，`cargo test` 全部通过
