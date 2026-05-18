# Libra Core Features — Demo Script（可执行版）

一条主线、六幕戏，约 12 分钟。目标是让演示命令与当前 `libra` 实际能力完全对齐，不靠“口头假设”。

> **故事线**：
> 监控告警提示 `libra status` 在大仓库慢。开发者进入 `libra code`，由 AI 生成并执行修复计划，最终把结果推送到 GitHub，并展示可审计的 Intent/Plan/Run/PatchSet 图谱。

---

## 先修正原稿里不对齐的点

1. `libra cloud backup --to r2` 不是现有命令。
   现状是：`libra cloud sync | restore | status`。
2. `libra automation schedule ...` 不是现有命令。
   现状是：通过 `.libra/automations.toml` 配规则，再用 `libra automation run` 执行（默认 dry-run，`--live` 才真执行）。
3. `/provider ollama`、`/provider gemini` 不是当前内建斜杠命令。
   现状是：provider 在 `libra code --provider ...` 启动时确定。
4. `libra code --stdio` 不是“接管同一个 TUI 会话”的入口。
   现状是：它是 MCP stdio 服务。要驱动已运行的 TUI，应使用 `libra code-control --stdio`（前提是 `libra code --control write`）。
5. “删掉本地对象后 `libra log` 自动从 R2 拉回”不是当前保证行为。
   现状是：`log` 缺对象会报错；恢复应显式跑 `libra cloud restore`。

---

## 道具准备（演示前 10 分钟）

| 道具 | 目的 |
|---|---|
| 一个中等规模 Libra 仓库 | 体现真实开发场景 |
| 已配置 AI provider（如 Gemini 或 Ollama） | 演示 `libra code` |
| 已配置 Cloud 环境变量（D1/R2） | 演示 `libra cloud sync/restore` |
| GitHub 远端仓库 | 演示 Git 协议互操作 |
| 终端分屏（TUI + 辅助终端） | 演示会话、控制与审计 |

---

## 第 1 幕：Git 协议兼容，但本地是 Libra 仓库（45 秒）

```bash
libra clone https://github.com/<user>/<repo>.git demo
cd demo
libra log --oneline | head -5
ls .libra/
```

**讲解要点**：

- 对外（clone/fetch/push）兼容 Git 协议，直接接入 GitHub/GitLab。
- 对内是 `libra` 自己管理的 `.libra/`（不是 `.git/`），包含 SQLite 元数据与 AI 相关对象。
- 在 Libra 仓库中，读写操作应通过 `libra` 命令完成。

---

## 第 2 幕：`libra code` 同会话多视图（2 分钟）

```bash
libra code --provider gemini --control write
```

- TUI 启动后，浏览器打开 `http://127.0.0.1:3000`（默认端口）可看到同一会话。
- 辅助终端可展示本地控制信息：

```bash
cat .libra/code/control.json
```

可选（强调本地自动化控制，不是 MCP）：

```bash
libra code-control --stdio --url http://127.0.0.1:3000 --token-file .libra/code/control-token
```

---

## 第 3 幕：IntentSpec → 计划 → 执行 → 修复回路（5 分钟）

**叙事目标**：观众离场时能复述这条主线——"自然语言诉求 → 可审 IntentSpec → 可审执行计划 → 沙箱内执行 → 失败自动 replan → 一条可回放的 thread"。这是整场 demo 的主菜，其它幕都是配菜。

> **前置状态**：从第 2 幕进入 `libra code --provider gemini --control write`，TUI 主面板可见；辅助终端保留空闲窗口，留给末尾的 `libra graph`。

### 节奏总览

| 时间 | 镜头焦点 | 关键动作 |
|---|---|---|
| 00:00–00:30 | TUI 顶部状态栏 | 铺垫：这不是聊天，是受约束的工作流 |
| 00:30–01:15 | 消息流 | `/plan …` 触发 IntentSpec 生成 |
| 01:15–02:00 | IntentSpec 面板 | `/intent modify` 把"感觉慢"改成可验证标准 |
| 02:00–02:45 | Plan 面板 | `/intent execute` 进入计划生成，二道闸 |
| 02:45–03:45 | 工具调用流 | 计划确认 → 沙箱执行 |
| 03:45–04:20 | replan 提示 | 失败 → 自动修复 → 阈值停顿 |
| 04:20–04:45 | 退出提示 | run 完成，PatchSet 摘要，thread id |
| 04:45–05:00 | 辅助终端 | `libra graph <id>` 走一遍版本链 |

