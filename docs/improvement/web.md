# Libra Code Web UI 接入计划

## Context

本文记录 `libra code` 浏览器 UI 的已落地基线和后续收口计划。它是 [agent.md](agent.md) 中 Code UI Source of Truth / Local TUI Automation Control 的前端落地补充，不替代 Agent runtime 主计划。

截至 2026-05-08，Web UI 已经不再是 mock shell：

- Rust 侧 [src/internal/ai/web/mod.rs](../../src/internal/ai/web/mod.rs) 已服务静态 `web/out/`、`/api/repo`、`/api/repo/status`、`/api/code/session`、`/api/code/events`、`/api/code/diagnostics`、`/api/code/threads`、controller attach/detach、message submit、interaction respond、turn cancel。所有 `/api/*` 仍由 `ensure_loopback_api_request` 强制 loopback；写路径有 256 KiB body limit 和 audit sink。
- Wire contract 已固定在 [src/internal/ai/web/code_ui.rs](../../src/internal/ai/web/code_ui.rs) 与 [web/src/lib/code-ui/types.ts](../../web/src/lib/code-ui/types.ts)。Rust 结构字段使用 `camelCase`，枚举使用 `snake_case`；[tests/ai_code_ui_wire_test.rs](../../tests/ai_code_ui_wire_test.rs) 覆盖 snapshot、controller、capabilities、transcript kinds、interaction kinds、thread list 等 JSON 形态。
- 前端生产路径已改走 [web/src/lib/code-ui/client.ts](../../web/src/lib/code-ui/client.ts)、[store.tsx](../../web/src/lib/code-ui/store.tsx)、[controller.tsx](../../web/src/lib/code-ui/controller.tsx) 和 [view-model.ts](../../web/src/lib/code-ui/view-model.ts)。`web/src/lib/mock/` 已不存在；`rg 'from "@/lib/mock' web/src` 应无结果。
- 浏览器写控制已通过 `--browser-control <off|loopback>` 落地。TUI 默认 `off`；`--web-only --provider codex` 默认 `loopback`；非 Codex web-only 默认 `off`。浏览器只持有 `X-Code-Controller-Token` lease；automation 额外需要 `X-Libra-Control-Token`。
- `--web-only --provider ollama` 已有 Phase 3 v0 的 [HeadlessCodeRuntime](../../src/internal/ai/web/headless.rs)：支持 browser submit、streaming assistant reply、cancel、只读本地工具；暂不支持 approvals、mutating tools、plan/patchset workflow、session persistence/resume。其它非 Codex provider 仍会回退 placeholder。

## 目标与非目标

**目标：**

- Browser UI 继续以 Rust `CodeUiSessionSnapshot` 为唯一运行时事实源；前端 view model 只能派生，不维护第二份 session state。
- 让 TUI session、Codex web-only、Ollama headless web-only 的浏览器观察与写控制都有自动化覆盖。
- 补齐 live UI 的可靠性：SSE 恢复、controller lease、错误态、长输出、diff parser、thread/status refresh、capability gating。
- 把 Web build、embedded assets、docs 和 command reference 纳入发布门。

**非目标：**

- 不开放公网写控制。Code UI v1 仍以 loopback 为安全边界。
- 不把浏览器 UI 做成多用户协作产品；同一 session 同时只有一个 active controller lease。
- 不在 Web terminal 提供任意 shell prompt。命令执行必须继续通过 agent/tool/approval 路径。
- 不在 headless v0 复制 TUI 的完整 IntentSpec/Plan workflow；后续要抽共享 session driver，而不是复制 ratatui `App` 状态机。

## 已落地基线

