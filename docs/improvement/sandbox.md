# Sandbox 安全隔离改进详细计划

## 所属计划

AI Agent 子系统专项计划，与 [agent.md](agent.md) Part B 并列。两者的边界：

- [agent.md](agent.md) Part B：负责 Phase 0–4 工作流、Snapshot/Event/Projection 对象模型、provider bootstrap。
- 本计划：负责 `libra code` 在宿主机上执行 Shell / ApplyPatch / Write 等 mutating tool 时的**操作系统级隔离层**，保障 provider / AI agent 失控时的爆炸半径。

## Context

AI Agent 在本地执行命令是 `libra code` 的核心能力，但也是攻击面最集中的入口：提示词注入、恶意 MCP server、失控的 provider 调用都可能通过 Shell tool 直达宿主机。当前 Libra 已经具备多档 `SandboxPolicy`、`SandboxEnforcement::{Required, PreferStrict, BestEffort}`、Seatbelt 策略拼装、macOS 敏感路径拒读、Linux 外部 helper + **内建 bwrap 直调（v0.17.724）**、sandbox 子进程 `setsid()`、审批审计、危险命令解析、危险 writable root 拒绝、每命令 0o700 私有 tmp 清理、`libra sandbox status` 自检与 `SandboxEvidenceSink` 结构化审计（v0.17.720..v0.17.726）（见 [src/internal/ai/sandbox/](../../src/internal/ai/sandbox/)），但相较 Claude Code 官方公开的 Bubblewrap 方案仍有若干关键缺口，包括：**Linux 沙箱默认仍是 best-effort 兼容模式，`Required` 会拒绝降级，`PreferStrict` 会在 helper 缺失时要求审批确认后才允许裸跑**、**Linux bwrap 敏感路径遮蔽仍待审计**、**默认 seccomp BPF 策略仍待提供（v0.17.725 已落地 `--seccomp <fd>` wiring + `seccomp_policy_path` 配置位；用户需自备编译后的 BPF 文件，缺省 `None`）**、**`NetworkAccess` 三态枚举已迁移（v0.17.723），但 per-allowlist 代理后端仍是 stub**。

本计划对齐 Claude Code 官方沙箱文档（`code.claude.com/docs/en/sandboxing`）与 Bubblewrap 工程实践，目标是把 Libra 在 AI Agent 失控场景下的实际爆炸半径降到与 Claude Code 相当的水平，并保证改动与 [agent.md](agent.md) Part B 的 Runtime 正式写入层兼容。**网络服务访问采取默认拒绝（default deny）策略**：沙箱内除 loopback 外的一切出站连接默认被 OS 层阻断，只能通过显式白名单放行。

## 0.17.37 落地审计快照（2026-05-12）

### 审计范围

- 代码版本：`libra 0.17.37`（`Cargo.toml`）
- 审计对象：本计划“阶段 1～阶段 7”是否已在主代码路径落地（不按文档目标推断，以代码事实为准）

### 审计结论

- **阶段 1、阶段 2 已收口（v0.17.724 内建 bwrap 真实执行 + v0.17.725 `--seccomp <fd>` wiring），阶段 3 / 4 继续收口，阶段 5 与阶段 6 已收口，阶段 7 stub 链 + 三态 `NetworkAccess` 枚举迁移已落地（v0.17.723）**。当前状态是“诊断面 + 显式 required enforcement + prefer_strict 降级审批 + sandbox 子进程新 session + macOS 敏感路径拒读 + Linux 外部 helper / 内建 bwrap 双路径 + seccomp `--seccomp <fd>` wiring + 危险挂载拒绝 + per-command tmp + Phase 7 stub（`NetworkProtocol`/`NetworkService` schema + `NetworkProxy` trait + `NoopProxy`/`LoopbackOnlyProxy` stubs + `NetworkEnforcementFailed` transform variant + `select_network_proxy` 决策树）+ 结构化 `SandboxEvidenceSink`（v0.17.720..v0.17.726）已具备；默认 seccomp BPF 策略、per-allowlist 代理后端、默认强制隔离仍待落地”。
- 与早期审计相比，`sandbox` 关键缺口中的危险 writable root 拒绝、`sandbox status`、`SandboxEnforcement::Required`、`SandboxEnforcement::PreferStrict` 降级审批、sandbox 子进程 `setsid()`、macOS 敏感路径拒读、Linux bwrap 参数构造层、内建 bwrap 真实执行、`--seccomp <fd>` 参数注入、`NetworkAccess` 三态枚举、per-command tmp、结构化 Evidence sink 均已进入主干；默认 seccomp BPF 策略与 per-allowlist 代理后端仍未进入主干实现。

### 分阶段状态（当前代码）

| 阶段 | 目标 | 现状 |
|---|---|---|
| 阶段 1 | `SandboxEnforcement` + `libra sandbox status` | 已落地：`sandbox status`、`Required` 拒绝降级、`PreferStrict` 降级审批确认均已接线；默认仍保持 `BestEffort` 兼容模式 |
| 阶段 2 | 内建 bwrap 直调 + seccomp | 已落地：v0.17.724 接入真实执行选择（`locate_bwrap_binary` 探测 `PATH` + `LIBRA_BWRAP_BINARY` 覆盖），v0.17.725 接入 `--seccomp <fd>` 参数 + `seccomp_policy_path` 配置位与 `pre_exec` FD 注入；默认仍不强制 BPF 策略（缺省 `None`），用户需自备编译后的 BPF 文件 |
| 阶段 3 | `setsid` / `--new-session` | 已落地：sandbox 子进程 Unix `setsid()` 已落地；bwrap 参数构造已包含 `--new-session` / `--die-with-parent`，内建 bwrap 真实执行路径（v0.17.724）下两者均在生效路径中 |
| 阶段 4 | 敏感路径拒读（`deny_read`） | 已落地：macOS Seatbelt 默认敏感路径拒读与 `.libra/sandbox.toml deny_read` 已落地；Linux bwrap 参数构造已把 `deny_read` 映射为 `--tmpfs` 遮蔽，内建 bwrap 真实执行路径（v0.17.724）下生效；剩余风险是默认 deny_read 清单仍较保守 |
| 阶段 5 | 每命令 0o700 tmp + 清理 | 已落地（0.17.37） |
| 阶段 6 | 危险挂载拒绝清单 | 已落地（0.17.25） |
| 阶段 7 | 网络三态 + allowlist/proxy | 部分落地：stub 链已就位（v0.17.668..v0.17.673）—— `NetworkProtocol`/`NetworkService`/`NetworkServiceValidationError` schema、`NetworkProxy` trait + `NoopProxy`/`LoopbackOnlyProxy` 两个零成本 stub impl、`NetworkEnforcementFailed { reason }` transform error variant、`NetworkAccess::is_restricted` + `all()`、`select_network_proxy` 三态决策树（`NetworkAccessMode { Denied, Allowlist, Full }` × `Option<&dyn NetworkProxy>` × `ProxyEnforcement { Required, PreferStrict, BestEffort }`）+ end-to-end 集成测试 `phase7_stub_proxy_chains_validated_allowlist_entry_to_proxy_decision`。剩余：`NetworkAccess` 2 态 → 3 态枚举迁移（breaking change，需逐 callsite audit）、Phase 7.4 真实 SNI/Host-header 匹配 proxy（替换 `LoopbackOnlyProxy` stub） |

