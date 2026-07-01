# research-git 与 Libra 能力差距分析

> 核对基线：2026-07-01 本地工作区。
> - 参考项目：`/Volumes/Data/competition/StepzeroLab/research-git`，版本 `0.0.2`（Alpha，Python 3.11+，依赖仅 `libcst>=1.1` + `mcp>=1.2`）。已交付 v1（Memory Loop）、v2（Graph Intelligence + Ambient Capture）、v3（Research Layer），并新增多宿主安装/guidance 平面。
> - Libra 仓库：`/Volumes/Data/GitMono/libra`，版本 `0.17.1777`。
> - 目标：分析 research-git 的产品/技术能力与 Libra 现状的差距，给出 Libra-native 的补齐方向。本文只记录分析与计划，不改变当前命令面。
>
> 本次刷新校正了三处对 Libra 现状的高估/漏记：① Libra 已有 tree-sitter 符号抽取器（`semantic/extractor.rs`），“无 symbol slice service”属高估；② Libra 的 `agent_session.agent_kind` 已归一 7 类宿主 agent，在“多宿主接入”维度其实领先 research-git；③ `McpAuthorizer` 目前是 schema-only 占位，尚未接入请求路径。

## 执行结论

`research-git` 不是一个替代 Git 的 VCS，而是一个叠加在普通 Git 仓库之上的“研究记忆层”。它把一次探索中的一个想法抽成 `Feature Capsule`，把 capsule、run、metric、edge、proposal 写入 `.rgit/graph.db` 和 `.rgit/objects/`，再通过只读 MCP 和本地 agent subagent 完成召回、组合与再生。其定位已从“Claude Code 插件”扩展为“**Works with Claude Code, and any MCP-capable client（Codex / GPT / …）**”，并配套了把自身写成默认能力的多宿主 installer（claude-code / codex / gemini / opencode）。

Libra 的优势在另一层：它已经是 AI-agent-native VCS，拥有 `.libra/libra.db`、对象存储、Git 兼容命令、AI workflow 对象、Session JSONL、外部 agent checkpoint（已归一 7 类宿主）、redaction、usage 统计、MCP tools/resources、skills/sub-agent runtime、tree-sitter 符号抽取和 thread graph。但 Libra 目前缺少一个等价于 `Feature Capsule` 的一等对象：可以把“一个可复活的想法/实验变体”从会话、diff、checkpoint 和 metric 中独立出来，作为可查询、可审查、可重放到当前代码的语义单位。

建议不要把 `research-git` 的 Python `.rgit` 存储原样嵌入 Libra。更合理的方向是新增 Libra-native 的 `research memory` 平面：使用普通 Libra/Git blob + `refs/libra/research` + SQLite projection，复用现有 redaction、agent session、usage、MCP authorizer（需先接线）、skills、semantic extractor 和 internal sub-agent dispatcher，在此之上实现 capsule capture / review / recall / compose / compare / ablation / provenance。

## research-git 能力拆解

### 定位与边界

- 源码入口：`src/rgit/cli.py`，发布入口为 `rgit = "rgit.cli:main"`，包名 `research-git`，版本 `0.0.2`（Development Status: Alpha）。
- 运行环境：Python 3.11+，核心依赖只有 `libcst`（symbol map）和 `mcp`（read-only 共享平面）。
- 存储根：`rgit init` 在 Git root 下创建 `.rgit/`，不是替换 `.git/`。
- 客户端边界：从 Claude-Code-only 扩展为“任意 MCP 客户端”。智能动作（segment / regenerate / edge-judge）始终走宿主 agent 的 subagent，**不调用任何 paid API**；确定性 engine（graph、CAS object store、git diff、byte-exact freeze）不调用 LLM。
- MCP 边界：`mcp_server.py` 用 FastMCP 暴露 7 个**只读**工具（recall / compose / get_feature / list_features / compare / ablation / provenance）；所有写路径（approve / dismiss / edges / segment / metric-dir）只走 CLI。

### 完整 CLI 命面（17 个子命令，已全部交付）

