# Operation Log 移植开发文档（A-1 至 A-5）

## 0. 文档目的

本文档汇总成员 A 在 A-1 至 A-5 阶段的完整交付：

1. 冻结需求边界与验收口径。
2. 识别状态写入路径与风险分级。
3. 完成 operation 数据模型与索引设计。
4. 定义数据库迁移、兼容与回滚策略。
5. 定义 with_operation_log 事务语义与接口契约。

目标是为后续实现阶段提供单一事实来源，确保“不遗漏、不重复、可直接执行”。

---

## 1. 全局约束（跨 A-1 至 A-5）

### 1.1 设计原则

1. operation log 与现有 reflog 并存，不替换。
2. 命令级审计（operation）与 ref 级审计（reflog）职责分离。
3. 业务写入与 operation 记录必须事务原子一致。
4. 先交付最小可用（op log/show/restore），再扩展并发与高级能力。

### 1.2 工程与质量约束

1. 生产代码禁止 `unwrap()`/`expect()`（测试或显式 INVARIANT 场景除外）。
2. 错误必须可读、可定位、可操作。
3. migration 必须幂等、非破坏、可回退。
4. 变更必须具备集成测试与回归测试入口。

### 1.3 本期目标能力

1. 新增 operation 数据层。
2. 新增统一事务包装器 with_operation_log。
3. 支持 op log / op show / op restore。

---

## 2. A-1：需求冻结与范围定义

### 2.1 目标

冻结首期范围，防止实现期范围漂移。

### 2.2 In Scope（本期必须交付）

1. operation 主记录、父关系、视图快照数据模型。
2. SQLite 增量迁移与旧仓库自动补齐。
3. with_operation_log 统一事务包装语义。
4. 命令能力：op log、op show、op restore。
5. 与 reflog 并存的兼容策略与回归要求。

### 2.3 首批接入命令（冻结列表）

1. commit
2. merge
3. rebase
4. reset
5. switch
6. cherry-pick
7. clone
8. fetch
9. push

### 2.4 A-1 DoD

1. 范围文档明确 in/out。
2. 首批命令列表冻结。
3. 兼容与质量约束冻结。
4. B/C 评审无阻塞问题。

---

## 3. A-2：状态写入点盘点与风险分级

### 3.1 目标

识别所有仓库状态写入路径，建立“命令 x 状态”矩阵，作为统一接入基线。

### 3.2 盘点维度

1. HEAD 写入
2. Branch/Ref 写入
3. Reflog 写入
4. RebaseState 写入
5. 当前事务包装方式（with_reflog / 手动事务 / 混合）

### 3.3 结果矩阵（汇总）

1. 已较规范（with_reflog）：commit、clone、merge、reset、switch、cherry-pick。
2. 手动事务 + 手动 reflog：fetch、push。
3. 混合路径（高风险）：rebase（既有 with_reflog，也有多处直写 HEAD 与 rebase_state）。
4. 手动事务但无统一审计：bisect（HEAD 更新路径）。
5. 初始化直写引用：init（创建 HEAD/intent branch）。

### 3.4 风险分级

#### 高风险

1. rebase：同命令内存在多种写入方式，易导致审计链不完整。
2. fetch/push：日志写入风格与其他命令不统一，后续接入 operation 易漏字段或行为不一致。

#### 中风险

1. bisect：更新 HEAD 但缺少统一 operation 语义。
2. init：初始化引用写入未纳入统一审计流程。

#### 低风险

1. 已用 with_reflog 的命令整体可迁移性较好，主要是统一包装改造成本。

### 3.5 A-2 DoD

1. 状态写入命令识别完成。
2. 每类状态写入路径归档完成。
3. 高中低风险清单可供排期。
4. 可进入 A-3 模型设计。

---

## 4. A-3：数据模型与索引设计

### 4.1 目标

建立可支撑 op log/show/restore 的最小且可扩展模型。

### 4.2 实体与关系

1. `operation`：一次操作主记录。
2. `operation_parent`：操作父边关系（支持 DAG 扩展）。
3. `operation_view`：操作完成时仓库视图主快照。
4. `operation_view_ref`：视图下引用快照。
5. `operation_view_workspace`：视图下工作区指针快照。

关系约束：

1. operation 与 operation_view：1:1（当前版本）。
2. operation 与 operation_parent：1:N（父边）。
3. operation_view 与 ref/workspace：1:N。

### 4.3 字段设计关键点

1. oid 字段统一 TEXT，不假设 sha1/sha256 固定长度。
2. operation 必含：op_id、repo_id、view_id、command_name、description、actor、start/end_ts、status。
3. operation_parent 复合主键：(op_id, parent_op_id)。
4. operation.view_id 唯一，避免重复绑定。
5. view 中保存最小恢复必要信息：HEAD、refs、workspace 指针。