### 关键证据（代码锚点）

- Linux helper 缺失时，默认 `best_effort` 仍会 `warn` 并回退到无沙箱；`LIBRA_SANDBOX_ENFORCEMENT=required` 或 runtime config 设为 `Required` 时，`SandboxManager::transform()` 会返回 `SandboxTransformError::EnforcementFailed`；`prefer_strict` 在 shell approval 路径会先要求确认降级。
- `SandboxEnforcement` 与 `EnforcementFailed` 已落地；`NetworkEnforcementFailed { reason: String }` transform error variant 已在 v0.17.670 落地（Display 模板 `"network enforcement failed: {reason}"`，与 `EnforcementFailed` 唯一区分以便 audit 消费者按子串过滤）。三态 `NetworkAccess` 枚举迁移仍待 callsite audit。
- `libra sandbox status` 已提供自检入口，输出平台、当前可用后端、当前 enforcement、writable roots、network/proxy 占位、helper/bwrap/Seatbelt 探测和降级告警；`required` 模式会把内部沙箱缺失报告为将失败，而不是描述为可接受降级。
- Linux 执行路径仍是外部 helper 参数转发路径（`--sandbox-policy` / `--use-bwrap-sandbox`）；`src/internal/ai/sandbox/runtime.rs` 已新增内建 `create_bwrap_command_args()` 参数构造层，但尚未接入真实执行选择。
- macOS 读权限仍保留 `(allow file-read*)` 作为项目可读基线，但已在其后追加默认敏感路径和 `.libra/sandbox.toml deny_read` 的 `(deny file-read* ...)` 规则；Linux bwrap 参数构造层已把 `deny_read` 映射为 `--tmpfs` 遮蔽。
- `run_command_spec` 已在每次执行前创建 `libra-sandbox-<uuid>` 私有 tmp、覆盖 `TMPDIR` / `TEMP` / `TMP`，并在命令退出后清理；清理失败目前记录 `tracing::warn!`。
- 网络模型仍是 `Restricted/Enabled` + `bool network_access`，不是 `Denied/Allowlist/Full`：`src/internal/ai/sandbox/policy.rs::NetworkAccess`（当前位于 policy.rs:79-83）、`src/internal/ai/sandbox/mod.rs` 内部 `effective_network_access()`、以及 `src/command/code.rs` 中传递 `--network-access` 的入口。锚点改为函数 / 枚举名而非行号，避免 sandbox 子系统继续重构时再次失锚。

## 0.17.25 增量收口（2026-05-12）

- **阶段 6 已落地**：`SandboxPolicy::validate_writable_roots_with_cwd()` 会拒绝 `/`、`/proc`、`/sys`、`/dev`、Docker/containerd socket、libvirt 控制路径，以及 `**/docker.sock` / `**/containerd.sock` 形态的危险 writable root。
- **执行入口已接线**：`SandboxManager::transform()` 在非 escalated 调用进入命令构造前先执行上述校验；错误通过 `SandboxTransformError::InvalidPolicy` 返回给 shell tool，而不是继续构造裸命令。
- **显式升级仍可绕过**：`SandboxPermissions::RequireEscalated` 表达用户批准的宿主级访问；该路径继续走 `SandboxType::None`，但必须由审批/提升权限流程显式触发。
- **回归覆盖**：`policy.rs` 覆盖 socket、kernel/device 路径和普通 workspace root；`runtime.rs` 覆盖危险 root 在 transform 阶段被拒绝，以及 explicit escalation 绕过策略校验。

## 0.17.37 增量收口（2026-05-12）

- **阶段 5 已落地**：`run_command_spec()` 现在会为每次 AI shell/tool 命令生成 `<SystemTmp>/libra-sandbox-<uuid>/`，并覆盖传入命令环境中的 `TMPDIR` / `TEMP` / `TMP`，避免复用宿主或调用方提供的临时目录。
- **私有权限已固定**：Unix 平台创建目录时使用 0o700，并在创建后再次设置权限；非 Unix 平台至少保证每命令唯一目录。
- **退出清理已接线**：命令成功、非零退出或命令构造失败后都会尝试 `remove_dir_all`；清理失败不覆盖命令结果，只写 `tracing::warn!`。
- **回归覆盖**：`command_tmpdir_is_private_0700_and_cleanup_removes_it` 验证 0o700 与清理；`run_command_spec_injects_private_tmp_and_cleans_it` 验证环境覆盖、命令内可写和命令后清理。

## 0.17.44 增量收口（2026-05-12）

- **阶段 1 显式 required enforcement 已落地**：`SandboxEnforcement::{Required, PreferStrict, BestEffort}` 已进入策略层并导出给 runtime config；默认保持 `BestEffort` 以维持既有行为。
- **Linux 静默降级已可关闭**：`SandboxManager::transform()` 在 `Required` 且内部 sandbox policy 需要 OS 后端时，不再允许 Linux helper 缺失后继续裸跑，而是返回 `SandboxTransformError::EnforcementFailed`。
- **环境开关已接线**：`LIBRA_SANDBOX_ENFORCEMENT=required|prefer_strict|best_effort` 会影响 `libra code` 命令构造路径；无效值返回用户可读错误。
- **诊断面同步**：`libra sandbox status` 的 `enforcement` 字段读取同一环境开关；`required` + Linux helper 缺失时告警会说明相关命令将失败。
- **仍待收口**：内建 bwrap 未落地前，Linux required 模式仍依赖外部 helper。

