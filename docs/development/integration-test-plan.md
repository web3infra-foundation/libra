# Libra 集成测试计划（可落地执行版）

> 目标：把当前仓库的集成测试能力整理成一份“今天就能跑”的执行手册。
> 原则：只写仓库里真实存在的命令、测试文件、脚本和 gate；未实现能力统一标记为 `BASELINE_GAP-*`。

---

## 0. TL;DR（AI agent / Reviewer 必读，5 分钟版）

**默认阻断门**：Wave 0 + Wave 1 + Wave 2 全绿。Wave 3/4/5/6 按需启用。

**测试引用规范**（跨 agent 沟通统一使用）：

- 文件级：`code_ui_remote_lease_matrix`（即 `cargo test --test` 后跟的名字）
- 测试级：`code_ui_remote_lease_matrix::lease_expires_after_ttl`（三段式）
- 不要用文件相对路径或行号——rename / refactor 会失效；三段式稳定

**4 条最常用命令**：

```bash
# Wave 0：编译 + 格式 + lint + 文档一致性
cargo +nightly fmt --all --check && \
  cargo clippy --all-targets --all-features -- -D warnings && \
  cargo test --no-run --all-targets --features test-provider,test-network,test-live-ai,test-live-cloud && \
  bash scripts/check_compat_matrix.sh && bash scripts/check_docs_consistency.sh && \
  bash scripts/check_integration_plan_consistency.sh

# Wave 1：命令层 + 兼容性
cargo test --test command_test -- --test-threads=1

# Wave 2：Code UI 矩阵（最小子集）
cargo test --features test-provider --test code_ui_scenarios -- --test-threads=1

# Wave 4：Live AI（需 DEEPSEEK_API_KEY；务必先设成本闸门，见 §9.3）
cargo test --test ai_agent_test -- --test-threads=1
```

**3 个最常踩的坑**：

1. `cargo test --features A,B --test X --test Y` 的 features 是 union——不要把不需要某 feature 的测试和需要的合并到一条命令，否则前者也会被错误地带上 feature 编译。
2. `set -a; source .env.test; set +a` 会把密钥导出到当前 shell——后续工具/日志/截图都可能泄密。优先 `env $(grep -v '^#' .env.test | xargs) cargo test ...`。
3. 默认所有 wave 都加 `--test-threads=1`。当前仓库 64 个测试带 `#[serial_test::serial]`，并发会触发竞态。

**改完代码该跑哪些 wave**：见 §3.3 Path → Wave 表。
**review/handoff 协议**：见 §9 PR / Review 协议。

---

## 1. 现状基线（已存在资产）

| 资产 | 现状 | 证据 |
|---|---|---|
| 命令层集成测试汇总入口 | 已存在 | `tests/command_test.rs` + `tests/command/*.rs` |
| 兼容性专项测试 | 已存在 | `tests/compat/*.rs` + `Cargo.toml` `[[test]]` 注册 |
| Code UI PTY harness | 已存在 | `tests/harness/code_session.rs` |
| Code UI 事件流 harness | 已存在 | `tests/harness/event_stream.rs` |
| Code UI 数据驱动矩阵 runner | 已存在 | `tests/code_ui_remote_{lease,sse,state,security,generation,approval,model_generation}_matrix.rs` |
| Code UI 场景回归 | 已存在 | `tests/code_ui_scenarios.rs` |
| MCP 双入口回归 | 已存在 | `tests/code_mcp_dual_entry_test.rs` |
| resume 回归 | 已存在 | `tests/code_resume_test.rs` |
| codex runtime 回归 | 已存在 | `tests/code_codex_runtime_test.rs` |
| 网络层集成测试 | 已存在 | `tests/network_remotes_test.rs`（`test-network`） |
| Cloud live 集成测试 | 已存在 | `tests/cloud_storage_backup_test.rs`、`tests/publish_live_test.rs`（`test-live-cloud`） |
| 文档一致性脚本 | 已存在 | `scripts/check_docs_consistency.sh` |
| 兼容矩阵一致性脚本 | 已存在 | `scripts/check_compat_matrix.sh` |

---

## 2. 本计划范围

### 2.1 纳入范围（本版必须可执行）

1. 单机可执行的 L1/L2 集成回归。
2. 通过 feature/env gate 启用的 L3 live 回归（AI / Cloud）。
3. 明确的波次执行顺序、命令、通过标准、产物归档方式。

