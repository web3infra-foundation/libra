# Sandbox 安全隔离改进详细计划

## 所属计划

AI Agent 子系统专项计划，与 [code.md](code.md) 并列。两者的边界：

- [code.md](code.md)：负责 Phase 0–4 工作流、Snapshot/Event/Projection 对象模型、provider bootstrap。
- 本计划：负责 `libra code` 在宿主机上执行 Shell / ApplyPatch / Write 等 mutating tool 时的**操作系统级隔离层**，保障 provider / AI agent 失控时的爆炸半径。

## Context

AI Agent 在本地执行命令是 `libra code` 的核心能力，但也是攻击面最集中的入口：提示词注入、恶意 MCP server、失控的 provider 调用都可能通过 Shell tool 直达宿主机。当前 Libra 已经具备多档 `SandboxPolicy`、Seatbelt 策略拼装、Linux 外部 helper、审批审计与危险命令解析等基础设施（见 [src/internal/ai/sandbox/](../../src/internal/ai/sandbox/)），但相较 Claude Code 官方公开的 Bubblewrap 方案仍有若干关键缺口，包括：**Linux 沙箱依赖的外部二进制缺失时静默降级**、**Seatbelt 允许 `file-read*` 导致敏感路径默认可读**、**无 `--new-session` 防护 TIOCSTI 终端注入**、**无每命令 0o700 tmp 清理**、**无危险挂载拒绝清单**、**无 `libra sandbox status` 自检**。

本计划对齐 Claude Code 官方沙箱文档（`code.claude.com/docs/en/sandboxing`）与 Bubblewrap 工程实践，目标是把 Libra 在 AI Agent 失控场景下的实际爆炸半径降到与 Claude Code 相当的水平，并保证改动与 [code.md](code.md) 的 Runtime 正式写入层兼容。**网络服务访问采取默认拒绝（default deny）策略**：沙箱内除 loopback 外的一切出站连接默认被 OS 层阻断，只能通过显式白名单放行。

## 已完成前置条件与当前代码状态

### 已确认落地的基线

**策略层** [src/internal/ai/sandbox/policy.rs](../../src/internal/ai/sandbox/policy.rs)
- `SandboxPolicy` 四档：`ReadOnly` / `WorkspaceWrite` / `ExternalSandbox` / `DangerFullAccess`
- `WritableRoot` 写入根 + 保护子路径（`.git` / `.libra` / `.codex` / `.agents`）
- 路径通过 `canonicalize` 规范化（policy.rs:164-170）
- `/tmp` 与 `TMPDIR` 由策略显式纳入写入根

**运行时层** [src/internal/ai/sandbox/runtime.rs](../../src/internal/ai/sandbox/runtime.rs)
- macOS：`sandbox-exec` + 动态 `.sbpl` 模板（runtime.rs:282-313），`seatbelt_base_policy.sbpl` / `seatbelt_network_policy.sbpl` 已嵌入
- Linux：调用外部 `libra-linux-sandbox` 可执行文件，支持 seccomp 或 bwrap 两种模式，经 `LIBRA_LINUX_SANDBOX_EXE` 与 `LIBRA_USE_LINUX_SANDBOX_BWRAP` 控制
- Windows：`SandboxTransformError::WindowsSandboxNotImplemented`，与 Claude Code 当前状态对齐
- 网络控制：沙箱策略联动 `LIBRA_SANDBOX_NETWORK_DISABLED` 环境变量和 Seatbelt 网络策略

**审批与命令安全** [src/internal/ai/sandbox/mod.rs](../../src/internal/ai/sandbox/mod.rs) + [src/internal/ai/sandbox/command_safety.rs](../../src/internal/ai/sandbox/command_safety.rs)
- `AskForApproval` 四档（Never / OnFailure / OnRequest / UnlessTrusted）
- 会话级审批缓存 `ApprovalStore`
- tree-sitter bash 解析 + 安全命令白/黑名单
- 沙箱拒绝关键词触发升级重试提示（mod.rs:178-186）
- 默认 10 秒超时、100 KiB 输出上限（mod.rs:175、[src/internal/ai/tools/handlers/shell.rs:35](../../src/internal/ai/tools/handlers/shell.rs:35)）