## 0.17.45 增量收口（2026-05-12）

- **阶段 3 部分落地**：`ExecEnv::into_command()` 在 Unix 平台对实际启用的 Libra 内部 sandbox 命令调用 `setsid()`，让 macOS Seatbelt / Linux helper 子进程脱离调用方 session。
- **生效范围**：仅当 `effective_sandbox` 为 `MacosSeatbelt` 或 `LinuxSeccomp` 时启用；显式 escalated、`DangerFullAccess`、`ExternalSandbox` 和默认 fallback `SandboxType::None` 不会被这个路径隐式改写。
- **回归覆盖**：`exec_env_new_session_runs_child_as_session_leader` 在 Unix 上验证 `setsid()` 后 child PID 与 session ID 一致；fallback / escalated transform 继续断言不会设置 `new_session`。
- **仍待收口**：内建 bwrap 尚未落地，因此阶段 2 的 `--new-session` 参数覆盖仍是后续工作；Seatbelt 读权限收紧也仍在阶段 4。

## 0.17.46 增量收口（2026-05-12）

- **阶段 4 macOS 基线已落地**：`sensitive_read_paths()` 定义默认敏感读取清单，覆盖 HOME 下的 `.ssh`、`.aws`、`.gnupg`、`.netrc`、`.config/gcloud`、`.kube`、`.config/libra/vault` 以及 `/etc/shadow`。
- **Seatbelt 拒读已接线**：`create_seatbelt_command_args()` 在 `(allow file-read*)` 后追加参数化 `(deny file-read* (subpath ...))`，保持项目文件可读，同时默认挡住常见 credential 目录。
- **回归覆盖**：策略层测试验证默认清单；macOS runtime 测试验证 Seatbelt deny policy 与参数表包含 HOME credential 路径和 `/etc/shadow`。
- **当时仍待收口**：Linux bwrap `--tmpfs` 遮蔽仍是阶段 4 后续工作；更广泛的浏览器/token 默认路径清单已在 0.17.76 扩展。

## 0.17.76 增量收口（2026-05-13）

- **阶段 4 macOS 默认拒读清单继续扩展**：`sensitive_read_paths()` 除 SSH/AWS/GPG/netrc/gcloud/kube/Libra vault 外，新增常见 token 与浏览器 profile 路径，包括 `.azure`、`.docker`、`.npmrc`、`.pypirc`、Cargo/Gem credentials、GitHub/Hub config、Firefox/Chrome/Chromium/Brave profile、Flatpak Firefox profile 和 macOS `Library/Cookies`。
- **Seatbelt 自动继承**：macOS Seatbelt policy 继续通过同一个 `sensitive_read_paths()` 入口生成参数化 `(deny file-read* ...)`，不需要额外配置即可挡住这些默认路径。
- **回归覆盖**：`sensitive_read_paths_include_home_credentials_and_system_shadow` 扩展断言 token 与浏览器路径。
- **仍待收口**：Linux bwrap 的 `--tmpfs` 遮蔽仍待阶段 2/4 合并推进；网络三态仍在阶段 7。

## 0.17.47 增量收口（2026-05-12）

- **自定义 deny_read 已落地**：`src/internal/ai/sandbox/mod.rs` 现在读取 `.libra/sandbox.toml` 的 `deny_read = [...]`，缺文件时 no-op，读取或 TOML 解析失败时返回用户可读错误。
- **路径解析契约**：`deny_read` 支持绝对路径、相对 sandbox cwd 的路径，以及 `~/...` HOME 展开；runtime-provided `SandboxRuntimeConfig::deny_read_paths` 会追加到同一清单并去重。
- **Seatbelt 接线**：macOS Seatbelt policy 会把默认敏感路径和自定义 `deny_read` 路径合并后生成参数化 `(deny file-read* (subpath ...))`。
- **回归覆盖**：新增测试覆盖缺失配置 no-op、相对/绝对路径解析、runtime config 追加和 TOML parse error；macOS policy 测试覆盖自定义 deny 路径进入参数表。
- **仍待收口**：Linux bwrap 的 `--tmpfs` 遮蔽仍待阶段 2/4 合并推进；网络三态仍在阶段 7。

## 0.17.48 增量收口（2026-05-12）

- **阶段 1 `PreferStrict` 审批确认已落地**：当 Linux 内部沙箱需要生效、`LIBRA_SANDBOX_ENFORCEMENT=prefer_strict` 或 runtime config 设为 `PreferStrict`，且未配置 `LIBRA_LINUX_SANDBOX_EXE` 时，shell 执行会先通过 `ExecApprovalRequest` 要求用户确认降级。
- **拒绝即停止执行**：用户拒绝、abort、approval channel 缺失，或 approval policy 为 `never` 时，命令不会在无内部沙箱下继续运行。
- **审批不可缓存**：sandbox fallback confirmation 使用独立 uncached approval request，并在请求中标记 `cache_disabled_reason = "sandbox fallback approvals are not cached"`。
- **回归覆盖**：Linux 单测验证 helper 缺失时会发出 outside-sandbox approval，拒绝后命令不会创建 marker 文件。
- **仍待收口**：默认 enforcement 仍保持 `BestEffort` 以兼容既有运行环境；内建 bwrap 和网络三态仍分别在阶段 2、阶段 7。

## 已完成前置条件与当前代码状态

### 已确认落地的基线

**策略层** [src/internal/ai/sandbox/policy.rs](../../src/internal/ai/sandbox/policy.rs)
- `SandboxPolicy` 四档：`ReadOnly` / `WorkspaceWrite` / `ExternalSandbox` / `DangerFullAccess`
- `WritableRoot` 写入根 + 保护子路径（`.git` / `.libra` / `.codex` / `.agents`）
- 路径通过 `canonicalize` 规范化（policy.rs:280 / 296）
- `/tmp` 与 `TMPDIR` 由策略显式纳入写入根
- 危险 writable root 拒绝：`/`、`/proc`、`/sys`、`/dev`、Docker/containerd socket、libvirt 控制路径，以及 `**/docker.sock` / `**/containerd.sock`

