# Lore ⇄ Libra: A Bidirectional Capability-Gap Completion Plan

> Canonical planning document for closing the capability gap **in both directions** between **Epic Games' Lore VCS** and **Libra**.
>
> **The document has two halves, each anchored to the *other* system's identity:**
> - **Part I — Lore → Libra** (§1–§7 + Appendix A): take **Lore** as the reference; plan what **Libra** must build to match it **while keeping Libra Git-format-compatible and AI-agent-native**. *(Original plan, unchanged.)*
> - **Part II — Libra → Lore** (after Appendix A): the mirror image — take **Libra** as the reference; plan what **Lore** must build to match it (`libra 要补齐 lore 缺失功能`) **while keeping Lore content-addressed, partition-based, no-index, API-first, and server-centric.** The headline reverse-gap is Libra's entire **AI-native runtime**, which Lore lacks completely; the second is a set of **Git-style history-rewrite/inspection verbs** (rebase, squash, stash, reflog, blame, tag, grep, hooks).
>
> Neither half proposes importing the other system's *substrate* — Part I never adopts BLAKE3/partitions/no-index, and Part II never adopts the Git on-disk format/index/SQLite-refs. Each adopts the other's **user-facing capability**, expressed idiomatically to its own architecture. Where an idea conflicts with the host's identity, the plan **adapts or defers** it and says so (Part I §6; Part II §P2.6).
>
> 本文为开发设计文档，主体用英文以保持技术术语精确（与 `docs/development/` 下多数设计文档一致）；下方 **§0 中文摘要** 给出两个方向的完整中文概览。

## 0. 中文摘要（Executive summary）

**目标。** 以 Epic Games 的 **Lore**（开源、集中式、内容寻址、面向二进制大文件的 VCS）为参照，找出 **Libra** 相对缺失的能力，并设计一份「在保持 Git 磁盘格式兼容 + AI 原生定位不变」前提下、可落地执行的补全计划。

**两套架构的根本差异（决定了哪些能"抄"、哪些不能）。**

- **Lore**：存储子系统与版本控制子系统**解耦**、**API-first**（以 `lore-capi/lore.h` 这套 C ABI 为第一产物）；不可变内容寻址存储用 **BLAKE3**；大文件按 **FastCDC** 分块 + 递归分片去重；**无 Git 式 index**，文件系统即真相，dirty/staged 是 Merkle 树上的标志位；**partition**（16 字节访问边界）= 多租户隔离；links/layers/forks/instances/shared-store 组合能力；字节级 **obliteration**；按需水合的 **VFS**；6 种语言 SDK。
- **Libra**：实现 Git 客户端、**完整磁盘格式兼容**（loose/index/pack，SHA-1 与 SHA-256 双哈希），但把 refs/HEAD/config/reflog 放进 **SQLite**（sea-orm）；自带分层 S3/R2 存储 + D1/R2 备份 + 只读 Cloudflare Worker 发布；最大的差异化是一整套 **AI 子系统**（agents/orchestrator/MCP/sandbox/automation）——这是 Lore 完全没有的。

**"补齐"指什么。** 采纳 Lore 的**用户可见能力**，但用 **Libra 自己的底座**（SQLite 侧表、`Storage` trait、LFS、分层/云存储、hooks、MCP/agent 接口）来实现。**不**照搬 BLAKE3 寻址、320 字节 revision state、node-block、FastCDC 对象分块、partition=能力 等——它们与 Git 磁盘格式根本冲突，照搬等于重写一个 Lore。凡冲突处，本文都明确标注「改造」或「推迟」。

**最大的差距（按主题）。** 稀疏/VFS/惰性水合；工作区人体工学（持久化 dirty 集合、常数时间 status、**每个 worktree 独立的 HEAD/index/refs**、大小写变更处理）；冲突解决 UX（`restore --ours/--theirs`、diff3 标记、`merge --dry-run`）；branch 便捷命令（`branch diff`/`reset`）；diff/merge 深度（位置化 revspec、空白选项、`--diff3`）；元数据与溯源（branch/repo/file 的 KV 元数据、commit trailer）；合规删除（obliteration）；认证/可观测性（`libra auth`、OTLP、shell 补全）；全局资源/并发控制旋钮（`--max-connections`、文件/压缩限制、`--non-interactive` 等）；锁的强制力（从 push 强制扩展到 commit/add）；健壮性（云端退避、取数时校验哈希、`fsck --heal`）；组合能力（layers 可行；links/partitions/forks 多数推迟）；**服务端/协议/复制**（Libra 是纯 Git 客户端——无服务端、无自研 QUIC/gRPC 协议、无复制/分区——这一整类**多为架构性推迟**，可落地的仅 429 退避、token 认证、push `--atomic` 协议补齐、worktree 隔离）。

**分阶段计划速览。**

- **Phase 0 — 速赢**（各几天，纯增量、与 Git 格式无关）：`libra completions`、D1/R2 退避、取数即校验、`fsck --heal`、`flush(sync_data)`、`exist_batch`、滚动日志、`--offline/--local/--remote`、全局资源/并发控制旋钮、store/cache 可调参数。内部次序：**0.2→0.3→0.4**（共用云端写缓存路径）。
- **Phase 1 — 基础**（高价值、可行、解锁后续）：dirty 集合 + `libra dirty` + `status --cached`；`restore --ours/--theirs`（**前置条件已满足**，见 §4.4）；diff3 + `merge --dry-run/--restart`；位置化 diff + 空白选项；`branch diff`/`reset`；**branch/repo 元数据 KV（基石）**；typed metadata 命令族；`libra auth`（v1 token + host 作用域）；本地 `libra service`/notification v1；OTLP；`merge --autostash`；commit trailer + `log --trailer`；文件大小写变更处理。
- **Phase 2 — 组合与规模**（结构性、部分牵涉面大）。**建议次序：2.3（object alternates/共享库）→ 2.2（稀疏）→ 2.1（worktree 隔离）**——2.3 独立、1–2 周、价值高且可能可从历史恢复；2.2 v1 **不依赖** 2.1。另含 index 标记式 obliteration、统一冲突 sequencer、interactive auth + OS keyring、commit/add 锁强制、后台缓存淘汰、`shared-store set-use-automatically`。
- **Phase 3 — Lore-parity gated extensions**：文件依赖图 + dependency-filtered clone/sync、LFS 内 FastCDC 分块、水合式 VFS、link/subtree 组合 RFC。它们不再只是“永远推迟”，但必须等 metadata、sparse、shared-store、auth 的基石落地后再开工。
- **Later / 推迟**：partitions/forks、C ABI + 多语言 SDK、QUIC/gRPC 存储协议、分布式锁存储、Hosted Libra Server——要么与 Git 格式冲突，要么需要一个目前并不存在的 Libra 托管服务端。

**最重要的一件事。** ⭐ **每个 worktree 独立的 HEAD/index/refs（计划项 2.1）**：当前 worktree 通过符号链接共享 `.libra`，导致 HEAD/index/refs 共享、两个 worktree 无法停在不同分支——这是已记录的「坑」，也是并行 AI agent 工作流的主要障碍。

**明确不做（§6）。** BLAKE3 对象寻址、320 字节 state/node-block、把 FastCDC 当对象寻址、仓内 partition、Context 文件身份字段、移除 index、内联 blob 的原地擦除、树内冲突标志位、把 C ABI 当第一产物、自研 QUIC/gRPC 存储协议、SWFS 专有驱动、跨仓 `parent_repository` 合并字段。理由统一是：**守住 Git 磁盘格式兼容这条 Libra 的立身之本**。

### 0.B 反向摘要：Part II — Libra → Lore（以 Libra 为参照补齐 Lore）

**目标。** 与上文相反：以 **Libra** 为参照，找出 **Lore** 相对缺失的能力，并设计一份「在保持 Lore 内容寻址 + 分区 + 无 index + API/C-ABI 优先 + 服务端中心」前提下、可落地的补全计划。约束镜像翻转——这一半要守住的是 **Lore 的立身之本**，不照搬 Git 磁盘格式/index/SQLite-refs/Git 协议/libvault。

**最大的两个差距。** ① **整套 AI 原生子系统**——Lore 树中 AI/agent/LLM/MCP 命中数为 **0**，这是 Libra 独有、Lore 完全没有的能力（agents/orchestrator/MCP/sandbox/skills/automation/goal-supervisor/providers/usage/session/prompt/context-budget/TUI `libra code`）。② **Git 式历史改写与检视动词**：`rebase`、`squash`、`stash`、`reflog`、`blame`、`shortlog`、`describe`、`grep`、`tag`、客户端 `hooks`——Lore 命令面均无（`rebase`/`squash`/VFS 本就在 Lore 自己的路线图上）。

**关键洞察。** Lore 现有底座**异常适合**承载 AI 层：① `lore service`/`notification` 的 **UDS IPC 守护进程** = 现成的 MCP/事件传输（Libra 在 Part I 1.11 还要费力从 `libra code` 里抽一个出来，Lore 已经有）；② **partitions/instances/shared-store** = 现成的「每 agent 独立可变状态」隔离（正是 Libra Part I 2.1 多月重构要解决的痛点，Lore 天生具备）；③ **typed metadata + JWT 鉴权 + OTLP + C-ABI** = 现成的溯源/权限/可观测/工具集成基座。所以「把 Libra 的 AI 能力搬到 Lore」是**顺架构**而非嫁接。反过来，`blame` 对 Lore 近乎白送——它内部已织入每文件历史（`NodeFileMetadata.revision[2]`），只差一个动词。

**分阶段计划速览（详见 §P2.5）。**

- **Phase L0 — 速赢**（小、独立、复用 Lore 现有引擎）：`lore tag`、`lore blame`（暴露既有织入历史）、`lore stash`（基于 staged-anchor + dirty-set）、`lore shortlog`/`describe`、`lore grep`、客户端 hook 生命周期（pre-commit/commit-msg/pre-push）。
- **Phase L1 — 历史改写引擎**（对齐 Lore 路线图）：`rebase`、`squash`、interactive rebase、`merge --autostash`、全局 `reflog`（扩展 branch-latest-history）；落在 Lore 的 MergeType 引擎 + staged anchor + 一个 mutable-store 里的 sequencer 上。
- **Phase L2 — AI 基座（头号差距，地基）**：新建 `lore-agent` crate——CompletionModel + provider 适配（anthropic/openai/…）、**MCP over `lore service` UDS**、工具注册表（read_file/apply_patch/shell/grep，落在 lore-revision/lore-storage 上）、sandbox（seatbelt/bwrap）、session 存储（内容寻址 / typed-metadata）、usage 计量（OTLP）。
- **Phase L3 — AI 编排与自治**：orchestrator（plan/decide/execute/verify/replan + gate/ACL）、goal/supervisor/verifier、skills、automation（规则运行时 + 调度器，挂到 `lore-notification` 事件上）、context-budget、permission/intentspec/projection、observed-agent 捕获、prompt 工程、codex 桥。
- **Phase L4 — 交互与分发**：`lore code` 式 agent TUI（复用 lore-client TUI 基建）、AI usage 仪表盘、只读 serverless 发布（`lore publish` 类比）、客户端托管备份。
- **明确不做（§P2.6）。** Git 磁盘格式（loose/pack/index）、Git index/暂存快照模型、SQLite 作为权威 ref 存储、Git smart-HTTP/SSH/LFS 协议、libvault（Lore 已有 keyring + HashiCorp Vault）。理由统一是：**守住 Lore 内容寻址/无-index/分区/API-first 这条立身之本**。

**最重要的一件事（反向）。** ⭐ **`lore-agent` AI 基座（计划项 L2）**：它是 Libra↔Lore 之间最大的、唯一「整类缺失」的能力，而 Lore 的 IPC/partition/metadata/JWT/OTLP 底座恰好为它铺好了路——这是反向补全里价值最高、且最顺架构的一项。

> 如需把整篇文档转为中文，或反过来只保留英文，告诉我即可——这是一次机械的后续操作。

---

# PART I — Lore → Libra (Libra closes its gaps against Lore)

> **Reading guide.** Part I takes **Lore as the reference** and plans what **Libra** must build to reach parity *while staying Git-format-compatible and AI-agent-native*. Its non-goals (§6) are the Lore ideas Libra must **not** copy. The mirror image — **Part II — Libra → Lore** (Libra as the reference; what **Lore** must build, staying content-addressed/API-first) — begins after Appendix A.

## 1. Framing: two architectures, one comparison

**Lore** (Epic Games, Rust, `0.8.4-nightly`) is a centralized, content-agnostic ("binary-first") version control system built to scale on every axis — file count, file size, history depth, branch count, concurrent users, repos-per-backend. Its defining architectural choices are:

- A **decoupled two-subsystem split**: a standalone, partition-based, content-addressed **storage subsystem** (`ImmutableStore`/`MutableStore` traits, BLAKE3 addressing, FastCDC chunking, recursive fragmentation) with the **version-control subsystem** (revisions, branches, merges, sync) layered on top as just another consumer.
- An **API-first** posture where the flat **C ABI** (`lore-capi/lore.h`) is the *primary artifact*; CLI, server, IDE tooling, and per-language SDKs are equal thin clients.
- **No Git-style index** — the filesystem is ground truth; dirty/staged are orthogonal flags on the Merkle tree.
- **Genuine byte-level obliteration**, **per-directory access via partitions/links**, **per-machine layers**, **shared stores/instances**, and an on-demand-hydrating **VFS**.

**Libra** is an **AI agent–native** VCS in Rust that implements a Git client with **full on-disk format compatibility** (loose objects, index, packfiles/pack-index via `git-internal`), while moving refs/HEAD/config/reflog into **SQLite** (`.libra/libra.db`, sea-orm). It adds tiered S3/R2 storage, a D1/R2 backup path, read-only Cloudflare-Worker publishing, and a large AI subsystem (agents, orchestrator, MCP server, sandbox, automation, providers, skills, goal/supervisor, TUI). **This AI substrate has no equivalent in Lore** and is the primary reason many plan items (dirty fast-path, per-worktree isolation, low-level tree construction, local service notifications) are prioritized — they are force-multipliers for concurrent AI agents and human+agent workflows.

### What "fill the gap" means here

- **Adopt the user-facing capability** Lore offers, expressed in a way that is **idiomatic to Libra's Git-format + SQLite + AI-native architecture**.
- Prefer **SQLite side-tables, the existing `Storage` trait, LFS, tiered/cloud storage, hooks, and the MCP/agent surface** as the implementation substrate.
- **AI ergonomics lens.** Several Lore features become dramatically more valuable in an agent-native setting: constant-time dirty status (agents must not pay O(tree) for every decision), per-worktree mutable-state isolation (enables true parallel agent workstreams), local service notifications (watchers and sub-agents subscribe instead of polling), and low-level in-memory tree construction (agents and pipelines operate on structure without forcing checkouts that bloat context and disk). The phased plan explicitly weights these items.

### What it does NOT mean

- It does **not** mean adopting BLAKE3 object addressing, the 320-byte revision state, fixed-size node-blocks, FastCDC fragment chunking, or the partition-as-object-capability model. Those are **fundamentally incompatible with Git on-disk format** — Libra's core promise — and would amount to rebuilding Lore. Where Lore's idea conflicts with Git compatibility, this plan **adapts or defers** it explicitly and says so.

---

## 2. Executive summary

### 2.1 The biggest gaps, grouped into themes

| Theme | What Libra lacks vs Lore | Headline gaps |
|---|---|---|
| **Sparse / VFS / lazy** | No sparse-checkout, no inbound view filter, no lazy on-open hydration, no cross-clone shared store | view filter; view-filtered checkout/sync; object alternates (shared store); hydrating FUSE VFS |
| **Working-copy ergonomics** | No persisted dirty-set / notification path; status always recomputes; worktrees share HEAD/index/refs; no explicit case-change handling | `libra dirty` + dirty-set cache; `status --cached`; **per-worktree HEAD/index/refs isolation**; case-change (`--case=keep|rename|error`) |
| **Conflict UX** | No `resolve --ours/--theirs`, no diff3 markers, no `merge --dry-run`, three drifting sequencers | restore `--ours/--theirs`; diff3 conflict style; merge dry-run; unified sequencer |
| **Branch convenience** | No `branch diff`, no `branch reset` | `branch diff <branch> [<branch>]`, `branch reset <branch> [<revision>]` |
| **Diff/merge depth** | No space-separated `diff A B` or three-dot `A...B` merge-base, no whitespace flags, no diff3 output; merge base-relative preview missing (two-dot `A..B` shipped) | `diff A B`; `A...B`; whitespace flags; `--diff3`; merge-preview |
| **Metadata & provenance** | No typed metadata blob, no branch/repo/file metadata KV, no metadata search | branch/repo metadata KV (incl. `protect`, `archived`); commit trailers + `log --trailer` |
| **Deletion / compliance** | No obliteration of any kind | index-flagged object obliteration (loose + LFS media); two-phase crash-safe state machine |
| **Auth / ops** | No `libra auth`, no token storage, no OTLP telemetry, no shell completions | `libra auth` over vault; OTLP feature; `libra completions` |
| **Global CLI controls** | Missing global `--identity`, `--non-interactive`, resource/concurrency limits, `--search-limit/--search-nearest`, `--gc` | Global flags over `TieredStorage`/command dispatch |
| **Locking enforcement** | LFS locks are push-enforced but not commit/add-enforced; advisory beyond push | extend lock enforcement to commit/add; optional local lock store |
| **Robustness** | No SlowDown/backoff, no fetch-time hash verification, no fsck `--heal` | D1/R2 backoff; verify-on-cache; `fsck --heal` |
| **Composition (links/partitions/forks)** | No links, layers (partial), partitions, forks | **layers** (tractable); links/partitions/forks (largely defer) |
| **Server / protocol / replication** | No server, no custom QUIC/gRPC protocol, no replication/partitions | mostly defer (Libra is a Git client by design); actionable: 429 backoff, token auth, push `--atomic`, worktree isolation |

