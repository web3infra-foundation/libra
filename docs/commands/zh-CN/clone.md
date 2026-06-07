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

## 能力矩阵与决策账本

Libra 以其事务型 SQLite 元数据（`.libra/libra.db`）、vault 秘密与分层对象存储为权威实现，同时在协议层对标常见 `git clone` 标志的行为。**clone 核心不需要任何 SQL schema 迁移**（元数据在 SQLite；对象在分层存储；部分克隆的 promisor 状态记录在 `config_kv`）。权威兼容级别见 [`COMPATIBILITY.md`](../../../COMPATIBILITY.md)；下表汇总决策：

| 能力 | 标志 | Libra 级别 | 说明 |
|---|---|---|---|
| 按数量浅克隆 | `--depth` | supported | 复用 `.libra/shallow` 边界 + deepen 协商 |
| 按日期浅克隆 | `--shallow-since` | supported | `deepen-since`；组合时取代普通 depth |
| 按 ref 浅克隆 | `--shallow-exclude` | supported | `deepen-not`；组合时取代普通 depth |
| 拒绝浅源 | `--reject-shallow` | supported | 浅源时失败（128） |
| 全部/单分支 | `--single-branch` / `--no-single-branch` | supported | Git 风格反义，后者生效 |
| 自定义远程名 | `-o/--origin` | supported | 命名被跟踪的远程 |
| 跳过检出 | `-n/--no-checkout` | supported | 仅元数据，无工作树 |
| 镜像 | `--mirror` | partial | 隐含 bare；写入 `+refs/*:refs/*` + `mirror = true`；克隆分支头（尚未实现 tags / 精确 `refs/*` 镜像） |
| 参考复用 | `--reference` / `--reference-if-able` | intentionally-different | 复制语义（不借用 `info/alternates`） |
| Dissociate | `--dissociate` | intentionally-different | 确认完全本地（复制语义） |
| 本地优化 | `-l/--local` / `--no-hardlinks` | supported | 硬链接（或复制）本地源对象 |
| 共享对象 | `-s/--shared` | intentionally-different | 复制语义，无 alternates |
| 并行任务 | `-j/--jobs` | intentionally-different | 校验 1..=16，预留/no-op（串行传输） |
| 部分克隆 | `--filter` | partial | 白名单 spec；promisor 配置；无按需补取 |
| 稀疏检出 | `--sparse` | declined | 见 [declined.md#d10](../../improvement/compatibility/declined.md#d10-clone---sparse-与顶层-sparse-checkout-命令) |
| 子模块 | `--recurse-submodules` | declined | 见 [declined.md#d4](../../improvement/compatibility/declined.md#d4-clone---recurse-submodules) |

**云克隆**（`libra+cloud://`）从 Cloudflare D1/R2 恢复完整的已发布对象集，并在任何 clone-domain 配置查找或目录创建之前，对上表所有 Git 整形标志**快速失败（退出码 129）**（请用 URL 中的 `?ref=<branch|tag|full-ref>` 选择检出目标）。新增的 `StableErrorCode` 变体（如有）记录在 [`docs/error-codes.md`](../../error-codes.md)。

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
首个 Cloudflare 恢复表面不接受 Git 传输整形标志：`--branch`、`--depth`、`--single-branch`、`--bare`、`--shallow-since`、`--shallow-exclude`、`--reject-shallow`、`--origin`、`--no-checkout`、`--mirror`、`--reference`、`--reference-if-able`、`--dissociate`、`--local`、`--shared`、`--no-hardlinks`、`--jobs` 和 `--filter` 会在查找 clone-domain 配置之前、创建目标目录之前返回 `LBR-CLI-002` 或 `LBR-CLI-003`。请在源 URL 上使用 `?ref=<branch|tag|full-ref>` 选择检出目标。

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

### `--no-single-branch`

`--single-branch` 的反义形式；克隆所有分支。这是 Git 风格的反义：当 `--single-branch` 与 `--no-single-branch` 同时出现在命令行时，**后出现者生效**（由 clap 的 `overrides_with` 原生处理）。二者同传**不是**用法冲突，不会报错。

```bash
libra clone --no-single-branch git@github.com:user/repo.git
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

### `--shallow-since <time>`

创建浅克隆，仅保留晚于 `<time>` 的提交历史。接受日期（`2024-01-01`）、RFC3339 时间戳、Unix 纪元秒，或 `2 weeks ago` 这样的相对形式。格式非法时在任何网络访问之前以 `LBR-CLI-002`（退出码 129）拒绝。可与 `--depth` 组合；由于 `git-upload-pack` 拒绝同时发送普通 `deepen` 与 `deepen-since`/`deepen-not`，在协议层基于时间/ref 的请求会取代普通 depth。只有 Git 远程支持浅传输；`libra+cloud://` 恢复会拒绝它。

```bash
libra clone --shallow-since 2024-01-01 git@github.com:user/repo.git
```

### `--shallow-exclude <revision>`

创建浅克隆，排除从给定 ref 或 revision 可达的历史（`deepen-not`）。可**重复**以排除多个 ref（每个值一个 `deepen-not` 帧），并可与 `--depth` 组合（与 `--shallow-since` 一样，exclude 请求会取代普通 depth）。只有 Git 远程支持；`libra+cloud://` 恢复会拒绝它。

```bash
libra clone --shallow-exclude refs/tags/v1.0.0 git@github.com:user/repo.git
```

### `--reject-shallow`

当源仓库本身是浅仓库时直接失败（退出码 128），而不是对浅源再做一次浅克隆。本地源在创建任何目录之前即被检测；远程源则根据 fetch 期间通告的 shallow 边界检测。

```bash
libra clone --reject-shallow git@github.com:user/repo.git
```

### `-o, --origin <name>`

用 `<name>` 代替 `origin` 作为被跟踪的远程名。远程 URL、fetch refspec 与 `branch.<branch>.remote` 配置都会记录在该名称下。`libra+cloud://` 恢复会拒绝它。

```bash
libra clone -o upstream git@github.com:user/repo.git
```

### `-n, --no-checkout`

克隆后不检出 HEAD。元数据、refs 与 config 仍会写入；仅跳过工作树检出（以及依赖它的 `.gitignore` → `.libraignore` 转换）。`libra+cloud://` 恢复会拒绝它。

```bash
libra clone --no-checkout git@github.com:user/repo.git
```

### `--mirror`

建立源仓库的镜像。隐含 `--bare`，记录镜像 refspec（`+refs/*:refs/*`）与 `remote.<name>.mirror = true`，并克隆所有分支头。**已知限制（partial）：** 分支 ref 以 remote-tracking（`refs/remotes/<name>/*`）形式存储，tags 及其它 ref 命名空间尚未按精确 `refs/*` 名称镜像，因此还不是完整的 Git 风格镜像。`libra+cloud://` 恢复会拒绝它。

```bash
libra clone --mirror git@github.com:user/repo.git
```

### `--reference <repo>` / `--reference-if-able <repo>`

复用**本地**参考仓库的对象以减少工作量。**与 Git 有意不同**：由于 Libra 的对象读取没有 `info/alternates` 回退，这两个标志采用**复制语义**——把参考仓库的对象复制到新克隆的分层存储，克隆不携带任何长期 alternates 依赖（不写 `info/alternates`）。源必须是真实（非符号链接）的本地 libra 或 git 仓库；符号链接源会以退出码 128 拒绝，路径长度上限为 4 KiB。`--reference-if-able` 在路径不存在时降级为普通克隆并给出警告，而 `--reference` 会失败。`libra+cloud://` 会拒绝二者。

```bash
libra clone --reference /srv/mirror/repo git@github.com:user/repo.git
libra clone --reference-if-able /srv/mirror/repo git@github.com:user/repo.git
```

### `--dissociate`

确保克隆对参考没有借用依赖。在默认复制语义下对象本就完全本地，因此该标志只是确认这一状态（JSON 中报告 `dissociated = true`）——绝不会留下悬空的 alternates 引用。需要 `--reference` 或 `--reference-if-able`；单独使用是用法错误（退出码 129）。

```bash
libra clone --dissociate --reference /srv/mirror/repo git@github.com:user/repo.git
```

### `-l, --local` / `--no-hardlinks`

从**本地**仓库克隆时，直接复用其对象而非重新传输：`--local` 把源的松散对象与 pack 文件硬链接进新克隆（共享 inode），跨文件系统或指定 `--no-hardlinks` 时回退为复制。符号链接对象源会被拒绝（退出码 128）。若源不是本地仓库，该标志会被忽略并给出警告。

```bash
libra clone -l /srv/repos/project.git my-project
libra clone -l --no-hardlinks /srv/repos/project.git my-project
```

### `-s, --shared`

通过**复制语义**复用本地源仓库的对象（不写 `info/alternates`，与 Libra 中 `--reference` 一致）。由于 Libra 的对象读取没有 alternates 回退，这与 Git 的 alternates 共享有意不同。

```bash
libra clone -s /srv/repos/project.git my-project
```

### `-j, --jobs <n>`

**Libra 扩展（RESERVED 预留）。** 校验到 1..=16 范围（0 或 >16 退出 129）并保留，但当前是 no-op——Libra 的传输是串行的，没有下游消费者。Git 的 `clone --jobs` 控制 submodule 并行获取，Libra 不支持 submodule，故该名称为未来的传输并发上限预留。

```bash
libra clone --jobs 4 git@github.com:user/repo.git
```

### `--filter <spec>`

部分克隆：请求远程省略匹配 `<spec>` 的对象以减少传输。支持的 spec（白名单；未知 spec 退出 129，超长 spec 受 4 KiB 上限限制）：`blob:none`、`blob:limit=<n>[kmg]`、`tree:<depth>`。克隆会记录 promisor 配置（`remote.<name>.promisor = true`、`remote.<name>.partialclonefilter = <spec>`），但**不**做按需补取缺失对象。由于非裸默认检出需要 blob 内容，请将 `--filter` 与 `--no-checkout` 或 `--bare` 搭配；否则在命中被过滤的 blob 时检出会以清晰的部分克隆诊断失败（退出码 128）。需要服务端允许过滤（`uploadpack.allowFilter`）。`libra+cloud://` 会拒绝它。

```bash
libra clone --filter=blob:none --no-checkout git@github.com:user/repo.git
libra clone --filter=tree:0 --bare git@github.com:user/repo.git
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

浅克隆对于 CI/CD 流水线和不需要完整历史的大型 monorepo 很重要。Libra 支持与 Git 语义相同的 `--depth N`：历史会截断到指定提交数。depth 值在解析时校验（必须是正整数），并传递到 fetch 协议层。Libra 还支持基于日期边界的 `--shallow-since`（`deepen-since`）、基于 ref 边界的 `--shallow-exclude`（`deepen-not`），以及拒绝浅源的 `--reject-shallow`。`--depth` 可与 `--shallow-since`/`--shallow-exclude` 组合：由于 `git-upload-pack` 拒绝同时发送普通 `deepen` 与 `deepen-since`/`deepen-not`，在协议层基于时间/ref 的请求会取代普通 depth。浅克隆完成后，fetch 侧的 `libra fetch --unshallow` 可将浅仓库还原为完整仓库，`libra fetch --deepen N` 可继续加深历史。

### `--sparse` 被有意不支持

稀疏检出（`git clone --sparse`、`git sparse-checkout`）被有意不实现。Sparse cone/skip-worktree 依赖 Git 管理的工作树配置，而 Libra 已将 config / HEAD / refs 迁移到 SQLite。桥接并非零成本；基于审计的决策是推迟 `--sparse`，直到出现无法通过分层云存储满足的具体 monorepo 子树检出需求。重启条件见 [`docs/improvement/compatibility/declined.md`](../improvement/compatibility/declined.md) 条目 **D10**。

### `--recurse-submodules` 被有意不支持

按照更广泛的产品边界（没有 submodule 子命令表面），`clone --recurse-submodules` 也不受支持。重启条件见 [`docs/improvement/compatibility/declined.md`](../improvement/compatibility/declined.md) 条目 **D1**（submodule）和 **D4**（clone --recurse-submodules）。

### `--single-branch` 标志

与 `--branch` 组合时，`--single-branch` 通过只获取指定分支的历史来减少 clone 期间传输的数据量。这对包含许多长期分支的大型仓库尤其有用，例如 CI 构建某个特定 release 分支时只需要一个分支。Git 也支持此能力；jj 不支持，因为它的 operation-log 模型按设计获取所有 refs。

### 元数据写入与凭据脱敏

克隆元数据原子写入：分支 ref、`HEAD` 以及 `branch.<branch>.merge` / `branch.<branch>.remote` / `remote.<name>.url` / `remote.<name>.fetch` 配置项都在同一个事务内写入，失败时全部回滚（不留下半配置仓库）。空仓库只写 `remote.<name>.url` 与 `remote.<name>.fetch`（不写合成的分支跟踪）。fetch refspec 对普通克隆为 `+refs/heads/*:refs/remotes/<name>/*`，对 `--mirror` 为 `+refs/*:refs/*`（并记录 `remote.<name>.mirror = true`）。

克隆 URL 中内嵌的凭据（HTTP(S) token 或密码）会从每个输出与持久化面脱敏——"Connecting to …" 行、存储的 `remote.<name>.url`、reflog 的 `clone: from <url>` 条目、JSON 的 `remote_url` 以及错误消息。SSH 风格的 `git@host` 用户前缀是约定俗成的，会被保留。原始 URL 仅用于实际传输。

## 参数对比：Libra vs Git vs jj

| 参数 / 标志 | Git | jj | Libra |
|---|---|---|---|
| 远程 URL（位置参数） | `git clone <url>` | `jj git clone <url>` | `libra clone <url>` |
| 目标目录 | `git clone <url> <dir>` | `jj git clone <url> <dir>` | `libra clone <url> <dir>` |
| 指定分支 | `-b` / `--branch` | `-b` / `--branch`（jj 0.17+） | `-b` / `--branch` |
| 单分支 | `--single-branch` | N/A | `--single-branch` |
| 单分支后的全分支反向选项 | `--no-single-branch` | N/A | `--no-single-branch` |
| 裸克隆 | `--bare` | N/A | `--bare` |
| 浅克隆（depth） | `--depth <n>` | N/A | `--depth <n>` |
| 按日期浅克隆 | `--shallow-since=<date>` | N/A | `--shallow-since <date>` |
| 排除浅边界 | `--shallow-exclude=<rev>` | N/A | `--shallow-exclude <rev>` |
| 拒绝浅源 | `--reject-shallow` | N/A | `--reject-shallow` |
| 自定义远程名 | `-o` / `--origin <name>` | N/A | `-o` / `--origin <name>` |
| 镜像克隆 | `--mirror` | N/A | Partial：bare + mirror 配置；精确 `refs/*` 镜像延后 |
| 引用仓库 | `--reference <repo>` / `--reference-if-able <repo>` | N/A | 复制语义，无 alternates 借用 |
| 从引用仓库脱离 | `--dissociate` | N/A | 确认复制语义 clone 已完全本地化 |
| 本地 clone 优化 | `-l` / `--local` | N/A | 可行时硬链接本地对象 |
| 共享 clone | `-s` / `--shared` | N/A | 复制语义，无 alternates 借用 |
| 禁用硬链接 | `--no-hardlinks` | N/A | 复制本地对象 |
| 并行任务 | `-j` / `--jobs <n>` | N/A | 预留/no-op，校验 1..=16 |
| 递归 submodule | `--recurse-submodules` | N/A | N/A（无 submodule） |
| 浅 submodule | `--shallow-submodules` | N/A | N/A |
| 独立 git dir | `--separate-git-dir=<dir>` | N/A | N/A（已移除） |
| 模板目录 | `--template=<dir>` | N/A | N/A（由 init 内部处理） |
| Quiet 模式 | `-q` / `--quiet` | `--quiet` | `--quiet`（全局标志） |
| Verbose / 进度 | `--progress` / `--verbose` | N/A | 分阶段 stderr 进度（默认） |
| 不检出 | `-n` / `--no-checkout` | N/A | `-n` / `--no-checkout` |
| 稀疏检出 | `--sparse` | N/A | N/A |
| Filter（部分克隆） | `--filter=<spec>` | N/A | Partial：promisor 配置，无 lazy backfill |
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
- `--mirror` 是 partial：它隐含 bare 并写入 mirror 配置，但精确 `refs/*` 镜像延后
- `--reference`、`--reference-if-able`、`--shared` 与 `--dissociate` 使用复制语义，不创建长期 `info/alternates` 依赖
- `--filter` 记录 promisor 配置，但不会按需补取缺失对象；请搭配 `--no-checkout` 或 `--bare`
- `--jobs` 在校验 `1..=16` 后作为预留 no-op 接受；传输仍为串行
- Clone 始终引导 vault 签名；如有需要，可在克隆后使用 `libra config` 禁用
- `--depth` 值必须是正整数；0 或负数会在解析时被拒绝
- 需要只写元数据而不物化工作树时可使用 `--no-checkout`