---

### 镜头 1（00:00–00:30）—— 铺垫

**焦点**：TUI 顶部状态栏（provider / agent / model）→ 输入框。

**台词**：
> "下面我不会用聊天问 AI 怎么改代码。我会让它先产出一份**可审计的意图说明书**，再产出**可审计的执行计划**，然后我们一起决定要不要让它动手。"

---

### 镜头 2（00:30–01:15）—— 触发 IntentSpec

**操作**：在输入框敲入

```
/plan libra status 在大仓库偏慢，请定位瓶颈并给出修复，约束：不破坏现有测试，且补丁不大于 200 行。
```

**应看到**：
1. 进入 Phase 0，状态条显示 "Generating IntentSpec…"。
2. LLM 调用 `submit_intent_draft`，IntentSpec 渲染为结构化 Markdown：`problem_statement` / `scope` / `constraints` / `acceptance_criteria` / `risks`。
3. 末尾固定提示：*Confirm this IntentSpec to generate an execution plan, modify it to revise scope, or cancel.*

**台词**：
> "注意——AI 此刻**没有动任何文件**。它给我的是一份意图说明书。这是 libra 跟普通 AI 编程的第一道分水岭。"

**兜底**：若 30s 内 IntentSpec 不出现，按 `Esc` 取消，切到第二个 provider 重试一次；再不行就切预录 cast。

---

### 镜头 3（01:15–02:00）—— 审 + 改 IntentSpec

**焦点**：IntentSpec 的 `acceptance_criteria` 区域（用箭头/光标圈出）。

**操作**：
```
/intent modify 把 acceptance_criteria 中"通过现有测试"改成"通过现有测试且 cargo test command::status_test 在 5 秒内完成"
```

**应看到**：IntentSpec 重新渲染，新条目落入 `acceptance_criteria`，spec 版本号/哈希变化。

**台词**：
> "我刚才做的事——把'感觉慢'变成一条**机器能验证的退出条件**。我们要的是**可被检验的需求**，不是一段聊天记录。"

**兜底**：modify 失败就跳过，口播走查 IntentSpec 内容即可，不影响主线。

---

### 镜头 4（02:00–02:45）—— Plan 生成与二道闸

**操作**：
```
/intent execute
```

**应看到**：
1. 进入 Phase 1："Generating execution plan…"。
2. LLM 调用 `submit_plan_draft`，渲染**有序任务列表**：每步含 tool、输入摘要、期望产物。
3. 末尾要求开发者确认。
4. 若计划触发网络/敏感目录，弹出 **Network Policy** 对话框（脚本里能稳定触发就保留，否则不要等）。

**台词**：
> "这是计划——读哪些文件、跑哪些命令、补丁打到哪儿。它要等我点确认才会动手，这是第二道闸。"

**操作**：确认 → 进入执行。

---

### 镜头 5（02:45–03:45）—— 执行

**焦点**：右侧/下方 tool-call 流面板。

**应按序看到**（大致顺序，因 LLM 而异）：

| Tool | 用途 |
|---|---|
| `read_file src/command/status.rs` | 读源 |
| `grep -n "fn execute" src/command/` | 定位 |
| `shell cargo check` | 编译预检 |
| `apply_patch …` | 打补丁 |
| `shell cargo test command::status_test` | 验证 |

**台词**（节奏感地讲）：
> "每一次工具调用打在屏幕上，每一个 shell 命令在执行前都过了一道安全分类——sandbox 第 4 幕详谈。"

**兜底**：执行卡住超过 60s 就 `Esc`，跳到镜头 6 讲 replan 机制；或切预录回放。

---

### 镜头 6（03:45–04:20）—— 失败 → replan