| 区域 | 已落地 | 仍需收口 |
|------|--------|----------|
| Wire contract | `CodeUiSessionSnapshot`、8 个 capability flag、5 个 interaction kind、3 类 controller 初始模式、serde golden；`docs/commands/code.md` 已补稳定字段表、thread list envelope 与 Code UI error code 表 | 后续新增字段/错误码时继续同步 Rust/TS/wire test/命令文档 |
| API | `/api/repo/status` 复用 `libra status --json` envelope；`/api/code/threads` 复用 `ThreadProjection::list_active`，`limit` clamp 到 200；写路径统一 body limit/audit；SSE lag 已恢复为完整 `session_updated` snapshot | API client、组件、browser audit scenario 已有回归；后续新增字段/错误码时继续同步测试 |
| Frontend data | `CodeUiProvider` 首屏拉 repo/status/session/threads，连接 SSE，status debounce 5s；Chat/Sidebar/Workflow/Summary/Diff/Terminal/Settings 都走 live store | 长 transcript、长 diff、长 tool output 已默认 collapse；旧 demo fixture 文案已从生产组件移除；loopback Web app + browser submit smoke 已纳入 scenario |
| Browser write | `BrowserControllerProvider` lazy attach，token 只在内存；submit/respond/cancel/detach 已接线；`BROWSER_CONTROL_DISABLED` 等错误能显示 | 五类 interaction 组件测试、lease retry/conflict、audit log scenario 已落地；lease 过期/多 tab 端到端 UI 行为仍可继续扩充 |
| TUI write bridge | `--browser-control loopback` 打开 browser write；TUI default 保持 `off`；TUI reclaim 会清 browser lease | 需要继续验证 `{off, loopback} x {host} x {TUI, web-only}` 的矩阵数据驱动化 |
| Headless web-only | Ollama v0 可由浏览器驱动直接 turn，capabilities 为 `messageInput`、`streamingText`、`toolCalls`；provider bootstrap 复用 `ProviderFactory` | 缺 mutating tools sandbox/approval、request-user-input、session persistence/resume、plan/patchset |
| Docs | [web/README.md](../../web/README.md) 与 [docs/commands/code.md](../commands/code.md) 已描述 live API、browser-control、token 分工、256 KiB 限制 | 本文需要作为后续 PR 的剩余工作清单；remote notice 已落地，仍需后续扩展浏览器端组件/客户端测试 |

## API 边界

所有 `/api/*` 请求必须来自 loopback。即便 `--host 0.0.0.0` 绑定成功，远程客户端访问 API 也会收到 `LOOPBACK_REQUIRED`。前端只发 same-origin 请求，不持有、不发送 `X-Libra-Control-Token`。

| 操作 | Endpoint | 状态与约束 |
|------|----------|------------|
| Liveness | `GET /api/health` | 已落地，纯文本 `ok` |
| 仓库元信息 | `GET /api/repo` | 已落地，用于 Sidebar/Header |
| Git 状态 | `GET /api/repo/status` | 已落地，返回 `{ ok, command: "status", data }`，shape 与 `libra status --json` 一致 |
| Thread 列表 | `GET /api/code/threads?limit&offset` | 已落地，active non-archived projection，`limit` 默认 50，最大 200 |
| 初始 session | `GET /api/code/session` | 已落地，返回完整 `CodeUiSessionSnapshot`；无 runtime 时 `CODE_UI_UNAVAILABLE` |
| 实时更新 | `GET /api/code/events` | 已落地，SSE event type 为 `session_updated` / `status_changed` / `controller_changed`；`Lagged` 会恢复为完整 `session_updated` snapshot |
| 诊断 | `GET /api/code/diagnostics` | 已落地，redacted runtime info |
| 取得 lease | `POST /api/code/controller/attach` | browser body `{ clientId, kind: "browser" }`；automation attach 额外要求 `X-Libra-Control-Token` |
| 释放 lease | `POST /api/code/controller/detach` | `X-Code-Controller-Token`；automation lease 额外要求 control token |
| 发送消息 | `POST /api/code/messages` | `X-Code-Controller-Token`，body `{ text }`，256 KiB limit |
| 响应交互 | `POST /api/code/interactions/{id}` | `CodeUiInteractionResponse`，支持 `selectedOption` / `approved` / `applyToFuture` / `note` / `answers` |
| 取消 turn | `POST /api/code/control/cancel` | browser 只需 lease token；automation 还需 control token；与 TUI `Esc` 对齐 |

## 当前实现注意点

