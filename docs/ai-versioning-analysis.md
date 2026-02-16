# Libra AI 版本管理能力分析

> 分析日期: 2026-02-16
> 基于 Libra v0.1.0 当前代码库

## 概述

本文档分析 Libra 当前代码库中与 AI 版本管理相关的已满足条件和未满足条件。Libra 定位为 "AI Agent-Native 版本控制系统"，在 Git 兼容的基础上创新性地将 AI 工作流对象纳入版本控制体系。

---

## 一、已满足的条件

### 1. 内容寻址对象存储

**位置**: `src/utils/storage/`

- 标准 Git 对象格式 (blob, tree, commit, tag)，SHA-1 和 SHA-256 双哈希支持
- 松散对象 + Pack 文件两种存储格式
- Zlib 压缩
- `Storage` trait 提供统一的异步存储接口 (`get`/`put`/`exist`/`search`)

**意义**: AI 模型文件、权重文件、数据集等任意二进制文件均可作为 blob 对象存储，天然具备内容去重和完整性校验。

### 2. 分层存储系统 (Tiered Storage)

**位置**: `src/utils/storage/tiered.rs`

- 本地文件系统 (`LocalStorage`) + S3 兼容远程存储 (`RemoteStorage`)
- 按文件大小阈值 (默认 1MB) 自动分层:
  - 小对象: 本地 + 远程双写 (永久)
  - 大对象: 远程为主，本地 LRU 缓存
- 支持 AWS S3、Cloudflare R2、MinIO 等后端
- LRU 缓存自动清理，可配置磁盘用量上限

**意义**: 为 AI 大文件 (模型权重通常 GB 级别) 提供了基础的分层存储能力，远程持久化 + 本地缓存的模式适合 AI 场景。

### 3. AI 工作流过程对象体系

**位置**: `src/internal/ai/mcp/resource.rs`, `git-internal` crate

已实现 10 种结构化过程对象:

| 对象类型 | 用途 | 关键字段 |
|---------|------|---------|
| **Intent** | 用户意图/Prompt | content, status, parent_id, commit_sha |
| **Task** | 任务定义 | title, goal_type, constraints, acceptance_criteria |
| **Run** | 执行记录 | task_id, base_commit_sha, status, agent_instances, metrics |
| **Plan** | 执行计划 | run_id, plan_version, steps |
| **PatchSet** | 代码补丁集 | run_id, generation, touched_files, apply_status |
| **Evidence** | 验证证据 | kind (test/lint/build), exit_code, summary |
| **ToolInvocation** | 工具调用记录 | tool_name, args, io_footprint, status |
| **Provenance** | 模型溯源 | provider, model, parameters, token_usage |
| **Decision** | 决策记录 | decision_type, chosen_patchset_id, rationale |
| **ContextSnapshot** | 上下文快照 | base_commit_sha, items, selection_strategy |

所有对象均:
- 使用 UUID v7 (时间有序) 作为标识
- 支持 JSON 序列化/反序列化
- 通过 `Identifiable` trait 统一接口
- 支持 Actor 归属 (human/agent/system/mcp_client)

**意义**: 已具备完整的 AI 工作流过程追踪能力，每次 AI 操作的意图、计划、执行、验证、决策全链路可追溯。

### 4. AI 历史分支管理 (HistoryManager)

**位置**: `src/internal/ai/history.rs`

- 专用孤立分支 `refs/libra/intent` 存储所有 AI 对象
- Git Tree 结构按类型组织: `intent/`, `task/`, `run/`, `plan/` ...
- 追加式提交历史，每次对象变更生成新 commit
- GC 安全: 对象从 ref 可达，不会被垃圾回收
- 支持按类型+ID 查找、跨类型查找、列举等操作

**意义**: AI 元数据与代码历史并行存储、互不干扰，同时共享 Git 基础设施获得不可变性和可审计性。

### 5. MCP 服务器接口