### 2.2 暂不纳入（转 `BASELINE_GAP`）

1. 多机编排脚本（`scripts/integration/*`）与远程分发自动化。
2. 自定义 `FA-*` 测试 ID 体系（当前仓库无对应测试命名体系）。
3. `tests/integration/scenarios/*.yml` 驱动器（当前仓库无该目录/运行器）。

---

## 3. 执行前准备（Wave 0）

### 3.1 必须通过

```bash
cargo --version
rustup show active-toolchain

# 格式 + lint（CI 阻断门，本地先过；少跑一遍能省 30 分钟返工）
cargo +nightly fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings

# 编译基线（两条都必须 pass，feature 代码路径才被覆盖）
cargo test --no-run --all-targets
cargo test --no-run --all-targets --features test-provider,test-network,test-live-ai,test-live-cloud

# 文档/兼容性一致性
bash scripts/check_compat_matrix.sh
bash scripts/check_docs_consistency.sh
bash scripts/check_integration_plan_consistency.sh   # 本计划自检（见 §9.6）
```

### 3.2 Live 测试前置（可选）

```bash
cp .env.test.example .env.test
chmod 600 .env.test
```

加载方式（所有 live wave 共用）：

```bash
# 推荐：仅注入 cargo 子进程，不污染当前 shell
env $(grep -v '^#' .env.test | xargs) cargo test ...

# 备用（注意：会把密钥导入当前 shell，跑完用 `unset` 清掉）：
set -a; source .env.test; set +a
```

### 3.3 Path → Wave 映射

修改某路径后**最小**应跑的 wave 集合（Wave 0 始终默认，下表省略）。

| 修改路径 | 必跑 Wave | 推荐补充 |
|---|---|---|
| `src/command/*.rs`（非 cloud / code） | 1 | + 2，若涉及 `mod.rs` 共享 helper |
| `src/command/cloud.rs` | 1 | + 5（需 D1/R2 env） |
| `src/command/code.rs`、`src/command/code_control*.rs` | 1, 2 | + 4，若动到 provider 接线 |
| `src/internal/tui/**` | 2 | — |
| `src/internal/ai/agent/**`、`src/internal/ai/orchestrator/**` | 2 | + 4 |
| `src/internal/ai/providers/**` | 2 | + 4 |
| `src/internal/ai/tools/**` | 2 | — |
| `src/internal/ai/intentspec/**`、`workflow_objects.rs` | 2 | + 4 |
| `src/internal/ai/mcp/**` | 2 | — |
| `src/internal/protocol/**`、`src/git_protocol.rs` | 1 | + 3 |
| `src/utils/client_storage.rs`、`src/utils/d1_client.rs` | 1 | + 5 |
| `src/internal/model/**`、`sql/*.sql` | 1, 2 | — |
| `Cargo.toml`、`.env.test.example` | 0, 1, 2 | 强制跑 `check_integration_plan_consistency.sh` |
| `docs/**`、`README.md` | 仅 Wave 0（docs 一致性） | — |
| `tests/**` | 仅相关 wave | 若动 `tests/harness/**`，跑全 Wave 2 |

**用法**：

```bash
git diff --name-only origin/main...HEAD | \
  xargs -n1 -I{} echo "{}" | \
  awk '...'   # 后续可由 scripts/integration/pick-waves.sh 自动化（见 BASELINE_GAP-INTEG-006）
```

在该脚本落地前，作者手动对照本表，PR 描述里在 `## Test Plan` 段列出实际跑的 wave 集合（见 §9.1）。

---

## 4. 执行波次

## 4.1 Wave 1：命令与兼容性（必跑）

```bash
cargo test --test command_test -- --test-threads=1

cargo test --test compat_stash_subcommand_surface -- --test-threads=1
cargo test --test compat_bisect_subcommand_surface -- --test-threads=1
cargo test --test compat_worktree_delete_dir -- --test-threads=1
cargo test --test compat_checkout_alias_help -- --test-threads=1
cargo test --test compat_matrix_alignment -- --test-threads=1
cargo test --test compat_branch_lossy_wrapper_guard -- --test-threads=1

# Unwrap guards
cargo test --test compat_lfs_client_production_unwrap_guard -- --test-threads=1
cargo test --test compat_config_production_unwrap_guard -- --test-threads=1
cargo test --test compat_head_production_unwrap_guard -- --test-threads=1
cargo test --test compat_util_production_unwrap_guard -- --test-threads=1
cargo test --test compat_client_storage_production_unwrap_guard -- --test-threads=1
cargo test --test compat_extra_production_unwrap_guard -- --test-threads=1
cargo test --test compat_all_production_unwrap_guard -- --test-threads=1
cargo test --test compat_agent_run_non_exhaustive_guard -- --test-threads=1

# Database migrations
cargo test --test db_migration_test -- --test-threads=1
```