**运行时层** [src/internal/ai/sandbox/runtime.rs](../../src/internal/ai/sandbox/runtime.rs)
- macOS：`sandbox-exec` + 动态 `.sbpl` 模板（runtime.rs::create_seatbelt_command_args，当前位于 :520；`seatbelt_base_policy.sbpl` / `seatbelt_network_policy.sbpl` 通过 `include_str!` 嵌入，runtime.rs:526-527）
- Linux：调用外部 `libra-linux-sandbox` 可执行文件，支持 seccomp 或 bwrap 两种模式，经 `LIBRA_LINUX_SANDBOX_EXE` 与 `LIBRA_USE_LINUX_SANDBOX_BWRAP` 控制
- Windows：`SandboxTransformError::WindowsSandboxNotImplemented`，与 Claude Code 当前状态对齐
- 网络控制：沙箱策略联动 `LIBRA_SANDBOX_NETWORK_DISABLED` 环境变量和 Seatbelt 网络策略

**审批与命令安全** [src/internal/ai/sandbox/mod.rs](../../src/internal/ai/sandbox/mod.rs) + [src/internal/ai/sandbox/command_safety.rs](../../src/internal/ai/sandbox/command_safety.rs)
- `AskForApproval` 四档（Never / OnFailure / OnRequest / UnlessTrusted）
- 会话级审批缓存 `ApprovalStore`
- tree-sitter bash 解析 + 安全命令白/黑名单
- 沙箱拒绝关键词触发升级重试提示（mod.rs::is_likely_sandbox_denied，当前位于 mod.rs:1769；调用点在 mod.rs:1039）
- 每次命令执行前注入私有 0o700 tmp，并在执行后清理 `TMPDIR` / `TEMP` / `TMP` 指向目录
- 默认 60 秒超时、100 KiB 输出上限（[src/internal/ai/sandbox/mod.rs](../../src/internal/ai/sandbox/mod.rs) `DEFAULT_TIMEOUT_MS=60_000`、[src/internal/ai/tools/handlers/shell.rs](../../src/internal/ai/tools/handlers/shell.rs)）

**Worktree FUSE overlay** [src/command/worktree-fuse.rs](../../src/command/worktree-fuse.rs)
- 基于 `libfuse_fs::overlayfs` 的 COW 隔离，与 AI 沙箱解耦，当前仅服务 `git worktree --fuse`，尚未接入 AI 命令执行

### 基于当前代码的 Review 结论

- Linux 外部 helper 缺失时，默认 `BestEffort` 仍走 `tracing::warn!` 后“裸跑”；显式 `Required` 已返回 `EnforcementFailed`，`PreferStrict` 在 approval shell 路径会要求用户确认降级，不再无感知降级。
- Seatbelt 策略对读操作仍使用 `(allow file-read*)` 全局基线，但已拒读默认敏感路径（`~/.ssh` / `~/.aws` / `~/.gnupg` / `~/.netrc` / `.azure` / `.docker` / `.npmrc` / `.pypirc` / Cargo/Gem credentials / `~/.config/gcloud` / `~/.config/gh` / `~/.config/hub` / `~/.kube` / `.config/libra/vault` / Firefox、Chrome、Chromium、Brave profile / macOS `Library/Cookies` / `/etc/shadow`），并支持 `.libra/sandbox.toml deny_read` 追加本地路径。
- `ExecEnv::into_command()` 会对实际启用的 macOS Seatbelt / Linux helper sandbox 子进程执行 `setsid()`；内建 bwrap 的 `--new-session` 参数仍待阶段 2 一并实现。
- `run_command_spec` 已覆盖调用方传入的 `TMPDIR` / `TEMP` / `TMP` 并在命令后清理；剩余风险是清理失败仅进入 tracing，尚未写入 agent Runtime 的结构化 Evidence。
- `WorkspaceWrite::writable_roots` 已拒绝危险挂载清单；剩余风险是尚未把拒绝事件写成 agent Runtime 的 `ToolInvocation[E]` / `Evidence[E]` 结构化记录。
- `libra sandbox status` 已落地，用户可确认当前 `SandboxType` 与 `SandboxEnforcement` 诊断状态；剩余风险是默认 enforcement 仍为 `BestEffort`，尚未切换为默认强制或默认询问。

## 目标与非目标

**本轮目标（P0 / P1）：**

- **P0** 堵住 Linux 静默降级：引入 `SandboxEnforcement`，`Required` 下不得无沙箱执行
- **P0** 补齐终端注入防御（macOS Seatbelt + Linux bwrap 的 `--new-session` / `setsid`）
- **P0** 内建 Bubblewrap 直调，摆脱对外部 `libra-linux-sandbox` 的强依赖
- **P0** **网络服务默认拒绝**：沙箱内出站网络默认在 OS 层阻断（macOS Seatbelt 不注入网络策略 / Linux bwrap `--unshare-net`），仅放行 loopback；白名单只能通过显式配置放行
- **P1** 收紧 Seatbelt 读权限，对默认敏感路径（`~/.ssh`、`~/.aws`、`~/.gnupg`、`~/.netrc`、`~/.config/gcloud`、`~/.kube` 等）默认拒读
- **P1 ✅** 每命令 0o700 专属 tmp，退出即清
- **P1 ✅** 危险挂载拒绝清单（`/var/run/docker.sock` / `containerd.sock` / `/proc` / `/sys` / `/dev`）
- **P1 ✅** 新增 `libra sandbox status` 子命令，输出当前生效的隔离模式与降级告警

**后续维护目标：**

- WSL2 二次嵌套 / Docker-in-Docker 环境的自适应弱隔离告警
- 与 [agent.md](agent.md) Part B Runtime `write_run` / `write_tool_invocation` 的联动审计（沙箱拒绝事件 → `Evidence[E]`）

**本批非目标：**

- **不实现 Windows 沙箱**，保持与 Claude Code 当前状态一致
- **不实现完整的域名/SNI 过滤代理守护进程**：本轮落地数据结构 + OS 级默认拒绝 + 白名单配置解析 + stub 代理入口；基于 `hickory-dns` + TLS SNI 过滤的完整代理实现单列后续批次
- **不把 FUSE overlayfs 接入 AI 命令执行层**（价值高但侵入大，列为后续独立批次）
- **不重构 `libra-linux-sandbox` helper**（保留其作为兼容回退路径）

## 差距分析