**位置**: `src/internal/ai/mcp/server.rs`, `src/internal/ai/mcp/resource.rs`

- 实现标准 Model Context Protocol (MCP)
- 20+ 工具方法用于 AI 对象的 CRUD 操作
- 资源 URI 系统:
  - `libra://history/latest` - 当前 AI 分支 HEAD
  - `libra://context/active` - 活跃的 Run/Task/ContextSnapshot
  - `libra://objects/{type}` - 按类型列举对象
  - `libra://object/{id}` - 读取单个对象的完整 JSON

**意义**: 外部 AI 客户端可通过标准协议与 Libra 交互，是 AI 版本管理的标准化接口。

### 6. AI 溯源追踪 (Provenance)

**位置**: `git-internal` crate 中的 `Provenance` 对象

- 记录 AI 提供商 (provider) 和模型名称 (model)
- 支持存储模型参数 JSON (parameters_json)
- 支持存储 Token 用量 JSON (token_usage_json)
- 每个对象关联具体 Run ID
- Actor 身份标记

**意义**: 可追踪 "哪个模型、什么参数、在什么时候、产生了什么结果"，是 AI 可审计性的基础。

### 7. Git LFS 支持

**位置**: `src/command/lfs.rs`, `src/utils/lfs.rs`

- 完整的 Git LFS 协议实现 (track/untrack/lock/unlock/ls-files)
- 指针文件机制，大文件不直接进入 Git 对象库
- 文件锁定机制，防止并发修改冲突

**意义**: 为大型 AI 模型文件提供了现成的存储和传输方案。

### 8. SQLite 引用管理

**位置**: `src/internal/model/`, `sql/sqlite_20240331_init.sql`

- 分支、标签、HEAD 通过 SQLite 管理
- Reflog 记录所有引用变更历史
- 事务安全操作

**意义**: 比纯文件的 Git ref 更可靠，为 AI 分支管理提供了数据库级别的一致性保证。

### 9. 多 LLM Provider 代理框架

**位置**: `src/internal/ai/providers/`, `src/internal/ai/agent/`

- 5 个 LLM 提供商 (Anthropic, OpenAI, Gemini, DeepSeek, Zhipu)
- 可插拔 `CompletionModel` trait
- 工具调用框架 (`Tool` trait, `ToolSet`, `ToolRegistry`)
- 有状态对话代理 (`ChatAgent`)
- DAG 工作流集成 (`node_adapter.rs`)

**意义**: AI Agent 基础设施完备，可驱动自动化的版本管理流程。

### 10. 存储扩展层 (StorageExt)

**位置**: `src/utils/storage_ext.rs`

- `put_json` / `get_json` — 结构化对象的序列化存储
- `put_tracked` — 自动纳入历史管理的对象存储
- `put_artifact` — 原始二进制内容存储，返回带完整性哈希的 `ArtifactRef`
- 统一的 `Identifiable` trait 适配所有 10 种过程对象

**意义**: 在底层 Storage 之上提供了类型安全的高层 API，降低了 AI 对象管理的使用门槛。

---

## 二、未满足的条件

### 1. AI 模型文件的专用管理

**缺失项**:
- 无模型文件格式感知: 不识别 SafeTensors、ONNX、PyTorch (.pt/.pth)、TensorFlow (SavedModel)、GGUF 等格式
- 无模型元数据自动提取: 无法从模型文件解析架构信息、层数、参数量等
- 无模型专用 chunking 策略: 模型权重文件 (通常 GB 级) 需要分块存储和传输
- 无二进制增量压缩: 相邻 checkpoint 间大量权重相似，缺少针对 tensor 的 delta 编码

**现状**: 虽然可通过 `Storage::put` 存储任意二进制文件，且 LFS 支持大文件，但均为通用处理，没有 AI 专用优化。

### 2. 模型注册中心 (Model Registry)