**触发方式**（演示前预选其一）：

- **自然路线**：让 `cargo test` 实际失败（道具仓库里预埋一处会 fail 的 assertion）。
- **强制路线**：道具仓库里预置一个第一次必败、第二次 replan 后通过的 fixture，确保观感稳定。

**应看到**：
1. 失败信号被 orchestrator 捕获，进入"自动修复"循环。
2. 第二次 replan + apply_patch + cargo test，绿。
3. 若到达连续 replan 阈值（见 `automatic_plan_repair_stops_at_threshold`），TUI 主动停下，要求开发者确认是否继续。

**台词**：
> "它**自己看见了红**，**自己重新规划**，到阈值会停下问我——不是无限循环烧 token。这条修复回路才是真正能进生产的关键。"

---

### 镜头 7（04:20–04:45）—— 收口

**应看到**（TUI 退出前的状态摘要）：
- run 状态：success
- PatchSet 摘要：files changed / additions / deletions
- 复制即用的命令行：`libra graph <uuid>`

**操作**：`/quit` 退出 TUI。

**台词**：
> "记住这条 thread id——这是后面所有审计的钥匙。"

**兜底**：若 TUI 没打印 thread id，去 `.libra/code/threads/` 取最近一条目录名即可。

---

### 镜头 8（04:45–05:00）—— 辅助终端 `libra graph`

**操作**（辅助终端，已切到道具仓库目录）：
```bash
libra graph <粘贴 thread id>
```

**应看到**：进入 graph TUI，呈现 Intent / Plan / Task / Run / PatchSet 节点与边。用方向键沿一条边走一遍。

**台词**（收束全幕）：
> "**代码不是我们交付的全部**——我们交付了意图、计划、执行轨迹、补丁集，每一个都能被回放、对比、审计。这才是 libra 这个名字的意思。"

---

### 演示前 30 分钟预演清单

1. 在道具仓库跑一遍 `cargo test command::status_test`，确认稳定通过/失败行为符合预期（镜头 5/6）。
2. 预录一份 cast，作为 provider 抖动时的兜底：
   ```bash
   asciinema rec scripts/demo/scene3.cast
   ```
3. 跑 `cargo test intent_flow_test`，确认 IntentSpec/Plan schema 没有意外变更。
4. 在 `.libra/code/threads/` 备好"上一次成功 thread id"，作为镜头 7 的 fallback。

---

## 第 4 幕：Sandbox 与命令安全（1 分钟）

**叙事钩子**：第 3 幕里你看到 AI 跑了好几个 `shell` 命令——它们都在沙箱里。

```bash
libra sandbox status
```

**应看到**：当前沙箱后端（macOS 是 seatbelt）、生效策略概要、当前是否处于审批链路中。

**台词**（一句话）：
> "AI 调用 shell 之前过一道安全分类，落到沙箱后端执行。这个诊断面板让我能在任何时候验证'真的被隔离了'，而不是相信文档。"

**裁剪线**：原稿的"读 seatbelt sbpl 源码"已删——观众看代码无收益，时间留给主菜。

---

## 第 5 幕：Cloud 同步与显式恢复（1.5 分钟）

**叙事钩子**：第 3 幕产出的 IntentSpec / Plan / Run / PatchSet 现在都在本地。把它们同步到云、再到新机器恢复回来。

**同步**（30 秒）：

```bash
libra config set cloud.name demo-repo
libra cloud sync && libra cloud status --verbose
```

**应看到**：`cloud status` 里对象数 / D1 引用数 / 上次 sync 时间，三个数字。这是云端真的有东西的证据。

**新机器恢复**（60 秒，辅助终端）：

```bash
libra init ../demo-restore && cd ../demo-restore
libra cloud restore --name demo-repo
libra log -1
```

**应看到**：restore 进度 → `log -1` 打印的 commit 就是第 3 幕推上去的那一条。

**台词**：
> "显式 `sync/restore`——不是任意命令自动回源，但足够证明跨环境闭环。"

---

## 第 6 幕：推送 GitHub + 自动化规则（1.25 分钟）

**推送**（15 秒）：