| # | Claude Code 做到 | Libra 现状 | 严重度 | 落地阶段 |
|---|---|---|---|---|
| G1 | Linux 沙箱开箱即用（bwrap 直调） | 依赖外部 `libra-linux-sandbox` 二进制，未配置时 warn + 裸跑 | ★★★ | 阶段 1 + 阶段 2 |
| G2 | `--new-session` 阻断 TIOCSTI | sandbox 子进程已 `setsid()`；内建 bwrap `--new-session` 仍待阶段 2 | ★★★ | 阶段 3 部分落地 |
| G3 | tmpfs 空白根 + `--ro-bind` 精选注入 | macOS Seatbelt 已拒读默认敏感路径并支持自定义 deny_read；Linux bwrap 遮蔽仍待实现 | ★★ | 阶段 4 部分落地 |
| G4 | 默认拒绝 + 域名白名单的网络策略 | 仅 `network_access: bool`；enforcement 依赖环境变量 + 部分 Seatbelt 策略，未在 Linux 默认 `--unshare-net` | ★★★ | 阶段 7 本轮 OS 层 default deny + 白名单配置；域名过滤代理单列 |
| G5 | 每命令 0o700 tmp + `cleanupAfterCommand()` | 已在 `run_command_spec` 前后注入并清理私有 tmp；清理失败 Evidence 仍待 Runtime 接线 | ✅ | 阶段 5 已落地 |
| G6 | 内置 Seccomp 过滤器 | 依赖外部 helper | ★★ | 阶段 2（随 bwrap 直调） |
| G7 | 明确警示 Docker socket 挂入 = 逃逸 | 已在 `SandboxPolicy` + `SandboxManager::transform()` 拒绝危险 writable root | ✅ | 阶段 6 已落地 |
| G8 | `/sandbox` 自检状态 | `libra sandbox status` 已输出 OS backend 与降级告警 | ✅ | 已落地 |
| G9 | 嵌套容器 / WSL 的自适应降级告警 | 无 | ★ | 后续维护 |
| G10 | Windows 规划中 | 同样未实现 | — | 本轮不处理 |

## 改进阶段

### 阶段 1：堵住 Linux 静默降级 + 新增 `sandbox status` 子命令（P0）

**目标**：Linux 上"以为有沙箱、实际裸跑"的情况必须被消除；用户能在终端自查当前隔离模式。

1. **引入 `SandboxEnforcement` 枚举**（`policy.rs`）
   - 已新增 `enforcement: Required | PreferStrict | BestEffort`；当前默认保持 `BestEffort`，避免未显式配置的既有运行环境突然失败
   - `Required` 语义：内部 sandbox policy 需要 OS 后端时，若无法构造有效 sandbox，则返回 `SandboxTransformError::EnforcementFailed`
   - 与现有 `SandboxPermissions::RequireEscalated` 解耦：后者表达"这次调用合法地需要无沙箱"，前者表达"系统配置强制要求沙箱生效"；当前实现仍允许显式 escalated 调用绕过内部策略校验
2. **修改降级路径**（`runtime.rs::SandboxManager::transform` 内的 `EnforcementFailed` 返回；当前位于 runtime.rs:316 和 :364 两处）
   - 已改为根据 `enforcement` 决策：
     - `Required` → `SandboxTransformError::EnforcementFailed { reason }` 返回给调用方
    - `PreferStrict` → 已在 shell approval 路径复用 `ExecApprovalRequest` 弹用户确认；拒绝时不得裸跑
     - `BestEffort` → 保留现状
3. **新增 `libra sandbox status` 子命令（已落地）**
   - 已新增 `src/command/sandbox.rs`，挂到 [src/cli.rs](../../src/cli.rs) 顶层，并通过 [docs/commands/sandbox.md](../commands/sandbox.md) 记录输出契约。
   - JSON / machine / human 输出包含：当前平台、实际可用的 sandbox backend、当前 enforcement、writable roots、network access、helper 路径是否存在、Seatbelt / bwrap 探测结果和降级告警。
   - `Required` 运行时拒绝路径已落地；`PreferStrict` 审批确认已落地。稳定错误码仍保留为后续兼容性细化。

**本阶段非目标**：不改 Seatbelt 读权限、不实现 bwrap 直调（在阶段 2 / 阶段 4 分别处理）。

### 阶段 2：内建 Linux Bubblewrap 直调（P0）

**目标**：Linux 沙箱不再强依赖外部二进制，Libra 自身就能以非特权用户调 bwrap。

1. **新增 `create_bwrap_command_args(command, policy, cwd)`**（`runtime.rs`）
   - 与现有 `create_seatbelt_command_args` 对称，内建在 Libra 进程里
   - 参数集合至少包含：
     - `--unshare-all`、`--share-net`（受 `SandboxPolicy::has_full_network_access()` 控制）
     - `--ro-bind` 注入 `/usr`、`/lib`、`/lib64`、`/bin`、`/etc/resolv.conf`
     - 对每个 `WritableRoot`：`--bind <root> <root>`；对 `read_only_subpaths` 追加 `--ro-bind` 覆盖
     - `--proc /proc`、`--dev /dev`、`--tmpfs /tmp`（与阶段 5 的 0o700 tmp 目录叠加）
     - `--die-with-parent`、`--new-session`（阶段 3 会进一步使用）
   - 对 `.git` / `.libra` / `.codex` / `.agents` 复用 `protected_subpaths()` 生成额外 `--ro-bind`
2. **Linux 选择优先级**（`SandboxManager::select_initial` / `transform`）
   - 新顺序：内建 bwrap 直调 → 外部 helper → 按 `enforcement` 决定拒绝 / 降级
   - bwrap 可执行性探测放在 `SandboxManager::new()` 里缓存，避免每条命令 `which` 一次
3. **Seccomp profile 注入**
   - 将 Claude Code 同类方案中常见的 seccomp 黑名单（`mount`、`umount2`、`swapon`、`kexec_load`、`reboot`、`init_module` 等）内嵌为 Rust 常量，通过 `--seccomp <fd>` 传给 bwrap
   - 外部 helper 已有的 seccomp 策略继续保留作为回退

**本阶段非目标**：不把外部 helper 删除（保留为用户显式关闭内建 bwrap 时的回退；由环境变量 `LIBRA_SANDBOX_PREFER_HELPER` 控制）。