通过标准：全部 green，无 skip/fail。

## 4.2 Wave 2：Code UI 与本地自动化控制（必跑）

```bash
# Block 1: L1 local AI, goal, session and VCS safety (no features)
cargo test \
  --test agent_capture_migration_test \
  --test ai_agent_baseline_test \
  --test ai_automation_test \
  --test ai_classifier_test \
  --test ai_code_ui_projection_test \
  --test ai_code_ui_wire_test \
  --test ai_command_safety_test \
  --test ai_compaction_filter_test \
  --test ai_concurrency_lock_test \
  --test ai_context_budget_test \
  --test ai_context_compaction_prune_test \
  --test ai_context_frame_test \
  --test ai_dag_tool_loop_test \
  --test ai_dagrs_081_spike_test \
  --test ai_dynamic_prompt_test \
  --test ai_file_undo_test \
  --test ai_goal_completion_gate_test \
  --test ai_goal_flag_off_regression_test \
  --test ai_goal_resume_test \
  --test ai_goal_state_test \
  --test ai_goal_supervisor_test \
  --test ai_goal_verifier_test \
  --test ai_hardening_contract_test \
  --test ai_json_repair_test \
  --test ai_libra_vcs_safety_test \
  --test ai_memory_anchor_test \
  --test ai_mock_provider_test \
  --test ai_projection_resolver_test \
  --test ai_provider_context_overflow_compact_loop_test \
  --test ai_provider_error_taxonomy_test \
  --test ai_provider_retry_policy_test \
  --test ai_provider_transform_test \
  --test ai_runtime_contract_test \
  --test ai_scheduler_plan_set_test \
  --test ai_schema_migration_test \
  --test ai_security_runtime_test \
  --test ai_semantic_rust_test \
  --test ai_semantic_tools_test \
  --test ai_session_jsonl_test \
  --test ai_skill_test \
  --test ai_source_pool_test \
  --test ai_storage_flow_test \
  --test ai_usage_stats_test \
  --test ai_usage_tui_test \
  --test ai_validation_decision_flow_test \
  --test diagnostics_redaction_test \
  --test e2e_mcp_flow \
  --test intent_flow_test \
  --test local_client_test \
  --test mcp_integration_test \
  --test redaction_contract_test \
  -- --test-threads=1

# Block 2: Matrix, session, and event stream automation (requires test-provider)
cargo test --features test-provider \
  --test harness_self_test \
  --test code_ui_scenarios \
  --test code_ui_remote_lease_matrix \
  --test code_ui_remote_sse_matrix \
  --test code_ui_remote_state_matrix \
  --test code_ui_remote_security_matrix \
  --test code_ui_remote_generation_matrix \
  --test code_ui_remote_approval_matrix \
  --test code_cli_dispatch_test \
  --test code_provider_boot_test \
  --test code_tool_acl_test \
  --test code_mcp_dual_entry_test \
  --test code_resume_test \
  --test code_codex_default_tui_test \
  --test code_codex_runtime_test \
  --test ai_code_ui_headless_test \
  --test ai_approval_ttl_test \
  --test ai_compaction_handoff_e2e_test \
  --test ai_context_handoff_test \
  -- --test-threads=1

# Block 3: Subagent contract and multi-agent e2e (requires test-provider, subagent-scaffold)
cargo test --features test-provider,subagent-scaffold \
  --test ai_subagent_contract_test \
  --test ai_multi_agent_e2e_test \
  -- --test-threads=1

# Block 4: Publish pipeline local tests (no features)
cargo test \
  --test publish_ai_export_test \
  --test publish_ai_object_model_contract_test \
  --test publish_incremental_test \
  --test publish_preflight_test \
  --test publish_redaction_contract_test \
  --test publish_refs_test \
  --test publish_snapshot_test \
  --test publish_upload_test \
  --test publish_worker_template_embed_test \
  -- --test-threads=1
```