| 命令 | 作用 |
|---|---|
| `init` | 在 git root 建 `.rgit/`（不装 hook） |
| `run` | 跑实验 → byte-exact freeze artifact → 记录 run + edges → 暂存 proposal；`--from`/`--with`/`--refresh-guide-file`/`--init` |
| `capture` | 把工作树 diff 切成 proposal 候选（Phase 1 免费 segmenter）；`--trigger {manual,commit,run,watch}` |
| `review` | 列 open proposals；`--approve <pid> [--name/--index]` / `--dismiss <pid>` |
| `features` | 列已 approve 的 capsule |
| `mcp` | 启动只读 MCP server |
| `edges` | `--apply`（确定性 overlaps）/ `--candidates`（depends_on 候选）/ `--add TYPE SRC DST` |
| `pending` | 列 open proposal（带 diff/candidates，`--json`） |
| `resegment` | 用 agent 质量 capsule 替换启发式候选（`<pid> --from-json`） |
| `watch` | 前台 watch loop，空闲去抖暂存 Phase-1 proposal；`--interval/--idle/--once` |
| `install` | 把 plugin + MCP 装进 AI 客户端；`PLATFORM`、`--list/--uninstall/--dry-run`、`--guidance {default,manual-only,none}`、`--scope {user,project,local}` |
| `install-hooks` | 装 post-commit 自动 capture hook（不动外部 hook） |
| `compare` | 变体簇按 metric 排名（只读）；`TARGET --metric --higher/--lower` |
| `ablation` | 对 capsule 幂集建 base/+A/+A+B 网格（只读） |
| `provenance` | 某 run 的 per-slice clean vs adapted 审计（只读） |
| `metric-dir` | `set <metric> {higher,lower}` / `list` / `suggest` |
| `graph` | 渲染 graph：`--mermaid/--dot/--text`（默认 mermaid），`--runs` |

### 数据模型

`src/rgit/store/db.py` 定义六类核心表，`src/rgit/store/models.py` 定义对应 dataclass：

| 表 | 作用 |
|---|---|
| `features` | Feature Capsule：name、intent、status、base_commit、knobs、data_assumptions、resurrection_guide、result_summary、payload_hash |
| `runs` | 一次实验运行：cmd、artifact_hash、metrics、base_commit、env、created_at、returncode |
| `edges` | capsule/run 之间的 11 类关系（见下表） |
| `proposals` | capture 产生的候选队列：trigger、diff_ref、candidates、status、run_id、from_features |
| `events` | comment-in/out toggle 形成的 activate/deactivate 事件 |
| `metric_directions` | metric 越大越好或越小越好的 verdict 配置 |

`Capsule` 是核心资产——不是 patch，而是 intent + `code_slices` + knobs + data_assumptions + result_summary + resurrection_guide 的“想法规格书”。两个子结构值得注意：

- `CodeSlice`：`file`、`symbol`、`anchor`、`code`、`kind`（`add`/`wrap`/`insert`）——capsule 的代码切片单位。
- `ResultSummary`：`verdict`（`improved`/`neutral`/`regressed`）、`key_delta`、`failure_reason`、`notes`——把“这个想法赢没赢”结构化。

**11 类 edge**（确定性 baseline + agent-judged 语义边）：

| edge | 方向 | 语义 |
|---|---|---|
| `produced` | 有向 | capsule → 它产生的 run（frozen artifact） |
| `touches` | 有向 | capsule → 它改动的 module/file |
| `active` | 有向 | run → 运行时声明 active 的 capsule（ablation 的 active-set 依据） |
| `variant_of` | 有向 | capsule 是另一个的再生/变体（regeneration lineage） |
| `depends_on` | 有向 | X 用到 Y 定义的 symbol（确定性 over-produce 候选 → edge-judge 确认） |
| `supersedes` | 有向 | X 严格替代 Y（agent-judged） |
| `derived_from` | 有向 | lineage（schema 中保留，当前用得少） |
| `overlaps` | 对称 baseline | X/Y 触及同一 (file, top-level symbol)，中性 |
| `alternative_to` | 对称 agent-judged | 竞争实现，二选一 |
| `composable_with` | 对称 agent-judged | 可叠加共存 |
| `conflicts_with` | 对称 agent-judged | 真正不兼容 |

### 捕获与冻结

- `runner.py::run_experiment` 执行用户命令，随后 `gitutil.py::freeze_worktree` 把当前工作树打成确定性 tar，写入 content-addressed object store；即使命令失败也记录 artifact 和 `returncode`（v2 区分成功/失败 run）。
- `gitutil.py::diff_since` 用 `git diff HEAD` 加 `git diff --no-index /dev/null <untracked>` 捕获 tracked 与 untracked 变化。
- `segmenter.py`：`HeuristicSegmenter`（免费，confidence 0.3，按 file 分组）做 Phase 1，再把 proposal 交给 `capsule-segmenter` subagent 做 Phase 2 重分割。
- `astmap.py`：`libcst` 的 `changed_symbols()`（hunk→top-level def/class）、`read_symbol_source()`、`symbol_at_line()`。
- `watch.py` 用 mtime_ns snapshot 做 idle debounce（`tick()` 只在空闲且 diff 变化且未暂存时落 proposal）。
- `toggles.py` 识别 Python `#` 注释开关（hunk 级），把“启用/禁用一个 capsule”的信号写入 events。
- `hooks.py` 装/卸 post-commit hook，用 marker 自标记、绝不覆盖外部 hook，返回结构化 action report。