**缺失项**:
- 无 "Model" 一等公民对象: 现有 10 种过程对象中无 Model 类型
- 无模型版本目录: 缺少 `model_name -> [v1, v2, v3, ...]` 的版本链管理
- 无模型元数据 schema: 架构、框架、超参数、性能指标等
- 无模型标签系统: 如 `production`、`staging`、`canary`、`v1.0` 等语义标签
- 无模型血缘图: 预训练 → 微调 → 部署的模型谱系

**影响**: 用户无法在 Libra 中 "注册" 一个 AI 模型并追踪其版本演进。

### 3. 数据集版本管理

**缺失项**:
- 无数据集对象或元数据 schema
- 无数据集内容变更追踪 (新增/删除/修改了哪些样本)
- 无数据血缘追踪 (原始数据 → 清洗 → 增强 → 训练集)
- 无数据集拆分元数据 (train/validation/test 的划分记录)
- 无数据集统计信息存储 (样本数、分布、特征等)

**影响**: 训练数据的可复现性无法保证。

### 4. 实验管理与对比

**缺失项**:
- `Run` 对象存在但缺少训练指标时间序列 (loss, accuracy 按 step/epoch)
- 无超参数版本化管理
- 无实验对比功能 (A vs B 的指标对比)
- 无可视化集成 (TensorBoard, W&B 等)
- 无实验搜索/过滤 (按指标范围、按超参数值等)

**现状**: `Run` 对象有 `metrics_json` 字段，但仅为扁平 JSON，无结构化时间序列支持。

### 5. AI 对象的 Diff / Merge

**缺失项**:
- 无模型权重二进制 diff: 无法展示两个模型版本间的权重差异
- 无模型架构语义 diff: 无法对比两个模型的结构变化
- 无 AI 对象三方合并: 并行修改同一 Task/Run 时无冲突检测
- 无 AI 对象冲突解决策略

**现状**: 代码 diff/merge 完备 (`src/command/diff.rs`, `src/command/merge.rs`)，但 AI 对象层无此能力。

### 6. Pipeline / 工作流版本管理

**缺失项**:
- `Plan` 对象存在但缺少 DAG 式 pipeline 定义
- 无可复现的 pipeline 版本化 (数据处理 → 训练 → 评估 → 部署)
- 无环境/依赖追踪 (Python 版本、CUDA 版本、pip/conda 依赖)
- 无容器镜像版本关联

**现状**: `dagrs` 依赖存在但仅用于 Agent tool loop，未扩展到 ML pipeline。

### 7. 部署版本追踪

**缺失项**:
- 无模型部署记录 (哪个模型版本部署在哪个环境)
- 无 A/B 测试基础设施
- 无模型级别的回滚机制
- 无模型 serving 配置版本管理

**现状**: `Decision` 对象的 `DecisionType` 包含 `Commit`/`Rollback` 等，但面向代码而非模型部署。

### 8. 并发写入安全

**缺失项**:
- `HistoryManager::append` 通过文件写入更新 ref，无原子性保证
- 多进程/多 Agent 并发写入 AI 分支时存在竞态条件
- 无分布式锁或乐观并发控制
- AI 分支的 ref 管理未使用 SQLite (与代码分支的管理方式不一致)

**现状**: 代码分支使用 SQLite 管理 (事务安全)，但 AI 历史分支使用裸文件 (`refs/libra/intent`)。

### 9. 对象关系图查询

**缺失项**:
- 对象间通过 UUID 关联但无图查询 API
- 无法一步获取 `Intent → Task → Run → PatchSet → Evidence → Decision` 完整链路
- 无对象关系可视化
- 无反向查询 (给定 commit SHA，查找所有关联的 AI 对象)

**现状**: 各 `list_*` 方法按类型列举，跨类型查询需要客户端自行遍历关联。

### 10. AI 对象生命周期管理

**缺失项**:
- 无 AI 对象的保留策略 (保留最近 N 次 Run、自动清理失败的 Run 等)
- 无过期/归档机制
- 无存储用量统计和告警
- 孤立分支持续增长无上限

