# Libra LFS 配额托管服务设计

## 状态

Proposed

## 日期

2026-06-05

## 背景

Libra 当前已经有内置 LFS 客户端能力：

- `libra lfs track/untrack/ls-files/locks/lock/unlock` 已由 `src/command/lfs.rs` 提供。
- `src/utils/lfs.rs` 负责 `.libra_attributes`、pointer 文件、SHA-256 OID、本地 `.libra/lfs/objects`。
- `src/internal/protocol/lfs_client.rs` 已能按 Git LFS Batch API 发起 upload/download，按 Locking API 发起 locks/lock/unlock/verify_locks。
- `libra push` 会从 Git object blob 中识别 LFS pointer，并调用 `LFSClient::push_objects()` 上传真实大文件对象。

缺口是服务端：当前仓库没有“Libra 注册用户、仓库权限、LFS 配额、对象账本、托管对象存储”的写路径。`worker/` 是 `libra publish` 的只读浏览 Worker，已有文档和代码都强调它只从 D1/R2 读取发布快照，不应该把 LFS 上传写入混进去。

本设计目标是给 Libra 注册用户提供可计费、可限额、可审计的托管 LFS 服务，同时尽量复用 Git LFS 协议，让现有 Libra 客户端只做必要扩展。

## 外部协议约束

本方案遵守 Git LFS 的三个关键事实：

1. LFS Batch URL 是在 LFS server URL 后追加 `/objects/batch`，请求和响应使用 `application/vnd.git-lfs+json`。
2. Batch upload 响应可以给对象返回 `upload` 和 `verify` actions；对象已存在时应省略 `actions`，客户端据此跳过上传。
3. Locking API 在 LFS server URL 后追加 `/locks`、`/locks/verify` 和 `/locks/:id/unlock`，创建锁和验证锁都要求 push 权限。

参考：