**Worktree FUSE overlay** [src/command/worktree-fuse.rs](../../src/command/worktree-fuse.rs)
- 基于 `libfuse_fs::overlayfs` 的 COW 隔离，与 AI 沙箱解耦，当前仅服务 `git worktree --fuse`，尚未接入 AI 命令执行

### 基于当前代码的 Review 结论

- Linux 外部 helper 缺失时走 `tracing::warn!` 后"裸跑"（runtime.rs:224-228），这是**静默安全降级**，用户无感知。
- Seatbelt 策略对读操作使用 `(allow file-read*)` 全盘放行（runtime.rs:293），`~/.ssh` / `~/.aws` / `~/.netrc` / 浏览器 cookie / 各类 token 默认可被 agent 读取并外发。
- `create_seatbelt_command_args` 与外部 Linux helper 都没有对沙箱进程做 `setsid` / `--new-session`，TIOCSTI 终端注入路径未封堵。
- `CommandSpec::env` 目前由调用方传入，未专门为沙箱进程准备 0o700 私有 tmp，`$TMPDIR` 直接复用宿主。
- `WorkspaceWrite::writable_roots` 装载不做危险路径校验，用户若为兼容 Docker 工具链写入 `/var/run/docker.sock`，沙箱一挂即可逃逸。
- 缺少 `libra sandbox status` 入口，用户无法确认当前 `SandboxType` 的实际生效状态。

## 目标与非目标

**本轮目标（P0 / P1）：**

- **P0** 堵住 Linux 静默降级：引入 `SandboxEnforcement`，`Required` 下不得无沙箱执行
- **P0** 补齐终端注入防御（macOS Seatbelt + Linux bwrap 的 `--new-session` / `setsid`）
- **P0** 内建 Bubblewrap 直调，摆脱对外部 `libra-linux-sandbox` 的强依赖
- **P0** **网络服务默认拒绝**：沙箱内出站网络默认在 OS 层阻断（macOS Seatbelt 不注入网络策略 / Linux bwrap `--unshare-net`），仅放行 loopback；白名单只能通过显式配置放行
- **P1** 收紧 Seatbelt 读权限，对默认敏感路径（`~/.ssh`、`~/.aws`、`~/.gnupg`、`~/.netrc`、`~/.config/gcloud`、`~/.kube` 等）默认拒读
- **P1** 每命令 0o700 专属 tmp，退出即清
- **P1** 危险挂载拒绝清单（`/var/run/docker.sock` / `containerd.sock` / `/proc` / `/sys` / `/dev`）
- **P1** 新增 `libra sandbox status` 子命令，输出当前生效的隔离模式与降级告警

**后续维护目标：**

- WSL2 二次嵌套 / Docker-in-Docker 环境的自适应弱隔离告警
- 与 [code.md](code.md) Runtime `write_run` / `write_tool_invocation` 的联动审计（沙箱拒绝事件 → `Evidence[E]`）

**本批非目标：**

- **不实现 Windows 沙箱**，保持与 Claude Code 当前状态一致
- **不实现完整的域名/SNI 过滤代理守护进程**：本轮落地数据结构 + OS 级默认拒绝 + 白名单配置解析 + stub 代理入口；基于 `hickory-dns` + TLS SNI 过滤的完整代理实现单列后续批次
- **不把 FUSE overlayfs 接入 AI 命令执行层**（价值高但侵入大，列为后续独立批次）
- **不重构 `libra-linux-sandbox` helper**（保留其作为兼容回退路径）

## 差距分析