- `CodeUiProvider` 在 SSE 错误后退避重连并重新拉 `GET /api/code/session`；server 端收到 `BroadcastStream::Lagged` 时已发送一次完整 `session_updated` snapshot，避免静默丢事件。
- `controller.canWrite` 表示当前 controller state，不等价于 capability。UI 写控件必须同时检查 `capabilities.*`、session `status`、controller ownership 和 browser hook error。
- `LocalTui` 与 `Fixed { Tui }` 语义不同：前者是可被 browser/automation lease 接管的可见 TUI owner，后者是只读观察时的永久阻断。
- Headless v0 只注册 `read_file`、`list_dir`、`grep_files`、semantic read 类工具。不要在 approval routing 落地前注册 `apply_patch`、`shell`、`web_search`，否则会绕过 TUI 路径已有 sandbox/network policy。
- `web/out/` 是 Rust embed 输入。任何 UI source 变更都必须 `pnpm --dir web build`，否则二进制仍服务旧页面。

## 后续实施计划

### Phase A：现有 live UI 可靠性与测试

**目标：** 不扩功能，先把已落地行为变成可回归的契约。

**任务：**

- [x] 增加 `web/src/lib/code-ui/client.test.ts`：覆盖 fetch error mapping、`RepoStatusEnvelope`、thread list query、SSE parse、controller token header、256 KiB client-side guard；`web/package.json` 已增加 `test` 脚本用于本地回归。
- [x] 增加 store/controller hook 测试：`web/src/lib/code-ui/store.test.tsx` 覆盖首屏加载、`CODE_UI_UNAVAILABLE`、SSE error reconnect、status debounce；`web/src/lib/code-ui/controller.test.tsx` 覆盖 lease retry once、`CONTROLLER_CONFLICT` 不重试。
  - [x] `web/src/lib/code-ui/controller.test.tsx` 已覆盖 browser controller lazy attach、stale token retry once、`CONTROLLER_CONFLICT` 不重试、`BROWSER_CONTROL_DISABLED` error surface。
- [x] 增加组件测试：`message.test.tsx` 覆盖 streaming assistant message；`review-view.test.tsx` 覆盖无 session 空态、empty diff、parse error、long diff collapse；`interaction-panel.test.tsx` 覆盖 read-only controller、`BROWSER_CONTROL_DISABLED` 和 pending interaction 五种 kind。
- [x] Rust：`tests/code_ui_scenarios.rs` 已补 `browser_write_appends_redacted_control_audit`，覆盖 browser lease 的 interaction/respond、message submit、turn cancel 审计日志与 client id redaction；`/api/code/threads` invalid limit / clamp 场景已由 `tests/code_ui_remote_security_matrix.rs` + `tests/data/code_ui_remote/security_cases.json` 覆盖。
- [x] 修复 SSE lag 可观测性：`BroadcastStream` 收到 `Lagged` 时不能静默丢弃；server 会发送一次完整 `session_updated` snapshot。

**验收：**

```bash
pnpm --dir web lint
pnpm --dir web build
LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --features test-provider \
  --test ai_code_ui_wire_test \
  --test code_ui_scenarios \
  --test code_ui_remote_lease_matrix \
  -- --test-threads=1
```

### Phase B：Remote Access Notice（已落地）

**目标：** 远程访问 `--host 0.0.0.0` 时不再加载一个永远 403 的 SPA，而是展示明确的 loopback-only 静态说明页。

**当前状态：** 已实现。[static_handler](../../src/internal/ai/web/mod.rs) 读取 `ConnectInfo<SocketAddr>`；非 loopback 客户端访问 HTML navigation 时返回零 JS remote notice，asset / API fallback 返回 404，避免启动 SPA。英文与中文静态页位于 [web/public/remote-notice/](../../web/public/remote-notice/) 并同步进入 [web/out/remote-notice/](../../web/out/remote-notice/)。

**任务：**

- [x] 将 `static_handler` 改成可读取 `ConnectInfo<SocketAddr>`。非 loopback 且请求 HTML 时返回 `remote-notice/index.html`；asset 请求返回 404，避免 SPA bootstrap。
- [x] 新增英文/中文零 JS 静态页：`web/out/remote-notice/index.html` 与 `web/out/remote-notice/zh-CN/index.html`。页面只展示 bind、remote IP、版本、commit 占位符，不展示 thread/provider/path/token。
- [x] 按 `Accept-Language` 选择中文页；Rust 响应阶段替换 `__LIBRA_BIND__`、`__LIBRA_REMOTE__`、`__LIBRA_VERSION__`、`__LIBRA_COMMIT__`。
- [x] 增加 server 单测：模拟非 loopback `ConnectInfo` 请求 `/`，断言 200、`text/html`、body 含 `loopback`，且不含 `<script`。