**0.17.161 增量状态**：`src/internal/ai/sandbox/runtime.rs` 已新增 `create_bwrap_command_args()` 参数构造层，覆盖 `--unshare-all`、默认 `--unshare-net` / full network `--share-net`、`--new-session`、`--die-with-parent`、workspace writable root bind、受保护子路径 read-only 覆盖、`--tmpfs /tmp` 与 `deny_read` 路径 `--tmpfs` 遮蔽。当前增量没有接入真实 bwrap 执行选择、可执行性探测缓存或 seccomp fd 注入，这些仍保留在阶段 2 后续任务中。

### 阶段 3：终端注入防御（P0）

**目标**：即使 AI 产出包含 TIOCSTI ioctl 的恶意命令，也无法把指令注入到宿主 TTY。

1. **macOS**（`create_seatbelt_command_args`）
   - 在 `seatbelt_base_policy.sbpl` 追加 `(deny iokit-open (iokit-user-client-class "IOTTYClient"))` 类规则
   - 已给 sandbox 子进程加 `setsid`（通过 `Command::pre_exec` 在 Unix 下设置 `setsid()`）
2. **Linux**（`create_bwrap_command_args` / 外部 helper）
   - 外部 helper sandbox 子进程已通过 `ExecEnv` 进入新 session
   - 内建 bwrap 参数追加 `--new-session` 仍待阶段 2 落地
3. **单元测试**
   - 已通过 Unix 单测验证 `setsid()` 后 child PID 与 session ID 一致；完整 Linux bwrap `tty=?` 覆盖仍待阶段 2

### 阶段 4：Seatbelt 读权限收紧（P1）

**目标**：AI 默认无法读敏感路径，即使被提示词注入或模型越权。

1. **引入 `sensitive_read_paths()`**（`policy.rs`）
   - 已新增默认清单：`~/.ssh`、`~/.aws`、`~/.gnupg`、`~/.netrc`、`.azure`、`.docker`、`.npmrc`、`.pypirc`、Cargo/Gem credentials、`~/.config/gcloud`、`~/.config/gh`、`~/.config/hub`、`~/.kube`、`~/.config/libra/vault`、Firefox/Chrome/Chromium/Brave profile、macOS `Library/Cookies`、`/etc/shadow`
   - 已支持用户在 `.libra/sandbox.toml` 的 `deny_read` 字段追加自定义路径（与 Claude Code `denyRead` 语义对齐）
2. **macOS Seatbelt 策略**
   - 已在 `create_seatbelt_command_args` 的 `file_read_policy` 之后追加 `(deny file-read* (subpath "..."))`，对每个敏感路径做参数化拒绝
   - 保持 `(allow file-read*)` 的全局放行基线（避免 agent 无法读项目文件和依赖），依赖 deny 规则覆盖敏感子树
3. **Linux bwrap**
   - 对敏感路径执行 `--tmpfs <sensitive_path>`（在沙箱内遮蔽为空目录）
   - 对 `~/` 本身不做 `--tmpfs`，只遮蔽子树，保证 shell 能正常启动
4. **配置加载**
   - `.libra/sandbox.toml` 解析入口已放在 [src/internal/ai/sandbox/mod.rs](../../src/internal/ai/sandbox/mod.rs)；缺文件时 no-op，解析/读取错误返回用户可读错误。

### 阶段 5：每命令 0o700 tmp + 退出清理（P1，已落地）

**目标**：命令间不留 token / cookie / 缓存残留，不同 AI 调用之间互相不可见。

1. **`run_command_spec` 前置**（`mod.rs`）
   - 已在调用方进入前生成 `<SystemTmp>/libra-sandbox-<uuid>/`
   - Unix 平台创建后固定 0o700 权限；非 Unix 平台至少保持每命令唯一目录
   - 已覆盖注入到 `CommandSpec::env` 的 `TMPDIR` / `TEMP` / `TMP`
2. **命令退出后异步清理**
   - 已在 `run_command_spec` 返回前执行 `tokio::fs::remove_dir_all`
   - 清理失败不阻塞主流程，当前走 `tracing::warn!`；记录 `ToolInvocation[E]` 元数据仍保留给 [agent.md](agent.md) Part B 的 Runtime 正式写入层
3. **cleanupAfterCommand 对齐**
   - macOS 下若出现 Seatbelt "ghost dotfiles"（允许写 + 实际被 deny → 0 字节占位），也在清理阶段统一擦除

### 阶段 6：危险挂载拒绝（P1，已落地）

**目标**：用户/配置错误不能让沙箱"自己打开后门"。

1. **`WorkspaceWrite::writable_roots` 装载校验**（`policy.rs`）
   - 已拒绝：`/var/run/docker.sock`、`/run/docker.sock`、`/run/containerd/containerd.sock`、`/proc`、`/sys`、`/dev`、`/var/run/libvirt/*`、`/` 本身
   - 已覆盖 glob 风险：`**/docker.sock` 与 `**/containerd.sock` 形态按文件名拒绝
2. **错误信息**
   - 已按用户友好约束指出被拒的 writable root、拒绝原因和最小修复方向（改用非特权项目目录、窄代理，或显式提升权限）
   - 当前错误通过 `SandboxTransformError::InvalidPolicy` 进入 tool 层；显式 `StableErrorCode` 仍等待 `libra sandbox status` / sandbox command surface 一并设计

### 阶段 7：网络服务默认拒绝 + 白名单放行（P0）

**目标**：沙箱内除 loopback 外的一切出站网络服务默认在 OS 层被阻断；只能通过显式白名单放行；enforcement 必须发生在内核/策略层而不是可被 agent 旁路的应用层。

1. **`NetworkAccess` 语义重整**（`policy.rs`）
   - 重构为三态：
     ```rust
     pub enum NetworkAccess {
         Denied,                                      // default
         Allowlist { services: Vec<NetworkService> }, // 仅放行显式声明
         Full,                                        // 仅 DangerFullAccess / 用户显式升级
     }
     pub struct NetworkService {
         pub host: String,          // 支持通配符，如 "*.npmjs.org"
         pub ports: Vec<u16>,       // 为空视为允许所有端口
         pub protocol: Option<NetworkProtocol>, // tcp / udp，默认 tcp
     }
     ```
   - `Default` 改为 `NetworkAccess::Denied`（当前隐式默认 `false` → 显式 `Denied`，消除语义歧义）
   - 序列化：`#[serde(tag = "mode", rename_all = "kebab-case")]`；对老 JSON 字段 `network_access: bool` 提供一次性迁移（`true → Full`、`false → Denied`）