### 2.2 One-glance priority table

| Priority | Items | Rationale |
|---|---|---|
| **Phase 0 — Quick wins** | `libra completions`; D1/R2 SlowDown backoff; verify-on-cache; `fsck --heal`; `flush(sync_data)`; `exist_batch`; rolling logs; global resource-limit flags; store/cache tunables | Small, additive, Git-format-neutral, immediate robustness/UX value |
| **Phase 1 — Foundational** | dirty-set + `libra dirty` + `status --cached`; restore `--ours/--theirs`; diff3 markers + `merge --dry-run`; positional diff + whitespace flags; typed metadata + branch/repo KV (`protect`/`archive`); `libra auth`; local service/notification v1; OTLP; `merge --autostash`; commit trailer + `log --trailer`; `branch diff`/`reset`; file case-change handling; low-level revision tree (1.15); store tunables (0.10) | High user value, tractable, unblock later work |
| **Phase 2 — Composition & scale** | **per-worktree HEAD/index/refs isolation**; sparse view filter + view-filtered ops; object alternates (shared store); layers; index-flagged obliteration; unified sequencer; interactive auth + OS-keyring credentials | Structural improvements; some large blast radius |
| **Phase 3 — Lore-parity gated** | dependency graph + dependency-filtered clone/sync; LFS FastCDC chunking; hydrating VFS; link/subtree RFC | Real Lore parity items, but gated on Phase 1/2 foundations |
| **Later / Defer** | partitions/forks; C ABI + multi-language SDKs; QUIC/gRPC storage protocol; distributed lock store; hosted multi-tenant server | Architectural conflict with Git format, or needs a hosted Libra server that does not exist |

> **⭐ Keystone structural item — per-worktree HEAD/index/refs isolation (plan item 2.1).** Today Libra worktrees symlink `.libra` and therefore **share HEAD, index, and refs** (`src/command/worktree.rs:671,844`), so two worktrees cannot sit on different branches — a documented footgun and the primary blocker to parallel AI-agent workstreams. Namespacing the mutable state by a per-worktree `instance_id` (objects stay shared) is exactly Git's own worktree model. Widest blast radius, highest structural payoff. Detailed in §4.5; sequenced in §5 Phase 2 (after the cheaper, independent 2.2/2.3).

### 2.3 Current-source merge snapshot (2026-06-18)

This section records what was re-grounded before merging this plan into the existing document.

#### 2026-06-18 Analysis Refresh (Lore 0.8.4 + LEPs + Libra spot-check)

Fresh cross-check against live Lore tree (`/Volumes/Data/EpicGames/lore`), `lore --markdown-help` reference, CLI command modules, the Lore roadmap (`docs/roadmap.md`), and all three LEPs in `docs/proposals/`:

- **LEP 2026-04-27 (Lore Enhancement Proposals)** is a **meta-LEP** that defines the LEP process itself (template, lifecycle, directory). It introduces **no feature or capability** — it is a documentation/governance artifact. No plan item needed; recorded here only to close the LEP inventory.
- **LEP 2026-05-03 (Modified file tracking)** is exactly the dirty-set + `lore dirty` + `--scan`/`--cached` model already captured as plan item 1.1. The proposal's "notify without full scan" and "external integrations mark dirty" goals map 1:1 to `libra dirty` + SQLite `working_dirty` + watcher/agent integration.
- **LEP 2026-05-14 (Low-level memory-based revision control API)** is **not yet explicitly tracked**. It bridges storage (bytes) and FS revision (working tree) with an in-memory node-id tree handle for pipelines/importers that must build or walk revisions without a checkout. Libra's closest surfaces are plumbing commands + MCP tools; this warrants a dedicated tractable item (added below as 1.15).
- Command surface, global args, `auth` (token + `--no-browser` + resource scoping), `service` (UDS IPC loopback), `logfile`, `layer`, `file stage --case`, `file dirty move/copy`, `branch diff/reset`, `repository instance`/`update-path`, and store config tunables (`[store]` max_capacity/eviction/verify_write) are all covered by the existing Phase 0/1 items or explicitly mapped in Appendix A.
- Libra spot-check (2026-06-18 tree): `worktree.rs:671,844` still symlinks `.libra` (confirms 2.1 gap); no `working_dirty` table or `status --cached` (1.1); no `libra completions` (0.1); no dedicated `libra auth` (1.6); no global read-policy flags (0.8); `status` is rich Git porcelain but always full-reconcile; auth surface is basic-interactive only. **One stale claim corrected:** `diff A..B` two-dot positional revspec **now exists** via `normalize_diff_range` (`src/command/diff.rs:211-241`); the earlier "no positional revision args" assertion was true only for space-separated `A B` and three-dot `A...B` merge-base forms. All other plan claims remain accurate.
- Lore roadmap (VFS, scalable locks, links+layers, edge topologies, desktop/web/UE-editor clients, forks) largely maps to Phase 2/3 or "defer (needs hosted server or GUI)". The GUI/editor surfaces (VS Code plugin, desktop client, Unreal Editor plugin, web client) are out of scope for Libra's CLI/agent identity. Libra's unique AI substrate (agents, orchestrator, MCP, sandbox, TUI) has no Lore equivalent and is called out as a differentiator below.

The plan surface is complete w.r.t. Lore's public CLI, the roadmap, and all three LEPs. Two small additions are merged in this refresh (see §5); one stale claim (`diff A..B`) is corrected throughout.

**Lore evidence.**

- Workspace version is `0.8.4-nightly` in `/Volumes/Data/EpicGames/lore/Cargo.toml`; the generated CLI reference is from `lore --markdown-help` (`0.8.2-nightly+31`) and the live `lore-client/src/cli` tree confirms the same command families.
- `lore-client/src/cli/cli.rs` exposes global `--offline`, `--remote`, `--local`, `--sync-data`, resource/concurrency limits, `--gc`, and `--cache`; these map directly to Phase 0 read-policy, flush, cache, and background-maintenance work.
- The actual Lore CLI surface includes `repository metadata`, `repository instance`, `repository update-path`, `branch archive/protect/unprotect/latest/metadata`, `revision metadata/find number/find metadata`, `file metadata/dependency/dirty/obliterate`, `auth`, `layer`, `link`, `lock`, `service`, `notification`, `completions`, and `shared-store`. The earlier plan covered most themes but did not make `service`, `notification`, typed metadata, and dependency-filtered clone/sync explicit enough.
- `lore-capi/lore.h` is a 10,228-line generated C API with `LORE_INTERFACE_VERSION "0.8.4-nightly"`, allocator/thread/logging lifecycle, event callbacks, and operation-specific argument structs. This is a real surface, but still a Libra non-goal unless native SDK consumers appear.
- `lore-proto/proto/lore/storage/v1/storage.proto` confirms a storage RPC contract around `Get`, `GetMetadata`, `Put`, `Query`, `Verify`, `Copy`, `MutableLoad`, `MutableStore`, and `MutableCompareAndSwap`. This supports the plan's CAS/backoff/verify/heal analogs while keeping custom QUIC/gRPC as a non-goal for Libra.

**Libra evidence.**

- `COMPATIBILITY.md` still marks `sparse-checkout` unsupported, `clone --sparse` unsupported, `restore` conflict options unexposed, `push --atomic`/`--signed`/`--push-option`/`--follow-tags` unwired, and `pull --autostash` absent. (The `diff` row was updated 2026-06-18: two-dot `A..B` now works; space-separated `A B`, three-dot `A...B` merge-base, and whitespace/word/binary modes are still missing.)
- `src/command/diff.rs` accepts `--old`, `--new`, `--staged`, pathspecs, algorithm, output, name/stat modes; **two-dot `A..B` positional revspec now exists** via `normalize_diff_range` (`diff.rs:211-241`, invoked at `:204`). Space-separated `diff A B`, three-dot `A...B` merge-base, and whitespace flags (`--ignore-space-at-eol`/`-w`/`-b`) are still absent.
- `src/command/restore.rs` currently exposes pathspec, `--source`, `--worktree`, `--staged`, and pathspec-from-file; no `--ours`/`--theirs` surface exists.
- `src/command/clone.rs` currently exposes `--branch`, `--single-branch`, `--bare`, and `--depth`; no `--reference`, `--shared`, `--dissociate`, `--filter`, `--view`, or sparse materialization exists.
- `src/command/worktree.rs` still creates a `.libra` symlink to shared storage, so per-worktree mutable-state isolation remains the highest-impact structural gap.
- A full command-by-command inventory of `lore-client/src/cli/commands/*` surfaced a handful of gaps not yet in the plan: `branch diff`, `branch reset`, file case-change handling (`FileStageCase`), `shared-store set-use-automatically`, and a richer set of global CLI knobs (`--identity`, `--non-interactive`, `--max-connections`, `--file-count-limit`, `--file-size-limit`, `--compress-limit`, `--max-threads`, `--search-limit`, `--search-nearest`). These are added to §5 as small, tractable items and summarized in Appendix A.
- The 2026-06-18 refresh additionally incorporated (a) LEP 2026-05-14 low-level revision API (new 1.15) and (b) store/cache tunables surface parity (new 0.10), plus explicit UDS modeling for the local service (1.11). Both were cross-checked against live source and the two 2026-05 LEPs.

---

## 3. Capability comparison matrix

**Libra differentiators (no Lore equivalent).** Beyond closing gaps, Libra ships capabilities Lore does not have and is unlikely to grow:

- Full **Git on-disk and wire-format compatibility** (SHA-1/SHA-256 loose+pack+index, smart-HTTP/SSH/LFS interop with the entire existing ecosystem).
- A complete **AI-agent runtime** (orchestrator, goal/supervisor/verifier, MCP server, sandbox with macOS seatbelt + Linux bwrap, skills, automation, TUI `libra code`, usage accounting, prompt engineering, codex bridge, hooks integration).
- **Tiered cloud + managed backup** (local LRU + S3/R2 with threshold, D1/R2 incremental backup, read-only Cloudflare Worker `libra publish` for serverless edge hosting).
- **SQLite transactional substrate** for all mutable state (refs, reflog, config_kv, AI runtime contracts) with rich queryability.
- Vault-backed secret storage and first-class support for concurrent human + agent workflows inside the same repository.