```bash
libra push origin HEAD
```

切到浏览器/GitHub tab 看一眼新 commit 落到了远端——一句话："对外 Git 协议，完全互操作。"

**自动化规则**（60 秒）：

```toml
# .libra/automations.toml
[[rules]]
id = "hourly-cloud-sync"
enabled = true
trigger = { kind = "cron", schedule = "@hourly" }
action = { kind = "shell", command = "libra cloud sync", timeout_ms = 600000 }
```

```bash
libra automation list
libra automation run --rule hourly-cloud-sync --live
libra automation history --limit 3
```

**应看到**：`run` 输出一行结果、`history` 打印最近 3 条带状态。

**台词**（收口）：
> "规则是声明式的，执行有历史可查。今天 demo 我们手动触发，常驻调度器在 P0 路线图上。"

**裁剪线**：原稿用 `--now 2026-05-16T10:00:00Z` 强行让 cron 命中——演示日一过就显得假，改成 `--rule <id>` 绕开时间机器；`history --limit 20` 缩成 3。

---

## 备用彩蛋（按时间裁剪）

| 片段 | 时长 | 用途 |
|---|---|---|
| `libra code-control --stdio` attach/submit/reclaim | 1 min | 展示本地自动化控制闭环 |
| 故意触发失败，观察自动修复与 replan | 1.5 min | 突出鲁棒性 |
| `libra graph` 对比多次 run 的差异 | 1 min | 突出可审计与可追踪 |

---

## 如果要实现“原稿理想版”还需要补哪些代码

下面是把原稿中“理想但当前不存在/不完整”的能力补齐到可演示状态的最小开发清单。

### P0（建议先做，直接提升 Demo 成功率）

1. **会话内 provider 热切换命令**
   - 目标：支持 `/provider <name> [model]` 在同一 TUI 会话切换 provider。
   - 主要改动：`src/internal/tui/slash_command.rs`、`src/internal/tui/app.rs`、`src/command/code.rs`、provider runtime 切换逻辑。
   - 验收：切换后 `/model` 显示更新，后续 turn 使用新 provider，旧会话不崩溃。

2. **`libra cloud backup`/`libra cloud recover` 语义别名（或统一入口）**
   - 目标：降低演示认知成本，把 `sync/restore` 包装成更直观的一键命令。
   - 主要改动：`src/command/cloud.rs` + `docs/commands/cloud.md`。
   - 验收：`backup` 等价 `sync`，`recover` 在空仓可完成 restore 并给出明确进度与失败提示。

3. **自动化常驻调度器**
   - 目标：支持 `libra automation daemon`（按间隔轮询 cron due rules），不再手动 `run --now`。
   - 主要改动：`src/command/automation.rs`、`src/internal/ai/automation/scheduler.rs`、退出/信号处理。
   - 验收：后台进程可稳定执行 due 规则，`automation history` 有完整记录。

### P1（可选，增强演示观感）

1. **Demo 一键准备命令**
   - 目标：新增 `libra demo prepare` 自动生成演示仓库、样例慢路径与校验脚本。
   - 主要改动：新建 `src/command/demo.rs`（或脚本化到 `scripts/demo/`）。
   - 验收：新同事在 5 分钟内可复现同一演示环境。

2. **Cloud 恢复后健康检查命令**
   - 目标：新增 `libra cloud verify`，校验对象完整性、refs、HEAD、可读日志。
   - 主要改动：`src/command/cloud.rs` + 测试。
   - 验收：恢复后给出明确 pass/fail 和可操作修复建议。

3. **演示导出工件**
   - 目标：一键导出 thread 的 Intent/Plan/Run/PatchSet 摘要（Markdown/JSON）。
   - 主要改动：`src/command/graph.rs` 或新增 `graph export` 子命令。
   - 验收：可直接作为 PR 附件或复盘材料。

---

## 一句话宣传语（保持准确）

- **“对外兼容 Git 协议，对内沉淀 AI 意图与执行历史。”**
- **“不只提交代码，还提交可审计的实现过程。”**
- **“同一会话跨 TUI/Web，本地自动化可控、可追踪。”**