通过标准：全部 green。

## 4.3 Wave 3：网络层（可选，建议 nightly）

```bash
cargo test --features test-network --test network_remotes_test -- --test-threads=1
```

通过标准：green。若外网抖动，允许重试一次后再判定。

## 4.4 Wave 4：Live AI（可选，按成本启用）

> **成本警告**：本 wave 调用真实 LLM。开跑前必须设置成本闸门 env（见 §9.3）。
> 闸门 env 当前**未实现**自动 fail-fast，靠人工监控；详见 `BASELINE_GAP-INTEG-005`。

```bash
# DeepSeek live（ai_agent_test / ai_chat_agent_test 内部按 DEEPSEEK_API_KEY gate）
cargo test --test ai_agent_test -- --test-threads=1
cargo test --test ai_chat_agent_test -- --test-threads=1

# Code UI live model generation（ignored + 明确开关）
LIBRA_RUN_LIVE=1 cargo test --features test-provider \
  --test code_ui_remote_model_generation_matrix \
  -- --ignored --test-threads=1

# Ollama live gate (requires LIBRA_AI_LIVE_OLLAMA=1 and optional OLLAMA_HOST)
LIBRA_AI_LIVE_OLLAMA=1 cargo test --test ai_ollama_live_gate_test -- --test-threads=1
```

通过标准：
1. 若未配置 key，测试允许输出 skipped。
2. 若已配置 key，失败视为真实回归。
3. 单次 wave LLM 调用总数超过 `LIBRA_TEST_MAX_CALLS`（建议 200）时，必须中断，记录 issue 而非继续重试。

## 4.5 Wave 5：Live Cloud（可选，按环境启用）

```bash
# D1/R2 live gate（依赖 LIBRA_D1_* + LIBRA_STORAGE_*）
cargo test --features test-live-cloud --test cloud_storage_backup_test -- --test-threads=1

# publish live gate（额外要求 LIBRA_ENABLE_TEST_LIVE_CLOUD=1）
LIBRA_ENABLE_TEST_LIVE_CLOUD=1 cargo test --features test-live-cloud \
  --test publish_live_test publish_live -- --test-threads=1

# Storage R2 test
cargo test --features test-live-cloud --test storage_r2_test -- --test-threads=1
```

通过标准：
1. 缺少 live 环境变量时允许 skipped。
2. 环境齐全后失败视为真实回归。

## 4.6 Wave 6：性能 smoke（可选）

```bash
LIBRA_RUN_PERF=1 cargo test --features test-provider \
  --test code_ui_perf_smoke_test \
  -- --ignored --test-threads=1
```

通过标准：全部 green；用于趋势观察，不作为默认阻断门。

---

## 5. 归档与报告（落地可执行）

推荐每轮执行都产生日志目录：

```bash
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)"
RUN_DIR="target/integration-runs/$RUN_ID"
mkdir -p "$RUN_DIR"
```

执行命令时统一保留日志：

```bash
cargo test --test command_test -- --test-threads=1 2>&1 | tee "$RUN_DIR/wave1-command.log"
```

最小交付物：

1. `wave1-command.log`
2. `wave2-code-ui.log`
3. `wave3-network.log`（若执行）
4. `wave4-live-ai.log`（若执行）
5. `wave5-live-cloud.log`（若执行）
6. `summary.md`（人工汇总 pass/fail/skip 与失败链接）

---

## 6. 出口标准（Definition of Done）

### 6.1 本计划“可落地执行”判定

满足以下全部条件即可判定本计划可执行：

1. Wave 0 命令全部在当前仓库可运行。
2. Wave 1 / Wave 2 的命令只引用真实存在的测试目标并可执行。
3. Wave 3 / 4 / 5 明确采用 feature/env gate，不再依赖虚构脚本或虚构命令。
4. 文档内不存在仓库未实现的必跑命令。

### 6.2 作为发布阻断的最低门

默认阻断门仅包含：

1. Wave 1 全绿。
2. Wave 2 全绿。

Wave 3/4/5 作为 nightly 或 release 前增强门。

---

## 7. 集成测试机器配置文件设计（用于多机编排）