### 图谱、召回与研究分析

- `edges.py` 确定性写 `overlaps`（同 (file, top-symbol)），并按 name 引用 over-produce `depends_on` 候选（带 evidence），交给 `edge-judge` agent 判断。
- `ranking.py` + `recall.py`：wildcard-safe 的字段加权 lexical（intent/name ×3、knobs/result_summary ×2、code/guide ×1）+ structural boost + neighbor boost；不依赖 embedding；每个 hit 携带 depends_on / same-region 子图。
- `compose.py` 为 agent 再生生成 brief：capsule 字段、当前 symbol live source、冲突列表和 merge context。
- `compare.py` 按 `variant_of` 传递闭包构建变体簇，跟 `produced` run 的 metric 排名，给出 Δ 与 ★ winner。
- `ablation.py` 按 run 的 `active` edge（缺失时回退 `produced`）对 capsule 幂集建 base/+A/+A+B 网格。
- `provenance.py` 从 frozen artifact in-memory 解 tar，与 capsule clean slice 做 clean/adapted/missing 审计。
- `metricdir.py` 用启发式（loss/err/nll/ppl/perplex→lower；acc/f1/reward/score/bleu/rouge→higher，仅自信匹配）给出 metric 方向，`best_index` 据存储方向选最优。
- `graphview.py` 提供 Mermaid / DOT / text 的 capsule/run graph，并在有更精确 same-region 关系时抑制冗余 overlaps。
- `tables.py` 终端定宽表格 + ★ winner 标记 + clean/adapted diff 渲染（compare/ablation/provenance 的人读输出）。

### Plugin / subagent 平面

`src/rgit/_plugin/`（plugin.json v0.0.2）定义两个 skill + 三个 subagent：

| 组件 | 职责 |
|---|---|
| `rgit-capture` skill | 读取 pending proposals → 派 `capsule-segmenter` → 写回 resegment → `rgit edges --apply` → 派 `edge-judge` |
| `rgit-recall` skill | 调 MCP recall/compose → 派 `capsule-regenerator` 把 capsule 重应用到当前代码 → review → 用户 `rgit run --from` 冻结 |
| `capsule-segmenter` | 把 messy diff 切成高质量 capsule（输出含 `dropped` 的噪声项） |
| `capsule-regenerator` | 按 capsule intent/guide 在当前代码上重实现；**只 author**，不跑程序/不冻结/不提交 |
| `edge-judge` | 把 `depends_on` 候选与 `overlaps` baseline 细分成 alternative_to/composable_with/supersedes/conflicts_with |

这个拆分值得借鉴：共享 memory plane 保持 dumb/read-only，真正的智能动作在本地 agent plane 运行；agent 只负责 authoring，byte-exact replay 不依赖 agent。

### 多宿主安装与 agent guidance 平面（v3 新增，本次重点补记）

`install <platform>` 不只装 plugin/MCP，还通过 `agent_guidance.py` 把一段“何时该用 research-git”的 **managed guidance block** 写进各宿主的全局指令文件，使工具在 install + restart 后成为默认能力：

| 平台 | skills 落点 | guidance 文件 | reload |
|---|---|---|---|
| `claude-code` | `claude` CLI plugin | `~/.claude/CLAUDE.md` | 重启 / `/reload-plugins` |
| `codex` | `~/.agents/skills/` | `~/.codex/AGENTS.md` | 新开 Codex session |
| `gemini` | `~/.agents/skills/` | `~/.gemini/GEMINI.md` | 新开 Gemini CLI session |
| `opencode` | `~/.agents/skills/` | `~/.config/opencode/AGENTS.md`（XDG-aware） | 新开 opencode session |

- `agent_platforms.py::guidance_target(platform)` 给出 `{path, reload}`；`agent_guidance.py` 以 `<!-- research-git:start/end -->` 标记做幂等 upsert/remove、原子写、dry-run。
- guidance 模式 pinned 跨升级：`default`（改完代码考虑 capture）/ `manual-only`（仅显式请求）/ `custom`（继承 default + repo `.rgit/` 偏好）。
- 覆盖优先级：session/user 指令 > repo 偏好 > 全局 default。
- `installer.py` 支持 `--scope {user,project,local}`、`--uninstall`、`--dry-run`，对 agent-CLI family 用 `~/.agents/skills/` 双向符号链接，使 skill 能找到 bundled agent。

