# `libra clone`

将仓库克隆到新目录。

## 概要

```
libra clone [OPTIONS] <REMOTE_REPO> [LOCAL_PATH]
```

## 说明

`libra clone` 通过获取对象、配置 `origin` 并检出工作树，创建远程仓库的本地副本。它会初始化一个由 vault 支撑的仓库，并透明复用 `run_init()` 完成本地元数据设置。

克隆会从远程获取所有对象和 refs，创建带 SQLite 元数据存储的 `.libra` 目录，设置 `origin` 远程，并检出默认分支（或用 `-b` 指定的分支）。克隆期间始终会引导 vault 签名，与 `libra init` 的默认值一致。对于非裸克隆，检出的 `.gitignore` 文件会复制为对应的 `.libraignore` 文件，使 Libra 忽略规则立即生效。

对于裸克隆，不会执行工作树检出，仓库目录本身会直接成为对象存储。裸克隆不会创建 `.libraignore`。

## 选项

### `<REMOTE_REPO>`（必需）

要克隆的远程仓库 URL。支持 SSH（`git@host:user/repo.git`）和 HTTPS（`https://host/user/repo.git`）协议，也支持本地文件系统路径。`libra+cloud://` 发布源会被识别并严格校验。恢复开始前，克隆域名必须已在本地配置；否则 Libra 返回 `LBR-AUTH-001`，并且不会创建目标目录。已配置的云源会在创建目标目录前解析 D1 site、repository 行、已发布 refs、选中/默认 revision、对象索引和 R2 对象可用性。随后恢复会初始化本地 Libra 仓库，从 R2 下载已索引的 Git 对象，恢复 refs 元数据，写入 origin 云配置，并检出选中/默认 revision。云源绝不会回退到通用 Git discovery。

```bash
libra clone git@github.com:user/repo.git
libra clone https://github.com/user/repo.git
libra clone /path/to/local/repo
libra clone libra+cloud://code.example.com/kepler-ledger
libra clone libra+cloud://code.example.com/repo/rp_8f4c1b
libra clone "libra+cloud://code.example.com/kepler-ledger?ref=refs/tags/v1.0.0"
libra clone "libra+cloud://code.example.com/kepler-ledger?revision=latest"
```

对于 `libra+cloud://`，authority 是已配置的克隆域名。路径必须是 `/<slug>` 或 `/repo/<repo_id>`。只允许一个选择器：`?ref=<branch|tag|full-ref>` 或 `?revision=<oid|latest>`。
首个 Cloudflare 恢复表面不接受 Git 传输整形标志：`--branch`、`--depth`、`--single-branch` 和 `--bare` 会在查找 clone-domain 配置之前、创建目标目录之前返回 `LBR-CLI-002`。请在源 URL 上使用 `?ref=<branch|tag|full-ref>` 选择检出目标。

必需的 clone-domain 配置键：

```text
cloud.clone_domains.<domain>.account_id
cloud.clone_domains.<domain>.d1_database_id
cloud.clone_domains.<domain>.r2_bucket
```

云站点解析还要求 `LIBRA_D1_API_TOKEN`；Libra 先读取 `vault.env.LIBRA_D1_API_TOKEN`，再读取导出的环境变量，因此 CLI 可以在开始恢复前查询配置的 D1 数据库。

### `[LOCAL_PATH]`

可选目标目录。省略时，Libra 会从仓库 URL 推断目录名（例如从 `repo.git` 推断 `repo`）。如果无法推断，会返回错误，要求用户显式指定路径。

```bash
libra clone git@github.com:user/repo.git my-dir
```

### `-b, --branch <NAME>`

检出 `<NAME>`，而不是远程 HEAD。该分支必须存在于远程；否则会报 “remote branch not found” 错误。
对于 `libra+cloud://` 源，请改为在 URL 中使用 `?ref=<branch|tag|full-ref>`；`--branch` 会在恢复开始前被拒绝。

```bash
libra clone -b develop git@github.com:user/repo.git
```

### `--single-branch`

只获取通向单个分支 tip 的历史（HEAD，或 `-b` 给出的分支）。当大型仓库只需要一个分支时，可减少传输量。只有 Git 远程支持这种传输优化；`libra+cloud://` 恢复会拒绝它，因为恢复出的本地仓库必须保留所有已发布 refs。