### 7.1 设计目标（GAP-INTEG-001 的实施前置）

1. 用单一配置文件描述测试节点的能力、资源与执行边界，统一 `Wave -> Node` 分配。
2. 把 `env-file`、`features`、`provider` 与日志产物绑定，支持 `Wave 4 / 5` 的差异化路由。
3. 统一产物 schema，便于多节点结果汇总与回放。

### 7.2 推荐文件与示例（建议路径：`scripts/integration/nodes.yaml`）

```yaml
version: 1
runbook:
  run_id_hint: "YYYYMMDDTHHMMSSZ"
  artifact_root: "target/integration-runs/{run_id}"
  default_env_file: ".env.test"
  default_wave_timeout_sec: 1800
  default_test_threads: 1
  log_retention_days: 14

nodes:
  - id: "local-01"
    role: "controller"
    transport:
      mode: "local"
      shell: "bash -lc"
      workdir: "/data/ci/libra"
      concurrency: 1
    features: ["test-provider", "test-network", "test-live-ai", "test-live-cloud"]
    waves: [1, 2, 3]
    providers:
      - provider_id: "deepseek"
        models: ["deepseek-v4-flash"]
        required_env: ["DEEPSEEK_API_KEY"]
      - provider_id: "kimi"
        models: ["moonshot-v1-32k"]
        required_env: ["MOONSHOT_API_KEY"]
        optional_env: ["MOONSHOT_BASE_URL"]
    gate_env:
      LIBRA_CODE_TEST_PROVIDER: "deepseek"
      LIBRA_CODE_TEST_MODEL: "deepseek-v4-flash"
      LIBRA_ENABLE_TEST_PROVIDER: "1"

  - id: "live-ai-01"
    role: "live-ai-runner"
    transport:
      mode: "ssh"
      host: "10.0.0.22"
      user: "ci"
      workdir: "/home/ci/libra"
      ssh_key_ref: "CI_AGENT_KEY"
      concurrency: 1
    features: ["test-provider", "test-live-ai"]
    waves: [4]
    providers:
      - provider_id: "deepseek"
        models: ["deepseek-v4-flash"]
        required_env: ["DEEPSEEK_API_KEY"]
      - provider_id: "kimi"
        models: ["moonshot-v1-32k"]
        required_env: ["MOONSHOT_API_KEY"]
        optional_env: ["MOONSHOT_BASE_URL"]
    gate_env:
      LIBRA_RUN_LIVE: "1"
      LIBRA_ENABLE_TEST_PROVIDER: "1"

  - id: "cloud-01"
    role: "live-cloud-runner"
    transport:
      mode: "ssh"
      host: "10.0.0.23"
      user: "ci"
      workdir: "/home/ci/libra"
      ssh_key_ref: "CI_AGENT_KEY"
      concurrency: 1
    features: ["test-live-cloud"]
    waves: [5]
    required_env:
      - "LIBRA_D1_ACCOUNT_ID"
      - "LIBRA_D1_API_TOKEN"
      - "LIBRA_D1_DATABASE_ID"
      - "LIBRA_STORAGE_ENDPOINT"
      - "LIBRA_STORAGE_BUCKET"
      - "LIBRA_STORAGE_ACCESS_KEY"
      - "LIBRA_STORAGE_SECRET_KEY"

runtime:
  env_file:
    source: ".env.test"
    target: ".env.test"
    target_permission: "0600"
    redacted_reporting: true
  commands:
    preflight: "bash scripts/integration/preflight.sh"
    run_wave_tpl: "bash scripts/integration/run-wave.sh --run-id {run_id} --node {node_id} --wave {wave} --env-file {env_file}"
  reporting:
    format: "jsonl"
    summary_path: "target/integration-runs/{run_id}/nodes.jsonl"
```

### 7.3 约束与验收项

1. `nodes[*].id` 唯一。
2. `mode: local` 仅允许 `workdir/shell/concurrency`；
   `mode: ssh` 必须显式包含 `host/user/workdir`。
3. `waves` 与 `features` 要一致：`wave:4` 需具备 `test-live-ai`，`wave:5` 需具备 `test-live-cloud`。
4. `providers[].required_env` 缺失或为空值表示该 provider 在该节点上不可执行；对应波次必须禁止调度。
5. `.env.test` 下发后必须验证目标文件权限为 `0600`；日志不能输出其原文内容。
6. 每个 `(node, wave)` 输出一条 `jsonl` 结果，字段至少包含：
   - `run_id`、`node`、`wave`、`status`、`return_code`、`duration_ms`、`log_path`、`first_error_line`