> 这是 research-git 相对上一版本最实质的新增：它把“让宿主 agent 主动用这个工具”做成了一等的、跨 4 个宿主的可安装/可卸载/可 dry-run 的能力。

## Libra 当前相邻能力

### 已有强项

| Libra 能力 | 现状 |
|---|---|
| VCS 真源 | `.libra/libra.db`、对象存储、SQLite refs/index/reflog、Git 兼容命令与 `refs/libra/*` AI 分支，不依赖外部 `.git`。 |
| Session 事件流 | `src/internal/ai/session/jsonl.rs` 的 `SessionEvent`（9 个变体：SessionSnapshot / ContextFrame / CompactionEvent / MemoryAnchor / AgentRun / ToolCall / ToolResult / Goal / AiArtifact），append-only JSONL + unknown-event-safe 读取。 |
| Prompt 内记忆 | `context_budget/memory_anchor.rs` 的 `MemoryAnchor`（kind、scope、confidence、review_state{Draft/Confirmed/Revoked/Superseded}、expires_at、superseded_by、source_event_id 等 13 字段），可 replay 到 prompt section。 |
| 外部 agent 捕获（多宿主） | 迁移 `2026050303_agent_capture.sql` 的 `agent_session` 已用 `agent_kind` CHECK 归一 **7 类宿主**（claude_code / cursor / codex / gemini / opencode / copilot / factory_ai），含 `state`、`redaction_report`；`agent_checkpoint`（scope: temporary/committed/subagent）+ `refs/libra/traces`（checkpoint commit 仍属 Phase 2 接线中）。 |
| Lifecycle 归一 | `hooks/lifecycle.rs` 的 `LifecycleEventKind`（SessionStart…SessionEnd 11 类）+ envelope validation；`hooks/runtime.rs` 从 stdin 摄取。 |
| 符号抽取 | **`tools/semantic/extractor.rs` 已有 tree-sitter（Rust grammar）符号抽取**：`SemanticSymbol`（kind: Function/Method/Struct/Enum/Trait/Module/Const/Static/TypeAlias；scope: File/Module/Crate/Workspace/External；range/selection_range/byte_range/confidence/approximate/container）。当前单语言（Rust），但已是可复用的结构化基座。 |
| Patch 应用 | `tools/apply_patch/`（Codex 风格 `*** Begin Patch`，fuzzy seek_sequence）能拿到精确修改区间。 |
| MCP 平面 | `mcp/server.rs` + `mcp/resource.rs` 暴露 33 个 `#[tool]`（intent/task/run/plan/patchset/evidence/decision/context_frame/… 的 create+list）与 `libra://` 资源（object/objects/history/context）。`mcp/authz.rs` 的 `McpAuthorizer` **当前为 schema-only 占位，尚未接入请求路径（Phase 5）**。 |
| Skills / sub-agent | `skills/*`（parser/loader/dispatcher，project/user/embedded 三层）；`agent/runtime/sub_agent.rs` 的 `TaskInvocation`/`TaskResult`/`SubAgentDispatcher`，`TaskFailure` 含 13 类（含 PermissionEscalationDenied / SafetyDenied / BudgetExceeded / ApprovalRejected / Timeout），统一权限/预算/安全门。 |
| Usage / Graph | `command/usage.rs`（report/prune，`--by {Model, Agent, AgentProviderModel}`，`--session`/`--thread` 为过滤，Human/Json/Csv）；`command/graph.rs` 展示 AI thread projection graph（TUI）。 |
| 宿主集成 installer | `command/agent/`（含 `libra agent hooks`）已有把 capture/hook 装进宿主 agent 的命令面——与 research-git 的 `install <platform>` 同维度的入口已存在。 |
| 规划中的持久记忆 | `docs/development/memory.md`（2026-06-23，draft）已设计 branch-aware、namespace/path-keyed、review-gated 的 Memory 子系统；但当前源码**还没有 `src/internal/ai/memory` 实现**。 |
| 唯一现存“research”痕迹 | `prompt/embedded/contexts/research.md` 只是一个 prompt context 模板，**不是** research-memory 基础设施。 |

### 当前缺口概览