**硬约束：**

- 零 JavaScript、零外部请求、单页小于 30 KiB。
- 不暴露 session 信息、provider、工作目录、token、环境变量。
- 移动端 360px 可读，颜色对比度 ≥ AA。

### Phase C：Headless web-only v1

**目标：** 把 `--web-only --provider <non-codex>` 从 Ollama direct-chat v0 推到可用的 agent workflow。

**任务：**

- 已完成：`build_non_codex_headless_runtime()` 的 Ollama 路径收敛到 `ProviderFactory`，避免 Ollama 与 TUI provider bootstrap 分叉。
- 为 headless runtime 接入 `ToolRuntimeContext`、sandbox、approval store、network policy、usage recorder；mutating tools 必须通过 `CodeUiInteractionRequest` 显示 approval，不允许静默执行。
- 支持 `request_user_input`、`approval`、`sandbox_approval` 写回 `CodeUiInteractionResponse`。
- 接入 session persistence：`--resume <thread_id>` 能恢复 transcript、pending interaction、basic history；成功 turn 写回 session store。
- 支持 plan/patchset 最小投影，打开 `planUpdates` / `patchsets` capability 前必须有测试证明 UI 可重建。

**验收：**

- `libra code --web-only --provider ollama --model <model> --browser-control loopback` 能在浏览器完成 submit、tool approval、cancel。
- `--network-access deny` 下 headless `web_search` 不会绕过 policy。
- `--resume <thread_id>` 恢复后 `GET /api/code/session` 含历史 transcript 和 pending interaction。
- 非 Ollama provider 至少一个接入 `ProviderFactory` 路径并有 smoke。

### Phase D：Workflow / Summary / Diff / Terminal 完整映射

**目标：** 让页面主要区域对应真实 workflow，而不是低保真 snapshot 摘要。

**任务：**

- Workflow：`plans[].steps[].status` 与 `toolCalls[].status` 已映射到 phase strip / execution runs；detail panel 不再渲染旧 optimistic-mutation demo，run output 来自 tool snapshot details。后续仍需新增 `tasks[]` 单列，并把 patchsets / richer plan metadata 映射进详情页。
- Summary：继续以 `/api/repo/status` 为 branch source；保留文件计数，不做 mock 的行级 `+812 -214` shortstat；PR 字段 v1 不显示。
- Diff：统一 diff parser，支持多文件、binary/no diff、large diff collapse；解析失败 fail-open。
- Terminal：Sandbox / Tools / Agent tab 已展示真实 tool 与 info transcript；tool details 超过 200 KiB 默认截断并可展开；diagnostics 映射仍需后续接入。
- Sidebar：当前 thread + projection list 已有；后续新增 server-side `q=` 前先证明 client-side 50 条过滤不够。`New thread` 和 thread switch 仍引导 CLI，直到后端入口存在。
- Settings：保持只读；任何可修改项必须先有后端 endpoint 和权限模型。

**验收：**

- 页面无 demo 文案；空态明确。
- 长 transcript、长 diff、长 tool output 不阻塞主线程、不撑坏布局；chat pane 默认只渲染最新 transcript 窗口，单条超长消息按需展开。
- capability flag 改变时对应控件立即禁用或只读。

### Phase E：CI、文档与发布门

**目标：** 防止 Web source、embedded asset、Rust API contract 三者漂移。

**任务：**

- [x] CI 已有 `pnpm --dir web install --frozen-lockfile`、`pnpm --dir web lint`、`pnpm --dir web build` 专门 job；`web/pnpm-workspace.yaml` 允许 `msw`、`sharp`、`unrs-resolver` build scripts，避免 pnpm 11 `ERR_PNPM_IGNORED_BUILDS`。
- 继续检查 `web/out/` 与 source 变更同步。
- [x] 增加 browser smoke：`browser_static_app_loads_and_submit_updates_snapshot` 打开 `http://127.0.0.1:<port>`，断言页面不含旧 mock thread 内容，发送 browser message 后 snapshot 更新。
- [x] `docs/commands/code.md` 增加 Code UI snapshot 稳定字段表、thread list envelope、error code 表、`--browser-control` 矩阵。
- `docs/automation/local-tui-control.md` 与 `src/internal/ai/web/mod.rs` endpoint matrix 用 grep/脚本保持一致。