### 7.4 示例汇总行

```json
{"run_id":"20260516T0001Z","node":"live-ai-01","wave":4,"status":"passed","return_code":0,"duration_ms":382000,"log_path":"target/integration-runs/20260516T0001Z/wave4/live-ai-01.log","first_error_line":null}
{"run_id":"20260516T0001Z","node":"cloud-01","wave":5,"status":"failed","return_code":101,"duration_ms":795000,"log_path":"target/integration-runs/20260516T0001Z/wave5/cloud-01.log","first_error_line":"E0426: D1 bind failed with 403"}
```

### 7.5 与现有计划关系

- 本节是 `BASELINE_GAP-INTEG-001` 的控制面先行设计；不改变当前 Wave 1~6 现状执行方式。
- 落地 GAP 后，上述 `node.waves` 作为执行映射输入，Wave 命令应由统一 runner 生成，减少手工拼接。

## 8. BASELINE_GAP（后续扩展项）

以下能力不再伪装成“已可执行”，统一登记为后续工程任务。

### BASELINE_GAP-INTEG-001：多机调度脚本缺失

- 现状：`scripts/integration/` 不存在。
- 需要补充：
  1. `scripts/integration/preflight.sh`
  2. `scripts/integration/run-wave.sh`
  3. `scripts/integration/report.sh`

### BASELINE_GAP-INTEG-002：场景 YAML 驱动缺失

- 现状：`tests/integration/scenarios/*.yml` 与对应 runner 不存在。
- 需要补充：
  1. `tests/integration/harness/run.py`（或 Rust 等价实现）
  2. `tests/integration/scenarios/` 场景文件与断言 DSL

### BASELINE_GAP-INTEG-003：自定义 `FA-*` 测试 ID 体系未落地

- 现状：仓库没有 `FA-*` 对应测试目标。
- 决策：当前以真实 `cargo test --test <name>` 为唯一执行单位；后续如引入 `FA-*`，必须同步落地到实际 runner 与报告工具。

### BASELINE_GAP-INTEG-004：四节点并发预算编排未落地

- 现状：无 semaphore 调度器、无远程节点资源协调实现。
- 需要补充：
  1. 明确调度器实现与超时策略
  2. 节点健康检查与配额采集

### BASELINE_GAP-INTEG-005：Live test 成本闸门未实现

- 现状：`LIBRA_TEST_MAX_CALLS` / `LIBRA_TEST_BUDGET_USD` 仅写在文档里，测试代码未读这两个 env，超额不会 fail-fast。
- 需要补充：
  1. 在 `tests/harness/` 增加 budget counter，被所有 Live provider 调用前置 hook 拦截。
  2. 超限时输出可定位的 panic 信息，包含已用 / 上限 / 当前测试名。
- 优先级：P0（防止 AI agent 自动 retry 烧钱）。

### BASELINE_GAP-INTEG-006：`scripts/integration/pick-waves.sh` 未实现

- 现状：§3.3 Path → Wave 映射目前靠人工对照。
- 需要补充：
  1. 输入 `git diff --name-only`，输出推荐 wave 集合。
  2. 与 `scripts/check_integration_plan_consistency.sh` 共享同一份映射表（建议抽到 `scripts/integration/path-wave-map.toml`）。
- 优先级：P1。

### BASELINE_GAP-INTEG-007：`tests/INDEX.md` TODO 行收尾

- 现状：`tests/INDEX.md` 已建骨架，Wave 1~6 主要测试已索引；剩余 ~50 个测试目标标记为 TODO，待逐条补一句话功能描述与 `src` 关联。
- 需要补充：每条 TODO 替换为 `target | wave | one-line purpose | relevant src`，并移到对应 Wave 表。
- 优先级：P1（分散到日常 PR 中渐进收尾即可，单 PR 一次性补全反而难审）。

---

## 9. PR / Review 协议（AI 交叉开发约束）

本项目由 Claude + Codex 交叉开发与 review。以下约束必须遵守，避免双方互改导致死锁、flake 嘴炮、密钥泄露。

### 9.1 PR 描述必须包含 `## Test Plan` 段