**现状**: GC 安全 (不会误删)，但缺少主动的生命周期管理。

### 11. AI 专用 CLI 命令

**缺失项**:
- 无 `libra model` 命令族 (register/list/diff/checkout/deploy)
- 无 `libra dataset` 命令族 (track/version/diff/split)
- 无 `libra experiment` 命令族 (list/compare/reproduce)
- 无 `libra ai log` 命令 (查看 AI 对象历史)

**现状**: `libra code` 提供交互式 AI 编码界面，AI 对象仅通过 MCP 工具操作，无直接 CLI 支持。

### 12. 外部系统集成

**缺失项**:
- 无 Hugging Face Hub 集成 (推拉模型)
- 无 MLflow / Weights & Biases / Neptune 兼容
- 无 DVC (Data Version Control) 互操作
- 无容器注册中心集成
- 无模型 serving 框架集成 (TorchServe, Triton, vLLM)

**现状**: MCP 标准协议提供了扩展点，但无开箱即用的外部 ML 平台集成。

---

## 三、总结矩阵

| 能力维度 | 状态 | 说明 |
|---------|------|------|
| 内容寻址存储 | ✅ 满足 | Git 标准对象存储 |
| 分层存储 (本地+远程) | ✅ 满足 | Local + S3 + LRU |
| 大文件支持 (LFS) | ✅ 满足 | 完整 LFS 协议 |
| AI 过程对象体系 | ✅ 满足 | 10 种对象类型 |
| AI 历史分支 | ✅ 满足 | 孤立分支 + Tree 组织 |
| MCP 标准接口 | ✅ 满足 | 工具 + 资源 |
| 溯源追踪 | ✅ 满足 | Provenance + Actor |
| 引用管理 | ✅ 满足 | SQLite + Reflog |
| Agent 框架 | ✅ 满足 | 多 Provider + 工具 |
| 对象序列化层 | ✅ 满足 | StorageExt |
| 模型文件专用处理 | ❌ 缺失 | 无格式感知/分块/增量 |
| 模型注册中心 | ❌ 缺失 | 无 Model 对象类型 |
| 数据集版本管理 | ❌ 缺失 | 无 Dataset 对象 |
| 实验管理与对比 | ⚠️ 部分 | Run 存在但缺少指标时间序列 |
| AI 对象 Diff/Merge | ❌ 缺失 | 无二进制/语义 diff |
| Pipeline 版本管理 | ⚠️ 部分 | Plan 存在但无 DAG pipeline |
| 部署版本追踪 | ❌ 缺失 | Decision 存在但面向代码 |
| 并发写入安全 | ⚠️ 部分 | 代码分支安全，AI 分支存在竞态 |
| 对象关系图查询 | ❌ 缺失 | UUID 关联但无图 API |
| 对象生命周期管理 | ❌ 缺失 | 无保留/清理策略 |
| AI 专用 CLI | ❌ 缺失 | 仅 MCP 工具操作 |
| 外部 ML 平台集成 | ❌ 缺失 | 无开箱即用集成 |

---

## 四、优先级建议

基于当前基础设施成熟度和 AI 版本管理的核心需求，建议实现优先级:

1. **P0 (基础)**: Model 对象类型 + 模型注册中心 — 使模型成为一等公民
2. **P0 (基础)**: AI 对象并发安全 — 将 AI 分支 ref 迁移到 SQLite
3. **P1 (核心)**: AI 专用 CLI 命令 — `libra model` 系列
4. **P1 (核心)**: 对象关系图查询 — 支持全链路追踪
5. **P2 (增强)**: 模型文件专用处理 — 格式感知、增量存储
6. **P2 (增强)**: 实验管理增强 — 指标时间序列、实验对比
7. **P3 (生态)**: 外部系统集成 — Hugging Face Hub 等
8. **P3 (生态)**: 数据集版本管理