| 维度 | research-git | Libra 当前 | 差距 |
|---|---|---|---|
| 语义单位 | `Feature Capsule`（intent + code_slices + knobs + assumptions + guide + result_summary） | commit、PatchSet、Run、ContextFrame、MemoryAnchor、checkpoint | 缺“一条可复活的功能/实验想法”一等对象。 |
| 捕获入口 | `run` / `capture` / `watch` / post-commit hook | AI session、external hook（7 宿主）、checkpoint、apply_patch diff、普通 commit | 缺把一次 diff/session 切成 capsule proposal 的 pipeline。 |
| 人工审查 | `proposals` + `review --approve/--dismiss` + `curation.py` | MemoryAnchor 有 review_state；agent checkpoint 无 capsule review queue | 缺面向想法/capsule 的 review UX 与状态机。 |
| 代码切片 | Python-only `libcst` top-level symbol（窄但闭环） | **已有 tree-sitter Rust 抽取器**，但仅 Rust、未做成通用 slice service | 差距缩小为：把现有 extractor 扩到多语言 + 包成可复用 slice/anchor 服务 + 永远保留 raw diff fallback。 |
| 召回 | lexical + edge-aware recall（字段加权 + neighbor boost） | Memory 文档规划了 recall；源码无持久 recall；MCP 无 research recall | 缺 capsule 召回工具与排序策略。 |
| 再生 | `compose` brief + capsule-regenerator（只 author） | 内部 AgentRuntime/subagent 可执行任务，但无 capsule regeneration protocol | 缺把 recalled capsule 变成当前代码 diff 的 workflow。 |
| 实验分析 | compare / ablation / provenance / metric-dir | `usage` 聚合、graph TUI；无按 feature variant 的 metric lineage | 缺 variant/run/metric/capsule 关系与研究表格。 |
| 共享 | MCP query-only（7 tools）+ local subagent | MCP 有 33 objects/resources tools；`McpAuthorizer` 占位未接线 | 缺 research memory 的只读 MCP 工具；且需先把 authorizer 接进请求路径。 |
| 宿主可发现性 | `install <platform>` 写 managed guidance block 到 4 宿主 | `libra agent hooks` 已能装宿主 capture hook；但 research 能力本身无“默认可用”分发策略 | 思路可借鉴，但 Libra 应通过自身 MCP/skills/hooks 暴露，而非编辑各宿主 CLAUDE.md/AGENTS.md。 |
| 存储 | `.rgit/graph.db` + objects | `.libra/libra.db` + objects + refs | 需要 Libra-native projection/ref，而不是导入 `.rgit`。 |

## 关键差距详解

### 1. Libra 缺少 `ResearchCapsule` 这类一等对象

Libra 当前 AI 对象更偏向工作流执行：Intent、Plan、Task、Run、PatchSet、Evidence、Decision、ContextFrame、MemoryAnchor、agent checkpoint。它们能回答“这次 agent 做了什么、用了什么上下文、产生了什么证据”，但不能稳定回答：

- 这个曾经试过的“想法”是什么？
- 它触及哪些 symbol/file？
- 它的 knobs、assumptions、result_summary 是什么？
- 它和别的想法是依赖、替代、可组合、supersedes 还是冲突？
- 如何把它重新实现到今天的代码？

`MemoryAnchor` 可以保存短事实或约束，但粒度太小，不适合作为带 code_slices 和 metric lineage 的实验 capsule。`agent_checkpoint` 可以冻结 transcript/artifact，但粒度太大，不会把一个混杂 diff 切成多个正交 idea。

### 2. Libra 捕获的是 session/checkpoint，不是 idea

`research-git` 的核心价值不是 frozen artifact 本身，而是 `segment_diff -> proposal -> approve -> capsule`。Libra 已有更强的底层捕获点：

- `apply_patch` 路径能拿到精确修改区间。
- `SessionEvent` 能保存 tool call/result、goal、artifact。
- 外部 hooks 能捕获 7 类宿主（Claude/Codex/Gemini/opencode/Cursor/Copilot/Factory）session。
- `refs/libra/traces` 能持久化 redacted checkpoint。

但这些信号没有被收敛成“候选功能胶囊”。因此 Libra 现在更容易恢复会话或审计执行，不容易召回一个被混在会话中的实验想法。

### 3. symbol：research-git 窄但闭环，Libra 已有基座但未成服务

`research-git` 的 symbol mapping 只覆盖 Python top-level function/class（`astmap.py`），`toggles.py` 的注释开关也只识别 Python `#`——窄，但 capture→recall→regenerate 闭环完整。

Libra 这边的事实需要更新：它**已经有** tree-sitter 符号抽取器（`tools/semantic/extractor.rs`，Rust grammar，输出带 kind/scope/range/confidence 的 `SemanticSymbol`），并在依赖里挂了 `tree-sitter-rust` / `tree-sitter-bash`。所以“缺 symbol slice service”是高估——真实差距是：