| # | Claude Code 做到 | Libra 现状 | 严重度 | 落地阶段 |
|---|---|---|---|---|
| G1 | Linux 沙箱开箱即用（bwrap 直调） | 依赖外部 `libra-linux-sandbox` 二进制，未配置时 warn + 裸跑 | ★★★ | 阶段 1 + 阶段 2 |
| G2 | `--new-session` 阻断 TIOCSTI | 未设置 `setsid` / `--new-session` | ★★★ | 阶段 3 |
| G3 | tmpfs 空白根 + `--ro-bind` 精选注入 | Seatbelt `(allow file-read*)` 全盘读 | ★★ | 阶段 4 |
| G4 | 默认拒绝 + 域名白名单的网络策略 | 仅 `network_access: bool`；enforcement 依赖环境变量 + 部分 Seatbelt 策略，未在 Linux 默认 `--unshare-net` | ★★★ | 阶段 7 本轮 OS 层 default deny + 白名单配置；域名过滤代理单列 |
| G5 | 每命令 0o700 tmp + `cleanupAfterCommand()` | 无专属 tmp | ★★ | 阶段 5 |
| G6 | 内置 Seccomp 过滤器 | 依赖外部 helper | ★★ | 阶段 2（随 bwrap 直调） |
| G7 | 明确警示 Docker socket 挂入 = 逃逸 | `writable_roots` 不校验 | ★★ | 阶段 6 |
| G8 | `/sandbox` 自检状态 | 无 | ★ | 阶段 1（随 `sandbox status` 子命令） |
| G9 | 嵌套容器 / WSL 的自适应降级告警 | 无 | ★ | 后续维护 |
| G10 | Windows 规划中 | 同样未实现 | — | 本轮不处理 |

## 改进阶段

### 阶段 1：堵住 Linux 静默降级 + 新增 `sandbox status` 子命令（P0）

**目标**：Linux 上"以为有沙箱、实际裸跑"的情况必须被消除；用户能在终端自查当前隔离模式。

1. **引入 `SandboxEnforcement` 枚举**（`policy.rs`）
   - 新增 `enforcement: Required | PreferStrict | BestEffort`，默认 `PreferStrict`
   - `Required` 语义：若 `SandboxManager::select_initial` 返回 `SandboxType::None` 则视为失败
   - 与现有 `SandboxPermissions::RequireEscalated` 解耦：后者表达"这次调用合法地需要无沙箱"，前者表达"系统配置强制要求沙箱生效"
2. **修改降级路径**（`runtime.rs:224-228`）
   - 当前 `linux_sandbox_exe` 缺失走 `tracing::warn!` 静默绕过 → 改为根据 `enforcement` 决策：
     - `Required` → `SandboxTransformError::EnforcementFailed { reason }` 返回给调用方
     - `PreferStrict` → 返回结构化警告，审批层弹用户确认（复用 `ExecApprovalRequest` 通道）
     - `BestEffort` → 保留现状
3. **新增 `libra sandbox status` 子命令**
   - 新增 `src/command/sandbox.rs`，挂到 [src/cli.rs](../../src/cli.rs) 顶层；遵循第一/二批已确立的 run/render 拆分模式（参考 [src/command/worktree.rs](../../src/command/worktree.rs)）
   - JSON / machine / human 三种输出：当前平台、实际生效的 `SandboxType`、`SandboxEnforcement`、writable roots、network access、helper 路径是否存在、Seatbelt / bwrap 探测结果
   - 显式 `StableErrorCode` 与 README 全局层面 A/B 项保持一致（退出码模型、`--help EXAMPLES`）

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

### 阶段 3：终端注入防御（P0）

**目标**：即使 AI 产出包含 TIOCSTI ioctl 的恶意命令，也无法把指令注入到宿主 TTY。

1. **macOS**（`create_seatbelt_command_args`）
   - 在 `seatbelt_base_policy.sbpl` 追加 `(deny iokit-open (iokit-user-client-class "IOTTYClient"))` 类规则
   - 同时给 sandbox 子进程加 `setsid`（通过 `Command::pre_exec` 在 Unix 下设置 `setsid()`）
2. **Linux**（`create_bwrap_command_args` / 外部 helper）
   - 内建 bwrap 参数追加 `--new-session`
   - 外部 helper 的 CLI 契约里确认同名参数一致
3. **单元测试**
   - 在 Linux 沙箱内 `ps -o pid,sid,pgid,tty` 应显示 `tty=?`，与宿主 TTY 解绑

### 阶段 4：Seatbelt 读权限收紧（P1）

**目标**：AI 默认无法读敏感路径，即使被提示词注入或模型越权。

1. **引入 `sensitive_read_paths()`**（`policy.rs`）
   - 默认清单：`~/.ssh`、`~/.aws`、`~/.gnupg`、`~/.netrc`、`~/.config/gcloud`、`~/.kube`、`~/.config/libra/vault`、`/etc/shadow`
   - 支持用户在 `.libra/sandbox.toml` 的 `deny_read` 字段追加自定义路径（与 Claude Code `denyRead` 语义对齐）
