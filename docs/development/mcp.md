# `libra mcp` 独立命令拆分计划

> Status: draft
> Last updated: 2026-06-04
> Scope: 从 `libra code --stdio` / `--mcp-stdio` 中拆出 MCP protocol/tools/resources，形成独立 `libra mcp --stdio` 命令。Agent 外部调度仍走 WebSocket/Web API；MCP 不作为 Agent turn 控制面。

## Decision

`libra mcp` 是独立的 MCP protocol 命令族，不挂在 `libra code` 下。

目标是把 MCP client 集成面和 Code UI / Agent control 面拆开：

- `libra code`：Web Code UI + AgentRuntime，负责 message submit、interaction respond、cancel、observe、snapshot、controller lease。
- `libra mcp --stdio`：MCP tools/resources/protocol transport，服务 Claude Desktop 等 MCP client。
- `libra code-control`：TUI automation shim 的遗留命令；Web-only 后按 agent 迁移计划删除或改为纯 Web API shim。

## Final CLI Contract

最终保留：

```bash
libra mcp --stdio
libra mcp --stdio --cwd <path>
libra mcp --stdio --repo <path>
```

最终不再支持：

```bash
libra code --stdio
libra code --mcp-stdio
```

`libra code --stdio` / `--mcp-stdio` 必须 fail fast，并提示迁移到 `libra mcp --stdio`。该提示不得暗示 MCP 可以 submit/respond/cancel Agent turn。

## Boundaries

- `libra mcp --stdio` 不接受 Agent turn submit/respond/cancel/observe 请求。
- MCP resources/tools 可以继续暴露只读或受权限控制的 VCS / diagnostics / context 能力，但 mutating tools 必须经过既有 `McpAuthorizer`、tool policy、redaction 和 audit。
- 若未来支持 MCP notification/source，它只能作为 bounded event source 进入 runtime queue，且默认不注入 chat；外部主动调度 Agent 仍走 WebSocket/Web API。
- MCP stdio 独占 stdin/stdout；不得输出 warning、banner 或非 JSON-RPC 文本污染协议。
- `code-control --stdio` 不是 MCP server，不能复用为 `libra mcp` 实现。

## Implementation Plan

1. 新增 CLI 子命令：在 `src/cli.rs` 注册 `mcp`，在 `src/command/mcp.rs` 或等价模块定义 `McpArgs`。
2. 抽出 stdio runner：从 `src/command/code.rs::execute_stdio(args: &CodeArgs)` 提取协议无关 helper，例如 `run_mcp_stdio(working_dir: &Path) -> CliResult<()>`。
3. 工作目录解析：`McpArgs` 支持 `--cwd` / `--repo` 或等价路径解析，默认使用当前目录。
4. 旧入口迁移：`libra code --stdio` / `--mcp-stdio` 改为 usage error，错误文案指向 `libra mcp --stdio`。
5. 兼容矩阵：新增 `COMPATIBILITY.md` 条目，保证 `cargo test --test compat_matrix_alignment` 不因新命令漂移。
6. 命令文档：新增 `docs/commands/mcp.md`，并更新 `docs/commands/code.md` / `docs/commands/code-control.md` 中的 stdio 边界说明。
7. 测试索引：若新增 integration target，更新 `tests/INDEX.md`；若沿用既有 MCP e2e target，更新测试说明即可。

## Verification

- `libra mcp --help` 显示 stdio 用法。
- MCP stdio integration test 覆盖新命令入口。
- `libra code --stdio` 非零退出，并包含 `libra mcp --stdio` 迁移提示。
- 新 MCP stdio test 断言 Agent turn 控制面不从 MCP stdio 暴露。
- `cargo test --test compat_matrix_alignment` 通过。
- `rg -n "libra code --stdio|--mcp-stdio|MCP/stdio mode" docs/commands README.md tests` 只剩迁移说明或无结果。

## Relationship To Agent Plan

[`docs/development/code-agent-runtime.md`](code-agent-runtime.md) 只负责 Web-only AgentRuntime / ControlAdapter 收敛：TUI 不再作为生产操作入口，MCP 不作为 Agent 调度面。`libra mcp` 的 CLI grammar、stdio transport、compat docs 和 MCP e2e 验收全部由本文跟踪。