1. 把现有 extractor 从 Rust 扩到 TS/Python/Markdown/SQL（多 grammar 或 LSP 路线）。
2. 把它包成一个 capsule 用得上的 **slice/anchor 服务**（给定 diff hunk → (file, symbol, anchor, code, kind)）。
3. 未知语言降级到 file+hunk+anchor；docs/config 用 section/key anchor。
4. 永远保留原始 diff/artifact hash——symbol 只是定位加速，错了也要可审计。

### 4. Libra 的 MCP 平面还不是 research memory query plane，且 authorizer 未接线

Libra MCP 已暴露 33 个 AI workflow object 工具与 `libra://` 资源，但有两件事要先处理：

- `mcp/authz.rs` 的 `McpAuthorizer` 目前是 schema-only 占位，**尚未接入 server 请求路径**。research memory 的只读 MCP 要安全暴露给团队，前提是先把这个 authorizer 真正接线（按 namespace/actor/sensitivity 过滤）。
- research-git 的 MCP 设计有一条要沿用的边界：共享 plane 只读、只返回 graph snippets；智能再生在本地 agent plane 执行。

因此 Libra 应：

- `research recall/get/compose/compare/ablation/provenance` 暴露为 MCP 只读工具。
- 写入 capsule、approve proposal、改 metric direction、执行 regeneration 必须走 CLI/AgentRuntime，并经 permission、sandbox、audit。
- 不要把 MCP 变成 agent turn control 面；这与 `docs/development/mcp.md` 已有边界一致。

### 5. Libra 有 usage，但缺 feature/run metric lineage

`libra usage` 能按 `Model / Agent / AgentProviderModel` 聚合 token/cost（session/thread 作为过滤维度）。`research-git` 的研究层不是成本统计，而是“哪个变体赢了”“ablation 表怎么读”“再生后的代码是否忠实于原 capsule”。

Libra 需要新增的是实验语义关系：

- `ResearchCapsule -produced-> ResearchRun`
- `ResearchCapsule -variant_of-> ResearchCapsule`
- `ResearchRun -active-> ResearchCapsule`
- `ResearchRun.metrics` 与 metric direction（参考 `metricdir.py` 的启发式 + 显式覆盖）
- clean slice vs adapted slice provenance

这些可以消费现有 usage/agent run evidence，但不能被 usage 表直接替代。

### 6. Libra 应避免 research-git 的 Git 依赖与 Python-only 限制

`research-git` 通过 shell 调 `git rev-parse`、`git diff`、`git ls-files`、`git diff --no-index`。Libra 不能这样设计自己的核心闭环。实现时应走 Libra 内部对象、index、diff、worktree、ignore 和 storage API，保证：

- 在 `.libra` 仓库没有 `.git` 时仍工作。
- 与 Libra 的 SHA-1/SHA-256、SQLite refs、`.libraignore`/`.gitignore` 语义一致。
- 能用 `libra push` / cloud sync / publish 传播 research memory，而不是生成旁路 `.rgit` 状态。

## 建议的 Libra-native 方案

### Phase 0：定义研究记忆平面和契约

新增设计文档或扩展 `docs/development/memory.md`，先固定对象和边界：

- ref：`refs/libra/research`，用于 capsule/run/edge/proposal 的 Git blob/tree/commit 真源。
- projection：SQLite 表只做可重建索引，例如 `research_capsule`、`research_run`、`research_edge`、`research_proposal`、`research_metric_direction`。
- 对象：使用普通 JSON blob，不新增 git-internal typed object variant，保持与 Memory/agent traces 的存储纪律一致。
- schema：所有对象带 `schema_version`、`object_id`、`created_at`、`created_by`、`source_refs`、`trust`、`sensitivity`、`redaction_report`。
- 安全：code_slices 可能含 secret，必须在持久化和 MCP 返回前经过 redaction/sensitivity gate。

建议对象草案：

| 对象 | 必填字段 |
|---|---|
| `ResearchCapsule` | id、name、intent、status、base_commit、source_diff_oid、code_slices（含 file/symbol/anchor/kind）、knobs、data_assumptions、result_summary、resurrection_guide |
| `ResearchRun` | id、cmd、artifact_tree_oid 或 checkpoint/ref、metrics、return_code、env_summary、base_commit、active_capsules |
| `ResearchEdge` | src、dst、type、confidence、evidence、created_by |
| `ResearchProposal` | id、trigger、source_event/session/run、diff_ref、candidates、status |

### Phase 1：实现确定性捕获与 review queue

先实现不依赖 LLM 的 walking skeleton：