**验收命令：**

```bash
pnpm --dir web lint
pnpm --dir web build
cargo +nightly fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --features test-provider \
  --test ai_code_ui_wire_test \
  --test ai_code_ui_headless_test \
  --test code_ui_scenarios \
  --test code_ui_remote_lease_matrix \
  --test harness_self_test \
  --test code_codex_default_tui_test \
  -- --test-threads=1
```

## PR 切片建议

| PR | 范围 | 可独立验证 |
|----|------|------------|
| PR 1 | Phase A：frontend client/store/controller/component tests + SSE lag recover | web lint/build + code_ui scenarios |
| PR 2 | Phase B：remote notice static assets + `static_handler` remote HTML routing | curl + Rust handler tests |
| PR 3 | Phase C.1：headless ProviderFactory 收敛已完成；runtime context 注入（仍只读工具）待做 | headless tests + provider smoke |
| PR 4 | Phase C.2：headless approvals/request-user-input/mutating tools | browser interaction scenarios |
| PR 5 | Phase D：workflow/summary/diff/terminal 完整映射与长数据保护 | component tests + browser smoke |
| PR 6 | Phase E：CI/docs/release gate | full gate command |

## 风险与缓解

| 风险 | 影响 | 缓解 |
|------|------|------|
| SSE lag 静默丢弃 | UI 停在旧 snapshot，用户误判 turn 状态 | server 端把 lag 转成 full snapshot reload 信号；client 维持 reconnect + session refetch |
| 浏览器写控制暴露到非 loopback | 本地 workspace 被远程页面驱动 | `ensure_loopback_api_request` + `ensure_loopback_browser_control_host`；`loopback` 与非 loopback host 早失败 |
| TUI/browser/automation 同时写 | turn 状态错乱、approval 错配 | 单 controller lease；TUI reclaim；所有写路径走 `ensure_controller_write_access` |
| 浏览器误用 control token | 自动化级别提权泄露到网页 | frontend 禁止出现 `X-Libra-Control-Token`；浏览器只持有内存 lease token |
| Headless 注册 mutating tools 过早 | 绕过 sandbox/approval/network policy | Phase C 前仅注册 local read tools；capability 不提前打开 |
| Web build 未同步嵌入资源 | Rust server 服务旧 UI | CI 强制 `pnpm --dir web build` 并检查 `web/out/` |
| 长 diff/tool output 卡死 UI | 浏览器主线程阻塞或布局错位 | collapse/virtualize/truncate；组件测试覆盖长数据 |
| Docs/API drift | 用户按过期文档调用失败 | serde golden + docs consistency grep + command reference 字段表 |

## 决策与后续触发条件

- Remote notice 页面采用 hand-written static HTML，直接放入 `web/out/remote-notice/`。原因是零 JS、体积和泄露面可控；不得用 Next route 生成含 runtime bundle 的页面。
- Headless v1 先做 provider factory 和 runtime context 收敛，再逐步打开 mutating tools capability。不能只给 Ollama 打开 mutation 后把其它 provider 留在第二套启动路径。
- Thread list v1 不加 `q=` 后端搜索。默认 50 条 + client substring filter 是当前边界；只有当真实数据证明该限制不够，且已有 server-side pagination/search 测试时，才新增 `q=`。

## 完成定义

当以下条件全部满足时，Web UI 接入可视为完成：

- `rg 'from "@/lib/mock' web/src` 无结果。
- 浏览器刷新后能从 Rust snapshot 恢复真实 session（含 `transcript / plans / tasks / toolCalls / patchsets / interactions / controller`）。
- 浏览器能发送消息、取消 turn，并响应全部 v1 interaction kind。
- 8 个 capability flag 都被 UI 正确尊重，不可写场景一律置灰并解释原因。
- 普通 TUI、Codex web-only、Ollama headless web-only 都有自动化覆盖；headless mutating tools 不绕过 approval/sandbox/network policy。
- 远程非 loopback HTML 请求看到 static notice，而不是加载会 403 的 SPA。
- 文档明确 Web mode 能力边界、loopback 写控制、`X-Code-Controller-Token` vs `X-Libra-Control-Token`、256 KiB body limit、error code 与验收命令。