```bash
libra clone --single-branch -b main git@github.com:user/repo.git
```

### `--bare`

创建没有工作树的裸仓库。目标目录会直接成为对象存储。适用于中心/服务端仓库。
裸 Cloudflare 恢复不属于首个恢复表面；`libra+cloud://` 当前会显式拒绝 `--bare`。

```bash
libra clone --bare git@github.com:user/repo.git
```

### `--depth <N>`

创建浅克隆，将历史截断到指定提交数。`N` 必须是正整数。
只有 Git 远程支持浅传输。Cloudflare 恢复会拒绝 `--depth`，因为它必须下载完整的已发布对象集合。

```bash
libra clone --depth 1 git@github.com:user/repo.git
libra clone --depth 50 git@github.com:user/repo.git
```

## 常用命令

```bash
libra clone git@github.com:user/repo.git
libra clone https://github.com/user/repo.git
libra clone git@github.com:user/repo.git my-dir
libra clone --bare git@github.com:user/repo.git
libra clone -b develop git@github.com:user/repo.git
libra clone --single-branch -b main git@github.com:user/repo.git
libra clone --depth 1 git@github.com:user/repo.git
```

## 人工输出

默认人工模式将分阶段进度写入 `stderr`，最终摘要写入 `stdout`。

阶段：

- `Connecting to <url> ...`
- `Initializing repository ...`
- `Fetching objects ...`
- `Configuring repository ...`
- `Checking out working copy ...`（仅非裸仓库）

成功输出：

```text
Cloned into 'repo'
  remote: origin -> git@github.com:user/repo.git
  branch: main
  signing: enabled

Tip: using existing SSH key at ~/.ssh/id_ed25519
```

裸克隆：

```text
Cloned into bare repository '/path/to/repo.git'
  remote: origin -> git@github.com:user/repo.git
  branch: main
  signing: enabled
```

空远程：

```text
Cloned into 'empty'
  remote: origin -> git@github.com:user/empty.git
  signing: enabled

warning: You appear to have cloned an empty repository.
```

`--quiet` 会抑制所有进度和最终成功摘要，包括警告。

## 结构化输出

`libra clone` 支持全局 `--json` 和 `--machine` 标志。

- `--json` 向 `stdout` 写入一个成功信封
- `--machine` 以紧凑单行 JSON 写入相同 schema
- 两者都会抑制进度输出和嵌套的 init/fetch 输出
- 成功时 `stderr` 保持干净

示例：

```json
{
  "ok": true,
  "command": "clone",
  "data": {
    "path": "/Users/eli/projects/my-repo",
    "bare": false,
    "remote_url": "git@github.com:user/repo.git",
    "branch": "main",
    "object_format": "sha1",
    "repo_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "vault_signing": true,
    "ssh_key_detected": "/Users/eli/.ssh/id_ed25519",
    "shallow": false,
    "warnings": []
  }
}
```

空远程返回 `"branch": null` 和一个警告：

```json
{
  "ok": true,
  "command": "clone",
  "data": {
    "path": "/Users/eli/projects/empty-repo",
    "bare": false,
    "remote_url": "git@github.com:user/empty-repo.git",
    "branch": null,
    "object_format": "sha1",
    "repo_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "vault_signing": true,
    "ssh_key_detected": null,
    "shallow": false,
    "warnings": [
      "You appear to have cloned an empty repository."
    ]
  }
}
```

### Schema 说明

- `branch` 是实际检出的分支；远程没有 refs 时为 `null`
- 使用 `--depth` 时，`shallow` 为 `true`
- 普通 Git/本地克隆会省略 `source_kind` 和 `cloud_site`；`libra+cloud://` 克隆会加入它们，包含 clone domain、site id、slug、repo id、选中 ref 和恢复的 revision
- init 中的 `ref_format` 和 `converted_from` 被有意排除
- `objects_fetched` / `bytes_received` 在 fetch 改进落地前不暴露

## 设计动机

### 没有 `--recurse-submodules`