- 新增 `libra research init/status/capture/review/list` 或在 `libra code` 下提供内部入口；命令名需单独评审。
- 捕获输入来自 Libra 内部 diff/worktree（复用 `apply_patch` 与 worktree/diff API），而不是 `git diff`。
- 支持 `--from-session`、`--from-checkpoint`、`--from-run`、`--staged`、`--worktree` 等来源。
- 默认 segmenter 按 file/hunk 生成低置信 candidate，可调用 `tools/semantic/extractor.rs` 给 Rust 切片提精度，未知语言落 file+hunk；记录 raw diff 和 source event。
- `review --approve/--dismiss/--rename/--edit-intent` 将 proposal 提升为 capsule。
- 所有写入走 CAS ref update，projection 可从 `refs/libra/research` 重建。

验收重点：

- 在没有 `.git` 的 Libra 仓库中可运行。
- dirty worktree 下不改用户 index。
- capsule approve 后可通过 `--json` 列出，projection rebuild 后一致。
- redaction 失败 fail-closed。

### Phase 2：接入 agent 分割、edge judge 与 recall/compose

在确定性路径稳定后接入智能层：

- 内部 skill：`libra-research-capture`，读取 pending proposals，调用 sub-agent 生成高质量 capsules。
- 内部 skill：`libra-research-recall`，召回 capsule、读取 dependencies、生成 compose brief。
- edge baseline：确定性写 `overlaps` / `same_region`（复用 extractor 的 (file, symbol) 键）。
- edge judge：通过内部 `SubAgentDispatcher` 或现有 AgentRuntime 派生 reviewer，确认 `depends_on`、`alternative_to`、`composable_with`、`supersedes`、`conflicts_with`。
- recall：先实现 research-git 风格 lexical（字段加权）+ structure + neighbor boost；embedding 留作可选索引，不做真源。
- compose：返回 capsule intent/knobs/assumptions/guide、clean slices、current source、merge context。

注意：`research-git` 依赖宿主 CLI 的 subagent subscription；Libra 应优先复用自身 provider/runtime/usage/sandbox，不新增旁路 plugin 执行面。

### Phase 3：实现 regeneration 与 reproducibility close loop

目标是“再生由 agent author，复现由 Libra freeze/checkpoint 保证”：

- `research recall` 只产生候选 capsule 和 compose brief。
- `research apply` 或 skill 驱动 internal AgentRuntime 修改工作树，但不得自动提交。
- 用户或 workflow 跑测试/命令后，用 `libra research run --from <capsule>` 或已有 `libra code` run 记录 artifact/metrics。
- approval 后写 `variant_of` / `produced` / `active` edge。
- 如果 regeneration 改善了 guide，支持 `capsule update-guide`，保留旧 revision。

这能保留 research-git 的关键安全边界：agent 只负责 authoring，byte-exact artifact/replay 不依赖 agent。

### Phase 4：补研究分析层

对齐 research-git v3 能力，但做成 Libra-native 输出：

- `libra research compare <capsule|symbol>`：按 variant cluster + metric direction 排名（参考 `compare.py` 的传递闭包 + Δ + ★）。
- `libra research ablation <capsule...>`：按 active feature set 生成幂集网格。
- `libra research provenance <run>`：clean slice vs frozen/adapted slice。
- `libra research metric-dir set/list/suggest`：配置 metric 方向（含 `metricdir.py` 式启发式建议）。
- `libra research graph`：输出 JSON/Mermaid，后续接入 Web graph，而不是新增长期 TUI。

这些命令应复用 `usage` 的 CSV/JSON 输出习惯，但不要混淆成本 usage 与实验 metric。

### Phase 5：MCP 与团队共享

- 先把 `mcp/authz.rs` 的 `McpAuthorizer` 真正接入 server 请求路径（这是开放 research MCP 的硬前提）。
- MCP 暴露只读 `research_recall`、`research_get_capsule`、`research_compose`、`research_compare`、`research_ablation`、`research_provenance`。
- 所有 mutating 操作继续走 CLI/AgentRuntime，并经 `McpAuthorizer` / approval / sandbox。
- `refs/libra/research` 可由 `libra push` 或 cloud sync/publish 传播；projection 在 clone/restore 后重建。
- 对 secret/private capsules 做 namespace/actor/sensitivity gate，默认不向共享 MCP 返回。

## 不应照搬的部分