- [Git LFS Batch API](https://github.com/git-lfs/git-lfs/blob/main/docs/api/batch.md)
- [Git LFS Server Discovery](https://github.com/git-lfs/git-lfs/blob/main/docs/api/server-discovery.md)
- [Git LFS Locking API](https://github.com/git-lfs/git-lfs/blob/main/docs/api/locking.md)
- [Cloudflare R2 limits](https://developers.cloudflare.com/r2/platform/limits/)
- [Cloudflare R2 multipart upload from Workers](https://developers.cloudflare.com/r2/api/workers/workers-multipart-usage/)
- [Cloudflare D1 Worker Binding API](https://developers.cloudflare.com/d1/worker-api/d1-database/)

## 目标

- 给 Libra 注册用户、组织或团队分配 LFS 存储配额。
- 支持仓库级权限：有 read 权限才能 download，有 write/push 权限才能 upload、lock、unlock。
- 支持 Git LFS Batch API 和 Locking API，现有 `LFSClient` 可以演进接入。
- 上传前预留配额，上传完成后校验对象 SHA-256 和 size，校验通过才计入已用量。
- 对同一 owner namespace 内相同 `sha256` 对象去重计费，跨 owner 不去重，避免跨租户对象存在性侧信道。
- 支持对象生命周期：pending、verifying、ready、failed、deleted。
- 提供用户可见的 quota/status API 和 `libra lfs quota` 命令。
- 保留 `libra publish` Worker 的只读边界，不把 LFS 写 API 放进 publish 页面服务。

## 非目标

- 不把 Git LFS `.gitattributes` filter/hook 兼容作为前置目标；Libra 仍使用 `.libra_attributes`。
- 不要求 Git 官方 `git-lfs` 客户端支持 Libra 专用 multipart 扩展；Git LFS 客户端只需要走 `basic` transfer。
- 不把用户的 Cloudflare API token 暴露给 CLI；Libra 托管服务持有对象存储凭据，CLI 只持有 Libra 用户 token。
- 不在 v1 自动删除已提交历史仍引用的 LFS 对象；删除和 GC 必须基于引用账本和保留期。

## 总体架构

新增独立的 Libra LFS Service，逻辑上和 `publish` Worker 分离：

```text
libra CLI
  |
  | Git LFS Batch / Locking / Verify
  v
Libra LFS Service
  |-- Auth adapter: 解析 Libra 用户 token / PAT / OIDC JWT
  |-- Repo authz: 解析 repo_id、owner_account_id、read/write/admin 权限
  |-- Quota ledger: D1/SQL 配额预留、提交、释放、审计
  |-- Transfer broker: 生成 upload/download action URL
  |-- Verify worker: 校验 R2 对象 size + SHA-256
  |-- Lock service: 管理 LFS locks
  v
Object store: R2 or S3-compatible bucket
```

部署形态：

- Libra 官方托管：`https://lfs.libra.dev/<owner>/<repo>.git/info/lfs`。
- 自托管：使用相同协议，可以部署为单独 Worker 或 Rust HTTP 服务，绑定自己的 D1/R2/S3。
- 本仓库实现时建议新建 `lfs-worker/` 或独立 Worker entry，不直接复用 `worker/` 的 publish 路由。可以复用校验、错误 envelope、D1 prepared statement、R2 key 安全模式，但不能改变 publish Worker “只读”定位。

## LFS server URL 解析

当前 `generate_mono_lfs_server_url()` 对非 GitHub/Gitee 域名只取 scheme + host，这会丢失 repo path。托管 LFS 必须补齐显式配置优先级：

1. `remote.<name>.lfsurl`
2. `lfs.url`
3. Git LFS 默认 discovery：按远端 URL 规范化后追加 `/info/lfs`，例如 `<git remote>.git/info/lfs`
4. 现有 mono domain fallback，仅作为兼容旧配置的最后路径

新增命令建议：

```bash
libra lfs login --host lfs.libra.dev
libra config remote.origin.lfsurl https://lfs.libra.dev/acme/atlas.git/info/lfs
libra lfs quota --remote origin
```

本地 credential 存储：

- token 存入全局 vault scope，键名类似 `lfs.credentials.<host>.token`。
- repo config 只保存非 secret 的 `remote.<name>.lfsurl`、`lfs.owner`、`lfs.repo_id`。
- 请求优先发 `Authorization: Bearer <token>`；服务端也接受 Basic `username:token`，兼容 Git LFS credential 行为。

## API 契约

### Batch upload/download

路径：

```text
POST /<owner>/<repo>.git/info/lfs/objects/batch
```

请求沿用 Git LFS Batch API，并要求 Libra 客户端补齐 `ref`：

```json
{
  "operation": "upload",
  "transfers": ["basic"],
  "ref": { "name": "refs/heads/main" },
  "objects": [
    { "oid": "64-lower-hex-sha256", "size": 104857600 }
  ],
  "hash_algo": "sha256"
}
```

服务端规则：

- `operation=download` 要求 repo read 权限。
- `operation=upload` 要求 repo write 权限。
- `hash_algo` 只接受缺省或 `sha256`。
- `oid` 必须是 64 字符 lowercase hex。
- `size` 必须是 `0..=owner.max_object_bytes`。
- 单次 batch object 数量设硬上限，例如 100，超限返回 HTTP 413。
- 请求格式错误返回 HTTP 422；单个对象不可用时优先返回 HTTP 200 加 per-object error。

对象已存在且 `state=ready`：

```json
{
  "transfer": "basic",
  "objects": [
    {
      "oid": "64-lower-hex-sha256",
      "size": 104857600,
      "authenticated": true
    }
  ],
  "hash_algo": "sha256"
}
```

对象不存在且配额预留成功：

```json
{
  "transfer": "basic",
  "objects": [
    {
      "oid": "64-lower-hex-sha256",
      "size": 104857600,
      "authenticated": true,
      "actions": {
        "upload": {
          "href": "https://lfs.libra.dev/_lfs/uploads/<upload_id>",
          "header": {
            "Authorization": "Bearer <short-lived-upload-token>"
          },
          "expires_in": 900
        },
        "verify": {
          "href": "https://lfs.libra.dev/_lfs/uploads/<upload_id>/verify",
          "header": {
            "Authorization": "Bearer <short-lived-upload-token>"
          },
          "expires_in": 900
        }
      }
    }
  ],
  "hash_algo": "sha256"
}
```

配额不足：

```json
{
  "transfer": "basic",
  "objects": [
    {
      "oid": "64-lower-hex-sha256",
      "size": 104857600,
      "error": {
        "code": 507,
        "message": "LFS quota exceeded for account acme"
      }
    }
  ],
  "hash_algo": "sha256"
}
```

### Upload action

路径：

```text
PUT /_lfs/uploads/<upload_id>
```

规则：

- 只接受 upload action token。
- token scope 必须绑定 `upload_id`、`account_id`、`repo_id`、`oid`、`size`、`operation=upload`、过期时间。
- 上传写入临时对象 key，不直接覆盖 ready 对象：

```text
<account_id>/lfs/uploads/<upload_id>/object
```

- 对 `basic` transfer，服务端必须至少校验 Content-Length 等于预留 size。最终 SHA-256 校验在 verify action 完成。
- 如果 Worker relay 方式实现，必须流式写入 R2，不得把对象读入内存。
- 如果使用预签名 S3/R2 URL，upload action 可以直接指向对象存储临时 key，但 verify action 仍由 LFS Service 完成。

### Verify action

路径：

```text
POST /_lfs/uploads/<upload_id>/verify
```

规则：

- 服务端读取临时对象并计算 SHA-256，确认 `oid` 和 `size` 都匹配。
- 校验成功后将对象提升为 ready，并将配额从 reserved 转为 used。
- 校验失败时删除临时对象，释放 reserved，upload 进入 failed。
- v1 可以同步校验小于阈值的对象；大对象可以由队列执行，但客户端可见结果必须在本次 verify 请求结束前明确成功或失败，否则 `libra push` 无法给出确定结论。

当前 `src/internal/protocol/lfs_client.rs::upload_object()` 需要补齐：如果 batch response 带 `verify` action，PUT 成功后必须按该 action 发起 verify 请求，并把 verify 失败映射成 `LfsPushError`。

### Download action

路径：

```text
GET /_lfs/objects/<object_token>
```

规则：

- Batch download 先验证 repo read 权限和对象是否被该 repo 引用。
- action token 绑定 `account_id`、`repo_id`、`oid`、`operation=download`、过期时间。
- 服务端可以返回 Worker relay URL 或短期 presigned GET URL。
- 带宽配额不足时返回 per-object error `509`。

### Locking API

路径保持 Git LFS 习惯：

```text
GET  /<owner>/<repo>.git/info/lfs/locks
POST /<owner>/<repo>.git/info/lfs/locks
POST /<owner>/<repo>.git/info/lfs/locks/verify
POST /<owner>/<repo>.git/info/lfs/locks/:id/unlock
```

权限：

- `GET /locks`：repo read。
- `POST /locks`：repo write，`UNIQUE(repo_id, path)` 防止同路径双锁。
- `POST /locks/verify`：repo write，返回 `ours` 和 `theirs`。
- `unlock force=false`：锁 owner 或 repo admin。
- `unlock force=true`：repo admin/maintainer。

## 数据模型

以下 schema 是概念契约；实现时可以放入 `sql/lfs/0001_lfs.sql`，并为 Worker 复制迁移建立与 publish schema 类似的 byte-for-byte 测试。

```sql
CREATE TABLE lfs_accounts (
    account_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('user', 'org')),
    slug TEXT NOT NULL UNIQUE,
    plan TEXT NOT NULL,
    quota_bytes INTEGER NOT NULL CHECK (quota_bytes >= 0),
    used_bytes INTEGER NOT NULL DEFAULT 0 CHECK (used_bytes >= 0),
    reserved_bytes INTEGER NOT NULL DEFAULT 0 CHECK (reserved_bytes >= 0),
    bandwidth_quota_bytes INTEGER,
    bandwidth_used_bytes INTEGER NOT NULL DEFAULT 0 CHECK (bandwidth_used_bytes >= 0),
    status TEXT NOT NULL CHECK (status IN ('active', 'suspended', 'deleted')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (used_bytes + reserved_bytes <= quota_bytes)
);

CREATE TABLE lfs_repositories (
    repo_id TEXT PRIMARY KEY,
    owner_account_id TEXT NOT NULL REFERENCES lfs_accounts(account_id),
    slug TEXT NOT NULL,
    visibility TEXT NOT NULL CHECK (visibility IN ('private', 'internal', 'public')),
    status TEXT NOT NULL CHECK (status IN ('active', 'archived', 'deleted')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (owner_account_id, slug)
);

CREATE TABLE lfs_repo_members (
    repo_id TEXT NOT NULL REFERENCES lfs_repositories(repo_id) ON DELETE CASCADE,
    user_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('read', 'write', 'maintain', 'admin')),
    created_at TEXT NOT NULL,
    PRIMARY KEY (repo_id, user_id)
);

CREATE TABLE lfs_objects (
    account_id TEXT NOT NULL REFERENCES lfs_accounts(account_id),
    oid TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    r2_key TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'verifying', 'ready', 'failed', 'deleted')),
    ref_count INTEGER NOT NULL DEFAULT 0 CHECK (ref_count >= 0),
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    verified_at TEXT,
    deleted_at TEXT,
    CHECK (length(oid) = 64 AND oid NOT GLOB '*[^0-9a-f]*'),
    PRIMARY KEY (account_id, oid)
);

CREATE TABLE lfs_repo_objects (
    repo_id TEXT NOT NULL REFERENCES lfs_repositories(repo_id) ON DELETE CASCADE,
    account_id TEXT NOT NULL,
    oid TEXT NOT NULL,
    path_hint TEXT,
    first_seen_ref TEXT,
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    PRIMARY KEY (repo_id, oid),
    FOREIGN KEY (account_id, oid) REFERENCES lfs_objects(account_id, oid)
);

CREATE TABLE lfs_uploads (
    upload_id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES lfs_accounts(account_id),
    repo_id TEXT NOT NULL REFERENCES lfs_repositories(repo_id),
    oid TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    temp_r2_key TEXT NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN ('reserved', 'uploaded', 'verifying', 'verified', 'failed', 'expired')
    ),
    idempotency_key TEXT NOT NULL UNIQUE,
    reserved_bytes INTEGER NOT NULL CHECK (reserved_bytes >= 0),
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    completed_at TEXT,
    failure_code TEXT,
    failure_message TEXT,
    CHECK (length(oid) = 64 AND oid NOT GLOB '*[^0-9a-f]*')
);

CREATE TABLE lfs_quota_events (
    event_id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES lfs_accounts(account_id),
    repo_id TEXT,
    upload_id TEXT,
    oid TEXT,
    delta_used_bytes INTEGER NOT NULL,
    delta_reserved_bytes INTEGER NOT NULL,
    reason TEXT NOT NULL CHECK (
        reason IN ('reserve', 'commit', 'release', 'expire', 'delete', 'reconcile')
    ),
    created_at TEXT NOT NULL
);

CREATE TABLE lfs_locks (
    lock_id TEXT PRIMARY KEY,
    repo_id TEXT NOT NULL REFERENCES lfs_repositories(repo_id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    ref_name TEXT,
    owner_user_id TEXT NOT NULL,
    owner_name TEXT NOT NULL,
    locked_at TEXT NOT NULL,
    UNIQUE (repo_id, path)
);
```

## R2/S3 key layout

对象 key 必须只由服务端从 D1 行生成，客户端不能提交原始 key。

```text
<account_id>/lfs/objects/sha256/<oid[0..2]>/<oid[2..4]>/<oid>
<account_id>/lfs/uploads/<upload_id>/object
<account_id>/lfs/audit/<yyyy>/<mm>/<event_id>.json
```

说明：

- `account_id` 用 UUID 或不可枚举 ID，不用用户 slug。
- canonical object key 只在 verify 成功后可下载。
- upload temp key 可以定期清理。
- 跨 account 不共享对象 key，避免“我上传某个 hash 是否秒传”泄漏其他租户是否拥有该文件。

## 配额算法

### 预留

Batch upload 对每个 missing object 做原子预留：

```sql
UPDATE lfs_accounts
SET reserved_bytes = reserved_bytes + :size,
    updated_at = :now
WHERE account_id = :account_id
  AND status = 'active'
  AND used_bytes + reserved_bytes + :size <= quota_bytes;
```

若 `changes != 1`，该对象返回 `507 quota exceeded`。预留成功后，在同一个 D1 `batch()` 事务内插入或确认 `lfs_objects(account_id, oid, state='pending')`，再插入 `lfs_uploads` 和 `lfs_quota_events(reason='reserve')`。

同一 `account_id + oid` 已存在时的处理：

- `state='ready'`：不预留配额，只 upsert `lfs_repo_objects` 并返回无 `actions` 的 ready object。
- `state IN ('pending', 'verifying')`：返回 per-object `409 upload already in progress`，不新增 upload session，不重复预留。
- `state IN ('failed', 'deleted')`：允许重新预留并把对象状态改回 `pending`，但必须写新的 upload 和 quota event。

### 提交

Verify 成功后，在同一个事务内：

1. `lfs_uploads.status='verified'`
2. `lfs_objects.state='ready'`
3. `lfs_repo_objects` upsert repo 引用
4. `lfs_objects.ref_count` 只在新增 repo 引用时递增
5. `lfs_accounts.reserved_bytes -= size`
6. `lfs_accounts.used_bytes += size`
7. 写 `lfs_quota_events(reason='commit', delta_used_bytes=size, delta_reserved_bytes=-size)`

### 释放

Verify 失败、上传过期、用户取消时：

1. 删除临时 R2 对象。
2. `lfs_uploads.status='failed'|'expired'`
3. `lfs_accounts.reserved_bytes -= size`
4. 写 `lfs_quota_events(reason='release'|'expire')`

### 去重

- `lfs_objects(account_id, oid)` ready 时，同 account 下任何 repo 再引用该对象不增加 used quota。
- 不同 account 上传相同 oid，分别计费。
- 同 account 同 oid 正在 `pending/verifying` 时，第二个 upload 返回 `409 upload already in progress`，要求客户端稍后重试，不能省略 actions。

### 对账

增加周期性 reconciliation：

- 扫描 `lfs_uploads` 中过期但仍占 reserved 的行并释放。
- 按 `lfs_objects.state='ready'` 重算 used，并与 `lfs_accounts.used_bytes` 比较。
- 抽样 HEAD R2 canonical key，缺失则将对象标记为 failed/corrupt 并报警。

## Libra 客户端需要改动

### `src/lfs_structs.rs`

- `BatchRequest` 增加可选 `ref` 字段，复用现有 `Ref` 结构，serde 名称为 `"ref"`。
- `ResponseObject.actions` 中已支持 `verify` action，保持兼容。
- 如果要支持 Libra multipart 扩展，新增 transfer enum 值时必须标注为 Libra extension，不声称 Git LFS 通用兼容。

### `src/internal/protocol/lfs_client.rs`

- endpoint resolution 改为 `remote.<name>.lfsurl` / `lfs.url` 优先。
- `push_objects()` 和 `push_object()` 填充当前 branch 的 fully-qualified ref。
- `upload_object()` 在 PUT 成功后调用 `verify` action。
- 认证层从全局 BasicAuth 扩展为 credential provider：
  - Bearer token 来自 vault/env。
  - 401 + `LFS-Authenticate` 时再进入交互式登录或 Basic retry。
  - 不在 tracing/debug 中输出 token、action URL query secret、R2 key。
- 对 per-object 507/509 映射到稳定错误码：
  - `LBR-LFS-QUOTA-001`：storage quota exceeded。
  - `LBR-LFS-BANDWIDTH-001`：bandwidth quota exceeded。

### `src/command/lfs.rs`

新增子命令建议：

```text
libra lfs login --host <host>
libra lfs quota [--remote <name>] [--json|--machine]
libra lfs uploads [--remote <name>] [--state pending|failed|expired]
```

`quota` 输出：

```json
{
  "ok": true,
  "command": "lfs.quota",
  "data": {
    "account_id": "acct_...",
    "repo_id": "repo_...",
    "quota_bytes": 10737418240,
    "used_bytes": 5368709120,
    "reserved_bytes": 104857600,
    "available_bytes": 5263851520
  }
}
```

### `src/command/push.rs`

- LFS upload 失败必须阻断 Git object push，避免 pointer 已推上去但真实对象缺失。
- 如果服务端返回 409 pending upload，提示用户重试或运行 `libra lfs uploads` 查看未完成上传。

## 大文件传输策略

### v1：basic transfer

支持 Git LFS `basic` transfer，适合：

- Worker relay 的对象：受 Worker 请求体限制约束，适合中等对象。
- Presigned S3/R2 PUT 的对象：单 PUT 上限按对象存储平台限制；Cloudflare R2 当前单次上传上限接近 5 GiB，multipart 对象最大约 4.995 TiB。

推荐 v1 默认：

- 对象 `<= 4 GiB`：返回 presigned single PUT upload action。
- 对象 `> 4 GiB`：返回 per-object error `413 object requires Libra multipart transfer`，提示升级客户端或启用 multipart。

如果 LFS Service 只通过 Cloudflare Worker R2 binding 写对象，而不持有 S3 signing credential，则不能生成 presigned PUT；此部署形态必须使用 Worker relay 或 Libra multipart。官方托管可以由后端 signing service 生成短期 presigned URL，Worker 仍只负责 Batch、Verify、Quota 和 Locking API。

### v2：Libra multipart transfer

Libra 客户端可实现非 Git LFS 通用的 `libra-multipart` transfer：

```json
{
  "operation": "upload",
  "transfers": ["basic", "libra-multipart"],
  "objects": [{ "oid": "...", "size": 1099511627776 }]
}
```

服务端返回：

```json
{
  "transfer": "libra-multipart",
  "objects": [
    {
      "oid": "...",
      "size": 1099511627776,
      "actions": {
        "upload": { "href": "https://lfs.libra.dev/_lfs/multipart/<upload_id>" },
        "verify": { "href": "https://lfs.libra.dev/_lfs/uploads/<upload_id>/verify" }
      }
    }
  ]
}
```

配套端点：

```text
POST /_lfs/multipart/<upload_id>/parts
PUT  /_lfs/multipart/<upload_id>/parts/<part_number>
POST /_lfs/multipart/<upload_id>/complete
DELETE /_lfs/multipart/<upload_id>
```

实现要求：

- part size 默认 64 MiB，最小不低于对象存储要求。
- part 数量不得超过 R2 multipart 上限。
- complete 后仍必须执行 verify，未校验前不能计入 ready。
- 客户端并发上传需要限制默认并发，例如 4，避免触发 Worker/R2 连接限制。

## 安全和隐私

- 所有 API 必须先认证，再解析 repo 权限，再访问 D1/R2。
- 上传/download action token 必须短期、单用途、绑定 oid/size/repo/account/operation。
- D1 查询必须使用 prepared statements 和 bind 参数。
- 所有 path、ref、oid、size 都在边界校验；R2 key 只由服务端生成。
- 任何响应和日志不得包含用户 token、R2 secret、完整 presigned credential、内部 bucket 名。
- 跨 account 不 dedupe，避免通过秒传或错误码探测其他租户文件。
- `download` 只允许 repo 已引用的 ready 对象，不能只凭 oid 下载 account 内任意对象。
- `verify` 前对象不可下载。
- lock path 必须是 repo-relative path，不接受 absolute path、`..`、NUL、Unicode slash confusable。
- 生产 `src/**` 改动继续遵守不新增裸 `unwrap()`/`expect()` 的现有 guard；确需 `expect()` 必须有 INVARIANT 注释。

## 错误语义

服务端 HTTP 层：

| 场景 | HTTP / object error | 客户端稳定错误 |
| --- | --- | --- |
| 未登录 | 401 + `LFS-Authenticate` | `LBR-AUTH-001` |
| 无 repo read/write | 403 | `LBR-AUTH-002` |
| repo 不存在或无权知道 | 404 | `LBR-REPO-001` 或现有 repo not found |
| oid/size/hash_algo 无效 | 422 | `LBR-CLI-002` |
| batch 太大 | 413 | `LBR-CLI-002` |
| storage quota exceeded | per-object 507 | `LBR-LFS-QUOTA-001` |
| bandwidth quota exceeded | per-object 509 | `LBR-LFS-BANDWIDTH-001` |
| 上传冲突或锁冲突 | 409 | `LBR-CONFLICT-002` |
| R2/D1 不一致 | 500 typed internal, request_id | `LBR-INTERNAL-001` |

## 实施计划

### Phase 0：客户端协议缺口

验收：

- `BatchRequest` 支持可选 `ref`。
- `LFSClient` 能读取 `remote.<name>.lfsurl` 和 `lfs.url`。
- `upload_object()` 支持 `verify` action。
- mock LFS server 测试覆盖：upload+verify 成功、verify 422、507 quota、401 auth、ready object 无 actions。
- `docs/commands/lfs.md` 更新 login/quota 或明确仍未实现。

### Phase 1：LFS 服务端最小骨架

验收：

- 新增独立 LFS service package 或 Worker entry。
- 新增 `sql/lfs/0001_lfs.sql` 和 migration consistency test。
- 实现 auth adapter trait 和 fake auth 测试实现。
- 实现 D1 repo/account/quota/object/upload/lock DAO，全部 prepared statements。
- FakeD1/FakeR2 单元测试覆盖 schema invariants。

### Phase 2：basic upload/download + 配额

验收：

- Batch upload missing object 会预留配额并返回 upload/verify actions。
- Ready object 会省略 actions，重复 push 不重复计费。
- Verify 成功后 reserved 转 used，失败释放 reserved。
- Batch download 只返回 repo 引用过的 ready object。
- 对账任务能释放 expired uploads。
- CLI 级 mock 测试覆盖 `libra push` 被 quota 阻断时不推送 pointer。

### Phase 3：Locking API

验收：

- `/locks`、`/locks/verify`、`/:id/unlock` 全部实现。
- `UNIQUE(repo_id, path)` 防止双锁。
- `verify_locks` 返回 theirs 时 `libra push` 阻断。
- force unlock 只有 admin/maintain 可用。

### Phase 4：用户可见 quota 和运维

验收：

- `libra lfs quota --json` 输出 quota/used/reserved/available。
- 服务端暴露 `GET /_lfs/quota?repo=<repo_id>`。
- 添加 admin 操作：调整 quota、暂停 account、列出 pending/failed uploads。
- 添加审计日志和请求 `request_id`。

### Phase 5：Libra multipart

验收：

- 客户端协商 `libra-multipart`。
- 大于 single PUT 阈值的对象可分片上传、恢复、complete、verify。
- multipart 失败释放 reserved。
- live R2 测试覆盖至少一个超过 single Worker relay 限制的对象。

## 必跑验证

代码落地时按变更面运行：

```bash
LIBRA_SKIP_WEB_BUILD=1 cargo test --test command_test lfs -- --test-threads=1
LIBRA_SKIP_WEB_BUILD=1 cargo test --test compat_matrix_alignment
cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan
LIBRA_SKIP_WEB_BUILD=1 cargo clippy --all-targets --all-features -- -D warnings
cargo +nightly fmt --all --check
```

如果新增独立 Worker/package：

```bash
pnpm --dir lfs-worker lint
pnpm --dir lfs-worker test
pnpm --dir lfs-worker test:miniflare
pnpm --dir lfs-worker build
```

如果改动公开 LFS 命令：

- 更新 `src/cli.rs` after_help 示例。
- 更新 `docs/commands/lfs.md`。
- 更新 `COMPATIBILITY.md`。
- 更新 command -> scenario map 和 `cli.clean-rm-mv-lfs-basic` 或新增 `cli.lfs-quota-smoke`。
- 运行 `cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --only <owner-scenario-id>`。

## 关键决策

- LFS 托管服务是独立写服务，不复用 publish Worker 读路由。
- 配额以 account namespace 计费，同 account 去重，跨 account 不去重。
- 上传必须先预留配额，verify 成功后才转 used。
- 服务端不信任客户端 OID，必须校验 R2 对象 SHA-256。
- v1 优先支持标准 `basic` transfer，v2 再实现 Libra 专用 multipart。
- 客户端必须实现 `verify` action，否则不能安全支持托管配额上传。