```
## Test Plan
- New:      <target>::<fn>  // e.g. code_ui_remote_lease_matrix::lease_expires_after_ttl
- Modified: <target>::<fn>
- Deleted:  <target>::<fn>
- Waves run locally: 0, 1, 2 (+ 4 if applicable)
- jsonl summary: <inline ≤ 50 KB, or GitHub artifact URL>
- Commit SHA at run time: <sha>
```

**测试引用统一用三段式** `<cargo --test 名>::<test_fn 名>`。
文件路径仅作辅助；rename/refactor 不影响三段式 ID。

### 9.2 Reviewer 行为约束

1. **不要直接代笔测试**：若覆盖不足，留 review comment 提议补，由原作者补。这避免两个 agent 互相覆盖对方写的测试。
2. **复跑义务**：reviewer 在本地至少跑 Wave 1+2 一次，把 jsonl 摘要贴到 review。
3. **复现要求**：若 reviewer 报告失败，必须附 (`commit_sha`, `wave`, 完整 cargo invocation, log head 50 行 + tail 50 行)。缺字段的失败报告无效。
4. **flake 处理**：怀疑 flake 先查 `tests/flaky_quarantine.toml`（§9.5）。不在 quarantine 中的测试连挂 2 次才能开 flaky issue。

### 9.3 Live test 成本闸门

跑 Wave 4 / Wave 5 前**必须**设置：

```bash
export LIBRA_TEST_MAX_CALLS=200      # 单次 wave LLM 调用总数硬上限
export LIBRA_TEST_BUDGET_USD=2.0     # 软上限：仅 warn，不 fail
```

当前这两个 env **未被测试代码读取**（见 `BASELINE_GAP-INTEG-005`）。在 GAP 落地前，作者必须人工监控用量，超 200 次或耗时 > 10 min 立即中止。

### 9.4 PR 规模

- 单 PR 改动 src + tests 合计建议 ≤ 800 行（不计自动生成）。
- 超过上限：拆分；或在 PR 描述里说明无法拆分的原因，并请求第三个 agent（另一个模型）旁路 review。

### 9.5 Flake 隔离清单

新增/维护：`tests/flaky_quarantine.toml`

```toml
# 已知 flaky 但不阻断 wave 的测试。
[[entries]]
test    = "<target>::<fn>"
reason  = "<一句话>"
issue   = "<URL>"
last_seen_commit = "<sha>"
quarantined_at   = "<YYYY-MM-DD>"
```

- 每次把测试加入 quarantine，必须同时开 issue 跟踪。
- 修复后必须从 quarantine 移除并在 PR 描述说明；不允许"修了但忘记移除"。
- quarantine 文件由 `scripts/check_integration_plan_consistency.sh` 校验：每条 `test` 必须能 grep 到对应 `#[test]` 函数。

### 9.6 本计划自检脚本

`scripts/check_integration_plan_consistency.sh`（需新建）执行以下校验：

1. 计划里所有 `--test <name>` 必须对应 `tests/<name>.rs` 或 `Cargo.toml [[test]]` 条目。
2. 计划里所有 `--features <flag>` 必须出现在 `Cargo.toml [features]`。
3. 计划里所有 `LIBRA_*` / `*_API_KEY` / `*_BASE_URL` env 名必须出现在 `.env.test.example`（`LIBRA_TEST_*` 由本计划自身引入，豁免）。
4. `flaky_quarantine.toml` 里每条 `test` 三段式必须可解析为现有测试。

CI 在 Wave 0 调用此脚本，失败即阻断 PR。

---

## 10. 维护规则

1. 新增集成测试文件时，必须把执行命令补到本计划相应 Wave，并在 `tests/INDEX.md` 加一行索引。
2. 删除/重命名测试目标时，必须同步更新本计划命令、`tests/INDEX.md`、以及 `flaky_quarantine.toml` 中对应条目。
3. 未实现能力必须用 `BASELINE_GAP-*` 标记，不允许写成默认可执行步骤。
4. 若引入新的 live gate 环境变量，必须同步更新 `.env.test.example`、本计划 Wave 说明、`scripts/check_integration_plan_consistency.sh` 豁免列表（如需）。
5. 修改 §3.3 Path → Wave 映射，须同步更新 `scripts/integration/path-wave-map.toml`（如已落地）。