| research-git 做法 | Libra 不应照搬的原因 | Libra 替代 |
|---|---|---|
| `.rgit/graph.db` 作为事实源 | Libra 已有 `.libra/libra.db`、对象库和 refs；旁路存储会破坏 push/cloud/restore 一致性 | `refs/libra/research` + 可重建 SQLite projection |
| shell out 到 `git` | Libra refs/index/worktree 语义不同，且部分仓库无 `.git` | 使用 Libra 内部 diff/index/worktree/object API |
| Python-only `libcst` symbol map | Libra 面向多语言仓库 | 扩展已有 `semantic/extractor.rs`（tree-sitter）到多语言，未知语言 file+hunk fallback |
| MCP write tools | Libra MCP 已有控制面边界和授权计划 | 只读 MCP；写入走 CLI/AgentRuntime（先接线 `McpAuthorizer`） |
| subagent plugin 作为唯一智能入口 | Libra 已有 provider/runtime/skills/sub-agent/usage/sandbox（含 13 类 TaskFailure 门） | 内部 AgentRuntime 派生语义 agent，统一计费/审计/权限 |
| `install <platform>` 写 managed block 到各宿主 CLAUDE.md/AGENTS.md | Libra 不应通过编辑外部 agent 的全局指令文件来“宣传自己”；与 Libra 自有 skills/MCP/hooks 发现机制重叠且更难审计 | 通过 Libra 自身 MCP resources/skills/`libra agent hooks` 暴露 research 能力；guidance 留在 Libra 侧 |
| 只本地 `.rgit` share | 团队无法沿 Libra remote/cloud/publish 传播 | research ref 进入 Libra push/cloud/publish 模型 |

## 风险与设计约束

1. **隐私与 secret 泄露。** Capsule code_slices、resurrection_guide、metrics/env 可能携带密钥或私有实验数据。写入、MCP 返回、cloud sync 前必须有 redaction/sensitivity gate（且 `McpAuthorizer` 要先接线）。
2. **错误 capsule 会污染未来召回。** 必须保持 proposal/review 状态、confidence、source evidence、supersedes/revoke，而不是自动把每个 diff 变成 confirmed memory。
3. **symbol extractor 不可靠。** 即便 Libra 已有 tree-sitter 抽取器，symbol 也只是定位加速，不能成为唯一真源。所有 capsule 必须保留 raw diff/artifact/source refs。
4. **图关系过度智能化。** `depends_on` / `conflicts_with` 错边会比漏边更坏。默认写 neutral `overlaps`，高语义边需要 evidence 和可撤销。
5. **与 `Memory` 子系统边界。** `Memory` 适合长期事实与规则；`ResearchCapsule` 适合可再生实验想法。两者可以互相引用，但不要合并成一个表。
6. **与 external-agent checkpoint 边界。** `agent_checkpoint` 是外部会话审计与恢复；`ResearchCapsule` 是想法抽象。Checkpoint 可作为 evidence/source，不应被当成 capsule。
7. **性能。** 召回必须有 limit、分页、字节/token 上限；graph traversal 不得在大型 monorepo 中无界扩张。

## 推荐优先级

| 优先级 | 任务 | 原因 |
|---|---|---|
| P0 | 固定 `ResearchCapsule` / `ResearchRun` / `ResearchEdge` JSON schema 与 `refs/libra/research` 存储契约 | 防止实现先行导致事实源漂移 |
| P1 | 做确定性 capture/review/list walking skeleton | 不依赖 LLM，能尽快验证数据模型 |
| P1 | 从 Libra diff/apply_patch/session/checkpoint 建立 source evidence 引用 | 这是 Libra 相对 research-git 的结构性优势 |
| P1 | 把 `semantic/extractor.rs` 包成多语言 slice/anchor 服务 | capsule code_slices 的精度基座，已有 Rust 实现可复用 |
| P2 | 接入内部 skill/sub-agent 做 semantic segmenter 和 edge judge | 对齐 research-git 的核心智能闭环 |
| P2 | 先接线 `McpAuthorizer`，再加 read-only recall/compose MCP tools | 让外部 agent 可消费 memory，但不放开写面、不泄露 secret |
| P3 | compare/ablation/provenance/graph | 形成研究层产品差异，但依赖 capsule/run/edge 先稳定 |
| P4 | cloud/publish/team sharing policy | 需要等 redaction、sensitivity、namespace gate 稳定后再开放 |

## 一句话产品差距

Libra 已经能很好地保存“AI 和 VCS 发生过什么”（且在多宿主捕获、统一权限/预算/安全、tree-sitter 符号基座上结构性领先）；`research-git` 强在保存“这个探索里真正值得以后复活的想法是什么”，并已把它做成跨 4 个宿主默认可用的闭环。Libra 要补的不是另一个 Git wrapper，而是把“可复活的想法”变成与 commit、run、checkpoint 一样可版本化、可审查、可召回、可传播的一等对象。