2. **OS 层 default deny**
   - **macOS Seatbelt**（`create_seatbelt_command_args`）：
     - `Denied` / `Allowlist` 模式下**不再注入** `seatbelt_network_policy.sbpl`，并显式追加 `(deny network*)` + `(allow network* (local ip "localhost:*"))` 作为保险
     - `Full` 模式下才注入 `seatbelt_network_policy.sbpl`
     - `Allowlist` 模式的按域名放行由阶段 7.4 的代理 stub 承担
   - **Linux bwrap**（`create_bwrap_command_args`）：
     - `Denied` / `Allowlist` 模式下默认 `--unshare-net`（仅 loopback）
     - `Full` 模式下 `--share-net`
     - `Allowlist` 模式下通过 `--setenv HTTPS_PROXY=http://127.0.0.1:<port>` 指向本地代理（阶段 7.4）

3. **配置契约**（`.libra/sandbox.toml`）
   ```toml
   [sandbox.network]
   mode = "denied"  # denied | allowlist | full；默认 denied

   # 仅在 mode = "allowlist" 时生效
   [[sandbox.network.services]]
   host = "registry.npmjs.org"
   ports = [443]

   [[sandbox.network.services]]
   host = "*.pypi.org"
   ports = [443]

   [[sandbox.network.services]]
   host = "github.com"
   ports = [22, 443]
   ```
   - 解析走第一批 `config_kv` / `resolve_env()` 基础设施（详见 [config.md](config.md)）
   - 拒绝 `host = "*"` 或空字符串；拒绝未声明 `ports` 的 22/3389 高敏感端口

4. **本地白名单代理（本轮 stub + 后续完整实现）**
   - **本轮**：在 `src/internal/ai/sandbox/` 下新增 `proxy.rs`，提供 `NetworkProxy` trait 与 `NoopProxy`（拒绝一切）、`LoopbackOnlyProxy`（默认）两个实现
   - `Allowlist` 模式若代理未启动则按 `SandboxEnforcement` 决策：
     - `Required` → `SandboxTransformError::NetworkEnforcementFailed`
     - `PreferStrict` → 降级为 `Denied` 并在 `sandbox status` 与审批 UI 告警
     - `BestEffort` → 保持 `Denied`
   - **后续批次**：实现基于 `hyper` + `hickory-resolver` 的 HTTP CONNECT 代理，按 SNI / HTTP Host 过滤；不做 MITM（不需要注入 CA）

5. **审批与审计**
   - 任何 `NetworkAccess` 升级（`Denied → Allowlist`、`Allowlist → Full`）必须经过 `ExecApprovalRequest` 审批通道
   - 网络拒绝事件（连接被 OS 或代理阻断）写入 `ToolInvocation[E]` + `Evidence[E]`（详见 [agent.md](agent.md) Part B Runtime 正式写入层）
   - `libra sandbox status` 输出 `network.mode` / `network.allowlist` / `proxy_backend` / `effective_enforcement`

6. **兼容性处理**
   - 现有 `WorkspaceWrite { network_access: false, .. }` 配置自动映射到 `NetworkAccess::Denied`
   - 现有 `SandboxPolicy::ExternalSandbox { network_access: NetworkAccess::Enabled }` 映射到 `NetworkAccess::Full` + `SandboxEnforcement::BestEffort`
   - 默认策略 `SandboxPolicy::default()` 变更为 `NetworkAccess::Denied`（当前已是 `network_access: false`，行为等价但语义更清晰）

**本阶段非目标**：不实现完整域名过滤代理的 SNI 解析、TLS 握手透传、连接池与 metrics；这些单列后续批次。阶段 7 收口标准是"默认出站 deny 在 OS 层可验证生效；allowlist 配置能加载并在 stub 代理下表达出来"。

## 关键文件与可复用符号

| 目标修改点 | 文件 | 既有可复用符号 |
|---|---|---|
| 策略扩展（`enforcement` / `deny_read` / `NetworkAccess::{Denied,Allowlist,Full}` / `NetworkService`） | [src/internal/ai/sandbox/policy.rs](../../src/internal/ai/sandbox/policy.rs) | `SandboxPolicy`、`WritableRoot`、`protected_subpaths`、`resolve_root`、`NetworkAccess` |
| 网络代理 stub + trait | 新增 `src/internal/ai/sandbox/proxy.rs` | 无（新建）；后续接入 `hyper` / `hickory-resolver` |
| Linux bwrap 直调 + `--new-session` + seccomp | [src/internal/ai/sandbox/runtime.rs](../../src/internal/ai/sandbox/runtime.rs) | `create_linux_sandbox_command_args`（参考）、`create_seatbelt_command_args`（对称实现） |
| Seatbelt 读权限收紧 | [src/internal/ai/sandbox/seatbelt_base_policy.sbpl](../../src/internal/ai/sandbox/seatbelt_base_policy.sbpl) | 现有基础策略 |
| 命令执行 / 审批 / tmp 清理 | [src/internal/ai/sandbox/mod.rs](../../src/internal/ai/sandbox/mod.rs) | `run_shell_command_with_approval`、`SANDBOX_DENIED_KEYWORDS`、`ApprovalStore` |
| Shell handler 传递 enforcement | [src/internal/ai/tools/handlers/shell.rs](../../src/internal/ai/tools/handlers/shell.rs) | `ShellCommandRequest`、`ToolRuntimeContext` |
| `libra sandbox status` 子命令 | 新增 `src/command/sandbox.rs` + [src/cli.rs](../../src/cli.rs) | 参考 [src/command/worktree.rs](../../src/command/worktree.rs) 的子命令模式、第一批 `OutputConfig` / `emit_json_data()` / `info_println!()` |
| 配置读取（`.libra/sandbox.toml`） | [src/internal/ai/sandbox/mod.rs](../../src/internal/ai/sandbox/mod.rs) | 第一批 `config_kv` / `resolve_env()`（详见 [config.md](config.md)） |

## 与 agent.md Part B 的交接契约