The gap-closure plan deliberately amplifies these strengths (e.g. dirty cache and per-worktree isolation are AI multipliers; `libra publish` is already the read-only edge analog of Lore's tiering).

> These differentiators are exactly the inputs to the reverse direction: **Part II (Libra → Lore)** takes this list — above all the AI-native runtime — and plans how Lore would adopt each one idiomatically to *its* architecture. Read this matrix top-to-bottom for "what Libra must build" (Part I); read the same rows for "what Lore is missing" to seed Part II.

| Domain | Lore model (essence) | Libra status | Gap verdict |
|---|---|---|---|
| Storage & data model | BLAKE3 content-addressed immutable store + cas mutable store; FastCDC chunking; partitions; obliteration | Git loose/pack (SHA-1/256) + `Storage` trait (Local/Remote/Tiered) + D1/R2 | **present-different** core; real gaps in chunking, obliteration, batch/backoff |
| Revisions & history | 320-byte state, Merkle node-blocks, revision number, sync verb | Git commit/tree DAG + rich rev grammar; rebase/cherry-pick/revert/bisect shipped | **present** for most; gaps: revision-number addressing, history-aware merge, path-history index |
| Working copy & staging | No index; dirty/staged flags on tree; notify/scan/verify status | Git index + stat ladder; status `supported`; **worktrees share HEAD/index/refs** | **partial**; key gaps: dirty-set/notify, **worktree isolation** |
| Branching, merge, conflict | One MergeType engine; tree-embedded state; resolve mine/theirs; merge into; protection | Real 3-way merge + 3 SQLite sequencers; Git-format markers; force-with-lease | **partial**; gaps: resolve mine/theirs, diff3, dry-run, branch metadata/protect |
| Composition (links/layers/forks/partitions/instances) | Links, layers, forks, partitions, instances over shared store | Worktrees (shared, non-isolating); tiered storage; publish | **mostly missing**; layers tractable, rest defer |
| File locking | Pluggable LockStore (local + DynamoDB); advisory→enforced roadmap | LFS lock-server flow; **push-enforced (client-cooperative)** | **partial**; ahead on push enforcement (client-cooperative, bypassable by non-Libra clients), behind on commit/add + scale |
| Metadata, dependencies, obliteration | Typed KV blob on 4 scopes; file dependency graph; byte-level obliteration | git notes + config_kv; no dep graph; no obliteration | **missing/partial**; metadata KV + obliteration tractable, dep graph is Phase 3 gated |
| VFS / hydration / sparse | Sparse-by-default view; lazy fragment fetch; ProjFS/SWFS VFS; shared store | Bare clone, shallow, `.libraignore`, tiered LRU; FUSE worktrees | **partial**; gaps: view filter, shared store, hydrating VFS |
| SDK / C-ABI / embeddability / ops | C ABI primary; SDKs; auth/JWT; OTLP; notifications; AWS plugin | CLI + MCP/agent API; vault; tracing logs; tiered/cloud | **missing/partial**; auth/OTLP/completions tractable, C-ABI defers |
| Server, transport & replication | Centralized server; versioned QUIC + gRPC storage protocol; CAS serialization; edge/replica tiering; partitions/JWT | Git client only (smart-HTTP/SSH/LFS); no server/daemon, no quinn/tonic/prost; `libra publish` read-only edge | **mostly defer (architectural)**; actionable: 429 backoff, token auth, push `--atomic`, worktree isolation |

---

## 4. Per-domain deep sections

Each section: **Lore's model → Libra today (with file/command evidence) → concrete gaps → Libra strengths.**

### 4.1 Storage subsystem & data model

**Lore.** Standalone storage API: append-only `ImmutableStore` keyed by 32-byte BLAKE3; narrow `MutableStore` whose `cas` is the *only* serialization point. Partitions (16-byte access boundaries), Context (per-file ID) → 48-byte Address. FastCDC + recursive fragmentation + O(log n) sparse range reads. Per-fragment Zstd. Fragment-level **obliteration** (two-phase, crash-safe). QUIC+gRPC protocol. Replaceable backends (local packfiles, S3+DynamoDB). SlowDown backpressure.

**Libra today.**
- Git on-disk format via `git-internal`; SHA-1 **and** SHA-256 (`src/cli.rs` `core.objectformat` preflight + `set_hash_kind`).
- `Storage` trait (`src/utils/storage/mod.rs:18`) with `LocalStorage`, `RemoteStorage` (object_store S3/R2/Azure/GCP, `src/utils/storage/remote.rs`), `TieredStorage` (size-threshold + LRU disk cache, `src/utils/storage/tiered.rs`).
- D1/R2 backup (`src/command/cloud.rs`, `src/utils/d1_client.rs`) with `object_index` (`o_id/o_type/o_size/repo_id/is_synced`) for incremental sync.
- `fsck`, `verify-pack`, `index-pack`, `gc`, `prune`, `maintenance`.

**Gaps & verdicts.**

| Lore feature | Libra | Verdict / approach |
|---|---|---|
| BLAKE3 addressing | present-different | **Defer** — SHA-256 already gives cryptographic strength; BLAKE3 breaks Git format. |
| cas as sole atomicity primitive | present-different | SQLite txns already serialize; add optimistic CAS (`WHERE old_oid = ?`) only for multi-writer D1. **low** |
| Partitions as access boundary | partial | Repo-as-boundary via `repo_id` + token-scoped R2 prefixes; in-repo partitions conflict with Git OID=capability. **low/defer** |
| Context / file-ID | missing | Optional SQLite file-ID side-table only if scoped-obliterate/move-blame demand it. **defer** |
| FastCDC chunking | missing | **Medium** — only viable *inside the LFS media layer* (chunk → R2 chunk store → manifest pointer), never the Git object graph. Highest-value binary-asset item. |
| Recursive fragmentation / sparse range reads | missing | Defer; only meaningful atop LFS chunking manifests. |
| Per-fragment Zstd | partial | Cloud tier may use Zstd (R2 objects are Libra-private); local stays zlib. **low** |
| `exist_batch` dedup | partial | Add `exist_batch(&[hash])` to `Storage` trait for push/sync pre-checks. **low, easy** |
| SlowDown backpressure | missing | **Medium, easy** — exponential backoff + jitter on 429/503 in `D1Client`/`RemoteStorage`, honor `Retry-After`. |
| Resumable-then-atomic finalize | partial | Tighten: advance D1 ref pointer only after all referenced objects `is_synced=1`; make it conditional (CAS). **medium** |
| Server-side push validation | partial | For cloud path: connectivity walk before pointer advance, reusing `fsck` reachability. **low** |
| Stateless-read scalability / leaderless reads | present-different | **Defer** — horizontal read-scaling behind a `cas` write-bottleneck is a hosted-server concern. Libra is local-first (SQLite single-writer); reads are served from the local store/tier. No hosted Libra cluster exists to scale. |
| Split disk-pack vs per-fragment wire format | present-different | Git already separates on-disk packfiles from smart-protocol thin packs on the wire; Libra inherits both via `git-internal`. Different mechanism (whole-object, not fragment), equivalent transfer outcome. **none** |
| Obliteration | missing | See §4.7. |
| verify/heal | partial | `fsck --heal`: re-fetch corrupt/missing object from remote tier (`restore_indexed_objects_from_remote`). **Medium**. |
| Durable flush flags | partial | `is_synced` already = local-vs-durable; add optional `flush(sync_data)` fsync. **low, easy** |
| repo/branch UUID identity | partial | repo has stable `repo_id`; add optional `branch_id` UUID column to `reference`. **low** |

**Libra strengths.** Git-ecosystem interop (Lore has none); dual SHA support; trait-abstracted swappable object backends; tiered LRU = Lore's composite cache; `is_synced` durable/local distinction; vault-backed secrets/PGP.

### 4.2 Revisions, Merkle tree & history model

**Lore.** 320-byte immutable state blob (`StateData`), fixed-size 96-byte nodes in 49280-byte node-blocks (zero-copy mmap), monotonic revision **number** + `branch@N` addressing, woven per-file history (`NodeFileMetadata.revision[2]`), history-aware streaming diff3, unified `sync` verb, cross-repo merge parent.

**Libra today.** Git commit/tree DAG is the native equivalent: content-addressed, parent-chained, tamper-evident, with free subtree dedup. Rich rev grammar (`rev-parse`, `rev-list` `A..B`/`A...B`/`^`, `~N`, `HEAD`). `log --follow`, grep/author/committer filters, pickaxe `-S/-G`. **Shipped and ahead of Lore:** full cherry-pick sequencer (`src/command/cherry_pick.rs`), revert (`revert.rs`), stateful `bisect` (`src/command/bisect.rs`, SQLite state + `run`), and `rebase` (`src/command/rebase.rs`).

**Gaps.**

| Lore feature | Libra | Approach |
|---|---|---|
| 320-byte state / node-blocks | missing / present-different | **Defer** — Git commit object + packfiles are the idiom; node-blocks break format. |
| Revision number / `branch@N` | missing | **Medium** — derived per-branch first-parent ordinal in SQLite (cache, not in commit), `<branch>@{N}` via rev-parse. Unstable across rewrite — document. |
| History-aware conflict suppression | partial | **Medium** — proper recursive merge with virtual merge-bases (DAG-native) + optional rerere store, not woven per-file flags. |
| Two-revision positional diff | partial | **Easy** — two-dot `A..B` already shipped (`normalize_diff_range`, `diff.rs:211-241`); add space-separated `A B` + three-dot `A...B` merge-base. |
| Unified `sync` verb | present-different | **Low** — keep distinct Git verbs; expose composite move in `libra agent`/`code` layer, not as a default human verb. |
| Find by number/metadata | partial | `find-number` needs ordinal cache; `find-metadata` via commit trailers + `log --trailer`. **medium** |
| Per-file woven history | present-different | **Medium** — SQLite path-history index (rebuildable cache), not tree-embedded. |
| Built-in provenance metadata | partial | Standardize commit **trailers** (`Reviewed-by`, `Cherry-picked-from`, `Change-Request`) + `log --trailer`. **medium** |
| Cross-repo merge parent | missing | **Defer** — subtree-style merge if ever needed; no repo-id field. |
| Low-level in-memory revision API | partial | Promote internal builders or add `commit-tree`/`update-index` plumbing; MCP/apply_patch is the agent path. **low** |

**Strengths.** Full Merkle model; dual-hash tamper-evidence; shipped rebase/bisect/cherry-pick/revert (Lore lists rebase/squash as roadmap); `fsck` = Lore's verify; `find_best_merge_base` = `find_branch_point`.

### 4.3 Working copy, staging model & core CLI workflow

**Lore.** No index — filesystem is ground truth. Dirty/Staged/Action flags on Merkle nodes in one per-instance **staged anchor**. Three reconciliation paths: **notify** (`lore dirty`, the IDE/FSEvents/inotify/VFS integration target — marks dirty *without reading content*), **scan** (`--scan`, O(tree)), **verify** (`--check-dirty`, O(dirty-set)). Staging = path intent; fragments produced at commit. `--targets` bulk ops everywhere.

**Libra today.** Index-based (`src/command/add.rs` `Index::load`/`save`). Stat ladder (size→mtime→hash) already in `Index::is_modified`/`refresh`/`verify_hash`. `status` is `supported` (porcelain v1/v2, `-z`, `--find-renames`, `--column`, `--ahead-behind`) but **always recomputes** worktree↔index↔HEAD. `diff` defaults to index-vs-worktree. `--pathspec-from-file`/`-file-nul` on add/restore/reset. `commit --amend`.

**Gaps.**

| Lore feature | Libra | Approach |
|---|---|---|
| No-index / filesystem ground truth | present-different | **Defer** — removing the index forfeits Git interop. Add `commit -a`-style ergonomics instead. |
| Dirty/Staged flags + staged anchor | present-different | **Medium** — SQLite `working_dirty` table (instance/worktree, path, action) as a cache *alongside* the Git index. |
| Dirty propagation early-out | missing | Parent-path prefix index on the dirty table → O(answer). **low** |
| `lore dirty` (mark without read) | missing | **Medium** — `libra dirty <paths>` (+ `move/copy`) classifies existence-vs-HEAD without hashing; the watcher/agent integration point. |
| Staging as path-intent | present-different | `libra add --intent` defers `blob.save` to commit; keep classic `add` for Git parity. **medium** |
| size→mtime→hash ladder | **present** | Already in git-internal Index; ensure `status` short-circuits too. |
| notify/scan/verify status | partial | **High** — `status --cached` (instant dirty-table read), keep full reconcile as `--scan`, add `--check-dirty`. Constant-time status matters for AI agents. |
| `--targets` everywhere | partial | Extend `--pathspec-from-file` to status/diff/grep. **easy** |
| Case-change handling (`stage --case=keep|rename|error`) | missing | **Low–Medium** — add `--case` to `add`/`mv`/restore paths; default `error` for Git compatibility. Plan 1.14. |
| Per-instance staged state / multi-worktree | **present-different (key gap)** | **High** — see §4.5; worktrees currently share HEAD/index/refs (`src/command/worktree.rs:671,844` symlinked `.libra`). |
| VFS-driven dirty | missing | **Defer** — needs `libra dirty` + sparse + FUSE hydration. |

**Strengths.** `status supported` (exceeds Lore's Git interop); stat ladder present; rename **detection** (Lore only records intent); atomic index save (`AddError::IndexSave` write-then-rename); `log --follow`; `hash-object`/`cat-file` = Lore's `file hash`/`file write`; `fsck`/`verify-pack` = `repository verify`.

### 4.4 Branching, merge & conflict sequencers

**Lore.** Branch = named pointer with stable 16-byte `BranchId`; lineage **STACK**; one MergeType-parameterized engine (merge/cherry-pick/revert); state encoded in the tree (StateFlags + per-node merge flags); resolve / resolve mine / resolve theirs / unresolve / restart / abort; diff3 markers + `.mine/.theirs/.base` sidecars; link-aware composition; `merge into`; `merge_carry`; metadata-bit branch protection (server-enforced); branch metadata KV; archive.

**Libra today.** Real 3-way merge with line-level auto-merge (`src/command/merge.rs` `resolve_three_way` + `try_merge_blob_contents` via `diffy::merge_bytes`); Git-format conflict markers (`write_conflict_markers`) **and** index conflict stages 1/2/3 (`add_blob_index_entry`/`entry.flags.stage`). Three resolution stores, two backends: cherry-pick (`cherry_pick_state` SQLite table), revert (`revert_sequence` SQLite table), merge (`merge-state.json`, a serde_json file holding `base`/`target`/`target_ref`/`orig_head`/`head_name`/`conflicted_paths`). merge/cherry-pick/rebase mutex. `merge --squash` = `--no-commit`. Reflog in SQLite. Upstream/tracking, contains-filters, create/delete/rename.

**Gaps.**

| Lore feature | Libra | Approach |
|---|---|---|
| Stable BranchId | missing | `branch_id BLOB(16)` column; survive rename. **low** |
| Lineage stack | missing | Substitute with `find_merge_base` (DAG) + optional `branch_lineage` provenance table. **low** |
| Unified MergeType engine | partial | **Medium** — consolidate 3 stores into one `SequenceState` table + `op_kind`; first migrate merge off its state-file. Maintainability, limited new UX. |
| Tree-embedded conflict state | present-different | **Defer** — SQLite + index stages 1/2/3 is the Git-idiomatic crash-safe equivalent. |
| merge start/restart/abort | partial | **Low–Medium** — `MergeState` (`merge.rs:135-142`, `merge-state.json`) **already persists** `base`/`target`/`target_ref`/`orig_head`/`head_name`. `--restart` re-derives the 3-way from those commit OIDs — **no schema/state-file extension needed**. Add `merge --dry-run` (compute resolution, no FS write). |
| resolve mine/theirs/unresolve | partial | **High value, Low–Medium effort** — `restore --ours/--theirs` (`-2`/`-3`) reading index stages. **Precondition VERIFIED**: both merge (`add_blob_index_entry` → `entry.flags.stage = stage`, `merge.rs:1183-1204`; `staged_conflict_paths` reads stages 1..=3, `merge.rs:1445`) and cherry-pick (`add_conflict_stage_entry` "1=base/2=ours/3=theirs", `cherry_pick.rs:1267-1291`) populate index conflict stages today — so the stages exist to read. Highest user-value gap here, and de-risked. |
| diff3 markers + sidecars | partial | **High** — per-hunk diff3 (`\|\|\|\|\|\|\| base`) via `similar`/`diffy`; optional `merge.conflictStyle`/sidecars; binary whole-file path. |
| Link-aware merge | missing | **Defer** — needs links/partitions (foreign to Git). |
| `merge into` | present-different | `merge --into <branch>` sugar using `find_merge_base` ancestry check. **low** |
| `merge_carry` | missing | **Medium** — `merge --autostash` reusing `src/command/stash.rs` (Git idiom). |
| Branch protect/unprotect | missing | **Medium** — `protected` bit in branch metadata table; enforce on delete/push + pre-receive hook. |
| Branch archive | missing | `archived` bit + `branch list --archived`. **low** |
| Branch metadata KV | missing | **Medium (keystone)** — `branch_metadata` table backs protect/archive/identity/lineage as reserved keys. |
| `branch diff` | missing | **Low** — `libra branch diff [<a>] [<b>]`, defaulting to `HEAD..<other>`; reuse `DiffArgs`. Plan 1.12. |
| `branch reset` | missing | **Low** — reset a branch tip to an arbitrary revision without touching the worktree; guard protected branches. Plan 1.13. |

**Strengths.** Real textual auto-merge (Lore frames diff3 as its path); Git-format markers (universal tool interop); crash-safe SQLite sequencers; cherry-pick/revert near-parity *and ahead* (rich flags); reflog = Lore's branch-latest-history; AI-orchestrator workspace merge has no Lore equivalent.

### 4.5 Composition: links, layers, forks, partitions, instances

**Lore.** Partition = access boundary (repo == partition). Links = pinned in-revision sub-repo references (travel with clone, path remap, per-link ACL, link-scoped commit/merge, auto-follow). Layers = local per-machine overlays (TOML in `.lore/`, never committed, metadata/branch revision selection). Instances = per-working-dir UUIDv7 identity with independent staged state over a shared store; **no privileged main repo**. Forks = COW partitions (roadmap).

**Libra today.** Worktrees (`src/command/worktree.rs`) symlink `.libra` to shared storage → **share HEAD/index/refs** (documented gotcha; *not* isolation). Tiered storage + D1 `(repo_id, o_id)` = repo isolation at the backup layer. `submodule` intentionally absent (product boundary, `COMPATIBILITY.md`). `publish` does read-only cross-repo composition.

**Gaps.**

| Lore feature | Libra | Approach |
|---|---|---|
| Partition access boundary | missing | **Defer** — Git OID=capability; do repo-as-boundary + storage-fetch authz instead. |
| Links (in-revision sub-repo) | missing | **Defer** — reverses the declined-submodule boundary; needs an RFC. If pursued, gitlink (mode 160000) + SQLite `link` table. |
| Transparent link traversal | missing | Defer (blocked on links; ~8-command ripple). |
| Link pin/auto-follow/merge | missing | Defer (blocked on links). |
| Per-link ACL | missing | Defer (blocked on links + authz). |
| **Layers** | missing | **Medium (tractable entry point)** — local-only TOML `LayerConfig` under `.libra/`, materialize source subtree into `target_path`, manifest for `--purge`. No commit involvement → sidesteps Git-format conflicts. |
| Layer revision selection | missing | Branch auto-follow on `switch` first; metadata-match later. **low** |
| Per-layer staging/commit | missing | Sub-clone-per-layer (Libra's shared-index worktrees make overlay indexes awkward). **low** |
| Instances (per-dir state) | present-different (key gap) | **Medium** — see below. |
| Symmetric multi-worktree | partial | Depends on instance isolation + standalone shared store. **low** |
| Instance list/prune/update-path | partial | `worktree list/prune/move` largely cover it; add stale detection + JSON. **easy/low** |
| Shared store (cross-clone) | partial | **High** — Git object **alternates**: `clone --reference/--shared/--dissociate` + `shared-store create/info`. Recover vanished prior impl via `libra show <commit>`. |
| `shared-store set-use-automatically` | missing | **Low** — persist default shared-store path in global config/vault; `clone` consults it. Plan 2.11. |
| Forks (COW partition) | missing | **Defer** — promisor/partial clone is the Git-idiomatic analog; roadmap even in Lore. |

**The per-worktree isolation fix (cross-cutting, Phase 2).** Add a per-worktree identity and namespace mutable state by it: `instance_id` column on `reference`/index/HEAD (sea-orm migration), so HEAD/current-branch/staged-index become per-worktree while objects stay shared. This is **Git's own worktree model** (shared objects, per-worktree HEAD/index) and removes the documented footgun. High blast radius (every ref/HEAD/index resolver), but it is the single most valuable structural improvement and unblocks parallel agent workstreams.

**Strengths.** Git object-format dedup (free at object granularity); SQLite substrate ideal for `link`/`layer`/`instance` side-tables; tiered storage already gives shared-backend "pay once"; `publish` cross-repo composition; AI orchestration layer to hang composition policy.

### 4.6 File locking

**Lore.** Pluggable `LockStore` (in-memory `LocalLockStore` + DynamoDB with transactional CAS acquire + 4 GSIs); branch-scoped `LockResource` (BLAKE3(path)+BranchId); all-or-nothing batched acquire with rollback + SlowDown; AdminLock / force-unlock; notifications; C-ABI surface. **Advisory today**, enforced + cross-branch-scalable on roadmap.

**Libra today.** Full Git-LFS-protocol lock client: `libra lfs lock/unlock [--force --id]`, `libra lfs locks` (`src/command/lfs.rs:69-93,170-301`). Branch-scoped via LFS `refspec`. **Push-enforced**: `LFSClient::push` → `verify_locks` hard-fails on others' locks (`src/internal/protocol/lfs_client.rs:201-278`) — *ahead of Lore's current advisory model.* Stable JSON output + error codes.

**Gaps.**

| Lore feature | Libra | Approach |
|---|---|---|
| Multi-path acquire / all-or-nothing | partial | Multi-path `lfs lock` + `--branch`; compensating unlocks on failure (best-effort). **medium** |
| Multi-path status | partial | `lfs locks --paths ...` per-path table. **medium** |
| Query by owner / cross-branch | partial | `--owner`, `--branch`/`--all-branches` (iterate refs from SQLite). **medium** |
| Bulk release / `--owner` | partial | `unlock --force` with no path = release all for (refspec, owner). **medium** |
| Branch-scoped key model | present-different | refspec+path is the Git-LFS idiom; no BLAKE3. **low** |
| Pluggable LockStore (server) | missing | **Defer** — server concern; SQLite `lock` table if Libra hosts one. |
| Local in-memory store | missing | Optional SQLite-backed local lock store for offline solo/agent coordination. **low** |
| DynamoDB scalable store | missing | **Defer** — D1 conditional INSERT / Durable Objects if hosted. |
| AdminLock (on-behalf-of) | partial | Force-unlock done; on-behalf-of needs non-standard server endpoint → document decline. **low** |
| Notifications | missing | **Defer** — surface over `libra code` SSE if hosted; else `locks --watch` polling. |
| C-ABI surface | missing | **Defer** — MCP lock tool + JSON CLI is the idiomatic multi-consumer surface. |
| Advisory→enforced | partial | **High** — extend push enforcement to **commit/add** (`lfs.lockEnforce=warn\|block` via `get_locks`), and allow locking any path. |

**Strengths.** Push enforcement (exceeds Lore's advisory); standard Git-LFS interop (works with any LFS lock server); correct refspec branch-scoping; `--force`/`--id` release; stable typed output; tiered storage as the durable asset substrate.

### 4.7 Metadata system, file dependencies & obliteration

**Lore.** One typed KV metadata blob (magic `meta`, 7 value types, 1 MiB cap) on repo/branch/revision/file; immutable when hash-referenced from state, mutable when cas-pointed. File **dependency graph** (`dependencies`/`dependents`, tagged edges, cycle detection, transitive closure, dependency-driven selective clone). **Obliteration**: byte-erase payload while keeping the address; typed-absence read; context-scoped; two-phase crash-safe; recursive sub-fragment with sharing detection; authorized distributed.

**Libra today.** git `notes` (`src/command/notes.rs`, `2026061401_notes.sql`) — mutable free-text annotation. `config_kv` — dotted-key KV with optional vault encryption. No typed metadata blob, no per-file metadata, no dependency graph, **no obliteration** (`prune`/`gc` only remove *unreachable* objects). `object_index` rows survive independently of payloads (natural obliteration-flag host). Note: `TieredStorage` `CachedFile::drop` (`src/utils/storage/tiered.rs:42-48`) unlinks only the **local LRU cache copy** of a large object on eviction (`// for "Cache", it's ephemeral`) — it does **not** delete the durable R2/loose payload, so it is *not* an obliteration head-start; it only demonstrates the local-unlink mechanic.

**Gaps.**

| Lore feature | Libra | Approach |
|---|---|---|
| Typed KV metadata blob | missing | **Medium** — typed blob (canonical-JSON-with-type-tags) as a Git blob; `src/internal/metadata.rs` accessors. No context/BLAKE3. |
| Immutable-vs-mutable attachment | partial | **Low** — immutable = hash-referenced blob/notes-tree; mutable = SQLite pointer (txn = cas). Per-file inline conflicts with Git tree → path-keyed side-tree. |
| Repo metadata get/set/clear | partial | `libra metadata repository ...` over `config_kv` reserved prefix + type tags. **easy/low** |
| Branch metadata get/set/clear | missing | **Medium** — `branch_metadata` table; wire reserved `protect`. |
| Revision metadata (staged-immutable) | partial | **Medium** — reserved commit trailers at create time + typed notes. |
| File/dir metadata | missing | **Low** — path-keyed committed side-tree (Git format forbids inline node metadata). |
| Metadata search | missing | `log --metadata <key>` (notes-backed SQL query / trailer walk). **medium** |
| File dependency graph | missing | **Phase 3 / gated** — large subsystem on path-keyed side-tree; only start after typed metadata + sparse/shared-store foundations. |
| Cycle detection / transitive closure | missing | **Phase 3 / gated** — included in dependency graph once 3.1 starts. |
| Dependency-driven clone | missing | **Phase 3 / gated** — needs dep graph plus sparse/alternates materialization semantics (3.2). |
| Context obliteration scope | missing | **Defer** — Git content-addressing = shared blob; cannot spare a co-content file. Scope via `object_index` reachability instead, documenting the limit. |
| Payload obliteration (keep address) | missing | **Medium** — `obliterated` state on `object_index` + storage; read path returns typed `ObliteratedObject`; physically remove loose/pack/R2 payload. Be precise: the result is **graph-resolvable but content-unfetchable** — the commit/tree graph *walk* still works (parent/tree pointers intact), but any content *fetch* of the obliterated OID returns the typed error. `fsck`/`fsck --heal` (0.4) MUST learn the obliterated state, or it will report the erased object as corruption and try to re-heal deliberately-deleted bytes from the remote tier. |
| Two-phase crash-safe machine | missing | **Medium** — SQLite `Obliterating`/`Obliterated` state + `doctor` recovery sweep (reuse sequencer pattern). |
| Recursive sub-fragment + sharing | missing | **Low** — collapses to one payload (no chunking); packed-object obliteration needs repack (gc doesn't repack today). |
| CLI obliterate by path/address | missing | **Medium** — `libra obliterate (--object\|--path)`, idempotent, stage delete, confirmation-gated. |
| Distributed authorized obliterate | missing | **Low** — parallel local + D1/R2 delete; `obliterate` permission via vault; hooks. |
| Documented obliteration limits | missing | **Low** — doc page incl. Git-format-specific caveats (shared-content blobs, repack, real-git readers hard-fail). |

**Strengths.** SQLite ACID metadata substrate; **notes shipped**; `config_kv` with per-row vault encryption; dual-hash; `object_index` survives independently of payloads (a clean place to flag obliterated state); vault for permission-gating; reachability tracing in gc/prune. (Note: the local-cache unlink in `TieredStorage` is *not* durable-payload deletion — see "Libra today" above — so it does not reduce obliteration effort.)

### 4.8 VFS, on-demand hydration & sparse workspaces

**Lore.** Sparse-**by-default**: glob `.lore/view` inbound filter + `.loreignore` + explicit `FilterMode`. Lazy fragment fetch (offset-indexed range reads). Local store doubles as LRU fragment cache. Shared store dedups across clones. True VFS: Windows ProjFS provider, cross-platform SWFS, `clone --virtual`, `--prefetch` warming.

**Libra today.** `clone --bare` (zero materialization). `--depth`/`--single-branch`. `.libraignore` with `IgnorePolicy` (Respect/IncludeIgnored/OnlyIgnored). `TieredStorage` LRU disk cache with budget + eviction-on-insert. Worktrees share one object store via symlink (intra-repo dedup). **No sparse-checkout** (`COMPATIBILITY.md` unsupported; `add/mv --sparse` no-ops). FUSE worktrees (`worktree-fuse` feature, `rfuse3`/`libfuse-fs`) = mount lifecycle, *not* lazy hydration.

**Gaps.**

| Lore feature | Libra | Approach |
|---|---|---|
| Sparse-by-default view filter | missing | **High** — `.libra/view` glob (reuse `src/utils/ignore.rs` matcher); thread into checkout/restore/switch write paths; per-worktree materialized-set in SQLite; `libra sparse-checkout set/add/list/disable`, `clone --view`. Hardest part: status/diff/add treating in-tree-but-unmaterialized as sparse, not deleted. |
| FilterMode split | partial | **Medium** — formalize CommittedState vs LocalState composing view + `.libraignore`. |
| Lazy fragment range fetch | missing | **Defer** — whole-blob Git format precludes sub-object range reads; LFS Range requests are the safe subset. |
| LRU fragment cache + evictor | partial | **Medium** — add background interval evictor; surface cache config. |
| Shared store (cross-clone) | partial | **High** — object alternates (`--reference/--shared/--dissociate` + `shared-store create/info`); gc must be alternates-aware. |
| `clone --virtual` hydrating VFS | partial | **Phase 3 / gated** — extend FUSE worktree to fault in whole objects from `TieredStorage` on open; gate behind `vfs`. |
| Windows ProjFS provider | missing | **Defer** — large native driver, Windows-only, narrow audience. |
| SWFS backend | missing | **Defer/decline** — proprietary Epic dep; FUSE is Libra's cross-platform story. |
| `--prefetch` warming | missing | **Phase 3 / gated** — trivial once a hydration substrate exists. |
| Bare clone | **present** | Done (`src/command/clone.rs:107-109`). |
| Dependency-based selective clone | missing | **Phase 3 / gated** — needs dependency graph plus sparse/alternates materialization semantics. |
| Remote/local/offline source control | missing | **Medium, easy** — global `--offline/--local/--remote` read-policy flags over `TieredStorage` get. |
| View-filtered committed ops | missing | **High** — thread view into all committed-state writers + `sparse-checkout reapply`. |
| Transparent server tiering | partial | **Low** — configure R2/Worker edge/warm/cold; little Rust. |
| Fragment query / cache inspection | partial | `cloud status <oid>` / `fetch --warm <rev>` over `object_index` + LRU. **low** |

**Strengths.** Real tiered LRU disk cache with eviction (= Lore's cache-with-budget at object granularity); object_store multi-cloud backend; cross-worktree disk dedup via symlink; bare clone done; `.libraignore` with tracked-vs-untracked protection; FUSE mount plumbing as a head start.

### 4.9 SDK / C-API / embeddability & ops (auth, credentials, telemetry, notifications, cloud)

**Lore.** C ABI (`lore-capi/lore.h`, ~10k lines) is the primary artifact; uniform global-args + typed-event callback model; sync+async per op; allocator/thread/lifecycle controls; structured logging; standalone storage SDK; embedded server; full auth (token/interactive/info/list/logout/clear); OS-keyring credentials with JWT domain-leak guard; OTLP telemetry; streaming notifications; AWS/Consul/Nomad plugins; chaos harness; versioned specs + semver forward-read; 6-language SDKs; shell completions.

**Libra today.** CLI-first + MCP/agent API. Tiered cloud storage + D1/R2 backup. `libra code` runs an in-process axum web + MCP server. Broadcast event stream (AI sessions only, `src/internal/ai/web/code_ui.rs:469-514`). libvault (AES-256-GCM, unseal key outside repo). tracing-subscriber console/file logs (`src/main.rs:90-160`). Remote auth = **interactive basic auth only** (`ask_basic_auth`, `src/command/mod.rs:189`).

**Gaps.**

| Lore feature | Libra | Approach |
|---|---|---|
| C ABI primary artifact | missing | **Defer** — Libra is process/agent-API-first; harden JSON CLI + MCP as the contract. Optional `libra-core` + thin `libra-capi` only if a native consumer appears. |
| Uniform args + event callback | partial | Generalize `CodeUiEvent` into a `--json --stream` NDJSON event taxonomy across handlers. **low** |
| Global CLI controls (`--identity`, `--non-interactive`, resource/concurrency limits, `--search-limit/--search-nearest`, `--gc`) | missing | **Low–Medium** — add global clap args and thread through dispatch context. Plan 0.9. |
| Sync+async dual entry | partial | Only meaningful with `libra-core`. **defer** |
| Allocator/thread/lifecycle | missing | Tractable subset: `LIBRA_MAX_THREADS`, `version()`/`user_directory()`; allocator/shutdown defer. **low** |
| Structured logging config | partial | **Medium, easy** — `tracing-appender` rolling files + size/count caps. |
| Standalone storage SDK | partial | Promote `ClientStorage`/`Storage` to `libra-core`; add in-memory backend + batch. **low** |
| Embedded server start/stop | partial | **Medium** — hoist `libra code` bootstrap into `start_server()/stop()` (HTTP+MCP, not gRPC). |
| `libra auth` (login/info/list/logout/clear) | missing | **High** — `libra auth` over libvault, per-domain tokens, credential-helper lookup replacing `ask_basic_auth`. |
| OS-keyring credentials | partial | **Medium** — `keyring` crate as optional vault-unseal/token backend with file fallback. |
| JWT domain-leak guard | missing | **Medium** — once auth exists, scope tokens by host before attaching `Authorization`. |
| Streaming repo notifications | missing | **Medium (local v1)** — extract a loopback-only event bus from `libra code` into `libra service` for status/dirty/cache notifications; hosted/networked pub-sub still defers. |
| OTLP telemetry | missing | **Medium** — optional `tracing-opentelemetry` + `opentelemetry-otlp` feature; command-latency/storage metrics; tower layer. |
| AWS plugin (S3+DynamoDB) | partial | S3 + SQLite/D1 index present; distributed CAS lock store **defers** (SQLite single-writer model). |
| Consul/Nomad discovery | missing | **Defer** — no multi-node Libra cluster. |
| Compiled-in plugin safety | partial | Document `Storage` trait as the backend spec; hard-fail unknown backend. **low** |
| Chaos/fault-injection harness | missing | **Low** — seedable weighted op driver (test binary) reusing test harness. |
| Repo instance/metadata ops | partial | Surface `config_kv`; `worktree list/prune` + worktree-repair path-update. **low** |
| Versioned specs + semver | partial | **Medium** — document Git-format forward-read, `Storage` trait spec, protocol versioning, `LIBRA_INTERFACE_VERSION`. |
| Multi-language SDKs | missing | **Defer** — MCP + JSON CLI is the cross-language surface. |
| Shell completions | missing | **High, easy** — `clap_complete` + `libra completions <shell>` (+ EXAMPLES const, docs page, compat-guard row per CLAUDE.md). |
| Forks (roadmap) | missing | **Defer** — server-side fork model needs auth + hosted server. |
| VFS (roadmap) | partial | See §4.8. |

**Strengths.** Tiered multi-cloud storage + managed D1/R2 backup (Lore has no managed backup); in-process `libra code` web+MCP server; live broadcast event stream; vault (exceeds plain TOML); configurable tracing logs; **AI-native MCP/agent API Lore lacks entirely**; per-call/per-repo identity cascade.

### 4.10 Server, wire protocol, transport & replication

**Lore.** A true **centralized client-server** system where the *same library* runs on both sides: the `loreserver` binary (`lore-server`) terminates the network protocol, owns the canonical latest-pointer, durability, and access control. The storage subsystem is exposed as one small command set (Authorize; Get/Put/Query/Verify/Copy; MutableLoad/Store/**CAS**) over a **versioned dual transport** — a binary **QUIC** protocol (`lore-transport`, quinn, ALPN `lore-storage/0.4`, 12-byte session-bearing headers, 8 multiplexed pipelined streams) **and** **gRPC/HTTP2** (`lore-proto`, prost: Storage/Revision/Repository/Lock/Admin/Notification/Replication services). It is "centralized in role, offline-capable for editing"; **all** consistency concentrates in the mutable-store **compare-and-swap** — the single serialization point ("CAS bottleneck", per-branch granularity). Production tiers storage hot/warm/cold with **edge servers + `ReplicatedStore` read replicas + intra-region peer replication/failover**; back-pressure is an explicit **`SlowDown`** signal (→ gRPC ResourceExhausted) with client exponential backoff; multi-tenancy is rooted in **partitions** behind JWT/JWK sessions with a "knows-the-hash" side-channel defense; plus server-side FF-merge, distributed locks, notification pub-sub, presigned-URL reads, pluggable backends, server hooks, mTLS internal endpoints, and full OpenTelemetry.

**Libra today.** **Purely a Git client.** No server/daemon/serve mode; **zero quinn/tonic/prost dependencies** — no custom QUIC/gRPC protocol; no server-to-server replication; no partitions or server-side JWT verification. It speaks Git **smart-HTTP/SSH/git://** and **LFS** to existing servers via a `ProtocolClient` trait (`src/internal/protocol/{https_client,ssh_client,git_client,lfs_client,local_client}.rs`). Its only "server" surfaces are the **loopback** `libra code` axum web + MCP dev server and the **read-only** `libra publish` Cloudflare Worker (D1/R2). Push uses Git receive-pack with `--force-with-lease` (the lease ≈ Lore's conditional-put). SQLite ref updates with busy-retry backoff (`src/internal/branch.rs` `SQLITE_BUSY_MAX_RETRIES`) are the *local* analog of Lore's CAS serialization point.

**Gaps.**

| Lore feature | Libra | Verdict / approach |
|---|---|---|
| Centralized server binary | missing | **Defer (architectural)** — replicating Lore's bespoke server would fork Libra off the Git ecosystem. If a server is ever wanted, implement Git's **smart-HTTP backend** (`info/refs` + `git-upload-pack`/`git-receive-pack`) over axum reusing Libra's object store + SQLite refs — *not* a custom protocol. Or rely on existing Git hosts (status quo). |
| Custom QUIC wire protocol | missing | **Defer** — bespoke `lore-storage/0.4` over quinn breaks Git interop. Prefer Git protocol v2 over HTTP/2 if throughput ever demands it. |
| gRPC service surface (prost/tonic) | missing | **Defer** — Libra's AI-native analog is the **MCP** surface (`src/internal/ai/mcp/`) + `--json`/`--machine` CLI; extend MCP resources, not gRPC. |
| Two-phase push (fragments → CAS advance) | present-different | **Medium** — Git ref-update + lease is the analog. Close the documented protocol-layer push gaps (`--atomic`/`--signed`/`--push-option`/`--follow-tags`) by extending `src/internal/protocol` receive-pack. See plan 2.10. |
| Per-fragment resumable/parallel transfer | partial | **Low** — Git packs aren't per-object-resumable; lean on partial clone/`--depth` (present) + LFS for large-object resumability. |
| Hot/warm/cold + edge tiering | partial | **Low** — Libra has client-side local/S3-R2 tiering + LRU (`tiered.rs`); a true *edge tier* presupposes a server. `libra publish` (R2 + CF edge) is the read-distribution analog. |
| ReplicatedStore / read replicas / failover | missing | **Defer** — for a client, "replication" = the object store's own multi-AZ/region durability (S3/R2) + the D1/R2 backup path. No server ⇒ N/A. |
| Server topology / peer discovery | missing | **Defer** — N/A without a server fleet; Git multi-remote is the closest concept. |
| Back-pressure / `SlowDown` | partial | **Medium, easy** — add HTTP **429/Retry-After** + S3/R2 throttling backoff (object_store surfaces it) to `https_client.rs`/`remote.rs`. Client-side resilience win, server-independent. Folded into plan 0.2. |
| Multi-tenant partitions + knows-the-hash | missing | **Defer** — only matters for a hosted shared backend; keep per-repo isolation. The publish path's site-scoped R2 keys + Worker auth cover its read-only case. |
| JWT/JWK auth across transports + REBAC | partial | **Medium** — client side: add **bearer/token auth** to `https_client.rs` (many Git hosts use it); folded into plan 1.6. Server-side JWKS verification needs a server ⇒ defer. |
| Distributed lock service (host one) | partial | **Low** — Libra is an LFS-lock *client* already (idiomatic); hosting locks needs a server ⇒ defer. See §4.6. |
| Notification pub-sub (networked) | partial | **Low** — local event stream exists over `libra code`/MCP; a networked subscribe API needs a server. Webhooks via the publish Worker could cover external reactivity. |
| Presigned-URL direct reads | partial | **Low** — use S3/R2 presign from object_store if a direct handoff is wanted; publish Worker already serves reads from R2 at the edge. |
| Read-only edge hosting / browser | present-different | **None** — `libra publish` (read-only CF Worker over D1/R2) *is* this, done serverless. A Libra strength. |
| Replaceable storage backends | present-different | **None** — `src/utils/storage/` over object_store (S3/R2/Azure/GCP) + SQLite mutable store = same outcome, different shape. |
| Server-side hooks (push/branch veto) | partial | **Low** — client + AI-lifecycle hooks exist; server-side veto belongs to the Git host ⇒ defer. |
| Shared store + symmetric instances | present-different (key gap) | **Medium** — worktrees already share one `.libra` store (closer to Lore's model than Git's main/linked split) but share HEAD/index/refs; per-instance isolation = plan **2.1**. |
| Local per-repo service daemon | partial | **Medium** — `libra code` is a richer interactive substrate, but Lore exposes headless `service run/start/stop`; extract a loopback-only `libra service` as plan 1.11 instead of building a hosted server. |

**Strengths.** Git-format wire interop (works with the entire existing Git server ecosystem — Lore's bespoke protocol works with nothing else); serverless read-only **edge hosting via `libra publish`** (the analog of Lore's edge read tier, zero-ops); tiered multi-cloud client storage + LRU; cloud durability via object-store replication (no replica machinery to run); **MCP/`libra code`/`--json` as the AI-native analog of Lore's thin-client gRPC compute**, scoped safely to loopback; shared-store worktrees suited to AI agents; SQLite transactional refs with busy-retry backoff as the local CAS analog. The missing piece is a headless, non-TUI service facade, not a new remote protocol.

---

## 5. Prioritized, phased completion plan

Each item: **what · why · feasibility · effort · dependencies · Libra-idiomatic approach.**

> **New-command contract (applies to every new visible command below** — `libra dirty`, `libra auth`, `libra obliterate`, `libra sparse-checkout`, `libra layer`, `libra shared-store`, `libra completions`, `branch metadata`, …**).** Per `CLAUDE.md` and the `tests/compat/` guards, each one must ship: a `<CMD>_EXAMPLES` const wired via `#[command(after_help = …)]`; a `docs/commands/<name>.md` page with an Examples / Common Commands heading; a Command-Groups row in `src/cli.rs` (`root_after_help_lists_every_visible_command`); a `tests/compat/` guard row (registered as `[[test]]` in `Cargo.toml` + a row in `tests/compat/README.md`); at least one end-to-end test plus a focused unit test; and — for any new `StableErrorCode` — a `docs/error-codes.md` entry (the `compat_error_codes_doc_sync` guard fails otherwise). Plus a `COMPATIBILITY.md` update when the Git surface changes. Budget ~1 extra day per new command for this contract; the effort cells below do **not** include it.

### Phase 0 — Quick wins (additive, Git-format-neutral, days each)

| # | Item | Why | Feasibility | Effort | Deps | Approach |
|---|---|---|---|---|---|---|
| 0.1 | **`libra completions <shell>`** | Standard ergonomics; satisfies help/compat guards | easy | 1–2 d | — | `clap_complete::generate` over the built Command tree; add EXAMPLES const + `docs/commands/completions.md` + Command-Groups row + compat guard. |
| 0.2 | **429/Retry-After backoff (cloud + Git HTTPS)** | Robustness under throttling on both the cloud tier and Git smart-HTTP (Lore's `SlowDown` analog, client-side) | easy | 2–3 d | — | Exponential backoff + jitter on 429/503 in `D1Client`/`RemoteStorage` **and** `src/internal/protocol/https_client.rs`; honor `Retry-After`; bounded retries with a clear final error. |
| 0.3 | **Verify-on-cache** | "No blind trust in remote" inline, not only at fsck | moderate | 3–5 d | — | In `client_storage`/`tiered` fetch path, assert `o_id == hash(bytes)` (matching `core.objectformat`) before writing to local cache. |
| 0.4 | **`fsck --heal`** | Recover corrupt/missing objects from durable tier | moderate | 1–2 w | cloud restore | Conservative re-fetch by OID from `RemoteStorage`/D1 (`restore_indexed_objects_from_remote`); never fabricate; re-verify. |
| 0.5 | **`flush(sync_data)` + `--sync-data`** | Media-durability guarantee on demand | easy | 2–4 d | — | Optional fsync of loose object + parent dir in `LocalStorage::put`; opt-in flag on commit/gc. |
| 0.6 | **`exist_batch(&[hash])`** | Faster dedup pre-check on push/sync | easy | 2–3 d | — | Additive `Storage` trait method; batch against remote tier. |
| 0.7 | **Rolling logs + `logfile info`** | Production log hygiene and discoverability | easy | 1–2 d | — | `tracing-appender` daily rotation + size/count caps behind `LIBRA_LOG_ROTATION`; define precedence vs `LIBRA_LOG_FILE`; add `libra logfile info` or fold into `libra logs info` if command naming stays Git-lean. |
| 0.8 | **`--offline/--local/--remote`** | User control over lazy-fetch source | easy | 3–5 d | — | Global flags setting a read policy on `TieredStorage` get; `--offline`/`--local` fail-fast on cache miss with clear error. |
| 0.9 | **Global resource/concurrency knobs (`--max-connections`, `--file-count-limit`, `--file-size-limit`, `--compress-limit`, `--max-threads`, `--search-limit`, `--search-nearest`, `--non-interactive`)** | Scripting/CI parity and safety; prevent runaway resource use on huge repos | easy | 3–5 d | — | Add global clap args; thread through `LoreGlobalArgs`-equivalent dispatch context; `--non-interactive` disables all prompts; numeric limits clamp parallel I/O/compression/thread pools. Keep `--identity` for auth v1 (1.6). |
| 0.10 | **Store / cache tunables surface (LRU budget, verify, compaction, eviction)** | Lore exposes first-class `[store]` (max_capacity, eviction_delay, verify_write, compaction) in per-repo config + CLI; Libra has the machinery (TieredStorage) but no user-visible equivalent | easy | 2–3 d | 0.3, 2.9 | Surface via `libra config` reserved keys or a small `libra cache configure` / `libra store` subcommand; wire into TieredStorage LRU + verify-on-write path; keep defaults conservative so the lean CLI remains unchanged. |

> **Phase 0 internal ordering:** land **0.2 (backoff) → 0.3 (verify-on-cache) → 0.4 (`fsck --heal`)** in that order — they share the cloud cache-write path. `fsck --heal` re-fetches from the remote tier, so it must inherit 0.2's backoff (or it will hammer a throttling R2/D1) and 0.3's hash-verification (healed bytes must be verified before they land in the cache). The other Phase 0 items are order-independent.

### Phase 1 — Foundational (high value, tractable, unblock later work)

| # | Item | Why | Feasibility | Effort | Deps | Approach |
|---|---|---|---|---|---|---|
| 1.1 | **Dirty-set cache + `libra dirty` + `status --cached`** | Constant-time status for AI agents on huge trees; watcher/agent integration point | moderate | 2–4 w | migration | SQLite `working_dirty` table (worktree, path, action) + parent-prefix index; `libra dirty <paths>`/`move`/`copy` classify existence-vs-HEAD without hashing; `status --cached` reads it, `--scan` reconciles + refreshes, `--check-dirty` re-verifies dirty set; keep full reconcile the safe default. |
| 1.2 | **`restore --ours/--theirs` (`-2/-3`)** | The missing conflict-resolution verb | moderate | 3–5 d | **precondition already met** — index stages 1/2/3 are written today (`merge.rs:1183-1204`, `cherry_pick.rs:1267-1291`) | Read index stages 2/3 for unmerged paths, write to worktree, optionally clear to stage 0; bulk via pathspec/`.`. No writer rework needed — stages exist; do a final confirm on rebase's conflict path before sign-off. |
| 1.3 | **diff3 conflict markers + `merge --dry-run`/`--restart`** | Line-level conflicts + safe merge preview (Lore's auto-resolve preview) | moderate | 1 w | 1.2 | Per-hunk diff3 (`\|\|\|\|\|\|\| base`) via `similar`/`diffy`; `merge.conflictStyle`/optional sidecars; `--dry-run` computes resolution in-memory, reports conflict vs auto-merge, zero FS writes; `--restart` re-derives the 3-way from the OIDs already in `merge-state.json` (no schema change). |
| 1.4 | **Positional diff + whitespace flags + `--diff3`** | Diff parity for review/merge | moderate | 1–2 w | rev-parse ranges | Two-dot `A..B` already shipped (`normalize_diff_range`, `diff.rs:211-241`); add space-separated `diff A B`, three-dot `A...B` merge-base, whitespace flags (`--ignore-space-at-eol`/`-w`/`-b`), `--diff3` reusing merge marker code. |
| 1.5 | **Branch/repo metadata KV (keystone)** | Backs `protect`, `archive`, identity, lineage | moderate | 1 w | migration | `branch_metadata(branch, key, value, value_type)` + reserved keys; `libra branch metadata get/set/clear`; wire `protected` into delete/push guards + pre-receive hook; `archived` into list filter; `repository metadata` over `config_kv` reserved prefix. |
| 1.6 | **`libra auth`** (v1: token-only) | No remote auth today beyond interactive basic-auth | moderate | 2–3 w | vault; per-domain token scoping | **v1 scope:** `libra auth login --token/--auth-url`, `info/list/logout/clear`; store per-domain tokens in libvault; credential-helper lookup in `https_client.rs` replaces `ask_basic_auth`; enforce host scoping (leak guard) **in the same change** before attaching `Authorization`. **Deferred to a follow-up:** interactive/OAuth/device-code login and OS-keyring backing (2.7). The 2–3 w reflects the repo's bar (no `unwrap`, actionable errors, EXAMPLES const, docs page, compat guards). |
| 1.7 | **OTLP telemetry (feature-gated)** | Operable fleet observability | moderate | 1–2 w | opentelemetry crates | Optional feature: `tracing-opentelemetry` + `opentelemetry-otlp` behind `LIBRA_OTLP_ENDPOINT`; instruments around CLI dispatch + `Storage` trait; tower-http metric layer on the code server; default CLI stays lean. |
| 1.8 | **`merge --autostash`** | Survive uncommitted edits across merge (Lore `merge_carry`) | moderate | 3–5 d | stash | Reuse `src/command/stash.rs`; auto-stash dirty tracked changes pre-merge, pop post-merge; on conflicted pop, follow Git (leave stash). |
| 1.9 | **Commit trailers + `log --trailer`** | Provenance/metadata search (Lore find-metadata) | moderate | 1 w | — | Standardize reserved trailers (`Reviewed-by`/`Cherry-picked-from`/`Change-Request`); `log --trailer <key>[=value]` walk + notes-backed SQL fast path. |
| 1.10 | **Typed metadata command family** | Lore exposes repository/branch/revision/file metadata as first-class APIs; Libra needs the same user-facing affordance without changing Git trees | moderate | 2–3 w | 1.5; 1.9; notes | Expose `libra metadata repository|branch|revision|file get/set/clear` (or nested `repository metadata`, `branch metadata`, etc. if CLI grammar prefers locality). Repo metadata uses reserved `config_kv`; branch metadata uses `branch_metadata`; revision metadata uses trailers + notes; file metadata uses a committed side-tree keyed by path. |
| 1.11 | **Headless local service + notification v1** | Lore's `service` and `notification subscribe` are real CLI surfaces; Libra's equivalent should reuse `libra code` infrastructure without requiring the TUI | moderate | 2–4 w | 1.1; event bus extraction | Add `libra service run/start/stop/status` as a **UDS/loopback-only IPC daemon** (matching Lore's UDS model) exposing status/dirty/cache-warm/notification events over NDJSON/SSE/MCP. The service is deliberately small and headless — distinct from the rich interactive `libra code` TUI+web+MCP server. Do not create a hosted server; this is a local repo daemon for agents, watchers, IDEs, and external scripts. |
| 1.12 | **`branch diff`** | Lore exposes branch-to-branch diff as a first-class verb; Libra requires users to know revspec syntax | easy | 2–3 d | rev-parse ranges | `libra branch diff [<branch-a>] [<branch-b>]` defaulting to `HEAD..<other>`; reuse `DiffArgs` engine; `--stat/--name-only/--json` parity. |
| 1.13 | **`branch reset`** | Reset a branch tip to an arbitrary revision without touching the worktree (Lore `branch reset`) | easy | 2–3 d | ref update safety | `libra branch reset <branch> [<revision>]`; default revision = HEAD of current branch? Update SQLite `reference` row; guard protected branches (1.5); emit reflog entry. |
| 1.14 | **Case-change handling for `add`/`stage`/`mv`** | Lore's `file stage --case=keep|rename|error` avoids accidental duplicate paths on case-insensitive filesystems | moderate | 3–5 d | index path normalization | Add `--case` to `add`, `mv`, and restore-style paths; `keep` updates filesystem to match index, `rename` updates index to match filesystem, `error` aborts on mismatch. Default `error` (Git-compatible). |
| 1.15 | **Low-level in-memory revision tree construction (LEP 2026-05-14)** | Pipelines, importers, mirrors, and AI agents need to read/build revisions by path/node without materializing a full working tree | moderate | 2–3 w | 1.10 metadata; plumbing commands | Expose Git-idiomatic "update-index / commit-tree" style or a `RevisionTree` handle (MCP-first, optional `libra revision-tree` plumbing) that lets callers resolve paths → tree entries, add/modify/delete/move by path or tree OID, then commit without a checkout. Reuses existing object writers + index machinery; no new on-disk format. Directly serves the "no working tree" use cases in the LEP while staying inside Git trees. |

### Phase 2 — Composition & scale (structural; some large blast radius)

> **Recommended ordering within Phase 2 (highest value-per-effort first):** **2.3 (object alternates) → 2.2 (sparse) → 2.1 (per-worktree isolation)**. Despite the numbering, **2.3 has no dependency on 2.1 or 2.2**, is only 1–2 weeks, delivers cross-clone dedup on its own, and may be partly recoverable from history — so do it first. **2.2 v1 is not gated on 2.1** either (see its Deps). The multi-month 2.1 refactor should come *after* the two high-value items it is wrongly perceived to block.

| # | Item | Why | Feasibility | Effort | Deps | Approach |
|---|---|---|---|---|---|---|
| 2.1 | **Per-worktree HEAD/index/refs isolation** | Removes the documented shared-worktree footgun; unblocks parallel agents | hard | multi-month | migration; touches every ref/HEAD/index resolver | Per-worktree `instance_id` (file in worktree dir, since `.libra` is a shared symlink); `instance_id` column on `reference`/index/HEAD; objects stay shared. Git's own worktree model. Highest structural value; high ripple. |
| 2.2 | **Sparse view filter + view-filtered committed ops** | Sparse-by-default working trees for million-file repos | moderate→hard | 2–3 w + 1 w | **NOT gated on 2.1** — v1 uses a single repo-level materialized-set in `config_kv` (Git's own non-worktree sparse-checkout model); ignore matcher | `.libra/view` glob (reuse `src/utils/ignore.rs`); thread into checkout/restore/switch/merge/rebase/pull writers; record the materialized-set (single set in `config_kv` for v1; per-worktree only once 2.1 lands); status/diff/add treat in-tree-unmaterialized as sparse (not deleted); `sparse-checkout set/add/list/disable/reapply`, `clone --view`. Per-worktree *divergent* views are a 2.1-dependent enhancement, not a v1 requirement. |
| 2.3 | **Object alternates / shared store** | Cross-clone disk dedup; VFS prerequisite | moderate | 1–2 w | gc alternates-awareness | `clone --reference/--shared/--dissociate`, `shared-store create/info`; `ClientStorage::get` consults an alternates fallback chain; recover the vanished prior impl via `libra show <commit>`; gc/prune must not gc objects an alternate references. |
| 2.4 | **Layers (local overlays)** | Tractable composition entry point; personal/CI overlays without polluting history | moderate | multi-week | subtree fetch; materialized manifest | TOML `LayerConfig` under `.libra/`; materialize source subtree into `target_path`; `layer add/remove(--purge)/list`; branch auto-follow on `switch`; never committed. |
| 2.5 | **Index-flagged obliteration** | GDPR/secret-deletion in a dedup store; compliance | hard | 1–2 mo | object_index flag; read-path typed error; repack for packed objects; **`fsck --heal` (0.4) must skip obliterated OIDs** | `obliterated` state on `object_index` + storage backend; read path returns typed `ObliteratedObject`; physically remove loose/pack/R2 payload. Result is **graph-resolvable / content-unfetchable** (graph walk OK, content fetch errors). Two-phase `Obliterating→Obliterated` SQLite state + `doctor` recovery; `libra obliterate (--object\|--path)` idempotent + confirmation-gated; distributed delete (local ∥ D1/R2) with `obliterate` permission via vault + hooks. **`fsck`/`fsck --heal` must treat obliterated OIDs as intentional-absence, not corruption.** Doc page with Git-format caveats (shared-content blobs, packed-object repack, real-git readers hard-fail). |
| 2.6 | **Unified merge/cherry-pick/revert sequencer** | One conflict UX; maintainability | moderate–hard | 2–3 w | port `merge-state.json` into SQLite first | Consolidate the two SQLite tables (`cherry_pick_state`, `revert_sequence`) + the structured `merge-state.json` into one `SequenceState` table + `op_kind`. The JSON file already carries structured fields, so this is "one JSON file + two tables → one table," not three divergent designs. Shared resolve/unresolve/restart/abort/continue dispatch; 3-way core stays shared. |
| 2.7 | **Interactive auth + OS-keyring credentials** | Lore supports interactive/no-browser login and secure token stores; Libra v1 token auth should grow into a full operator flow | moderate | 1–2 w | 1.6 | Add browser/device/no-browser login after token v1, then `keyring` crate as optional vault-unseal/token backend (Keychain/Credential Manager/Secret Service) with file fallback for headless/CI. Preserve host-scoped token attachment from 1.6. |
| 2.8 | **Lock enforcement on commit/add + local lock store** | Move advisory→enforced beyond push | moderate | 1–2 w | LFS locks; hooks | `lfs.lockEnforce=warn\|block` consulting `get_locks` at commit/add; allow locking any path; optional SQLite-backed local lock store for offline/agent coordination. |
| 2.9 | **Background cache evictor + config** | Decouple eviction from insert; cache hygiene | moderate | 3–5 d | — | Optional tokio interval evictor enforcing the byte budget oldest-first; surface `max_capacity`/`eviction_delay` in config. |
| 2.10 | **Push protocol-layer parity (`--atomic`/`--signed`/`--push-option`/`--follow-tags`)** | Git-format-compatible push completeness; the analog of Lore's two-phase atomic push | moderate→hard | 2–3 w | receive-pack protocol extension in `src/internal/protocol` | Wire the deferred push flags through the receive-pack client: `--atomic` (all-or-nothing ref update), `--push-option` (pass-through), `--follow-tags`, `--signed` (push-cert). Protocol-invasive (parked in the push goal-state for exactly this reason); reuses Git's model, no new object graph. |
| 2.11 | **`shared-store set-use-automatically`** | Lore can mark a shared store as the default for future clones on this machine | low | 1–2 d | 2.3 | Persist default shared-store path in libvault/global config; `clone` consults it before creating a standalone object store. |

### Phase 3 — Lore-parity gated extensions (after Phase 1/2 foundations)

These items are now part of the completion roadmap, but not part of the first two implementation waves. They become executable only after their dependencies make them safe.

| # | Item | Why | Feasibility | Effort | Deps | Approach |
|---|---|---|---|---|---|---|
| 3.1 | **File dependency graph** | Lore has `file dependency add/remove/list` and dependency-driven selective clone/sync; game-asset repos need graph-aware materialization | moderate→hard | 3–5 w | 1.10 metadata; migration | Store dependency edges in SQLite for query speed and optionally mirror them into a committed side-tree for portability. Support tags, recursive traversal, depth limits, cycle detection, and `--json` graph output. |
| 3.2 | **Dependency-filtered clone/sync** | Converts the dependency graph into user-visible scale wins | hard | 3–5 w | 2.2 sparse; 2.3 alternates; 3.1 | Add `clone --root-file --dependency-tag --dependency-recursive --dependency-depth-limit` and matching `pull/switch/sparse reapply` filters. Materialize roots + closure, keep non-closure paths sparse, and never treat out-of-closure tracked files as deletes. |
| 3.3 | **LFS FastCDC chunking** | Lore's binary-first advantage is sub-file dedup and resumable large-asset transfer | hard | 1–2 mo | LFS pointer v2 RFC; storage backoff; verify-on-cache | Keep Git blobs atomic. Put chunk manifests behind Libra LFS media only: file pointer → chunk manifest → R2/S3 chunks. Verify chunk hashes, support range reads, and document non-interop with Git LFS servers unless the remote advertises Libra chunk manifests. |
| 3.4 | **Hydrating VFS** | On-open materialization is Lore's largest UX difference for massive repos | hard | multi-month | 2.2 sparse; 2.3 alternates; 3.3 optional | Extend the FUSE worktree into a whole-object hydration provider first, then optionally range-hydrate chunked LFS media. Keep ProjFS/SWFS out until FUSE semantics and failure recovery are proven. |
| 3.5 | **Link/subtree composition RFC** | Lore links are real and useful, but Libra intentionally declined `submodule`; this needs a product decision before code | hard | RFC first | 1.10 metadata; 2.2 sparse; auth | Write an RFC comparing Git subtree, gitlink-mode 160000, and SQLite link side-tables. Only implement if monorepo + object storage + layers cannot satisfy the target workflow. |

### Later / Defer (architectural conflict, or needs a hosted server that does not exist)

| Item | Why deferred |
|---|---|
| **ProjFS/SWFS-specific VFS backends** | FUSE hydration is Phase 3; native Windows ProjFS is a separate driver-quality project, and SWFS is a proprietary Epic dep. |
| **FastCDC as Git object addressing** | Phase 3 allows chunking inside LFS media only. Sub-file chunks must never replace Git blob object IDs. |
| **Partitions / transparent traversal / per-link ACL** | Link/subtree composition needs an RFC first; in-repo partitions and per-link ACLs require a hosted authz layer that Libra does not have. |
| **Forks (COW partitions)** | Roadmap even in Lore; Git-idiomatic analog is promisor/partial clone + server-side repo fork (needs auth + hosted server). |
| **C ABI + multi-language SDKs** | Foundational inversion away from Libra's CLI/agent-API identity; ~140-function ABI to version; MCP + JSON CLI is the idiomatic cross-language surface. |
| **QUIC/gRPC storage protocol** | Breaks Git remote interop; LFS already gives chunked/resumable media transfer; no current consumer. |
| **Distributed CAS lock store / DynamoDB / Consul-Nomad / server sharding** | Server-cluster concerns foreign to Libra's local-first SQLite model; no hosted Libra server exists. |
| **In-repo partitions / Context file-ID / BLAKE3 / 320-byte state / node-blocks** | Fundamentally break Git on-disk format compatibility. |

---

## 6. Explicit non-goals (do NOT copy)

| Lore idea | Why Libra should not adopt it |
|---|---|
| **BLAKE3 object addressing** | Breaks Git on-disk format — Libra's defining promise. SHA-256 already provides cryptographic strength + dedup-by-construction. |
| **320-byte revision state / 96-byte node-blocks / mmap node format** | Storage-engine internals incompatible with Git commit/tree objects; packfiles + `object_index` are the Git-idiomatic compactness/lookup path. |
| **FastCDC fragment chunking as object addressing** | A Git blob is atomic and whole-file-hashed. Sub-file dedup can only live in the *LFS media layer*, never the object graph. |
| **In-repo partitions (hash ≠ capability)** | Git deliberately makes content-hash = read capability. Repo-as-boundary + storage-fetch authz is the compatible substitute. |
| **Context (per-file identity field)** | Git has no stable file identity by design; rename detection is heuristic at diff time (already done). A parallel identity model risks drift. |
| **Removing the index (filesystem-as-only-truth)** | Forfeits Git interop and the entire pack/index format contract. Add `commit -a`/intent-staging ergonomics instead. |
| **In-place byte-erasure of *inline* blobs preserving cross-file dedup** | Under Git content-addressing, two co-content files share one object; obliterating it removes it for both. Honestly document this limit; offer LFS-media obliteration as the genuine compliance win. |
| **Tree-embedded conflict/merge flags** | Git has no per-node staged-merge bits. SQLite sequencers + index stages 1/2/3 are the crash-safe Git-idiomatic equivalent. |
| **C ABI as the primary artifact + 6 binding repos** | Off-mission vs Libra's CLI + AI-agent (MCP) identity; large versioning/maintenance burden with low ROI. |
| **QUIC/gRPC bespoke storage protocol** | Breaks Git remote interop; no consumer; LFS covers resumable media. |
| **SWFS (proprietary Epic driver)** | Proprietary dependency; FUSE (+ future ProjFS) is Libra's cross-platform mount story. |
| **Cross-repo `parent_repository` merge field / Lore links as the composition model** | `submodule` is an intentional Libra product boundary; subtree-style merge is the Git-idiomatic alternative if composition is ever needed. |

---

## 7. Risks, sequencing & open questions

### 7.1 Sequencing dependencies

- **Branch metadata KV (1.5)** is the keystone for `protect`, `archive`, stable identity, and lineage — land it before those.
- **Typed metadata (1.10)** depends on branch/repo metadata and commit trailers; it is the reusable substrate for Phase 3 dependency graphs. Do not build dependency edges as a one-off table that cannot round-trip through future metadata/export surfaces.
- **Local service/notification v1 (1.11)** depends on dirty-set cache (1.1); otherwise the service either polls expensively or reports stale state. Keep it loopback-only until a hosted Libra server is explicitly approved.
- **Index conflict stages 1/2/3 are already written** (verified: `merge.rs:1183-1204`, `cherry_pick.rs:1267-1291`), so `restore --ours/--theirs` (1.2) and diff3 (1.3) have data to read — no writer rework. Only confirm rebase's conflict path writes stages before signing 1.2 off.
- **`libra auth` (1.6)** must include the domain-leak guard *in the same change* — adding token storage without host-scoping is a security regression.
- **One SQLite first-parent ordinal cache underpins `<branch>@{N}`, `find-number`, and ordinal display in `log` (§4.2) — build it once.** Three separate items depend on the same per-branch ordinal index; don't implement it three times.
- **Object alternates (2.3) is independent** of per-worktree isolation (2.1) and sparse (2.2) — do it first (see Phase 2 ordering note). **Sparse (2.2) is not gated on 2.1**: v1 uses a single repo-level materialized-set; per-worktree *divergent* views need 2.1. Per-worktree isolation (2.1) is the substrate for true instance symmetry.
- **Obliteration (2.5)** needs the `object_index` flag + read-path typed error + (for packed objects) a **repack** capability gc lacks today; **`fsck --heal` (0.4) must skip obliterated OIDs** or it fights 2.5; coordinate local + R2 + D1 backup so a backup never resurrects deleted bytes.
- **Dependency-filtered clone/sync (3.2)** is blocked on metadata (1.10), sparse view (2.2), object alternates (2.3), and the dependency graph (3.1). It must reuse sparse semantics instead of inventing a second materialization state.
- **Hydrating VFS (3.4)** is blocked on sparse view (2.2), object alternates (2.3), and a proven hydration/recovery story; LFS chunking (3.3) is optional for v1 but required for true range hydration.
- **`branch diff`/`reset` (1.12/1.13)** are independent of each other but should reuse the branch metadata KV (1.5) for protected-branch guards; implement them after 1.5 so `reset` cannot bypass `protect`/`archive` bits.
- **Low-level revision tree (1.15)** depends on typed metadata (1.10) for provenance attachment and on existing plumbing writers; it is intentionally sequenced after the metadata substrate so that agent-constructed revisions carry proper trailers and can be queried uniformly. It does **not** require sparse or worktree isolation.
- **File case-change handling (1.14)** should land after the dirty-set cache (1.1) is stable, because both touch path-normalization and index-worktree reconciliation; keep the default `error` behavior to stay Git-compatible.
- **`shared-store set-use-automatically` (2.11)** is a small config convenience that only makes sense after object alternates (2.3) are working.

### 7.2 Execution gates

Every plan item above is only considered done when the following are true:

- **Docs gate:** visible command changes update `src/cli.rs` command groups, `COMPATIBILITY.md` when Git-facing, `docs/commands/<cmd>.md`, Chinese command docs if the surface already has one, and `tests/INDEX.md`/compat registrations when new test targets are added.
- **Test gate:** each slice has at least one end-to-end command test and one focused unit/parser test where logic can be isolated. Metadata, auth, obliteration, sparse, and dependency graph changes also need Display/error-message pinning for new stable errors.
- **Safety gate:** no new production `unwrap()`/`expect()` without an `// INVARIANT:` comment; user-facing errors must name the affected resource and next action.
- **Compatibility gate:** Git object format remains SHA-1/SHA-256 compatible; no plan item may introduce BLAKE3 object IDs, tree-embedded Lore node metadata, or non-Git refs as the primary object graph.
- **Manual QA gate:** each new command must be driven through `libra <cmd> --help`, one successful local invocation, one failing invocation with an actionable error, and JSON/machine output if the command emits structured data.

### 7.3 Top risks

- **Two sources of truth (Git index + dirty-set cache).** Every mutating command must keep them consistent — the exact desync class Lore avoids by having one anchor. Default `status` must stay the safe full reconcile; `--cached` is opt-in.
- **Per-worktree isolation blast radius.** Touches every ref/HEAD/index resolver and changes the shared-symlink model that current concurrent-session workflows rely on. Requires a careful migration and broad test coverage.
- **Sparse misclassification.** Highest correctness hazard: status/clean/add must treat unmaterialized in-view paths as sparse, not deleted, or risk clobbering. Merge/rebase must update tree objects for out-of-view paths without materializing.
- **Obliteration honesty.** Git content-addressing means shared-content blobs are obliterated for all referents; real-git readers hard-fail on obliterated objects; backups can resurrect bytes. The doc page is mandatory and must be accurate about these sharper-than-Lore limits.
- **Recovering vanished work, and memory-vs-tree drift.** Several auto-memories assert features that are **NOT in the current tree** — verified against `src/`:
  - `diff` whitespace/word-diff/`-W` — *absent*; `DiffArgs` (`src/command/diff.rs:49-96`) has only `--old/--new/--staged/--algorithm/--output/--name-only/--name-status/--numstat/--stat`. (Two-dot `A..B` positional revspec **now exists** via `normalize_diff_range` at `diff.rs:211-241`; space-separated `A B`, three-dot `A...B` merge-base, and whitespace flags are still absent.)
  - `pull --autostash` / `run_merge_for_pull_with_autostash` — *absent*; zero `autostash` hits in `pull.rs`/`merge.rs`.
  - `restore --ours/--theirs` (`-2/-3`) — *absent*; `RestoreArgs` (`restore.rs:154-173`) has only `--staged/--worktree/--source/--pathspec-from-file`. (The index *stages* those flags would read **are** written — see §4.4 — so only the flag surface is missing.)
  - `clone --reference/--shared/--dissociate/--filter` — *absent* from `clone.rs`.

  This doc reflects the **tree**, not those memories. Recover genuinely-dropped implementations via `libra show <commit>` before re-implementing (to avoid drift), and confirm against `docs/development/commands` ground truth — not stale goal-state memories — before assuming any feature is present.
- **Feature-gating discipline.** OTLP and any VFS work must stay behind cargo features so the default CLI stays lean and the compat matrix (`compat-rustfmt`/`compat-clippy`/`compat-offline-core`) stays green.

### 7.4 Open questions

1. **Revision-number semantics under rewrite.** Lore mints fresh immutable revisions (stable ordinals); Libra rewrites move tips. Define cache-invalidation rules and merge-number behavior, and document `<branch>@{N}` as a convenience pointer, not an immutable name.
2. **Intent-staging vs Git mental model.** Capturing content at commit time (Lore-style) diverges from Git's stage-time snapshot. Keep it opt-in (`add --intent`) — or is the value high enough to make it default for tracked files?
3. **Does single-repo + object storage already cover the links use case?** The compat doc names the exact restart condition ("multi-repo dependency that monorepo/object-storage cannot solve"). Until that materializes, links/partitions stay deferred.
4. **Where should obliteration's packed-object repack live?** gc does not repack today; obliterating a packed object requires writing a new pack minus the object. Is loose-object-only obliteration (plus LFS media) sufficient for v1?
5. **Local lock store vs LFS lock server confusion.** A SQLite local lock store coordinates only one machine. How do we keep the local-vs-remote lock surfaces distinct and unambiguous?
6. **Hosted Libra server?** Notifications, distributed locking, server-side push validation/FF-merge, forks-with-access-control, and multi-language streaming all presuppose a hosted multi-tenant Libra server that does not exist (publish is a read-only Worker). Is building one in scope, or do these stay permanently deferred / pushed into the publish+code surfaces?


---

## Appendix A — Lore CLI command inventory → Libra mapping

This inventory was built from `lore-client/src/cli/commands/*` (`0.8.4-nightly`) and merged into the plan above. It makes the gap analysis executable: every Lore verb either has a Libra equivalent today, maps to a concrete plan item, or is explicitly deferred.

| Lore command / subcommand | Libra analog today | Plan / verdict |
|---|---|---|
| **Global args** `--offline`, `--remote`, `--local`, `--sync-data`, `--cache`, `--gc`, `--force`, `--dry-run`, `--json`, `--no-pager`, `--non-interactive`, `--identity`, `--max-connections`, `--file-count-limit`, `--file-size-limit`, `--compress-limit`, `--max-threads`, `--search-limit`, `--search-nearest` | `--json`, `--no-pager`, `--quiet` present; most resource knobs missing | 0.2, 0.5, 0.8, 0.9 |
| `repository status` | `libra status` | 1.1 (`status --cached` + dirty-set) |
| `repository info` / `repository list` | `libra remote -v`, `libra status` | present-different / low |
| `repository create` | `libra init` | present |
| `repository clone` | `libra clone` | 2.2 (`--view`), 2.3 (`--reference/--shared`) |
| `repository delete` | (remove `.libra` manually) | low; no plan |
| `repository verify state/fragment` | `libra fsck`, `libra verify-pack` | present-different; 0.4 (`fsck --heal`) |
| `repository dump` | `libra show`, `libra cat-file`, `libra rev-list` | present-different / low |
| `repository gc` | `libra gc`, `libra maintenance` | present |
| `repository store immutable query` | `libra cat-file`, `libra ls-tree` | present-different / low |
| `repository config get` | `libra config` | present |
| `repository metadata get/set/clear` | `libra config` (repo scope) | 1.5, 1.10 |
| `repository instance list/prune`, `repository update-path` | `libra worktree list/prune/move` | present-different; 2.1 |
| `branch list/info/create/switch/push` | `libra branch` / `switch` / `push` | present |
| `branch merge start/restart/resolve/abort/unresolve/mine/theirs/into` | `libra merge` / `cherry-pick` / `revert` | 1.2, 1.3, 2.6 |
| `branch diff` | `libra diff <branch>..<branch>` (requires revspec) | 1.12 |
| `branch reset` | no direct equivalent | 1.13 |
| `branch archive/protect/unprotect` | no direct equivalent | 1.5 |
| `branch latest` | `libra branch -v` partially | 1.5 (`archived` filter) |
| `branch metadata get/set/clear` | no direct equivalent | 1.5, 1.10 |
| `revision history` | `libra log` | present |
| `revision info` | `libra show` | present |
| `revision commit` / `revision amend` | `libra commit` / `commit --amend` | present |
| `revision sync` | `libra pull` | present |
| `revision diff` | `libra diff` (`A..B` two-dot done) | 1.4 (`A B` / `A...B` / whitespace) |
| `revision find metadata/number` | no direct equivalent | 1.9, §4.2 ordinal cache |
| Low-level in-memory revision tree (LEP) | plumbing + MCP only | 1.15 |
| `revision restore` | `libra restore --source` | present |
| `revision cherry-pick/revert` + resolve mine/theirs/unresolve/restart/abort | `libra cherry-pick` / `revert` | 2.6 (unified sequencer) |
| `revision bisect` | `libra bisect` | present |
| `revision metadata get/set/clear` | no direct equivalent | 1.10 |
| `file info` | `libra ls-files`, `libra status` | present-different |
| `file metadata get/set/clear` | `libra notes` (free-form) | 1.10 |
| `file dependency add/remove/list` | no direct equivalent | 3.1 |
| `file stage` / `file stage --scan` / `file stage move/merge` | `libra add` / `libra mv` | present-different; 1.1 (`--scan`) |
| `file dirty move/copy` | no direct equivalent | 1.1 |
| `file unstage` / `file reset` | `libra restore --staged` / `libra reset` | present |
| `file diff` | `libra diff` (`A..B` two-dot done) | 1.4 (`A B` / `A...B` / whitespace) |
| `file obliterate` | no direct equivalent | 2.5 |
| `file dump` / `file history` / `file write` / `file hash` | `libra cat-file` / `log --follow` / `hash-object` | present-different |
| `file stage --case={keep,rename,error}` | no direct equivalent | 1.14 |
| `auth login/info/list/logout/clear` | no `libra auth` command | 1.6, 2.7 |
| `layer add/remove/list` | no direct equivalent | 2.4 |
| `link add/remove/update/list` | no direct equivalent (submodule declined) | 3.5 (RFC) |
| `lock acquire/status/query/release` | `libra lfs lock/unlock/locks` | present-different; 2.8 |
| `service run/start/stop` | `libra code` (richer, TUI) | 1.11 |
| `notification subscribe` | AI broadcast stream only | 1.11 |
| `completions` | no `libra completions` | 0.1 |
| `shared-store create/info/set-use-automatically` | no direct equivalent | 2.3, 2.11 |
| `logfile info` | `LIBRA_LOG_FILE` only | 0.7 |
| Store / cache tunables (`[store]`, LRU, verify_write) | TieredStorage internals only | 0.10 |
| Low-level in-memory revision tree construction (LEP) | plumbing + MCP tools | 1.15 |

> **How to read the table.** "present" means Libra already ships a Git-idiomatic equivalent; "present-different" means the capability exists in another shape; a plan number means it is scheduled above; "low; no plan" means the gap is real but too small or too low-value to track as a standalone phase item. The mapping intentionally does not include low-level Lore storage-admin commands (`repository store immutable query`, `repository dump`) that have no user-facing Libra equivalent because Libra's object graph is the public API.

---

# PART II — Libra → Lore (Lore closes its gaps against Libra)

> **Reading guide.** Part II is the mirror image of Part I. It takes **Libra as the reference** and plans what **Lore** must build to reach parity — **while staying content-addressed (BLAKE3), partition-based, no-index, API/C-ABI-first, and server-centric.** The guiding constraint flips: Part I had to protect Libra's *Git-format compatibility*; Part II has to protect *Lore's* identity. Where a Libra idea conflicts with that identity (the Git on-disk format, the index, SQLite-as-canonical-refs, the Git wire protocols), Part II **adapts or defers** it — see the non-goals in §P2.6, the symmetric counterpart of §6.
>
> Evidence base: the live Lore tree (`/Volumes/Data/EpicGames/lore`, `0.8.4-nightly`), its `lore-client/src/cli/cli.rs` command enum, `docs/roadmap.md`, `docs/reference/lore-cli-commands.md`, and a full-tree probe (AI/agent/LLM/MCP matches in Lore source: **0**).

## P2.1 Framing: the same two architectures, read the other way

Part I established the asymmetry; Part II exploits it. The two systems **lead in different dimensions**, so the gap is lopsided rather than mutual:

- **Libra leads on AI-native development and Git-ecosystem history depth.** Its `src/internal/ai/` subsystem (agents, orchestrator, MCP server, sandbox, providers, skills, automation, goal/supervisor/verifier, usage accounting, context-budget, session store, TUI `libra code`) **has no Lore equivalent whatsoever** — a full-tree search of Lore returns zero AI/LLM/MCP hits. Libra also ships the deep Git history-rewrite surface (rebase, stash, reflog, blame, tag, grep, shortlog, describe, client hooks) that Lore either lists as roadmap or does not have.
- **Lore leads on storage scale, composition, and distribution** (BLAKE3 CAS, FastCDC chunking, partitions, links, layers, instances/shared-store, sparse/VFS, obliteration, the C-ABI, QUIC/gRPC protocol, a real multi-tenant server, scalable-locks roadmap, OS-keyring/JWT auth, OTLP). Part I is the plan for Libra to chase *those*.

So Part II's job is narrow and deep: **adopt Libra's user-facing AI capability and its history-rewrite verbs, expressed idiomatically to Lore's substrate — never by importing Git mechanics.**

**The keystone insight — Lore's substrate is unusually well-suited to host an AI layer.** The single hardest parts of Libra's AI stack were retrofits onto Git's model; Lore already has native equivalents:

| Libra had to build / struggles with | Lore already ships (head-start for the AI layer) |
|---|---|
| Extract a loopback event bus out of `libra code` for agent/watcher notifications (Part I plan **1.11**) | `lore service run/start/stop` + `lore notification subscribe` — a **UDS IPC daemon with an event-dispatch model already in production** (`lore-client/src/cli/commands/service`, `lore-notification`) |
| Per-worktree HEAD/index/refs isolation for parallel agents — a multi-month refactor of every ref/HEAD/index resolver (Part I plan **2.1**, the doc's "⭐ keystone") | **instances** (per-dir UUIDv7 identity, independent staged state) over a **shared-store** + **partitions** — symmetric, isolating, no privileged main repo, *by design* |
| Provenance/permission/observability scaffolding (commit trailers, vault, tracing) | **typed metadata KV** (file/revision/branch/repo), **JWT auth + REBAC + OS-keyring** (`lore-credential`), **OTLP** (`lore-telemetry`), and a **C-ABI** tool-integration surface |

The implication: porting Libra's AI runtime onto Lore is **with-the-grain**, not a graft. The control plane is new code, but the transport, isolation, identity, and telemetry it needs are already there.

## P2.2 The biggest gaps (what Lore lacks vs Libra), grouped into themes

| Theme | What Lore lacks vs Libra | Headline gaps |
|---|---|---|
| **AI-native development** | No agents, no LLM providers, no MCP server, no orchestrator, no agent sandbox, no skills, no automation rules, no goal/supervisor, no usage accounting, no agent session store | the whole **`lore-agent`** stack (P2.4.1) — the dominant gap |
| **History rewrite** | No `rebase`, no `squash` (cherry-pick workaround), no `stash`, no global `reflog` | rebase/squash engine on Lore's MergeType + a mutable-store sequencer; autostash; ref-movement log |
| **History inspection** | No `blame`, no `shortlog`, no `describe`, no `grep`, no `tag` | expose woven per-file history as `blame`; author/commit summaries; named immutable refs; content search |
| **Client-side lifecycle** | Server-side hooks only; no client `pre-commit`/`commit-msg`/`pre-push` lifecycle | client hook runner + templates (and AI-lifecycle hooks once L2 lands) |
| **Free-form annotation** | Typed metadata KV exists; no mutable free-text note attached post-hoc to a revision | `lore note` over a mutable-metadata pointer (mostly present-different) |
| **Zero-ops distribution** | Server-centric; needs `loreserver`; no serverless read-only snapshot hosting | `lore publish` analog (read-only snapshot to S3/CDN); client-managed backup |
| **Interactive agent surface** | CLI + GUI clients on roadmap; no terminal agent loop | `lore code`-style agent TUI (AI-gated, reuses lore-client TUI infra) |

**Not gaps — what Lore already has at parity or ahead (do *not* re-plan these).** Several capabilities that look like "Libra features" are equal or better in Lore, and Part II deliberately excludes them: shell **completions** (`lore completions`), **auth/JWT/OS-keyring/HashiCorp-Vault** (`lore-credential`/`lore-hashicorp` — ahead of Libra's basic-auth + libvault), **OTLP telemetry** (`lore-telemetry`), **service/notification IPC**, **shared-store/layers/links/partitions/instances**, **obliteration**, **sparse/VFS** (roadmap), **typed metadata KV**, **file-dependency graph**, **FastCDC chunking**, the **C-ABI**, and **file locking**. Proposing Libra-shaped versions of these would be redundant or regressive.

## P2.3 One-glance priority table (Lore's plan)

| Priority | Items | Rationale |
|---|---|---|
| **Phase L0 — Quick parity wins** | `lore tag`; `lore blame` (expose woven history); `lore stash`; `lore shortlog`/`describe`; `lore grep`; client hook lifecycle | Small, independent, reuse Lore's existing revision engine + dirty-set; no new subsystem |
| **Phase L1 — History-rewrite engine** | `rebase`; `squash`; interactive rebase; `merge --autostash`; global `reflog` | Aligns with Lore's *own* roadmap (rebase/squash); built on MergeType + staged anchor + a mutable-store sequencer |
| **Phase L2 — AI substrate (headline, foundational)** | `lore-agent` crate: completion + providers; **MCP over `lore service`**; tool registry over lore-revision/lore-storage; sandbox; agent session store; usage accounting | The dominant gap; unblocks all autonomy work; rides Lore's IPC/partition/metadata head-start |
| **Phase L3 — AI orchestration & autonomy** | orchestrator (plan/decide/execute/verify/replan, gate/ACL); goal/supervisor/verifier; skills; automation + scheduler (on `lore-notification`); context-budget; permission/intentspec/projection; observed-agent capture; prompt/codex bridge | Structural AI control plane; depends on L2 |
| **Phase L4 — Interactive & distribution surfaces** | `lore code`-style agent TUI; AI usage dashboards; read-only serverless publish analog; client-managed backup | High-visibility surfaces; gated on L2/L3 and on a distribution decision |
| **Later / Defer (non-goals)** | Git on-disk format; the index; SQLite-as-canonical-refs; Git smart-HTTP/SSH/LFS protocols; libvault | Conflict with Lore's content-addressed/no-index/API-first/server-centric identity (§P2.6) |

> **⭐ Reverse keystone — the `lore-agent` AI substrate (plan item L2).** It is the only *entire category* of capability that exists in one system and is wholly absent in the other. Unlike Part I's keystone (a painful Git retrofit), this one is with-the-grain: Lore's UDS service is the MCP transport, partitions/instances are the agent isolation, typed metadata is the provenance store, JWT is the permission model, OTLP is the telemetry. Land L2 and Lore gains the differentiator that today separates the two projects most sharply.

## P2.4 Per-domain reverse gaps

Each subsection: **Libra's capability (with file evidence) → Lore today → the Lore-idiomatic landing.** This is the inverse of Part I §4.

### P2.4.1 AI-native runtime — the headline gap

**Libra.** A large, production AI subsystem under `src/internal/ai/`: agent framework (profiles architect/coder/…, runtime, classifier, budget); `agent_run` records (context_pack, decision, evidence, patchset, permission, task); orchestrator (planner/decider/executor/verifier/replan/gate/ACL); goal (state/supervisor/verifier/spec); MCP server (`ai/mcp`); LLM providers (anthropic/openai/deepseek/gemini/kimi/zhipu/ollama/fake) behind a `CompletionModel` trait with retry/throttle/JSON-repair; sandbox (command-safety policy, macOS seatbelt, Linux bwrap); skills (loader/scanner/parser/dispatcher); automation (rule runtime, scheduler, history, executor); context_budget (window allocator, compaction, handoff, memory anchors); prompt engineering; intentspec/projection/permission; observed_agents (Claude Code, Gemini capture); usage accounting (recorder, pricing, query); session store (JSONL + file history); TUI `libra code`.

**Lore today.** **None.** Zero AI/agent/LLM/MCP code in the tree. But — per §P2.1 — the *substrate* is unusually ready.

**Lore-idiomatic landing** (introduce a new workspace crate `lore-agent`, plus `lore-agent-tui` for L4; keep the storage/revision crates untouched):

| Libra AI module | Lore landing (idiomatic) | Phase |
|---|---|---|
| `CompletionModel` trait + provider adapters (`ai/providers`, `ai/completion`) | New `lore-agent::completion` + provider clients (reqwest over `lore-transport`'s rustls); retry/throttle/JSON-repair ported verbatim — provider-agnostic, no Lore coupling | L2 |
| MCP server (`ai/mcp`) | Expose MCP **over the existing `lore service` UDS IPC** + `lore-notification` dispatch; reuse `lore-proto` codegen conventions for the resource schema | L2 |
| Tool registry + handlers `read_file`/`apply_patch`/`shell`/`grep`/`plan` (`ai/tools`) | Implement against **lore-revision** (staged anchor, dirty-set, `file stage`/`unstage`) and **lore-storage** (fragment reads); `apply_patch` writes through the staging path, not a Git index | L2 |
| Sandbox (`ai/sandbox`: seatbelt/bwrap, command-safety) | Port wholesale — OS sandboxing is substrate-neutral; gate `shell` tool through it; first real command-execution sandbox in Lore | L2 |
| Session store (`ai/session`: JSONL + file history) | Store sessions as **content-addressed blobs** in lore-storage (or typed metadata); use **instances/partitions** for per-agent isolation | L2 |
| Usage accounting (`ai/usage`: recorder/pricing/query) | Emit through **`lore-telemetry` (OTLP)** + a local usage ledger; `lore usage` query command | L2 |
| Orchestrator (`ai/orchestrator`) + goal/supervisor/verifier (`ai/goal`) | New `lore-agent::orchestrator`/`::goal` control plane; agent worktrees = **instances over shared-store** (Lore's native parallelism) | L3 |
| Skills (`ai/skills`) | New loader/scanner/dispatcher; skills as content-addressed, optionally partition-scoped | L3 |
| Automation + scheduler (`ai/automation`) | Rule runtime that **subscribes to `lore-notification` events** (branch push/create/delete, lock/unlock) — a natural trigger source Libra lacks | L3 |
| context_budget / prompt / intentspec / projection / permission | New agent-internal modules; **permission can ride Lore's JWT/REBAC** instead of a bespoke model | L3 |
| observed_agents (Claude Code/Gemini capture) + codex bridge | New adapters; redaction/preview as in Libra | L3 |
| TUI `libra code` (`internal/tui`) | New `lore-agent-tui` reusing **lore-client's existing TUI/paging/progress infra** | L4 |

**Why this ordering.** L2 is the irreducible foundation (a model can complete, call tools, run sandboxed, and persist a session). L3 is the autonomy layer (multi-step planning, goals, automation) that only makes sense once tools+sandbox+session exist. L4 is the human-facing surface. This mirrors how Libra's own stack is layered.

### P2.4.2 History rewrite & inspection verbs

**Libra.** Full Git-rich surface: `rebase` (`src/command/rebase.rs`, author/committer-preserving replay), `cherry_pick`/`revert` (SQLite sequencers), `stash`, `reflog` (SQLite, with `expire`), `blame`, `shortlog`, `describe`, `grep` (regex/context/binary modes), `tag` (lightweight + annotated), `bisect`, `merge --squash`/`--autostash` (planned), `notes`.

**Lore today.** Has `revision cherry-pick`/`revert`/`bisect`, three-way `merge` with explicit resolve, `revision restore`, and per-branch latest-history. **Missing:** `rebase`/`squash` (both on Lore's roadmap), `stash`, global `reflog`, `blame` (the data exists, woven into `NodeFileMetadata.revision[2]` — only the verb is missing), `shortlog`, `describe`, `grep`, `tag`, client hooks.

**Lore-idiomatic landing.**

| Libra verb | Lore landing | Phase / effort |
|---|---|---|
| `blame` | **Expose existing woven per-file history** — Lore already records per-file revision provenance; add a read-only `lore blame <path>` formatter. Near-free. | L0 / low |
| `tag` | Named **immutable refs** to a revision in the mutable store (lightweight); annotated tags carry typed metadata. Not Git tags — Lore-native named pointers complementing `branch@N` numbers. | L0 / low |
| `stash` | Snapshot the **staged anchor + dirty-set** into a stash entry (a revision-like object in storage); restore re-applies. Reuses dirty-set machinery. | L0 / medium |
| `shortlog` / `describe` | Aggregate over the revision DAG + `created-by`/`committed-by` metadata; `describe` = nearest-tag (once `tag` exists) + revision-number offset. | L0 / low |
| `grep` | Content search over materialized files (and, opportunistically, the fragment store); regex + context flags. | L0 / low–medium |
| client **hooks** | A client hook lifecycle (`pre-commit`/`commit-msg`/`pre-push`) invoked from the commit/push paths — Lore has only server hooks today. Foundation for AI-lifecycle hooks in L2/L3. | L0 / medium |
| `rebase` / interactive rebase | Replay a revision chain onto a new parent using the **MergeType engine** + staged anchor, with conflict resolution through the existing `merge resolve mine/theirs` UX; persist a **sequence-state in the mutable store** (Lore's CAS analog of Libra's SQLite sequencer). **Aligns with Lore's roadmap.** Preserve author, set committer. | L1 / hard |
| `squash` | Collapse a revision chain into one new revision (distinct from cherry-pick, per Lore's roadmap); shares the L1 sequencer. | L1 / medium |
| `merge --autostash` | Auto-stash dirty state across merge/rebase, restore after (depends on L0 `stash`). | L1 / low |
| global `reflog` | Generalize **branch latest-history** into a per-ref movement log with `expire`; store in the mutable KV store. | L1 / medium |
| `note` (free-form) | Mutable free-text annotation on an existing revision via a **mutable-metadata pointer** (Lore's typed metadata is immutable when hash-referenced) — mostly present-different. | L0 / low |

### P2.4.3 Client-side hook lifecycle

**Libra.** Git hook templates + an `ai/hooks` lifecycle (config, lifecycle, runner, providers) wired into commit/push and the AI runtime.

**Lore today.** Server-side hooks only (push/branch veto, noted in Part I §4.10). No client `pre-commit`/`commit-msg`/`pre-push`.

**Lore-idiomatic landing.** A client hook runner invoked from the `commit`/`push`/`stage` paths, plus hook templates under `.lore/`. Keep it small at L0; once L2/L3 land, add **AI-lifecycle hooks** (pre-plan/post-verify) so automation rules and agents can veto or gate operations — the analog of Libra's `ai/hooks`.

### P2.4.4 Zero-ops distribution & client-managed backup

**Libra.** `libra publish` snapshots a repo into a **read-only Cloudflare Worker over D1/R2** (serverless edge hosting, zero ops); `libra cloud` does **incremental D1/R2 backup** of the object graph from the client.

**Lore today.** Distribution is **server-centric** — reads/writes terminate at `loreserver` (QUIC/gRPC), with edge caches/read-replicas as a *server-fleet* concern. There is no client-driven, serverless, read-only snapshot of a repo for browse/clone without standing up a server, and no client-side "back up my repo to commodity object storage" path distinct from the production store.

**Lore-idiomatic landing.** Both are **present-different / L4**, and must respect Lore's server-centric identity rather than mimic Cloudflare:

- **Read-only snapshot publish** — export a revision's materialized tree (or a fragment manifest) to S3/CDN as a static, signed, read-only site, served without a `loreserver`. Reuses Lore's existing S3 backend (`lore-aws`) and content-addressing. *Decline* the Worker/D1 specifics (Cloudflare-bound).
- **Client-managed backup** — a `lore backup` verb that pushes the local immutable store to a second object-store target and records sync state, independent of the production server. Lore's CAS makes this naturally incremental.

These are lower priority than the AI layer and gated on a product decision about whether serverless read distribution belongs in Lore's centralized model at all.

## P2.5 Prioritized, phased completion plan (for Lore)

Each item: **what · why · feasibility · effort · dependencies · Lore-idiomatic approach.** Effort excludes each project's own per-command doc/test contract (Lore has its own equivalents: `docs/reference` regen, integration tests in `lore-integration-tests`, `lore --markdown-help`).

### Phase L0 — Quick parity wins (independent, reuse Lore's revision engine)

| # | Item | Why | Feasibility | Effort | Deps | Approach |
|---|---|---|---|---|---|---|
| L0.1 | **`lore blame <path>`** | Per-line provenance; data already exists | easy | 3–5 d | — | Read the existing woven per-file history (`NodeFileMetadata.revision[2]`); format per-line last-touched revision/author. Read-only; no engine change. |
| L0.2 | **`lore tag`** | Human-friendly immutable aliases beside `branch@N` | easy | 1 w | — | Named immutable refs in the mutable store; annotated tags carry typed metadata. Lore-native, not Git tags. |
| L0.3 | **`lore stash`** | Set work aside without a commit | moderate | 1–2 w | dirty-set | Snapshot staged anchor + dirty-set into a stash object in storage; `pop`/`apply`/`drop`/`list`. |
| L0.4 | **`lore shortlog` / `lore describe`** | Release notes + human-readable revision names | easy | 3–5 d | L0.2 (describe) | Aggregate the revision DAG + `created-by` metadata; `describe` = nearest tag + revision-number offset. |
| L0.5 | **`lore grep`** | Content search across the working set | easy | 1 w | — | Regex over materialized files (+ optional fragment scan); context/binary flags. |
| L0.6 | **Client hook lifecycle** | `pre-commit`/`commit-msg`/`pre-push` gating | moderate | 1–2 w | — | Hook runner invoked from commit/push/stage; templates under `.lore/`. Foundation for AI-lifecycle hooks. |
| L0.7 | **`lore note` (free-form)** | Mutable post-hoc annotation on a revision | easy | 3–5 d | — | Mutable-metadata pointer; complements immutable typed metadata. |

### Phase L1 — History-rewrite engine (aligns with Lore's own roadmap)

| # | Item | Why | Feasibility | Effort | Deps | Approach |
|---|---|---|---|---|---|---|
| L1.1 | **`lore rebase` (+ interactive)** | On Lore's roadmap; replay chains onto a new parent | hard | 1–2 mo | MergeType engine; mutable-store sequencer | Replay revisions via the existing three-way engine; conflict UX reuses `merge resolve mine/theirs`; persist `RebaseSequence` state in the **mutable CAS store**; preserve author, set committer. |
| L1.2 | **`lore squash`** | On Lore's roadmap; collapse a chain into one revision | moderate | 2–3 w | L1.1 sequencer | Distinct verb (not cherry-pick); reuse the L1.1 sequence-state + merge engine. |
| L1.3 | **`merge --autostash` / `rebase --autostash`** | Survive uncommitted edits across rewrite | easy | 3–5 d | L0.3 stash | Auto-stash dirty state pre-op, restore post-op. |
| L1.4 | **Global `reflog`** | Recover lost revisions/branch tips after rewrite | moderate | 1–2 w | mutable store | Generalize per-branch latest-history into a per-ref movement log with `expire`. |

### Phase L2 — AI substrate (the headline gap; new `lore-agent` crate)

| # | Item | Why | Feasibility | Effort | Deps | Approach |
|---|---|---|---|---|---|---|
| L2.1 | **Completion + provider adapters** | No LLM access today; the irreducible base | moderate | 3–5 w | — | `lore-agent::completion` + provider clients (anthropic/openai/deepseek/gemini/ollama/…) over `lore-transport` rustls; retry/throttle/JSON-repair. Provider-agnostic; zero Lore coupling. |
| L2.2 | **MCP server over `lore service`** | Standard tool/agent protocol for editors & external agents | moderate | 2–3 w | L2.1; existing service IPC | Serve MCP over the **existing UDS IPC daemon** + `lore-notification`; resource schema via `lore-proto` conventions. Big head-start vs Libra. |
| L2.3 | **Tool registry over revision/storage** | Agents must read/edit/run inside the repo | moderate→hard | 3–5 w | L2.2 | `read_file`/`grep` over lore-storage fragments; `apply_patch`/`stage` through the staged anchor + dirty-set; `plan`. No Git index. |
| L2.4 | **Agent command sandbox** | Safe `shell` execution for agents | moderate→hard | 3–4 w | L2.3 | Port Libra's command-safety policy + macOS seatbelt / Linux bwrap (substrate-neutral). Lore's first execution sandbox. |
| L2.5 | **Agent session store** | Durable, resumable agent runs; per-agent isolation | moderate | 2 w | L2.1; instances | Sessions as content-addressed blobs (or typed metadata); isolate with **instances over shared-store**. |
| L2.6 | **Usage accounting** | Cost/observability for model calls | moderate | 1–2 w | L2.1; lore-telemetry | Ledger + OTLP spans via `lore-telemetry`; `lore usage` query. |

### Phase L3 — AI orchestration & autonomy (control plane on L2)

| # | Item | Why | Feasibility | Effort | Deps | Approach |
|---|---|---|---|---|---|---|
| L3.1 | **Orchestrator (plan/decide/execute/verify/replan)** | Multi-step autonomous work, not single calls | hard | 1–2 mo | L2.3–L2.5 | New `lore-agent::orchestrator` with gate/ACL; agent worktrees = **instances over shared-store** (native parallelism). |
| L3.2 | **Goal / supervisor / verifier** | Bounded, checkable autonomous tasks | hard | 1 mo | L3.1 | Goal spec + supervisor loop + independent verifier; reuse Lore's three-way merge for workspace integration. |
| L3.3 | **Skills** | Reusable, discoverable agent capabilities | moderate | 2–3 w | L2.3 | Loader/scanner/dispatcher; skills content-addressed, optionally partition-scoped. |
| L3.4 | **Automation rules + scheduler** | Event-driven agent triggers | moderate | 3–4 w | L2.2; lore-notification | Rule runtime **subscribing to `lore-notification`** (push/create/delete, lock/unlock) — a trigger source Libra lacks. |
| L3.5 | **context-budget / prompt / intentspec / projection / permission** | Context-window discipline, provenance, gating | moderate→hard | 1 mo | L2.1–L2.5 | Agent-internal modules; **permission rides Lore's JWT/REBAC**. |
| L3.6 | **observed-agent capture + codex bridge** | Interop with external agents (Claude Code/Gemini/Codex) | moderate | 2–3 w | L2.2 | Capture adapters + redaction/preview. |

### Phase L4 — Interactive & distribution surfaces

| # | Item | Why | Feasibility | Effort | Deps | Approach |
|---|---|---|---|---|---|---|
| L4.1 | **`lore code`-style agent TUI** | Human-facing agent loop in the terminal | moderate→hard | 1–2 mo | L2/L3 | New `lore-agent-tui` reusing **lore-client's TUI/paging/progress** infra. |
| L4.2 | **AI usage dashboards** | Fleet cost/latency visibility | low–moderate | 1–2 w | L2.6 | Render the usage ledger; OTLP export already in place. |
| L4.3 | **Read-only serverless snapshot publish** | Browse/clone without standing up a server | moderate | 3–4 w | — | Static signed export to S3/CDN via `lore-aws`; *decline* Cloudflare/D1 specifics. Present-different vs `libra publish`. |
| L4.4 | **Client-managed backup** | Back up the local store to a second target | moderate | 2–3 w | — | `lore backup` pushes the immutable store to a second object-store target; CAS makes it incremental. |

### Later / Defer (non-goals — conflict with Lore's identity)

See §P2.6.

## P2.6 Explicit non-goals for Lore (do NOT copy from Libra)

The symmetric counterpart of §6. Each is a Libra trait Lore should **not** adopt, because it conflicts with Lore's defining choices.

| Libra idea | Why Lore should not adopt it |
|---|---|
| **Git on-disk object format (loose/pack/index, SHA-1/SHA-256 OIDs)** | Breaks Lore's BLAKE3 content-addressing, FastCDC fragmentation, and partition model — Lore's defining promise, the mirror of Libra's Git-compat promise. |
| **The Git index / stage-time snapshot model** | Lore deliberately has **no index** — the filesystem is ground truth and dirty/staged are flags. Adding an index forfeits the no-index ergonomics and the notify/scan/verify model. |
| **SQLite as the canonical ref/HEAD/config store** | Lore's mutable CAS store **is** its single serialization point; a second SQLite source of truth would fracture consistency. Use the mutable store for sequencer/reflog/tag state instead. |
| **Git smart-HTTP/SSH/git:// + Git-LFS wire protocols** | Lore has its own versioned QUIC + gRPC storage protocol; bolting on Git transports breaks its protocol story and buys nothing (Lore talks to `loreserver`, not Git hosts). |
| **libvault (Libra's AES-256-GCM secret store)** | Lore already has **OS-keyring + HashiCorp Vault + JWT** (`lore-credential`/`lore-hashicorp`) — equal or ahead. Reuse those for agent credentials. |
| **Cloudflare Worker / D1-specific publish + backup** | Vendor-bound. The *capability* (read-only snapshot, client backup) is L4.3/L4.4 over Lore's own S3 backend; the Cloudflare specifics are declined. |
| **commit *trailers* as the provenance mechanism** | Lore already has **typed metadata KV** on revision/branch/repo/file — richer and queryable. Use it instead of parsing free-text trailers. |
| **Per-worktree SQLite `instance_id` namespacing (Libra plan 2.1)** | Lore already solves this natively with **instances + shared-store + partitions**. Don't import Libra's retrofit; use the native model. |

## P2.7 Sequencing, risks & open questions (reverse direction)

**Sequencing.**
- **L2 before L3 before L4** is hard-ordered: orchestration (L3) is meaningless without tools+sandbox+session (L2), and the TUI (L4) renders both. Within L2: **L2.1 → L2.2 → L2.3 → L2.4** (each strictly enables the next); L2.5/L2.6 can land in parallel once L2.1 exists.
- **L0 and L1 are independent of the AI track** and can proceed in parallel with L2 — they touch the revision engine, not the agent crate. The **L1 mutable-store sequencer** (rebase/squash) should be built once and reused by both verbs (the mirror of Part I's "build the ordinal cache once").
- **L1 aligns with Lore's published roadmap** (rebase/squash) — coordinate with Lore maintainers so the engine work is not duplicated upstream.
- **Client hooks (L0.6)** should land before L3 so AI-lifecycle hooks have a runner to extend.

**Top risks.**
- **Scope realism.** The AI substrate (L2+L3) is a multi-quarter program — comparable in size to a large fraction of Libra's `src/internal/ai/`. It is the headline gap precisely because it is large; phase it (L2 alone is a shippable milestone: model + tools + sandbox + session).
- **Don't fork the storage/revision crates.** Everything new lives in `lore-agent`/`lore-agent-tui` and *consumes* lore-storage/lore-revision through their public APIs. Any change that reaches into the CAS/fragment internals is a smell — re-check against §P2.6.
- **Sandbox parity across platforms.** Lore targets the same OS matrix; port seatbelt/bwrap carefully and fail closed when no sandbox backend is available.
- **Provider-secret handling.** Route model API keys through Lore's existing keyring/JWT credential store (§P2.6), never a new bespoke store.
- **Upstream alignment.** Lore is an Epic open-source project with its own governance (LEPs, roadmap). A serious Libra→Lore contribution should enter as an **LEP**, especially the AI substrate, which is a strategic direction change for a "binary-first" VCS. This plan is the design input to such an LEP, not a unilateral fork.

**Open questions.**
1. **Is an AI substrate in scope for Lore at all?** Lore's stated identity is binary-first, scale-first version control; an agent runtime is a strategic addition. This plan assumes "yes, idiomatically"; the maintainers may scope it to the MCP/tool surface (L2.2–L2.3) only and stop short of full orchestration.
2. **Where does agent state live** — content-addressed blobs, typed metadata, or a new mutable namespace? Each has different obliteration/GC implications under Lore's model.
3. **Do instances/partitions fully cover agent isolation,** or is a lighter per-run scratch space needed for ephemeral agent work that should never become a revision?
4. **Serverless distribution vs centralized identity.** Does read-only snapshot publish (L4.3) belong in a system whose whole point is a canonical server, or is that a Libra-only ergonomic that Lore should decline?

---

## Appendix B — Libra capability/command → Lore mapping

The inverse of Appendix A. Every notable Libra capability either has a Lore equivalent today, maps to a Part II plan item, or is an explicit non-goal.

| Libra capability / command | Lore analog today | Plan / verdict |
|---|---|---|
| `libra code` (agent TUI + web/MCP server) | none (GUI clients on roadmap) | L4.1 (+ L2/L3 substrate) |
| `libra agent` / `agent_run` records | none | L2 / L3 |
| `libra automation` | none | L3.4 (on `lore-notification`) |
| MCP server (`src/internal/ai/mcp`) | `lore service`/`notification` UDS IPC (transport only) | L2.2 |
| AI providers + `CompletionModel` (`ai/providers`, `ai/completion`) | none | L2.1 |
| Tool registry (`ai/tools`: read_file/apply_patch/shell/grep/plan) | `file stage`/`unstage`, storage reads (no agent harness) | L2.3 |
| Command sandbox (`ai/sandbox`: seatbelt/bwrap) | none | L2.4 |
| Agent session store (`ai/session`) | instances/shared-store (isolation only) | L2.5 |
| Usage accounting (`ai/usage`) | `lore-telemetry` (OTLP) partial | L2.6 |
| Orchestrator/goal (`ai/orchestrator`, `ai/goal`) | none | L3.1 / L3.2 |
| Skills (`ai/skills`) | none | L3.3 |
| context-budget/prompt/intentspec/projection/permission | none (JWT/REBAC for permission) | L3.5 |
| observed-agents / codex bridge | none | L3.6 |
| `libra rebase` | roadmap (cherry-pick workaround) | L1.1 |
| squash (`rebase -i` / `merge --squash`) | roadmap | L1.2 |
| `libra stash` | none | L0.3 |
| `libra reflog` | branch latest-history (partial) | L1.4 |
| `libra blame` | woven per-file history (internal, no verb) | L0.1 (expose) |
| `libra tag` | none (uses `branch@N` numbers) | L0.2 |
| `libra grep` | none | L0.5 |
| `libra shortlog` / `describe` | `revision history` partial | L0.4 |
| client `hooks` lifecycle | server hooks only | L0.6 |
| `libra notes` | typed metadata KV (present-different) | L0.7 (free-form note) |
| `libra cherry-pick` / `revert` / `bisect` / `merge` | `revision cherry-pick`/`revert`/`bisect`, three-way `merge` | present |
| `libra publish` (read-only CF Worker over D1/R2) | server tiering (present-different) | L4.3 (decline CF specifics) |
| `libra cloud` (D1/R2 incremental backup) | S3/replication (server-tier) | L4.4 (present-different) |
| `libra completions` | `lore completions` | present (Lore parity) |
| `libra auth` (Part I 1.6, basic-auth today) | `lore auth` + JWT + OS-keyring + HashiCorp Vault | **present; Lore ahead** |
| OTLP / telemetry | `lore-telemetry` | present (Lore parity) |
| libvault secret storage | `lore-credential` (keyring) + `lore-hashicorp` | present-different (Lore ahead) — non-goal §P2.6 |
| Git on-disk format / index / SQLite refs / Git protocols | Lore CAS / no-index / mutable store / QUIC+gRPC | **non-goal (§P2.6)** |

> **How to read the table.** Same convention as Appendix A: "present" = Lore already ships an idiomatic equivalent; "present-different" = the capability exists in another shape; "Lore ahead" = Lore's version is more capable than Libra's; an `L#.#` is a Part II plan item; "non-goal" = deliberately declined to protect Lore's identity (§P2.6).