Git 的 submodule 系统（`--recurse-submodules`）经常给开发者带来摩擦：submodule 需要独立的 fetch/checkout 循环，会创建嵌套 `.git` 目录，并破坏许多假定单一工作树的工具。Libra 不实现 submodule。对于 monorepo 工作流，所有代码都位于单个仓库中。对于多仓库组合，Libra 鼓励使用显式依赖管理（包管理器、vendoring），而不是把仓库嵌进仓库。这让 clone 操作保持简单且可预测。

### 克隆期间引导 vault

Libra 在 clone 期间复用与 `libra init` 相同的 `run_init()` 路径来初始化由 vault 支撑的签名。这意味着每个克隆出的仓库无需额外设置即可立即生成签名提交。Git 要求用户在克隆后手动配置 GPG/SSH 签名，这意味着大多数克隆仓库默认会产生未签名提交。通过在克隆时引导 vault，Libra 确保克隆仓库的安全姿态与新初始化仓库一致。

### 忽略文件转换

Libra 使用 `.libraignore` 作为忽略策略。非裸克隆期间，每个检出的 `.gitignore` 都会复制到同级 `.libraignore`。已有的用户自有 `.libraignore` 文件会被保留并作为警告展示；原始 `.gitignore` 文件保持不变。

### 用 `--depth` 进行浅克隆

浅克隆对于 CI/CD 流水线和不需要完整历史的大型 monorepo 很重要。Libra 支持与 Git 语义相同的 `--depth N`：历史会截断到指定提交数。depth 值在解析时校验（必须是正整数），并传递到 fetch 协议层。与 Git 不同，Libra 尚不支持基于日期或 ref 边界的 `--shallow-since` 或 `--shallow-exclude`，以保持初始实现聚焦且可预测。

### `--sparse` 被有意不支持

稀疏检出（`git clone --sparse`、`git sparse-checkout`）被有意不实现。Sparse cone/skip-worktree 依赖 Git 管理的工作树配置，而 Libra 已将 config / HEAD / refs 迁移到 SQLite。桥接并非零成本；基于审计的决策是推迟 `--sparse`，直到出现无法通过分层云存储满足的具体 monorepo 子树检出需求。重启条件见 [`docs/development/commands/_compatibility.md`](../../development/commands/_compatibility.md) 条目 **D10**。

### `--recurse-submodules` 被有意不支持

按照更广泛的产品边界（没有 submodule 子命令表面），`clone --recurse-submodules` 也不受支持。重启条件见 [`docs/development/commands/_compatibility.md`](../../development/commands/_compatibility.md) 条目 **D1**（submodule）和 **D4**（clone --recurse-submodules）。

### `--single-branch` 标志

与 `--branch` 组合时，`--single-branch` 通过只获取指定分支的历史来减少 clone 期间传输的数据量。这对包含许多长期分支的大型仓库尤其有用，例如 CI 构建某个特定 release 分支时只需要一个分支。Git 也支持此能力；jj 不支持，因为它的 operation-log 模型按设计获取所有 refs。

## 参数对比：Libra vs Git vs jj

| 参数 / 标志 | Git | jj | Libra |
|---|---|---|---|
| 远程 URL（位置参数） | `git clone <url>` | `jj git clone <url>` | `libra clone <url>` |
| 目标目录 | `git clone <url> <dir>` | `jj git clone <url> <dir>` | `libra clone <url> <dir>` |
| 指定分支 | `-b` / `--branch` | `-b` / `--branch`（jj 0.17+） | `-b` / `--branch` |
| 单分支 | `--single-branch` | N/A | `--single-branch` |
| 裸克隆 | `--bare` | N/A | `--bare` |
| 浅克隆（depth） | `--depth <n>` | N/A | `--depth <n>` |
| 按日期浅克隆 | `--shallow-since=<date>` | N/A | N/A |
| 排除浅边界 | `--shallow-exclude=<rev>` | N/A | N/A |
| 镜像克隆 | `--mirror` | N/A | N/A |
| 引用仓库 | `--reference <repo>` | N/A | N/A |
| 从引用仓库脱离 | `--dissociate` | N/A | N/A |
| 禁用硬链接 | `--no-hardlinks` | N/A | N/A |
| 递归 submodule | `--recurse-submodules` | N/A | N/A（无 submodule） |
| 浅 submodule | `--shallow-submodules` | N/A | N/A |
| 独立 git dir | `--separate-git-dir=<dir>` | N/A | N/A（已移除） |
| 模板目录 | `--template=<dir>` | N/A | N/A（由 init 内部处理） |
| Quiet 模式 | `-q` / `--quiet` | `--quiet` | `--quiet`（全局标志） |
| Verbose / 进度 | `--progress` / `--verbose` | N/A | 分阶段 stderr 进度（默认） |
| 不检出 | `-n` / `--no-checkout` | N/A | N/A（bare 隐含不检出） |
| 稀疏检出 | `--sparse` | N/A | N/A |
| Filter（部分克隆） | `--filter=<spec>` | N/A | N/A |
| Bundle URI | `--bundle-uri=<uri>` | N/A | N/A |
| Vault 签名引导 | N/A | N/A | 始终启用（匹配 init） |
| SSH key 检测 | N/A | N/A | 自动检测 + 提示 |
| 结构化 JSON 输出 | N/A | N/A | `--json` / `--machine` |
| 错误提示 | 最少消息 | 最少消息 | 每种错误都有可操作提示 |