- **Runtime 正式写入**：沙箱拒绝事件、enforcement 失败、危险挂载拒绝必须落到 [agent.md](agent.md) Part B Runtime 的 `ToolInvocation[E]` + `Evidence[E]`，不能藏在 tracing 日志里
- **Phase 边界**：沙箱策略的选择与降级发生在 Phase 2 `CodexTaskExecutor` / `CompletionTaskExecutor` 进入 Runtime 写入之前；Phase 0 / 1 的 readonly tools 默认走 `SandboxPolicy::ReadOnly` + `SandboxEnforcement::Required`
- **Projection**：`libra sandbox status` 输出可作为 Projection 读取消费者（UI / MCP / diagnostics），但不成为真相源

## 验证方式

### 单元测试（新增 / 扩展）

1. [src/internal/ai/sandbox/policy.rs](../../src/internal/ai/sandbox/policy.rs)
   - `SandboxEnforcement` 的 serde round-trip
   - `sensitive_read_paths()` 在不同 `$HOME` 下的展开结果
   - `writable_roots` 含 `/var/run/docker.sock` 时的拒绝路径（已覆盖）
   - `NetworkAccess::{Denied, Allowlist, Full}` 序列化 + 老 `network_access: bool` 迁移
   - `NetworkService` 的 host 通配符匹配、端口列表、高敏感端口（22 / 3389）默认拒绝
   - `SandboxPolicy::default()` 的 `NetworkAccess` 分支应为 `Denied`
2. [src/internal/ai/sandbox/runtime.rs](../../src/internal/ai/sandbox/runtime.rs)
   - `create_bwrap_command_args` 的参数顺序、`--new-session` / `--die-with-parent` 存在
   - `create_bwrap_command_args` 对 `NetworkAccess::Denied` 必须包含 `--unshare-net`，对 `Full` 必须不含
   - `create_seatbelt_command_args` 对 `sensitive_read_paths()` 生成 deny 规则
   - `create_seatbelt_command_args` 对 `NetworkAccess::Denied` / `Allowlist` 不注入 `seatbelt_network_policy.sbpl` 且追加 `(deny network*)`
   - Linux `enforcement=Required` + `linux_sandbox_exe=None` + 未探测到 bwrap 应返回 `EnforcementFailed`
   - `NetworkAccess::Allowlist` + 代理未启动 + `enforcement=Required` 应返回 `NetworkEnforcementFailed`

### 集成测试（新增 `tests/sandbox_hardening_test.rs`）

1. **静默降级**：`enforcement=Required` 且 helper / bwrap 均缺失 → 执行返回 `EnforcementFailed`
2. **敏感读拒绝**：临时 `HOME` 下建 `~/.ssh/id_rsa` 伪文件，agent 通过 Shell tool `cat ~/.ssh/id_rsa` 应非零退出且 stderr 命中 `SANDBOX_DENIED_KEYWORDS`
3. **危险挂载拒绝（已覆盖）**：`writable_roots` 含 `/var/run/docker.sock` → `SandboxManager::transform()` 在命令构造前失败，错误消息包含用户可读提示
4. **0o700 tmp 清理**：任一沙箱命令退出后，`<SystemTmp>/libra-sandbox-<uuid>/` 已被移除；失败路径也至少清一次
5. **`--new-session` 生效（Linux only）**：沙箱内 `ps -o pid,sid,pgid,tty` 应显示 `tty=?`
6. **`libra sandbox status` 契约**：JSON 输出包含 `platform` / `sandbox_type` / `enforcement` / `writable_roots` / `network.mode` / `network.allowlist` / `proxy_backend` / `bwrap_available` / `helper_path`；退出码与全局三级模型一致
7. **默认网络拒绝**：不写任何配置的情况下，`curl https://example.com` 在沙箱内应失败（Linux 看到 `Could not resolve host` 或 `Network is unreachable`，macOS 看到 Seatbelt deny）
8. **白名单放行**：配置 `mode = "allowlist"` + `host = "registry.npmjs.org"` 后，`curl https://registry.npmjs.org` 在 stub 代理下应得到"代理未启动"的显式错误（而不是默默成功），完整代理放行留后续批次验证
9. **迁移兼容**：老 JSON `{"type": "workspace-write", "network_access": true}` 反序列化后 `NetworkAccess` 应为 `Full`；`{"network_access": false}` 应为 `Denied`

### 手工验证

```bash
cargo run -- sandbox status                        # 查看当前隔离状态
cargo run -- sandbox status --json | jq .          # 机器可读

# 模拟 helper 缺失 + Required → 必须拒绝执行
LIBRA_LINUX_SANDBOX_EXE= cargo run -- code
# 进入 TUI 后触发 agent 执行 `env | grep -E "AWS|AZURE|LIBRA"`，核对 token/env 被遮蔽
# 触发 agent 执行 `cat ~/.ssh/id_rsa`，应被拒绝

# 默认网络拒绝：不改配置，触发 agent 执行：
#   curl -v https://registry.npmjs.org     # 应失败
#   curl -v https://127.0.0.1:<local_port> # 应成功（loopback 放行）

# 显式 allowlist：在 .libra/sandbox.toml 写入
#   [sandbox.network] mode="allowlist"
#   [[sandbox.network.services]] host="registry.npmjs.org" ports=[443]
# sandbox status 应显示 network.mode="allowlist" + proxy_backend="stub"
```

### CI（延续 CLAUDE.md 约束）

1. `cargo +nightly fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test --all`（含新增 `tests/sandbox_hardening_test.rs`）
4. `docs/commands/sandbox.md`（随阶段 1 一起补齐）与命令输出保持一致

## 非本轮改动

- **Windows 沙箱**：保持 `WindowsRestrictedToken` 未实现，和 Claude Code 当前状态对齐
- **完整的域名过滤代理**：本轮落地 OS 层 default deny + 白名单配置 + stub 代理；基于 `hyper` + `hickory-resolver` 的 HTTP CONNECT 代理、SNI 解析、连接池与 metrics 单列后续批次
- **FUSE overlayfs 接入 AI 命令执行层**：等 [src/command/worktree-fuse.rs](../../src/command/worktree-fuse.rs) 稳定后再接入 AI 写入隔离
- **WSL2 / Docker-in-Docker 的自适应弱隔离告警**：放到后续维护批次，靠阶段 1 的 `sandbox status` 做诊断入口