### 4.4 索引策略（按查询路径）

1. op log：`(repo_id, end_ts DESC)`。
2. op show：`operation(op_id)`、`operation(view_id)`、`operation_view_ref(view_id, ref_kind, ref_name, ref_remote)`。
3. 父链遍历：`operation_parent(parent_op_id, op_id)`。
4. restore：`operation_view(repo_id, created_at DESC)`。

### 4.5 约束与一致性

1. operation/view/parent 同事务提交。
2. 快照是“提交后最终状态”，不是写前状态。
3. command 参数只存 digest，避免敏感信息泄漏。

### 4.6 A-3 DoD

1. ER 与字段字典冻结。
2. 索引策略冻结。
3. 一致性规则冻结。
4. 可进入 A-4 迁移方案。

---

## 5. A-4：迁移、兼容与回滚方案

### 5.1 目标

在不影响现有仓库可用性的前提下落地 operation schema。

### 5.2 迁移文件策略

1. 新增增量 SQL（建议命名：`sqlite_20260426_operation_log.sql`）。
2. 仅新增对象：表、索引、约束。
3. 使用 `IF NOT EXISTS`，保证重复执行安全。

### 5.3 应用接入顺序

1. 现有 bootstrap schema。
2. 现有 AI 迁移。
3. operation 迁移（新增）。

### 5.4 幂等与非破坏原则

1. 不删除、不重命名旧表。
2. 不修改现有 `reference/reflog` 表行为。
3. 迁移失败时，旧功能继续可用（仅 operation 能力不可用）。

### 5.5 失败场景与处置

1. SQL/对象冲突：终止 operation 初始化，保留旧能力。
2. 锁冲突：复用 busy timeout/retry。
3. 权限/IO 异常：给出可操作错误并终止本次升级。

### 5.6 回滚策略

1. 不自动 drop 新表。
2. 通过配置/feature 开关临时禁用 operation 写入路径。
3. 保证命令仍可走现有 reflog/reference 流程。

### 5.7 A-4 DoD

1. 迁移 DDL 冻结。
2. 初始化接入顺序冻结。
3. 失败回滚策略冻结。
4. 可进入 A-5 事务接口设计。

---

## 6. A-5：with_operation_log 事务语义与接口契约

### 6.1 目标

统一所有状态变更命令的审计入口，消除分散写入。

### 6.2 统一接口（概念）

1. 输入：
	- `OperationMeta`（command_name、description、actor、repo_id、args_digest）
	- `OperationScope`（是否采集 refs/workspace/remote-tracking）
	- 业务闭包（接收 `&DatabaseTransaction`）
2. 输出：
	- `OperationResult<T>`（业务返回 + op_id/view_id/end_ts）
3. 错误：
	- `OperationError` 分阶段错误（begin/business/snapshot/persist/commit/rollback）

### 6.3 强制事务时序

1. 预处理（生成 op_id/view_id、读取 parent、记录 start_ts）。
2. 开启事务。
3. 执行业务写入闭包（必须使用 `*_with_conn`）。
4. 采集事务内最终视图（HEAD/refs/workspace）。
5. 持久化 operation + parent + view。
6. 提交事务。
7. 返回结果。

失败语义：任一步失败即回滚，禁止半写入。

### 6.4 命令接入约束（给 B）

1. 命令层必须通过 with_operation_log 接入。
2. 禁止绕过包装器直接写 operation 表。
3. 闭包内禁止开新连接做并行写。
4. 允许保留 reflog，但其写入与 operation 应保持同事务一致性策略。

### 6.5 parent 规则（本期）

1. 默认单 parent：取当前最近成功 operation。
2. 无 parent：写 root operation（无父边）。
3. 多 parent 为后续扩展，不在本期实现。

### 6.6 幂等与重试

1. 用户重试命令产生新 op_id（多条记录是预期行为）。
2. 通过唯一约束防止父边重复与 view 绑定重复。

### 6.7 最小测试契约（给 C）

1. 成功路径：operation/view/parent 完整落库。
2. 失败路径：业务失败与快照失败均全量回滚。
3. 重试路径：多次执行链路正确。
4. 与 reflog 并存路径：审计一致性符合策略。

### 6.8 A-5 DoD

1. 接口签名与错误分类冻结。
2. 事务时序冻结。
3. B 接入契约冻结。
4. C 测试契约冻结。
5. 可进入 A-6 实现阶段。

---

## 7. 阶段出口与下一步

A-1 至 A-5 已形成完整“设计与契约层”闭环，可直接进入实现：

1. A-6：DAO/Service 模块实现。
2. A-7：with_operation_log 代码落地。
3. B 阶段：首批命令接入。
4. C 阶段：测试矩阵执行与回归收敛。

本文件即 A 阶段前半段（A-1~A-5）的唯一基线文档。