2. **macOS Seatbelt 策略**
   - `create_seatbelt_command_args` 在 `file_read_policy` 之后追加 `(deny file-read* (subpath "..."))`，对每个敏感路径做参数化拒绝
   - 保持 `(allow file-read*)` 的全局放行基线（避免 agent 无法读项目文件和依赖），依赖 deny 规则覆盖敏感子树
3. **Linux bwrap**
   - 对敏感路径执行 `--tmpfs <sensitive_path>`（在沙箱内遮蔽为空目录）
   - 对 `~/` 本身不做 `--tmpfs`，只遮蔽子树，保证 shell 能正常启动
4. **配置加载**
   - `.libra/sandbox.toml` 解析入口放在 [src/internal/ai/sandbox/mod.rs](../../src/internal/ai/sandbox/mod.rs)，沿用第一批已交付的 vault / config 基础设施（详见 [config.md](config.md) 的 `config_kv` / `resolve_env()`）

### 阶段 5：每命令 0o700 tmp + 退出清理（P1）

**目标**：命令间不留 token / cookie / 缓存残留，不同 AI 调用之间互相不可见。

1. **`run_command_spec` 前置**（`mod.rs`）
   - 调用方进入前：`tokio::fs::create_dir`（mode=0o700）生成 `<SystemTmp>/libra-sandbox-<uuid>/`
   - 注入到 `CommandSpec::env` 的 `TMPDIR` / `TEMP` / `TMP`
2. **命令退出后异步清理**
   - 在 `run_command_spec` 的 Drop / finally 路径 `tokio::fs::remove_dir_all`
   - 清理失败不阻塞主流程，走 `tracing::warn!` 并记录 `ToolInvocation[E]` 元数据（详见 [code.md](code.md) 的 Runtime 正式写入层）
3. **cleanupAfterCommand 对齐**
   - macOS 下若出现 Seatbelt "ghost dotfiles"（允许写 + 实际被 deny → 0 字节占位），也在清理阶段统一擦除

### 阶段 6：危险挂载拒绝（P1）

**目标**：用户/配置错误不能让沙箱"自己打开后门"。

1. **`WorkspaceWrite::writable_roots` 装载校验**（`policy.rs`）
   - 拒绝清单：`/var/run/docker.sock`、`/run/docker.sock`、`/run/containerd/containerd.sock`、`/proc`、`/sys`、`/dev`、`/var/run/libvirt/*`、`/` 本身
   - 支持 glob 匹配（例如 `**/docker.sock`）
2. **错误信息**
   - 遵循 CLAUDE.md "用户友好错误信息" 约束：指出是哪条 writable_root 被拒、原因（可能导致容器逃逸）、建议（改为挂载到非特权代理路径或关闭该工具链集成）
   - 映射到显式 `StableErrorCode`

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
   - 网络拒绝事件（连接被 OS 或代理阻断）写入 `ToolInvocation[E]` + `Evidence[E]`（详见 [code.md](code.md) Runtime 正式写入层）
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

## 与 code.md 的交接契约

- **Runtime 正式写入**：沙箱拒绝事件、enforcement 失败、危险挂载拒绝必须落到 [code.md](code.md) Runtime 的 `ToolInvocation[E]` + `Evidence[E]`，不能藏在 tracing 日志里
- **Phase 边界**：沙箱策略的选择与降级发生在 Phase 2 `CodexTaskExecutor` / `CompletionTaskExecutor` 进入 Runtime 写入之前；Phase 0 / 1 的 readonly tools 默认走 `SandboxPolicy::ReadOnly` + `SandboxEnforcement::Required`
- **Projection**：`libra sandbox status` 输出可作为 Projection 读取消费者（UI / MCP / diagnostics），但不成为真相源

## 验证方式

### 单元测试（新增 / 扩展）

1. [src/internal/ai/sandbox/policy.rs](../../src/internal/ai/sandbox/policy.rs)
   - `SandboxEnforcement` 的 serde round-trip
   - `sensitive_read_paths()` 在不同 `$HOME` 下的展开结果
   - `writable_roots` 含 `/var/run/docker.sock` 时的拒绝路径
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
3. **危险挂载拒绝**：`writable_roots` 含 `/var/run/docker.sock` → `SandboxPolicy` 装载失败，错误消息包含用户可读提示
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