## 错误处理

每个 `CloneError` 变体都映射到显式 `StableErrorCode`，不依赖消息子串推断。

| 场景 | 错误码 | 退出 | 提示 |
|------|--------|------|------|
| 无法推断目标路径 | `LBR-CLI-002` | 129 | "please specify the destination path explicitly" |
| 目标已存在且非空 | `LBR-CLI-003` | 129 | "choose a different path or empty the directory first" |
| 目标已包含仓库 | `LBR-REPO-003` | 128 | "the destination already contains a libra repository" |
| 无法创建目标目录 | `LBR-IO-002` | 128 | "check directory permissions and disk space" |
| 本地路径不存在 | `LBR-REPO-001` | 128 | "use a valid libra repository path or a reachable remote URL" |
| URL 格式错误或 scheme 不支持 | `LBR-CLI-003` | 129 | "check the clone URL or scheme" |
| 认证 / 权限拒绝 | `LBR-AUTH-002` | 128 | "check SSH key / HTTP credentials and repository access rights" |
| 网络不可达 | `LBR-NET-001` | 128 | "check the remote host, DNS, VPN/proxy, and network connectivity" |
| 协议 / discovery 错误 | `LBR-NET-002` | 128 | "the remote did not complete discovery successfully" |
| 找不到远程分支 | `LBR-REPO-003` | 128 | "use `-b <branch>` to specify an existing branch" |
| 对象格式不匹配 | `LBR-REPO-003` | 128 | "the remote and local repository use different object formats" |
| 检出解析失败 | `LBR-REPO-003` | 128 | "working tree checkout target could not be resolved" |
| 检出读取失败 | `LBR-IO-001` | 128 | "failed to read repository state while checking out" |
| 检出写入失败 | `LBR-IO-002` | 128 | "files could not be written" |
| 检出 LFS 下载失败 | `LBR-NET-001` | 128 | "LFS content transfer failed" |
| 内部不变量 | `LBR-INTERNAL-001` | 128 | Issues URL |

Init 错误会通过 `InitError -> CliError` 透明转发。

### 清理失败可见性

当 clone 失败时，`cleanup_failed_clone()` 会尝试删除部分创建的目录。如果清理本身也失败，该警告会通过 `with_priority_hint()` 附加到错误上，使其同时出现在人工和 JSON 错误输出中，而不是被静默吞掉。

### 非裸检出是成功条件

`setup_repository()` 使用 `execute_checked_typed()`，它返回类型化的 `RestoreError` 变体。如果检出失败，clone 会报告失败，不会静默成功并留下损坏的工作树。

## Vault 与身份

- Clone 始终使用 `vault: true` 初始化，与 `libra init` 默认值一致
- init 的 `vault_signing` 和 `ssh_key_detected` 会透明转发到 `CloneOutput`
- SSH key 检测使用 init 阶段隔离出的 `HOME`

## 兼容性说明

- 不支持 `--recurse-submodules`；Libra 不实现 submodule
- 不支持 `--mirror` 和 `--reference`
- Clone 始终引导 vault 签名；如有需要，可在克隆后使用 `libra config` 禁用
- `--depth` 值必须是正整数；0 或负数会在解析时被拒绝
- 没有单独的 `--no-checkout` 标志；对没有工作树的仓库使用 `--bare`
