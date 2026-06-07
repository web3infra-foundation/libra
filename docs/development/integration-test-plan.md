# Libra 版本管理集成测试计划

> 目标：把集成测试计划收敛到“编译后的 `libra` 命令在真实临时仓库中执行版本管理功能”的黑盒测试。测试对象是 `target/debug/libra` 或 release 构建产物，而不是 Rust 单元测试、Cargo `--test` 目标或直接调用内部模块。
> 原则：默认 Wave 0/1/2 只列当前仓库真实可执行、可在本机确定性复现的 CLI 功能场景；需要真实远端互操作时，使用独立的 Wave 3 GitHub live 场景。GitHub 仓库创建、查询和清理统一通过 `gh` 命令完成。交互界面、agent runtime、provider、publish 和真实云服务不属于本计划。

---

## 0. TL;DR

**完整性结论**：本计划作为“人工黑盒执行规范”和“runner 的需求说明”已可落地（覆盖矩阵、隔离安全模型、37 个具体场景 + 参数表 + 断言强化标准 + 输出契约 + PR/Review 协议 + 维护规则均已定义）。R0-R5 切片已全部落地：`tools/integration-runner/` 独立 crate、`list`、`check-plan`（yaml+MD+矩阵+registry 三方一致 + 收敛短形式 gate）、Wave 0 preflight、隔离 env_clear + SAFE_PATH + gitfix + gh 探针、37/37 场景（含 Wave 3 live 的 `run-live` + GhRepoCleanupGuard + delete_repo scope 预检 + 脱敏自检）的 typed Rust 执行 + 报告产出；`run --waves 0,1,2` / `run-live --only live.*` 均可执行并满足 §5 契约。check-plan 是稳定的一致性门，`run --waves 0,1,2` 是默认黑盒执行门（两者均在 CI 的 compat-offline-core 显式调用）。

场景登记使用 `docs/development/integration-scenarios.yaml`（元数据）+ `docs/development/integration-scenarios/<id>.md`（按场景拆分的可执行步骤与断言）+ 本文件（计划总则与 §2.3 矩阵）。`check-plan` 校验 yaml ↔ 拆分 MD ↔ runner registry 一致。

本次核查修正了 Agent 落地风险：每个 yaml 场景都必须在 MD 中有匹配的 `### <id>` 标题和 `SCENARIO=<id>` 代码块；config 子场景不得复用前一个场景的 `$RUN_DIR/config-repo`；`config --import` 的正向路径只放在显式 `requires_git` 的 `cli.config-import-path-edit` 场景中。

Agent 仍**不得**把 Wave 1/2 全矩阵一次性作为首个实现任务；必须按 §3.3.3 的 R0-R5 切片分批交付，每批可独立验证。

**推荐落地方式**：R0-R2 切片（骨架 + check-plan + 隔离运行时 + 初始 smoke）已实现；当前通过 R3+ 垂直迁移已覆盖 37/37 场景。Agent 必须按 §3.3.3 切片分批交付，每批独立可 `cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --only <ids>` + `check-plan` 全绿，产出 §5 报告，并保持黑盒边界（只执行编译后的 `libra`）。

**默认阻断门**：Wave 0 编译产物可用 + Wave 1 CLI 核心版本管理场景全绿 + Wave 2 CLI 兼容/存储场景全绿。

**GitHub 真实远端门**：当改动触达 `clone`、`fetch`、`pull`、`push`、`remote`、`ls-remote` 或协议层真实远端语义时，额外执行 Wave 3。Wave 3 必须用 `gh` 创建临时 GitHub 仓库，并用 `gh` 查询和删除该仓库；`libra` 只作为被测 VCS 命令访问该远端。

**测试引用规范**：

- 场景级：`cli.config-basic-kv`、`cli.config-git-compat-mode`、`cli.init-basic`、`cli.init-branch-and-format-options`、`cli.commit-status-log`。
- GitHub live 场景级：`live.github-create-push-clone-fetch`。
- 命令级：引用完整 `libra <subcommand>` 调用、退出码、关键 stdout/stderr 断言和执行目录。
- 不用 Cargo 测试目标名作为本计划的唯一引用；本计划关心用户可执行的 `libra` 行为。

**runner 落地原则**：正式 runner / plan consistency check 用独立 Rust 工具实现（当前为 `tools/integration-runner/`），显式 `cargo run --manifest-path ...` 调用；不得注册到根 `Cargo.toml [[test]]`，不得混入当前 `tests/` 集成测试体系。场景注册以 `docs/development/integration-scenarios.yaml`（结构化清单）为主要事实来源，MD 仅作为人类可读的可执行文档与示例；当前 `check-plan` 已验证 yaml 清单、MD 章节/`SCENARIO=`、§2.3 矩阵引用、Wave 3 gh 规则和已实现子集，断言强化模式的深度校验仍随 R3+ 逐步补齐。

**常用命令**：

```bash
# Wave 0：构建 libra 命令并确认可执行
LIBRA_SKIP_WEB_BUILD=1 cargo build --bin libra
./target/debug/libra --version
./target/debug/libra --help

# Wave 1 / Wave 2：在隔离 RUN_ROOT 中用编译产物执行 CLI 场景
BINARY="$(pwd)/target/debug/libra"
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
RUN_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/libra-integ-$RUN_ID.XXXXXX")"
mkdir -p "$RUN_ROOT"/{home,xdg-config,xdg-cache,repos,fixtures,logs,artifacts,tmp}
RUN_DIR="$RUN_ROOT/repos/cli.config-basic-kv"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    HOME="$RUN_ROOT/home" \
    USERPROFILE="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LIBRA_TEST=1 \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init config-repo
cd config-repo
libra config set user.name "Libra Config Test"
libra config get user.name

# Wave 3：用 gh 创建临时 GitHub 仓库，再用 libra 测真实远端
gh auth status --active --hostname github.com
OWNER="$(gh api user --jq '.login')"
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
REPO="$OWNER/libra-integ-$RUN_ID"
gh repo create "$REPO" --private --disable-issues --disable-wiki \
  --description "Temporary Libra integration test $RUN_ID"
gh repo view "$REPO" --json nameWithOwner,sshUrl,url
```

**最常踩的坑**：

1. 本计划不以 `cargo test --test <target>` 作为集成测试方案；Cargo 测试只能作为开发期辅助，不是本计划的默认门。
2. 每个场景必须先构建并固定同一个 `libra` 二进制路径，再在隔离临时目录中执行功能命令。
3. 场景断言至少包含退出码、关键输出、工作区文件变化和 `.libra/libra.db`/对象存储的可观察结果。
4. 默认 Wave 0/1/2 不要求任何密钥、外部账号、真实公网服务或真实云资源。
5. Wave 3 是显式的 GitHub live 场景：必须先确认 `gh auth status` 通过，必须创建临时私有仓库，必须在结束时用 `gh repo delete <owner/repo> --yes` 清理；不得使用 GitHub 网页 UI、手写 REST 脚本或在日志中输出 token。

**人工执行可行性指引**（runner 已落地；用于快速复现或调试单场景）：
- **最小冒烟子集**（人工快速验证，~15-20 分钟）：Wave 0 全 + `cli.init-basic`、`cli.config-basic-kv`、`cli.commit-status-log`、`cli.branch-switch-checkout`、`cli.cross-cutting-flags`、`cli.object-readback`、`cli.stash-bisect-worktree`、`cli.clone-fetch-pull-local`（本地 remote）。
- **完整矩阵**：仅在 PR 改动触达对应命令组、或准备提交前由 runner / 长时间手工执行。
- 所有执行必须严格使用 §3.3.1 的 `libra()`（含 `TMPDIR` + 智能 `SAFE_PATH`）；第 4 章内联 wrapper 若与 §3.3.1 不一致，以 §3.3.1 为准并必须先修正文档/runner。

**Agent 生成计划时的硬边界**：
- R0-R5 已实现：`tools/integration-runner/`、`check-plan`、`list`、`run --only`、Wave 0 preflight、Wave 1/2 默认门和 Wave 3 live 场景均已落地；后续只能按新增/修改场景的垂直切片推进，不得用大包改动绕过 `check-plan`。
- 必须把未完成的 `BASELINE_GAP-INTEG-*` 当作 backlog，而不是已完成能力；当前 `check-plan` 可用于 yaml+MD+矩阵+runner registry + assertion-category gate 的一致性结果，`run --waves 0,1,2` 是默认黑盒执行门。
- Agent 产出的 issue/任务必须以“可单独运行并验证的垂直切片”（R0/R1/R2...）为单位，不能按模块名大包拆分。新增场景必须包含 yaml 登记 + MD 章节。
- Scope 里必须显式包含 `integration-scenarios.yaml` 当场景元数据或列表变化时。

---

## 1. 现状基线

| 资产 | 现状 | 证据 |
|---|---|---|
| `libra` CLI 入口 | 已存在 | `src/main.rs` -> `src/cli.rs` -> `src/command/*::execute_safe` |
| 构建产物 | 已存在 | `cargo build --bin libra` 生成 `target/debug/libra` |
| 版本管理命令面 | 已存在 | `init`、`add`、`commit`、`status`、`log`、`branch`、`switch`、`checkout`、`worktree`、`stash`、`bisect`、`remote`、`fetch`、`push`、`pull`、`clone` 等 |
| 本地仓库状态 | 已存在 | `.libra/libra.db`、`.libra/objects`、工作区文件 |
| 文档一致性检查 | 已落地（去脚本化） | Code UI 路由 ↔ `docs/commands/code-control.md` 覆盖检查在 `tests/compat/matrix_alignment.rs::docs_consistency_covers_code_ui_router_matrix`；仓库无 `scripts/` 目录 |
| 兼容矩阵一致性检查 | 已落地（去脚本化） | `COMPATIBILITY.md` ↔ `src/cli.rs::Commands` 漂移检查在 `tests/compat/matrix_alignment.rs::compatibility_matrix_matches_cli_commands`；CI 以 `cargo test --test compat_matrix_alignment` 运行 |
| 集成计划自检工具 | R0-R5 + 37/37 场景已落地 | `docs/development/integration-scenarios.yaml` 是 runner / check-plan 的**唯一事实来源**（id、wave、gh_required、key_assertion_categories 等）；`tools/integration-runner` 提供 `check-plan`（结构一致性 + gh 规则 + 矩阵引用 + implemented 子集收敛 gate）和可执行 `run --only` / `run-live --only`，产出 §5 report。当前 37 个场景均有完整 Rust typed 实现 + 断言；compat_matrix_alignment 已去脚本化落地。 |
| GitHub CLI 操作面 | 外部前置条件 | Wave 3 使用 `gh auth status`、`gh repo create`、`gh repo view`、`gh api`、`gh repo delete` |
| 覆盖矩阵 + 安全清单 | 本次改进新增 | §2.3 命令覆盖矩阵 + §3.6 安全自检清单（重点解决覆盖完整性与测试环境安全问题） |

---

## 2. 本计划范围

### 2.1 纳入范围

1. 编译 `libra` 二进制后，通过 `libra <cmd>` 在临时仓库里执行的版本管理功能测试。
2. Git 兼容命令和 Libra 差异语义：stash、bisect、worktree、checkout alias、branch-name 处理等。
3. 引用、HEAD、分支、工作区、worktree、对象存储、schema migration 和本地协议/client 的用户可观察行为。
4. CLI 帮助、错误输出、JSON 输出、退出码和副作用的黑盒断言。
5. 文档、兼容矩阵和 CLI 场景清单与真实命令面的同步检查。
6. 真实 GitHub 远端互操作 smoke：仅限 Wave 3，通过 `gh` 创建临时 GitHub 仓库，并用 `libra` 对该远端执行 push/fetch/pull/clone/readback。

### 2.2 不纳入范围

1. Rust 单元测试、模块级测试、直接调用内部 API 的测试方案。
2. 以 Cargo `--test` 目标作为默认集成门的方案。
3. 交互式界面、终端界面、浏览器控制面、UI harness 和性能 soak。
4. agent runtime、provider、MCP、Codex、goal、usage、sub-agent、AI schema/runtime 派生记录。
5. 除 Wave 3 GitHub live 场景外的真实公网网络、publish、真实云存储、Cloudflare D1/R2、真实发布部署。
6. 多机调度、SSH 节点编排、成本预算、密钥分发、`.env.test` 装载。
7. 虚构的场景 YAML runner、自定义测试 ID 体系或尚未落地的 orchestration 脚本。

### 2.3 版本管理命令黑盒覆盖矩阵（Command Coverage Matrix）

本节提供用户可执行 `libra <cmd>` 面的覆盖现状，与 [`COMPATIBILITY.md`](../COMPATIBILITY.md) 及 `src/cli.rs` 保持同步。**Cargo 兼容性守卫**（`compat_*_guard`）负责 help/EXAMPLES/unwrap 审计；本计划关注可执行行为黑盒。

| 命令组 | 代表命令 | 兼容层级 | 本计划覆盖状态 | 主要场景 ID | 备注 |
|--------|----------|----------|----------------|-------------|------|
| Setup | init, clone, config | supported/partial | 优秀（参数矩阵全） | cli.init-*, cli.config-*, cli.clone-fetch-pull-local | clone 仅本地+Wave3 |
| Working Tree | status, add, rm, mv, restore, clean, stash, lfs, worktree | supported/partial/int-diff | 优秀（本地确定性命令全） | cli.commit-status-log, cli.restore-reset-diff, cli.stash-bisect-worktree, cli.clean-rm-mv-lfs-basic | `cli.stash-bisect-worktree` 覆盖 `stash push -u` / `-a` / `--all` / `--keep-index`；LFS 远端 lock API 不进默认 Wave |
| History | log, shortlog, show, show-ref, ls-remote, diff, grep, blame, describe | supported | 优秀（inspection 全） | cli.commit-status-log, cli.object-readback, cli.grep-blame-describe-shortlog, cli.clone-fetch-pull-local | 真实远端 refs 见 Wave3 |
| Branching | commit, branch, switch, checkout, tag, merge, rebase, reset, cherry-pick, revert | supported/partial | 优秀（核心闭环全） | cli.branch-switch-checkout, cli.restore-reset-diff, cli.commit-status-log, cli.tag-basic, cli.merge-rebase-cherry-revert-smoke, cli.merge-conflict-continue, cli.rebase-conflict-continue | merge/rebase 冲突续跑成功路径已有独立场景 |
| Remote | remote, fetch, pull, push, open | supported/partial | 良好（本地 Git clone/fetch/pull + 本地 file remote push 拒绝 + GitHub live push 闭环 + `clone --depth` / `fetch --deepen` 本地 shallow 实现目标，open 无副作用 smoke） | cli.clone-*, cli.push-local-file-remote-rejected, cli.open-smoke, cli.fetch-depth-local, live.github-create-push-clone-fetch | `pull --rebase` 真分叉冲突路径仍属深水区；真实 push 语义只在 Wave3 |
| Maintenance | db, fsck, cat-file, hash-object, verify-pack, rev-parse, rev-list, symbolic-ref, reflog, bisect, index-pack | supported/partial/int-diff | 良好（index-pack 除外） | cli.schema-*, cli.object-readback, cli.sha256-object-readback, cli.verify-pack-smoke, cli.stash-bisect-worktree, cli.reflog-symbolic-ref | index-pack 为隐藏内部命令，仅在 verify-pack 场景的 fixture 生成中作为辅助命令使用；sha256 端到端读写已独立覆盖 |
| Cross-cutting | --json/--machine/--quiet/--color/--progress/--exit-code-on-warning | supported | 良好（独立场景集中断言全局 flag 语义；warning=9 仍按 gap 跟踪） | cli.cross-cutting-flags | 详见下方「跨命令标志」 |
| AI/Cloud | code*, automation, cloud, publish, agent*, hooks | intentionally-different | 显式排除（见 2.2） | — | hooks 为兼容隐藏命令，由专属测试覆盖 |

**剩余覆盖缺口（BASELINE_GAP-INTEG-005）**：本次计划已补齐 tag、merge/rebase/cherry-pick/revert、merge/rebase 冲突续跑成功路径、grep/blame/describe/shortlog、clean/rm/mv/lfs、本地 reflog/symbolic-ref、verify-pack 的独立场景 + 参数表；本轮改进又补齐 **本地 file remote push 拒绝（`cli.push-local-file-remote-rejected`，覆盖 normal / dry-run / force / atomic / tags / mirror 的 fail-closed 形态）**、**GitHub live push（`push --dry-run` / `push -u` / refspec / `--tags` / delete / `--force` / `--mirror`）**、**`fetch --all`**、**fetch shallow 本地闭环（`clone --depth` + `fetch --deepen`）**、**`pull --rebase`**、**sha256 端到端对象读写（`cli.sha256-object-readback`）** 与 **全局 flag 集中断言（`cli.cross-cutting-flags`）**。仍需后续细化（按风险排序）：`pull --rebase` 真分叉路径、LFS 远端 lock API、更多 pack corpus 的 `index-pack`/`verify-pack` 深度 fixture、以及 `open` 的 JSON 无副作用行为是否足够覆盖真实系统 open。`cli.fetch-depth-local` 已定义本地 Git fixture 的 `clone --depth` 与 `fetch --deepen` 目标断言；若当前实现返回 `LBR-REPO-002 object not found`，应作为 shallow 对象闭包缺陷处理。注意 `push` 当前已有 `--force`/`-f`、`--force-with-lease`、`--force-if-includes`、`--atomic`、`--porcelain`、`--thin`/`--no-thin`、`--tags`、`--mirror`；`fetch` 当前已有 `--all`/`--depth`/`--deepen`/`--prune`/`--tags` 等关键 flags，本矩阵只登记已存在的 flag，避免引用不存在的参数。新增命令到 `src/cli.rs` 时必须同步更新本矩阵并至少添加一个 `cli.<cmd>-smoke` 场景。

**跨命令标志（Cross-cutting）**：`--json`/`--machine`/`--quiet`/`--color`/`--progress`/`--exit-code-on-warning` 是全局 flag（定义在 `src/cli.rs` 的 `Cli` 根结构，对所有子命令生效）。本轮改进新增 `cli.cross-cutting-flags` 场景集中断言其语义（JSON envelope 形态、`--machine` 蕴含 ndjson+quiet+no-pager+color=never、`--quiet` 抑制 stdout、无 warning 时 `--exit-code-on-warning` 不改变退出码、`--color=never`/`NO_COLOR`），不再依赖各功能场景顺带覆盖。确定性 warning 触发源尚未固化，warning 时退出码 9 按 BASELINE_GAP-INTEG-009 跟踪，不能在默认 Wave 中硬断言。

**故意差异回归防护（Intentional Differences Regression Guards）**：COMPATIBILITY.md 明确标注的 `intentionally-different` 行为必须有**正向断言**防止悄悄对齐 Git：
- `worktree remove` 默认保留目录（不隐式数据丢失）——已在 `cli.stash-bisect-worktree` 断言 `test -d`。
- `push` 拒绝本地文件 remote（仅支持 `git@` / `https` 等网络 remote）——已由 `cli.push-local-file-remote-rejected` 显式断言 `LBR-CLI-003` / "local file repositories is not supported"。
- `symbolic-ref` 仅支持 HEAD（其他符号引用因 SQLite 存储被拒绝）。
- 这些必须出现在对应场景的负向步骤或专用小节中；新增故意差异时必须同步矩阵备注 + 断言。

**与 tests/INDEX.md 关系**：Cargo 集成测试（Wave 1/2）提供 L1 确定性保障；本计划的黑盒 CLI 场景是用户视角的补充门，必须与当前 `tests/` 体系分开维护。`tests/INDEX.md` 只索引 Cargo `--test` 目标，不是本计划的场景 registry；若它与实际 `tests/*.rs` 暂时存在漂移，也不应影响本计划 runner 的场景清单。集成计划一致性检查已落在独立 Rust runner/tool 的 `check-plan` 子命令中，CI 可显式运行该工具，但**不得注册到根 `Cargo.toml [[test]]`、不得进入 `cargo test --all` 默认测试集、不得写入 `tests/INDEX.md`**。

**断言强化标准（Assertion Strengthening Standard）**：为确保 Agent 可解析性、安全隔离验证和 Git 兼容性，所有场景最终应逐步纳入以下可执行断言模式（已在 `cli.commit-status-log`、`cli.cross-cutting-flags`、`cli.tag-basic` 等场景示范）：
- 成功路径：至少一个 `--json`（或 `--machine`）调用 + `python3 -c "import json; d=...; assert d['ok'] is True; assert 'data' in d"`（ndjson 场景用逐行解析）。
- 错误路径：负向命令必须非 0 退出，stderr 捕获验证包含 `LBR-` 稳定码或特定可操作错误文本（例如 "not a libra repository"、"no such"）。
- 状态一致性：关键 mutating 操作后执行 `libra fsck --connectivity-only`（0 退出）或 `libra --json show-ref --heads` 验证 refs 健康。
- 隔离验证：涉及 config/vault/global 的场景，操作后用隔离 `LIBRA_CONFIG_GLOBAL_DB` 或 HOME 执行 `libra config --global list` 验证无本场景残留（或显式检查文件不存在）。
- 故意差异：COMPATIBILITY.md 中的 intentionally-different 行为必须有正面 `test` / `grep` / JSON 断言（例如 worktree remove 后目录仍存在、push 拒绝本地文件 remote）。
- 冲突/历史场景：冲突标记（`<<<<<<<`）、`libra --json status` 中 `data.merge_state.conflicted_paths[]` 非空、--continue 后该字段缺失或为空必须可自动断言；不得引用 Git-only 顶层 `ls-files`。
- 所有断言必须在 `libra()` 包装下执行，且使用 `$RUN_ROOT` 下的文件/输出，避免依赖主机状态。

本标准随 runner 落地将逐步自动化。当前 PR 贡献新场景时至少应包含 JSON envelope + 1 个 LBR- 错误验证 + fsck。

**本轮系统化补充断言已直接更新以下场景**（示范 + 强化）：
- `cli.commit-status-log`（基础闭环 + JSON + fsck + LBR + 隔离）
- `cli.tag-basic`、`cli.branch-switch-checkout`、`cli.object-readback`
- `cli.merge-rebase-cherry-revert-smoke`、`cli.merge-conflict-continue`
- `cli.reflog-symbolic-ref`、`cli.clean-rm-mv-lfs-basic`
- `cli.cross-cutting-flags`（先前已强化错误 JSON）

其余场景（config/*、init/* 变体、push-local-file-remote-rejected、clone 系列、schema、verify-pack、sha256、open、Wave 3 live.github-* 等）请对照本标准逐一补充相同模式的断言。目标：每个场景的“断言”部分最终都包含可直接在 `libra()` 下执行的 python/shell 检查，而非仅描述性文字。

### 2.4 Scenario Registry（结构化清单）

**三文件模型**（yaml 元数据 + 按场景拆分的可执行文档 + 计划总则）：

- `docs/development/integration-scenarios.yaml`：**机器可读的事实来源**（registry）。包含全部 `cli.*` / `live.github-*` 的 id、wave、group、purpose、gh_required、requires_git、key_assertion_categories 等。runner 的 `list`、`check-plan`、`run --waves` 必须以本文件为准。
- `docs/development/integration-scenarios/<id>.md`：**按场景拆分的可执行文档**（每个 id 一份）。必须包含 `### `<id>`` 标题、`SCENARIO="<id>"` 代码块、步骤与断言；跨命令参数矩阵在 [`integration-scenarios/_parameter-tables.md`](integration-scenarios/_parameter-tables.md)。索引见 [`integration-scenarios/README.md`](integration-scenarios/README.md)。
- `docs/development/integration-test-plan.md`：**计划总则**（范围、§2.3 覆盖矩阵、§3 隔离与安全、§5 报告契约、PR 协议、BASELINE_GAP）。§4 仅保留 Wave 索引，不再内联 37 份场景正文。

**添加/修改场景时的契约**（必须同步更新，见 §0 的完整 "Agent 添加/迁移场景的落地 checklist"）：
1. **必须先**在 `integration-scenarios.yaml` 新增/修改条目（必须填写 wave、gh_required、requires_git、key_assertion_categories、doc_section=id）。
2. 新增或更新 `docs/development/integration-scenarios/<id>.md`（短形式步骤 + 覆盖 key_assertion_categories 的补充断言；禁止无收敛说明的长 `libra() {` 块）。
3. 若触达新命令或 compat 语义，同步更新 §2.3 覆盖矩阵 + `COMPATIBILITY.md` + `docs/commands/<cmd>.md` + 运行 `cargo test --test compat_matrix_alignment`。
4. **必须**在 `tools/integration-runner/src/registry.rs` 的 `scenario_registry()` 增加条目，并在 `tools/integration-runner/src/scenarios/<id>.rs` 实现 `scenario_*`（见 §3.3.3）。
5. 任何 PR 必须让 `check-plan` 全绿 + `run --only <id>`（或批）全绿，并按 §8.1 填写 Test Plan。

`check-plan` 职责：
- 加载 yaml；扫描 `integration-scenarios/*.md`（`cli.*` / `live.*`）确认标题与 `SCENARIO=` 与 yaml 一致。
- 交叉检查 §2.3 矩阵引用的场景 id；对比 `scenario_registry()` 已实现子集。
- 验证 gh_required 仅出现在 Wave 3；已实现场景的 MD 须为短形式收敛。
- **断言类别覆盖启发式**：对每个已实现场景，校验其 yaml 声明的 **source-verifiable** `key_assertion_categories`（`json_envelope` / `fsck` / `gitfix_isolation` / `negative_exit` / `lbr_error` / `conflict_markers` / `gh_lifecycle` / `cleanup_guard` / `file_exists`）在 `tools/integration-runner/src/scenarios/<id>.rs` 中留有可检出的断言信号（如 `assert_json_ok`/`--json`、`fsck`、`ctx.gitfix`、`, false)` 负向调用、`assert_lbr_or_text`、`<<<<<<<`、`ctx.gh`、`GhRepoCleanupGuard`、`ensure_file`/`.join(`）；缺失即 `check-plan` 失败。这保证「声明了某类别就必须真的断言它」，是命令变更时集成测试不被悄悄削弱的同步门。runner 隔离强制或语义类（`global_db_isolation` / `vault_isolation` / `no_secret_leak` / `intentional_difference`）为 **advisory**，由运行时隔离/脱敏保证，不从源码门控。`check-plan` 输出 `assertion_category_checks=<n>` 记录本轮校验的类别数。

---

## 3. 执行前准备（Wave 0）

### 3.1 必须通过

```bash
cargo --version
rustup show active-toolchain

cargo +nightly fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
LIBRA_SKIP_WEB_BUILD=1 cargo build --bin libra

./target/debug/libra --version
./target/debug/libra --help

# 外部工具前置：libra 在以下场景会 fork/exec 系统 git / ssh，
# 而 §3.3.1 规范模板把 PATH 收窄到 /usr/bin:/bin:/usr/sbin:/sbin。
# 若 git / ssh 不在该 PATH（如 macOS 仅装 Homebrew git、未装 Xcode CLT），
# `config import`、`init --from-git-repository`、Wave 3 SSH 远端会“假失败”。
# 见 §3.3.0「外部工具依赖」。
command -v git    # cli.config-import-path-edit / cli.init-from-git-repository 需要
ls -l /usr/bin/git 2>/dev/null || echo "WARN: git 不在收窄 PATH，需按 §3.3.0 追加其目录"
command -v ssh    # Wave 3 SSH 远端需要（/usr/bin/ssh）
command -v gh     # Wave 3 需要

# 兼容矩阵 / docs 一致性检查已去脚本化为自包含 Rust 测试（无 scripts/ 目录）
cargo test --test compat_matrix_alignment
```

通过标准：`target/debug/libra` 存在且可执行；`--version` 和 `--help` 退出码为 0；格式、lint 通过；**`git` 与 `ssh` 可在 §3.3.1 收窄后的 `PATH` 中解析到**（否则按 §3.3.0 追加其所在目录，或把依赖 git/ssh 的场景标记 skip 并记录原因，而不是当作 libra 行为失败）；`compat_matrix_alignment` 测试通过（兼容矩阵与 Code UI docs 一致性的去脚本化检查）；`integration-scenarios.yaml` 存在且包含所有当前 Wave 场景；`cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan` 通过 yaml + MD + 矩阵 + runner 已实现子集一致性检查。

### 3.2 CLI 场景执行约束

每个 Wave 1 / Wave 2 场景必须遵守：

1. 使用 Wave 0 编译出的同一个 `libra` 二进制，不允许在场景中隐式 `cargo run`。
2. **严格使用 §3.3.1 规范模板** 创建隔离 RUN_ROOT + 每条命令完整 env wrapper（这是测试环境安全的第一道防线）。
3. 使用 `$RUN_ROOT` 下的全新临时目录；不要复用开发者当前仓库作为被测仓库。
4. 显式记录 `pwd`、二进制路径、完整命令、退出码、stdout、stderr。
5. 如需提交，场景必须在临时仓库内配置本地 `user.name` 和 `user.email`，不得依赖用户全局配置。
6. 断言用户可观察结果：命令输出、文件内容、ref/branch 状态、工作区状态、对象可读性或 SQLite schema 状态。
7. 场景结束后可删除临时目录；失败时保留目录路径用于复现。
8. 新场景代码块必须同时更新 §2.3 覆盖矩阵。

### 3.3 环境隔离规范

每轮集成测试必须创建独立 run root，所有本地状态都写入该目录，禁止读写开发者当前仓库、真实 HOME、真实全局配置或真实 vault key 目录。

推荐 run root 布局：

```text
$RUN_ROOT/
  home/                 # 隔离 HOME；承载 ~/.libra、vault keys、global config
  xdg-config/           # 隔离 XDG_CONFIG_HOME
  xdg-cache/            # 隔离 XDG_CACHE_HOME
  repos/                # 被测 Libra 仓库，每个场景一个子目录
  fixtures/             # Git fixture、本地 remote fixture
  logs/                 # 每条命令 stdout/stderr/exit code/cwd/env 摘要
  artifacts/            # 失败时保留的关键文件摘要
```

最小初始化：

```bash
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
RUN_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/libra-integ-$RUN_ID.XXXXXX")"
BINARY="$(pwd)/target/debug/libra"
mkdir -p "$RUN_ROOT"/{home,xdg-config,xdg-cache,repos,fixtures,logs,artifacts,tmp}
```

后续场景片段中的 `RUN_DIR` 不是新的全局临时根，而是场景局部目录别名，必须落在 `$RUN_ROOT/repos/<scenario-id>/` 或 `$RUN_ROOT/fixtures/<scenario-id>/` 下。例如：

```bash
SCENARIO="cli.init-basic"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
```

每条 `libra` 命令必须通过统一隔离环境执行。**权威形式见 §3.3.1 的 `libra()` 包装函数（`env -i` + 白名单 + `TMPDIR` + git/ssh 感知 `SAFE_PATH`）**；下面的概念骨架仅示意要覆盖的变量集合，不可直接照抄——注意必须是 `env -i`（清空后白名单），而非在主机环境上叠加 `env HOME=...`（后者被 §3.6 明令禁止，会泄露 API key / SSH-GPG agent / pager 等）：

```bash
# 概念骨架（权威实现见 §3.3.1）：必须 env -i 清空，再注入白名单
env -i \
  PATH="$SAFE_PATH" \
  HOME="$RUN_ROOT/home" USERPROFILE="$RUN_ROOT/home" \
  XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
  TMPDIR="$RUN_ROOT/tmp" \
  LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
  LIBRA_TEST=1 LANG=C LC_ALL=C \
  "$BINARY" <args...>
```

### 3.3.0 环境隔离原理（Why `env -i`）

Libra 的 Cargo 集成测试基础设施使用 `Command::env_clear()` 清空子进程环境后，只白名单注入必要变量（见 `tests/command/mod.rs::base_libra_command()`）。本计划的 bash 模板必须用 `env -i` 达到同等效果。

仅写 `env HOME=... XDG_CONFIG_HOME=... "$BINARY" ...` 不够安全，因为它会保留主机环境里的未覆盖变量，例如 `GIT_AUTHOR_NAME`、`GIT_COMMITTER_EMAIL`、`SSH_AUTH_SOCK`、`RUST_LOG`、`LIBRA_SANDBOX_ENFORCEMENT`、provider API key、Cloudflare 凭据或自定义 pager。默认 Wave 0/1/2 必须是无密钥、无外部账号、无交互的确定性测试，因此只能传递白名单变量。

白名单变量及理由：

- `PATH`：约束命令查找路径，避免 `node_modules/.bin`、shell 插件或本机自定义工具影响测试。
- `HOME` / `USERPROFILE`：隔离全局 config、vault key、SSH/GPG 相关默认路径。
- `XDG_CONFIG_HOME` / `XDG_CACHE_HOME`：隔离 XDG 兼容状态。
- `LIBRA_CONFIG_GLOBAL_DB`：显式指定全局 config SQLite 路径。
- `LIBRA_TEST=1`：禁用 pager/交互路径，避免测试阻塞。已在 `src/utils/pager.rs`（`LIBRA_TEST_ENV`）与 `src/command/config.rs` 的非交互分支中实际读取，是经过验证的控制位，不是约定俗成。
- `LANG=C` / `LC_ALL=C`：固定 locale，便于 stdout/stderr 断言。
- `TMPDIR="$RUN_ROOT/tmp"`：把 libra 内部临时文件（fetch/ssh 写入的临时私钥、克隆 scratch、pack 中转目录等）收敛进隔离 run root。`env -i` 会清空主机 `TMPDIR`，若不显式注入，这些临时文件会落到全局默认临时目录（`/tmp`、macOS `/var/folders/...`），既破坏“单场景单目录”隔离，又可能在并发时跨场景串扰，且 ssh 临时私钥会写到 run root 之外。

**外部工具依赖（容易被收窄 PATH 静默破坏）**：libra 并非纯自包含二进制，部分版本管理路径会 fork/exec 系统工具：

- `git`：`config import` / `config --import`（`src/command/config.rs`）、`init --from-git-repository`（`src/internal/protocol/local_client.rs`）会调用系统 `git`。因此 `cli.config-import-path-edit`、`cli.init-from-git-repository` 这两个已覆盖场景在收窄 `PATH="/usr/bin:/bin:/usr/sbin:/sbin"` 下要求 `git` 能被解析到。CI/开发机通常有 `/usr/bin/git`（Xcode CLT 或发行版自带），但仅装 Homebrew git、未装 Xcode CLT 的 macOS 机器只有 `/opt/homebrew/bin/git`，会让这些 `libra` 命令“假失败”。
- `ssh`：Wave 3 SSH 远端经 `src/internal/protocol/ssh_client.rs`（`LIBRA_SSH_COMMAND`，默认 `ssh`）调用。
- `LIBRA_TEST` 与上述同属经验证的实际 env 读取点；本计划不引入未被代码读取的“安全约定”。

处理方式（按可控性排序）：(a) Wave 0 用 `command -v git` / `command -v ssh` 预检；(b) 若 git/ssh 在收窄 PATH 之外，在 §3.3.1 `libra()` 包装函数里把其真实目录追加到 `PATH`（例如 `PATH="/usr/bin:/bin:/usr/sbin:/sbin:$(dirname "$(command -v git)")"`），并在报告里记录这一偏离；(c) 实在无法满足时，把依赖 git/ssh 的场景标记为环境 skip，**不得**记成 libra 行为失败。Libra 的 Cargo 集成测试 `base_libra_command()` 用的是同一份收窄 PATH，正因为目标机器恰好有 `/usr/bin/git` 才长期未暴露此依赖——本计划把它显式化，避免换机器后误判。

### 3.3.1 全场景代码块必须遵守的规范模板（Canonical Safe Invocation）

**所有 Wave 1/2 场景的最小可复制模板**（后续所有 bash 代码块必须以此为基线；为兼顾安全与可读性，推荐在每个场景的开头声明 `libra()` 包装函数）：

```bash
# === 标准前置（每个 RUN_ROOT 只建一次）===
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
RUN_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/libra-integ-$RUN_ID.XXXXXX")"
mkdir -p "$RUN_ROOT"/{home,xdg-config,xdg-cache,repos,fixtures,logs,artifacts,tmp}
BINARY="$(pwd)/target/debug/libra"   # Wave 0 产物，禁止 cargo run

# === 解析 libra 内部 fork/exec 的系统工具目录（见 §3.3.0「外部工具依赖」）===
# 默认收窄 PATH；仅当 git/ssh 不在其中时，追加其真实目录（不引入开发者整段 PATH）。
SAFE_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
GIT_BIN="$(command -v git || true)"
case ":$SAFE_PATH:" in *":$(dirname "${GIT_BIN:-/usr/bin/git}"):"*) ;; *)
  [ -n "$GIT_BIN" ] && SAFE_PATH="$SAFE_PATH:$(dirname "$GIT_BIN")" ;; esac

# === 场景局部（每个 cli.* 场景独立子目录）===
SCENARIO="cli.example-unique-id"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"

# === 声明安全隔离包装函数（关键安全防线，取代冗长的 env 前缀）===
libra() {
  env -i \
    PATH="$SAFE_PATH" \
    HOME="$RUN_ROOT/home" \
    USERPROFILE="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LIBRA_TEST=1 \
    LANG=C \
    LC_ALL=C \
    "$BINARY" "$@"
}

# === 每次调用 libra 均通过包装函数（与真实 CLI 体验完全一致，且 100% 隔离安全）===
libra init my-repo
cd my-repo
libra config set user.name "Test User"
# ... 所有后续命令同上

# === 日志与清理 ===
# 每条命令 stdout/stderr 写入 $RUN_ROOT/logs/，失败保留整个 $RUN_ROOT
# 成功可 rm -rf "$RUN_ROOT"
```

**手动执行 prelude（每轮 RUN_ROOT 复制一次）** — 已与 §3.3.1 模板完全一致，后续 §4 场景不再重复粘贴长 wrapper：

```bash
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
RUN_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/libra-integ-$RUN_ID.XXXXXX")"
mkdir -p "$RUN_ROOT"/{home,xdg-config,xdg-cache,repos,fixtures,logs,artifacts,tmp}
BINARY="$(pwd)/target/debug/libra"

SAFE_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
for t in git ssh; do
  b="$(command -v $t || true)"
  case ":$SAFE_PATH:" in *":$(dirname "${b:-/usr/bin/$t}"):"*) ;; *)
    [ -n "$b" ] && SAFE_PATH="$SAFE_PATH:$(dirname "$b")" ;; esac
done

libra() {
  env -i PATH="$SAFE_PATH" HOME="$RUN_ROOT/home" USERPROFILE="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LIBRA_TEST=1 LANG=C LC_ALL=C "$BINARY" "$@"
}
gitfix() {
  env -i PATH="$SAFE_PATH" HOME="$RUN_ROOT/home" USERPROFILE="$RUN_ROOT/home" \
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null TMPDIR="$RUN_ROOT/tmp" \
    GIT_AUTHOR_NAME="Libra Fixture" GIT_AUTHOR_EMAIL="fixture@example.invalid" \
    GIT_COMMITTER_NAME="Libra Fixture" GIT_COMMITTER_EMAIL="fixture@example.invalid" \
    LANG=C LC_ALL=C git "$@"
}
```

> **收敛后的 §4 编写规范（进一步收敛，2026-06 更新）**：为消除几十处重复 wrapper 造成的漂移风险和维护负担，§4 场景现在**只保留 SCENARIO / RUN_DIR / 特有命令 + 断言**。公共的 RUN_ROOT 创建、SAFE_PATH 解析（含 git/ssh）、`libra()` 和 `gitfix()` 定义全部收敛到本节 §3.3.1 的**单一规范模板**（Canonical Safe Invocation）。
当前所有 Rust registry 中实现的场景（~36 个，live.github 除外） 的 MD 章节均已收敛为短形式；check-plan 中的 convergence gate 会在新增 Rust 实现时强制 MD 也使用短形式（否则失败）。
>
> **人工执行时**：在每个 RUN_ROOT 开头复制一次 §3.3.1 的“标准前置 + SAFE_PATH + libra() + gitfix()”块（或直接复制下面“手动执行 prelude（每轮一次）”）。之后每个场景只需写本场景的 SCENARIO / RUN_DIR / mkdir/cd + 命令（不要重复长 wrapper）。示例（用真实 id 替换占位）：
> ```bash
> SCENARIO="PLACEHOLDER-USE-REAL-CLI-ID"   # 实际使用时替换为真实 "cli.config-..." 等（必须与 yaml 一致）
> RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
> mkdir -p "$RUN_DIR"
> cd "$RUN_DIR"
> # 直接使用已定义的 libra() / gitfix()
> libra init ...
> ...
> ```
> 所有场景的 wrapper 都必须与 §3.3.1 模板**语义一致**（env -i + 白名单 + TMPDIR + SAFE_PATH git/ssh 感知）。runner 完全不解析 MD 里的 wrapper，它有自己的 Rust 隔离实现（见 tools/integration-runner）。
>
> 这直接落实了 BASELINE_GAP-INTEG-001 的“收敛 N 份副本”的要求，也让 Agent 生成计划时只需关注场景特有逻辑，而非重复粘贴 15 行 env。

**违规示例（严禁出现在本计划或 runner 中）**：
- 裸 `mktemp` + 直接 `"$BINARY" cmd`（无 `env -i` + HOME/XDG/LIBRA_* 隔离）
- `env HOME=... "$BINARY" cmd` 叠加在主机环境上（会泄露 API key、Git/SSH/GPG agent、pager、sandbox 等变量）
- `env -i` 后未注入 `TMPDIR`，导致 libra 内部临时文件（含 ssh 临时私钥）落到 run root 之外
- 把开发者整段 `$PATH` 注入 wrapper（应只追加 git/ssh 真实目录，见 §3.3.0）
- 在开发者真实仓库或 $HOME 下执行
- 日志中出现 token、密钥或未脱敏 URL
- Wave 3 未用 `gh` + trap + `--yes` 清理

**审查要点**：PR 中新增场景的代码块必须能用 `shellcheck` 或人工确认每条 `libra` 行都有完整 env wrapper，且 wrapper 含 `TMPDIR` 与 git/ssh 感知 `PATH`。

隔离要求：

1. 每个场景使用 `$RUN_ROOT/repos/<scenario-id>/` 作为独立 cwd；不得复用其他场景的仓库状态。
2. Git fixture 放在 `$RUN_ROOT/fixtures/<scenario-id>/`；**所有使用 `git` 创建/操作 fixture 的场景必须定义并使用 `gitfix()` 包装**（与 `libra()` 对称），禁止裸 `git` 调用。推荐定义：
   ```bash
   gitfix() {
     env -i \
       PATH="$SAFE_PATH" \
       HOME="$RUN_ROOT/home" USERPROFILE="$RUN_ROOT/home" \
       GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
       TMPDIR="$RUN_ROOT/tmp" \
       GIT_AUTHOR_NAME="Libra Fixture" GIT_AUTHOR_EMAIL="fixture@example.invalid" \
       GIT_COMMITTER_NAME="Libra Fixture" GIT_COMMITTER_EMAIL="fixture@example.invalid" \
       LANG=C LC_ALL=C \
       git "$@"
   }
   ```
   必须同时 `unset GIT_DIR GIT_WORK_TREE ...`（或在 `gitfix` 内清空）。裸 `git` 或未包裹的 fixture 调用视为安全违规。
3. `config --global`、vault、`generate-ssh-key`、`generate-gpg-key` 场景必须使用隔离 `HOME` 和 `LIBRA_CONFIG_GLOBAL_DB`；不得触碰真实 `~/.libra/config.db`、`~/.libra/vault-keys` 或真实 SSH/GPG 配置。
4. 本地 remote 场景只能使用 `$RUN_ROOT/fixtures/` 下的路径 remote；真实远端只允许进入 Wave 3。
5. 默认串行执行场景。若 runner 未来支持并发，每个并发场景必须拥有独立 `HOME`、`XDG_CONFIG_HOME`、`XDG_CACHE_HOME`、`TMPDIR`、`LIBRA_CONFIG_GLOBAL_DB`、cwd、fixtures 和 logs；`TMPDIR` 必须独立，否则 libra 内部临时文件会在并发场景间串扰。
6. 成功时可删除 `$RUN_ROOT`；删除前必须 `cd "${TMPDIR:-/tmp}"` 或等价地把 cwd 移出 `$RUN_ROOT`；失败时必须保留 `$RUN_ROOT`，并在报告中给出场景 ID、失败命令、cwd、二进制路径和复现命令。
7. 默认 Wave 0/1/2 环境不得传入 provider API key、Cloudflare/D1/R2 凭据、`BRAVE_SEARCH_API_KEY`、`SSH_AUTH_SOCK`、`GPG_AGENT_INFO` 或其他外部服务凭据；确需真实远端认证时只能进入 Wave 3 并显式记录认证来源。

日志要求：

1. 每条命令至少记录 `<seq>.cmd`、`<seq>.stdout`、`<seq>.stderr`、`<seq>.exit`。
2. `<seq>.cmd` 必须包含场景 ID、cwd、脱敏后的完整命令、关键环境变量摘要和时间戳。
3. 日志不得包含 token、PAT、SSH 私钥、vault unseal key、root token 或带明文凭据的 URL。

### 3.3.2 CWD 安全与并发隔离

本计划默认串行执行。若未来 runner 支持并发，每个场景必须在独立子进程中使用 `(cd "$RUN_DIR" && ...)` 或等价机制执行，不能让多个场景共享同一个 shell 进程的 cwd 状态。Rust 测试里的 `ChangeDirGuard` 通过进程级锁序列化 cwd 变更；bash runner 不能假设有同等保护。

清理 `$RUN_ROOT` 前必须先离开该目录；否则当前 shell 的 cwd 会变成已删除目录，后续相对路径命令可能产生误导性失败。

### 3.3.3 Rust runner 实现边界与目录结构

正式落地时，runner 的核心功能应使用 Rust 开发，但它不是 Cargo 集成测试。它是一个独立开发工具，职责是编排黑盒 CLI 场景、隔离环境、记录日志、生成报告和做计划一致性检查；被测对象始终是同一个编译后的 `libra` 二进制。

#### Runner 架构决策

runner **不得**把 Markdown 代码块当作执行源。MD 里的 bash 片段服务于人工复现和 reviewer 阅读；正式执行必须来自 Rust typed scenario registry。原因：

1. Markdown 代码块包含 wrapper 函数、注释、`cd` 状态、负向 `!`、多行 heredoc、trap 和人工说明，解析为可靠 Step model 的复杂度接近重新实现 shell。
2. 直接执行文档片段会让安全边界依赖文本格式，容易绕过 `env_clear()`、日志脱敏、失败分类和 cleanup guard。
3. Agent 生成计划需要稳定的任务边界：yaml 定义“有哪些场景”，Rust registry 定义“如何执行”，MD 定义“为什么和如何人工复现”。三者由 `check-plan` 校验一致，而不是让 MD 成为唯一可执行程序。

因此正式 runner 的职责划分是：

- `integration-scenarios.yaml`：场景清单事实来源，提供 id、wave、requires_git、gh_required、断言类别等元数据。
- `tools/integration-runner/src/registry.rs` + `scenarios/*`：typed Step / Assertion 实现来源，执行时只调用编译后的 `libra` 二进制。
- `integration-test-plan.md`：人工可读规范、bash 复现参考、参数覆盖表、review 协议和 gap 记录。
- `check-plan`：校验 yaml id、MD 章节/`SCENARIO=`、§2.3 矩阵引用、Rust registry 已实现子集、Wave 3/gh 规则和断言类别，不负责执行 Markdown。

#### Agent 可执行落地切片（Implementation Slices）

为了让 Agent 能稳定生成计划并分批实现，runner 必须按以下垂直切片交付。每个切片都要能独立验证，不能只提交未接入的模型类型或空 registry。

| Slice | 交付内容 | 最小场景 | 验证命令 | 完成标准 |
|---|---|---|---|---|
| R0：工具骨架 | `tools/integration-runner/` crate、CLI 解析、`--help`、`list`、`check-plan`；**必须加载 `docs/development/integration-scenarios.yaml` 作为场景清单事实来源**，能解析 yaml + 交叉验证 MD 对应章节 | 无 | `cargo run --manifest-path tools/integration-runner/Cargo.toml -- --help`；`cargo run --manifest-path tools/integration-runner/Cargo.toml -- list` | 不进入根 `Cargo.toml [[test]]`；`list` 必须输出 yaml 里的全部 id（含 wave）；`check-plan` 必须报告“yaml 定义但 MD 缺章节”或“MD 有场景但 yaml 未登记”，而不是静默通过 |
| R1：Wave 0 preflight | 二进制定位/构建、`env_clear()` 白名单环境、SAFE_PATH git/ssh 解析、RUN_ROOT 创建、日志目录 | `wave0.build-and-help`（runner 内部 preflight，不是 §4 CLI 场景） | `cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --waves 0` | 产出 `report.json`、`summary.md`、`results.ndjson`；失败时包含命令、退出码、stderr tail |
| R2：最小 CLI smoke | Step/Assertion 模型、typed Rust registry、`run --only`、JSON 断言、stderr/LBR 断言、fsck 断言 | `cli.init-basic`、`cli.config-basic-kv`、`cli.commit-status-log` | `cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --only cli.init-basic,cli.config-basic-kv,cli.commit-status-log` | 三个场景全走编译后的 `target/debug/libra`；无裸 `git`；所有命令日志脱敏 |
| R3：本地协议与对象 | `gitfix()` 等价 fixture、文件断言、ref/object 断言、env-skip 分类 | `cli.object-readback`、`cli.clone-fetch-pull-local`、`cli.push-local-file-remote-rejected` | `cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --waves 2 --only ...` | git 缺失时为 `env-skip`；本地 file remote push 拒绝有正向断言 |
| R4：Wave 1 批量迁移 | 逐批迁移 §4.1 场景，保持每批 ≤5 个场景 | 按命令组拆分 | `run --waves 1` 的已注册子集 | `check-plan` 能区分“yaml/MD 已定义但未迁移”和“runner 已覆盖” |
| R5：Wave 3 live | `run-live`、`gh` preflight（含 delete_repo scope）、GitHub repo 自动创建/查询/删除、Rust GhRepoCleanupGuard + disarm、ctx.gh（host auth）、Wave 3 脱敏前置 + 报告 cleanup 字段 | `live.github-create-push-clone-fetch` | `cargo run --manifest-path tools/integration-runner/Cargo.toml -- run-live --only live.github-create-push-clone-fetch` | 无 delete_repo scope 时 preflight 直接 skip（不创建）；成功/失败均记录 cleanup 状态；绝不输出 token；guard 兜底 delete |

Agent 生成实现计划时，默认只规划下一到两个 slice。若要求“补全集成测试 runner”，也必须拆成上述 slices 并先落 R0-R2；R3 以后按改动风险或 reviewer 要求追加。

#### Agent 任务输入/输出契约

给 Agent 派发本计划相关任务时，推荐使用以下输入格式，避免把本文件误读成一次性大改：

```text
Goal: implement integration runner slice <R0|R1|R2|...>
Scope: only tools/integration-runner/** and docs/development/integration-scenarios.yaml + docs/development/integration-test-plan.md if contract or scenario metadata changes
Must not: add root Cargo.toml [[test]], modify tests/INDEX.md for runner scenarios, call libra internals directly, use cargo test target names as runner scenarios
Required scenarios: <scenario ids from yaml>
Verification: <exact cargo run --manifest-path ... commands>
Expected report fields: report.json, summary.md, failures.md, results.ndjson
Registry contract: runner must treat integration-scenarios.yaml as the list of truth for `list` and wave selection; check-plan must validate yaml vs MD headings vs implemented registry keys
```

Agent 输出必须包含：变更文件、已实现 slice、已注册场景、运行命令与结果、未完成的 `BASELINE_GAP-INTEG-*`。如果只完成 R0/R1，不得声称 Wave 1/2 默认门已自动化。

**Agent 添加/迁移场景的落地 checklist（必须严格遵守，否则 check-plan 或 run 会失败）**：
1. **先编辑事实来源**：在 `docs/development/integration-scenarios.yaml` 新增/修改条目（必须包含 id、wave、group、purpose、gh_required、requires_git、key_assertion_categories、doc_section=id）。
2. **同步 MD 人类文档**：在 `docs/development/integration-scenarios/<id>.md` 添加/更新 `### `<id>`` 完整章节，必须包含：
   - 以 `SCENARIO=...` 形式出现的场景 ID 赋值行（必须与 yaml id 精确一致；实际 bash 代码块中使用真实 "cli.xxx"）
   - 遵循 §3.3.1 的 `libra()` / `gitfix()` 规范模板（env -i + SAFE_PATH + TMPDIR）
   - 最小步骤 + 负向步骤
   - 至少覆盖该场景 key_assertion_categories 的“补充可执行断言”（JSON envelope + fsck + LBR-/negative + 隔离验证等；参见 §2.3 标准和 cli.commit-status-log 示范）
   - 参数覆盖表（如适用）
3. **如触达新命令或 compat 语义**：同步更新 §2.3 覆盖矩阵 + 运行 `cargo test --test compat_matrix_alignment`。
4. **实现 Rust 执行（单一注册点，已模块化）**：只在 `tools/integration-runner/src/registry.rs` 的 `scenario_registry()` 数组里增加一行 `("cli.xxx", scenario_xxx),`，并在 `tools/integration-runner/src/scenarios/<sanitized-id>.rs` 实现对应的 `pub(crate) fn scenario_xxx(ctx: &mut ScenarioCtx) -> Result<()> { ... }`。
   - 使用 `ctx.command` / `ctx.gitfix` / `ctx.command_with_stdin` + `assert_json_ok` 等 helper。
   - 必须实际触发该 id 在 yaml 里声明的 `key_assertion_categories`（断言强化标准）。**`check-plan` 现在会启发式校验 source-verifiable 类别**（`json_envelope`/`fsck`/`gitfix_isolation`/`negative_exit`/`lbr_error`/`conflict_markers`/`gh_lifecycle`/`cleanup_guard`/`file_exists`）：若 `scenarios/<id>.rs` 未留下对应断言信号（`assert_json_ok`/`--json`、`fsck`、`ctx.gitfix`、`, false)` 负向调用、`assert_lbr_or_text`、`<<<<<<<`、`ctx.gh`、`GhRepoCleanupGuard`、`ensure_file`/`.join(` 等），`check-plan` 直接失败。声明类别即承诺断言它。
   - 旧的”三个注册点”（const + match + fn）已收敛为 registry 数组 + fn 实现。
5. **MD 场景文档使用短形式（进一步收敛）**：新增/修改的 `docs/development/integration-scenarios/<id>.md` 必须使用短形式（只写 SCENARIO + RUN_DIR + mkdir/cd + 特有命令/断言），不要重复粘贴长 `libra()` wrapper 或 RUN_ROOT 创建。长 wrapper 只存在于 prelude 块和少量示范场景中。
6. **验证**：
   - `cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan` 必须 0 退出。
   - `cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --only <new-id>[,...]` 全绿 + 报告符合 §5。
   - 产出物不得含 secret，隔离正确。
7. **PR Test Plan stanza** 必须按 §8.1 列出受影响场景 + results + commit sha。

**当前 Rust 注册点（已进一步收敛，单一站点）**：
- 唯一站点是 `tools/integration-runner/src/registry.rs::scenario_registry()` 返回的 static 数组（包含所有 `("cli.xxx", ...)` + live.*）。添加新场景只需在此数组加一行 + 在 `src/scenarios/*.rs` 实现对应 fn。
- check-plan（implemented 集合）和 run/run-live 调度都只读取这个 registry。
- runner 入口已经拆分：`main.rs` 只负责 clap dispatch；`manifest.rs`/`plan.rs` 负责清单与收敛检查；`runner/` 负责执行上下文、普通 run 与 live run；`scenarios/` 每个场景一个文件。
- 另见上方 checklist 第4点和 runner 源码里的 `scenario_registry()` 注释。

当前目录结构：

```text
tools/integration-runner/
  Cargo.toml              # 独立工具 crate；不加入根 Cargo.toml [[test]]
  README.md
  src/
    main.rs               # CLI parse + 子命令 dispatch
    cli.rs                # clap 命令面
    manifest.rs           # integration-scenarios.yaml 读取
    plan.rs               # check-plan 收敛 gate
    registry.rs           # 唯一 scenario_registry() 实现清单
    runner/               # run/run-live 调度、ScenarioCtx、报告和命令执行 harness
    scenarios/            # 每个 scenario_* 一个文件，shared.rs 放共享 fixture helper
```

调用约定：

```bash
# 显式运行，不进入 cargo test --all，也不注册到根 Cargo.toml [[test]]
cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan
cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --waves 0,1,2
cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --only cli.commit-status-log
cargo run --manifest-path tools/integration-runner/Cargo.toml -- run-live --only live.github-create-push-clone-fetch
```

工程边界：

1. `tools/integration-runner/` 与 `tests/` 分开；不要新增 `tests/integration_runner*.rs`，不要把 `plan_check` 注册成 `[[test]]`。
2. 根 `Cargo.toml` 不新增 runner 的 `[[test]]` 条目；如 CI 需要门控，使用显式 `cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan` 或 `run` 步骤。
3. runner 可以有自己的 `Cargo.toml` 和依赖，但不应把主 crate 的内部模块当库直接调用；它通过进程执行 `target/debug/libra`，保持黑盒边界。
4. shell 代码块仍可作为场景文档和人工复现示例；正式 runner 中的核心执行、日志、脱敏、报告、计划检查和 GitHub cleanup 逻辑必须在 Rust 中实现。
5. 若未来需要复用 runner 逻辑，优先在 `tools/integration-runner/src/lib.rs` 内抽模块，不把这些 helper 混入 `src/` 生产代码或现有 `tests/command/` helper。

### 3.4 GitHub live 场景执行约束

每个 Wave 3 场景必须遵守：

1. 先满足 §3.3 的本地隔离规范；GitHub 仓库只是额外远端资源，不得替代本地 `RUN_ROOT`、logs 和复现信息。
2. GitHub 账号、仓库生命周期和远端状态检查只通过 `gh` 操作；允许使用 `gh api`，不允许使用浏览器 UI、`curl` 或手写 REST 客户端替代。
3. 运行前必须执行 `gh auth status --active --hostname github.com`；未登录、权限不足或网络不可达时，Wave 3 记为明确 skip/block，不得降级为本地 remote 测试。
4. 每次运行必须创建全新的临时私有仓库，仓库名包含 `libra-integ-<run-id>`；不得复用人工仓库或项目主仓库。
5. 默认从 `gh repo view <owner/repo> --json sshUrl --jq '.sshUrl'` 取得远端 URL；如改用 HTTPS，必须在测试说明中记录 Libra 使用的认证来源，并禁止把 `gh auth token`、PAT 或其他 secret 嵌入 URL 或日志。
6. 清理必须通过 `gh repo delete <owner/repo> --yes` 执行；如果 deletion scope 不足，场景应在创建前阻断，避免留下无法自动清理的仓库。
7. 失败时保留本地临时目录路径和 GitHub 仓库名；若自动删除失败，报告必须把 cleanup 状态标为 `cleanup_required`。

### 3.6 测试环境安全与隔离自检清单（Safety & Isolation Checklist）

**本计划最核心的安全要求**：永远不污染开发者真实环境、不泄露 secret、不留下不可清理的 GitHub 仓库。所有贡献者和 reviewer 必须在 PR 中逐条确认。

**本地隔离（每次运行前自检）**：
- [ ] 使用唯一 `mktemp` RUN_ROOT（含时间戳+$$）
- [ ] 所有 `libra` 调用都通过 `env -i` 清空环境后白名单注入 `PATH`、`HOME`、`USERPROFILE`、`XDG_*`、`TMPDIR`、`LIBRA_CONFIG_GLOBAL_DB`、`LIBRA_TEST=1`、`LANG=C`、`LC_ALL=C`
- [ ] `TMPDIR="$RUN_ROOT/tmp"` 已注入，libra 内部临时文件（含 ssh 临时私钥、pack 中转）不外泄到 `/tmp`
- [ ] git/ssh 可在收窄 PATH 解析到；如不在则只追加其真实目录（不灌入整段主机 `$PATH`），并在报告记录该偏离
- [ ] 每个场景独立 `$RUN_ROOT/repos/<scenario-id>/` 或 `$RUN_ROOT/fixtures/<scenario-id>/` cwd，不跨场景复用状态；禁止裸 `mktemp -d` 或无 `$RUN_ROOT` 前缀的临时目录
- [ ] vault / `--global` / keygen 场景显式用隔离 HOME
- [ ] Git fixture 设置 `GIT_CONFIG_NOSYSTEM=1` + `GIT_CONFIG_GLOBAL=/dev/null`（或 `$RUN_ROOT/home/.gitconfig`）+ 显式 user.name/email，并 `unset`/`env -i` 清掉主机 `GIT_DIR`/`GIT_WORK_TREE`/`GIT_INDEX_FILE`/`GIT_*_*`（防止 fixture git 被重定向到真实仓库）
- [ ] 默认 Wave 0/1/2 环境不含 provider API key、Cloudflare/D1/R2 凭据、`SSH_AUTH_SOCK`、`GPG_AGENT_INFO` 或其他外部服务凭据
- [ ] 日志文件（或 summary）不包含任何 token、PAT、私钥、unseal key 或明文凭据 URL
- [ ] **已执行机器化脱敏自检**（见下方命令），命中即阻断归档
- [ ] 成功清理 `$RUN_ROOT` 前 cwd 已移出 `$RUN_ROOT`
- [ ] 失败时保留整个 RUN_ROOT 路径供复现；成功可清理

归档前机器化脱敏自检（命中非空即视为泄露，阻断归档并要求处理）：

```bash
# 在 $RUN_ROOT/logs 与 artifacts 上扫描常见 secret 形态；任一命中即失败
grep -rolE \
  'ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|-----BEGIN [A-Z ]*PRIVATE KEY-----|AKIA[0-9A-Z]{16}|xox[baprs]-[A-Za-z0-9-]+|https?://[^/[:space:]]*:[^/@[:space:]]+@' \
  "$RUN_ROOT/logs" "$RUN_ROOT/artifacts" && {
    echo "SECRET LEAK DETECTED — 阻断归档" >&2; exit 1; } || echo "redaction self-check clean"
```

**Wave 3 GitHub 安全（必须）**：
- [ ] 运行前 `gh auth status --active --hostname github.com` 通过
- [ ] 仅用 `gh repo create --private` 创建 `libra-integ-*` 临时私有仓库
- [ ] 远端 URL来自 `gh repo view --json sshUrl`（或有记录的 HTTPS 认证源）
- [ ] 若依赖 `SSH_AUTH_SOCK`，PR/Test Plan 明确记录使用的是主机 SSH agent；若不依赖，则显式清空并使用测试专用认证来源
- [ ] 所有 `gh` 操作 + `libra` 操作的日志不输出 secret
- [ ] 使用 `trap 'gh repo delete "$REPO" --yes' EXIT` 或等价强制清理
- [ ] 失败时报告 `cleanup_required <owner/repo>`；人工确认删除

**禁止模式（触发 review 阻断）**：
- 直接在 `$HOME`、当前 monorepo 或真实 `.libra/` 下跑测试命令
- 裸 `"$BINARY" foo` 或 `env HOME=... "$BINARY" foo` 而无 `env -i` 白名单 wrapper
- 在日志或 PR 描述中贴出 `gh auth token`、PAT 或 vault 路径
- Wave 3 场景降级为“本地 remote”或跳过清理

**未来 runner 要求**：runner 必须在执行前 enforce 上述模板；对违规块给出清晰错误并阻断。

### 3.5 Path -> Wave 映射

Wave 0 始终默认执行，下表只列额外需要跑的 CLI 场景 wave。

| 修改路径 | 必跑 Wave | 推荐补充 |
|---|---|---|
| `src/cli.rs`、`src/command/*.rs`（版本管理命令） | 1 | 2，若触达 storage/protocol/schema；**必须同步更新 §2.3 覆盖矩阵** |
| `src/command/{stash,bisect,worktree,checkout,branch,fetch,push,pull,remote,clone,status,commit,reset,restore,switch,tag,merge,rebase,cherry_pick,revert,rm,mv,clean,grep,blame,reflog}.rs` | 1 | 2 |
| `src/internal/branch.rs`、`src/internal/head.rs`、`src/internal/reflog.rs`、`src/internal/tag.rs` | 1 | 2 |
| `src/internal/protocol/**`、`src/git_protocol.rs` | 2 | 3，若触达 GitHub/SSH/HTTP 真实远端语义 |
| `src/utils/storage/**`、`src/utils/object*.rs`、`src/utils/tree.rs`、`src/utils/worktree.rs` | 2 | 1，若用户命令输出变化 |
| `src/internal/db.rs`、`src/internal/db/**`、`sql/**`、`src/internal/model/**` | 2 | 1，若 CLI 输出变化 |
| `Cargo.toml`、`Cargo.lock`、`build.rs` | 0, 1, 2 | 确认 `libra` 二进制仍可构建和执行 |
| `docs/**`、`README.md`、`COMPATIBILITY.md` | 0 | 1，若改动命令/兼容矩阵语义；同步 §2.3 矩阵 |
| `tests/**` | 对应 wave | 只在其影响 CLI 场景或 runner 辅助逻辑时纳入 |
| `src/command/{clone,fetch,pull,push,remote,ls_remote}.rs` | 1, 2 | 3，若行为需要真实 GitHub 远端确认 |
| `src/command/{lfs,fsck,cat_file,verify_pack,symbolic_ref,shortlog,describe,open}.rs` | 1 | 2 |

---

## 4. 执行波次

> **按场景拆分**：每个 `cli.*` / `live.*` 的可执行步骤、断言与负向用例在 [`integration-scenarios/`](integration-scenarios/README.md) 下独立成文（`integration-scenarios/<id>.md`）。本节只保留 Wave 结构与索引；`check-plan` 扫描该目录而非本文件内联章节。

Wave 1 覆盖单仓库、无网络、无外部服务的核心版本管理闭环。

### 4.1 Wave 1：CLI 核心版本管理场景（必跑）

完整步骤与断言见 [`integration-scenarios/README.md#wave-1`](integration-scenarios/README.md#wave-1)。参数覆盖表见 [`integration-scenarios/_parameter-tables.md`](integration-scenarios/_parameter-tables.md)。

### 4.2 Wave 2：CLI 存储、schema 与本地协议场景（必跑）

完整步骤与断言见 [`integration-scenarios/README.md#wave-2`](integration-scenarios/README.md#wave-2)。

### 4.3 Wave 3：GitHub 真实远端场景（按需运行）

完整步骤与断言见 [`integration-scenarios/README.md#wave-3`](integration-scenarios/README.md#wave-3)。执行约束仍遵守 §3.4 与 §3.6。


## 5. 输出方案与测试报告（Output & Reporting）

本节定义本计划的**统一输出契约**：每个 `cli.*` / `live.github-*` 场景产出一条成功/失败任务记录；失败必须携带可直接用于下一步调试的错误信息；同时给出一份命令行易读的汇总和一份机器可解析的测试报告。runner 落地前（BASELINE_GAP-INTEG-001）按本契约手工记录；落地后必须原样产出本节定义的文件。

### 5.1 三层输出（设计总览）

| 层 | 受众 | 载体 | 时机 |
|---|---|---|---|
| L1 实时行 | 终端前的人 | stdout 每场景一行（`▶ running` → `✓ PASS` / `✗ FAIL` / `⚠ SKIP`） | 边跑边出 |
| L2 结果流 | runner / CI / 崩溃恢复 | `$RUN_ROOT/logs/results.ndjson`，每场景追加一行 JSON | 每场景结束即 flush（进程被杀也不丢已完成结果） |
| L3 终态报告 | reviewer / 下一步调试 | `report.json`（聚合）+ `summary.md`（人读）+ `failures.md`（调试交接）+ `rerun-failed.txt` | 全部结束后生成 |

状态词表（全计划统一，与 §3.4/§3.6/INTEG-010 对齐）：

- `pass`：所有命令退出码符合预期、所有断言（`test`/`grep`）通过。
- `fail`：某条应成功的 `libra` 命令非 0，或某条断言失败——**libra 行为失败，必须记错误信息**。
- `skip`：按 §3.5 路径映射本次无需运行（非缺陷）。
- `env-skip`：环境前置缺失（git/ssh 不在隔离 PATH，见 INTEG-010；Wave 3 未 `gh auth`）——**非 libra 缺陷**，与 `fail` 严格区分。
- `block`：Wave 3 前置被拒（如无删除权限）——不创建远端资源。

### 5.2 单命令记录（细粒度，沿用 §3.3 日志要求）

每条 `libra` 调用在 `$RUN_ROOT/logs/<scenario-id>/` 下落 `<seq>.cmd/.stdout/.stderr/.exit`（脱敏后）。这是失败时的取证底料；§5.3 的场景记录从中抽取摘要。

### 5.3 单场景结果记录（核心：失败错误捕获）

每个场景产出**一条** `results.ndjson` 记录。失败时 `failure` 字段非空，承载“下一步调试”所需的全部信息：

```json
{
  "scenario": "cli.branch-switch-checkout",
  "wave": 1,
  "status": "fail",
  "commands_total": 31,
  "commands_run": 18,
  "duration_ms": 712,
  "skip_reason": null,
  "failure": {
    "command": "libra branch -d feature/renamed",
    "exit_code": 2,
    "expected": "exit 0",
    "cwd": "$RUN_ROOT/repos/cli.branch-switch-checkout/branch-repo",
    "binary": "target/debug/libra",
    "stderr_tail": [
      "error: the branch 'feature/renamed' is not fully merged",
      "hint: use 'libra branch -D feature/renamed' to force delete"
    ],
    "reproduce": "cd $RUN_ROOT/repos/cli.branch-switch-checkout/branch-repo && libra branch -d feature/renamed",
    "log_dir": "$RUN_ROOT/logs/cli.branch-switch-checkout"
  }
}
```

`pass` / `skip` / `env-skip` 记录的 `failure` 为 `null`；`skip`/`env-skip` 必填 `skip_reason`。所有字符串入文件前先过 §3.6 脱敏自检。

**自动捕获机制（让“失败命令 + 错误信息”可机器获取）**：正式 runner 使用 Rust typed step model，而不是把 Markdown 代码块或 bash trap 当作核心执行逻辑。每个 `Step` 都声明 cwd、argv、环境类型（libra/gitfix/basic）、期望退出码和断言；runner 用 `std::process::Command::env_clear()` 执行，逐条落盘 stdout/stderr/exit，并在第一条不符合期望的 step 上生成 `failure`：

```rust
fn run_step(ctx: &ScenarioContext, step: &Step) -> StepResult {
    let output = Command::new(step.executable(ctx))
        .args(&step.argv)
        .current_dir(&step.cwd)
        .env_clear()
        .envs(ctx.env_for(step.environment))
        .output()?;

    write_command_logs(ctx, step, &output)?;

    let code = output.status.code().unwrap_or(128);
    if !step.expected_exit.matches(code) {
        return StepResult::fail(
            step.display_command(),
            code,
            step.expected_exit.to_string(),
            stderr_tail(&output.stderr, 20),
        );
    }
    
    run_assertions(ctx, step, &output)
}
```

要点：负向命令不靠 `! libra ...` 表达，而是显式建模为 `ExpectedExit::NonZero` 或 `ExpectedExit::Code(n)`；文件、JSON、stderr、ref、fsck 等断言也作为 Rust `Assertion` 执行。文档里的 bash 代码块仍用于人工复现和场景语义说明；迁移期若临时使用 shell 子进程封装旧场景，也必须通过同等 `env_clear()` 白名单环境和日志/脱敏/失败分类约束，最终收敛到 §3.3.3 的 typed Rust runner。

### 5.4 命令行易读汇总（L1）

边跑边出每场景一行；全部结束后打印分组合计 + 失败详情块。颜色用 `tput`，并遵守 `NO_COLOR` / 非 TTY 时自动关闭：

```text
Libra 集成测试   run=20260601T1530Z-48213   binary=target/debug/libra   commit=abc1234
──────────────────────────────────────────────────────────────────────────────
WAVE 1  CLI 核心版本管理
  ✓ PASS  cli.config-basic-kv              12 cmds    0.4s
  ✓ PASS  cli.commit-status-log            27 cmds    1.1s
  ✗ FAIL  cli.branch-switch-checkout       18/31      0.7s
  ⚠ SKIP  cli.config-import-path-edit      env-skip: git 不在隔离 PATH
WAVE 2  存储 / schema / 本地协议
  ✓ PASS  cli.clone-fetch-pull-local       24 cmds    2.3s
  ✗ FAIL  live.github-create-push-clone-fetch 09/22   8.2s
──────────────────────────────────────────────────────────────────────────────
合计  6 场景 ：3 pass   2 fail   1 skip          wave3 = not_run
RUN_ROOT（已保留供复现）：/tmp/libra-integ-20260601T1530Z-48213.Ab12Cd
脱敏自检：clean    机器报告：/tmp/libra-integ-.../report.json

失败详情（用于下一步调试）
✗ cli.branch-switch-checkout
    失败命令 ： libra branch -d feature/renamed
    退出码   ： 2   （期望 0）
    cwd      ： $RUN_ROOT/repos/cli.branch-switch-checkout/branch-repo
    stderr   ： error: the branch 'feature/renamed' is not fully merged
                hint: use 'libra branch -D feature/renamed' to force delete
    复现     ： cd <cwd> && libra branch -d feature/renamed
✗ live.github-create-push-clone-fetch
    失败命令 ： libra push --force origin main
    退出码   ： 128  （期望 0）
    cwd      ： $RUN_ROOT/repos/source
    stderr   ： fatal: unable to update remote ref refs/heads/main
    复现     ： cd <cwd> && libra push --force origin main
```

可读性约束：等宽对齐三列（状态 / 场景 ID / 进度·耗时）；`fail` 进度显示 `已过/总数`（如 `18/31`）以定位卡点；`env-skip` 同行附原因；失败详情统一缩进，`stderr` 多行对齐到同一列；退出码恒附“期望值”便于一眼判断是否预期。

### 5.5 测试报告（L3 终态文件）

落在 `$RUN_ROOT/`（脱敏后可复制到 `target/integration-runs/$RUN_ID/`，**只复制脱敏摘要，绝不复制 secrets / vault key / 完整 HOME**）：

- `report.json` — 机器可解析聚合：

```json
{
  "run_id": "20260601T1530Z-48213",
  "commit": "abc1234",
  "binary": "target/debug/libra",
  "platform": "darwin-arm64",
  "started_at": "2026-06-01T15:30:00Z",
  "finished_at": "2026-06-01T15:30:06Z",
  "waves_run": [0, 1, 2],
  "totals": { "pass": 3, "fail": 2, "skip": 1, "env_skip": 1, "block": 0 },
  "wave3_cleanup": "not_run",
  "run_root": "/tmp/libra-integ-20260601T1530Z-48213.Ab12Cd",
  "run_root_state": "preserved",
  "redaction_self_check": "clean",
  "scenarios": [ /* §5.3 的逐场景记录数组 */ ]
}
```

- `summary.md` — 人读总表：commit / 平台 / 二进制路径 / 每场景 status / `RUN_ROOT` 清理状态 / 每个 fail 的失败命令首行 + 复现命令 / Wave 3 `gh` cleanup 状态。
- `failures.md` — **调试交接专用**：只含 `fail` 场景，每条给“失败命令 + 退出码 + stderr 尾部 + cwd + 复现命令 + `log_dir`”，是把现场移交给下一步 debug（人或 agent）的最小充分集。无 `fail` 时写 `no failures`。
- `rerun-failed.txt` — 每行一个失败场景 ID，供下一轮 `cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --only "$(paste -sd, rerun-failed.txt)"` 只重跑失败项。
- 分 wave 原始日志（沿用旧契约）：`wave0-build.log` / `wave1-cli-core.log` / `wave2-cli-storage-protocol.log` / `wave3-github-live.log`（未运行写 skip/block 原因）。

### 5.6 退出码与 CI 对接

runner 进程退出码：`0` = 无 `fail`（`skip`/`env-skip` 不算失败）；`1` = 至少一个 `fail`；`2` = 前置/编译失败（Wave 0 未过）。CI 以退出码门控，以 `report.json.totals.fail == 0` 复核，并把 `failures.md` 贴进失败 job 摘要。**`env-skip` 不得让 CI 变绿掩盖问题**：当某 wave 因环境缺失被整体 `env-skip` 时，runner 退出码仍为 0 但必须在 stdout 和 `summary.md` 顶部高亮 `WARN: <wave> env-skipped (<reason>)`，由 reviewer 判断是否可接受。

报告中所有 URL / 路径 / 命令在写盘前都必须通过 §3.6 的脱敏自检；命中即标记该 run 为 `redaction_self_check: "leak-blocked"` 并拒绝归档。

### 5.7 实现现状：runner 实际产出的报告 schema（Implemented report schema）

§5.1–§5.6 描述输出契约的完整设计模型；本节记录 `tools/integration-runner` **当前实际产出**的字段形态（事实来源为 `tools/integration-runner/src/runner/types.rs` 与 `support.rs::write_report`），保证文档与代码一致。runner 在每轮结束写盘下列文件：`report.json`、`results.ndjson`、`summary.md`、`failures.md`、`rerun-failed.txt`，以及 `logs/<scenario-id>/<seq>.{cmd,stdout,stderr,exit}`、`artifacts/`。

`report.json`（聚合，实际字段）：

```json
{
  "run_id": "20260605T013913Z-12345",
  "commit": "a1b2c3d",
  "started_at": "2026-06-05T01:39:13.123456+00:00",
  "finished_at": "2026-06-05T01:39:19.654321+00:00",
  "waves_run": [0, 1],
  "wave3_cleanup": "not_run",
  "run_root_state": "preserved",
  "generated_at": "2026-06-05T01:39:13.700114+00:00",
  "platform": "macos-aarch64",
  "run_root": "/tmp/.../libra-integ-XXXX",
  "binary": "/abs/path/target/debug/libra",
  "redaction_self_check": "clean",
  "totals": { "pass": 37, "fail": 0, "skip": 0, "env_skip": 0, "block": 0 },
  "passed": 37,
  "failed": 0,
  "skipped": 0,
  "results": [ /* 每场景一条 ScenarioResult */ ],
  "scenarios": [ /* 同 results（设计模型对齐别名，兼容并存） */ ]
}
```

`results.ndjson`：每场景一行 `ScenarioResult` JSON，字段为 `id`、`wave`、`status`、`duration_ms`、`run_dir`、`commands[]`（每条 `seq`/`command`/`cwd`/`exit_code`/`success`/`stdout_log`/`stderr_log`/`stderr_tail`）、`error`（失败信息，pass 时 `null`）、`cleanup`（仅 Wave 3：`deleted <repo>` / `cleanup_required <repo>` / `null`）。`report.json.results` 与本文件内容一致。

**状态词映射（实现 vs §5.1 设计）**：runner 当前发出的 `status` 字符串为 `passed` / `failed` / `skipped`，分别对应 §5.1 的 `pass` / `fail` / `skip`；`env-skip` 与 `block` 暂统一记为 `skipped`（计数留在 `totals.env_skip` / `totals.block`，当前恒 0）。`redaction_self_check` 为 `clean`：写盘前每条命令的原始 stdout/stderr 都过 `ensure_no_secret_leak`，命中即中止该场景（不产出泄漏报告），故已产出的 report 恒为 leak-clean。退出码语义见 §5.6（`passed/failed/skipped` 计数即 §5.6 的门控来源；`failed==0` 时退出 0）。

**仍待对齐设计模型的字段**（按 BASELINE_GAP 跟踪，非阻断）：`run_id` / `commit` / `started_at` / `finished_at` / `waves_run` / `run_root_state` / `wave3_cleanup` 以及 `env-skip` vs `block` 的独立状态区分和 advisory 类别更深语义已部分对齐（见 BASELINE_GAP-INTEG-002）。2026-06 对齐工作（仅 tools/integration-runner/** + plan.md；README/yaml 无漂移故未动）已向 `Report` 追加（serialize-only）以上字段（+ `scenarios` 别名），在 `write_report` / normal/live / RunContext / summary 中填充；`get_source_commit` 安全调用（带 GIT_* 隔离，不清空 runner 自身 PATH）。`run_root_state` 当前恒为 "preserved"（runner 始终 .keep() 目录，报告与日志同在其中）；`wave3_cleanup` 由 live 场景的 cleanup 聚合推导（现通过 guard arm 后立即 set_cleanup("cleanup_required") 使错误路径真正携带）。更新了 §5.7 示例 schema。`env-skip`/`block` 计数仍走 totals，状态词映射为 `skipped`（与实现现状一致）。详见 runner/types.rs、support.rs（含 derive_wave3_cleanup + tests）、util.rs（含 make_run_metadata + tests）、normal.rs/live.rs（含 ctx 注释）、scenarios/live_*.rs 及跨 harness 的 §3.3.1/§3.6 引用。同步本节完成。报告契约与元数据路径的执行覆盖仅来自 `run`/`run-live` 样本（runner crate 此前无 `#[test]`；check-plan 仅静态）。wave3_cleanup "deleted"/"cleanup_required" 分支的完整覆盖需 live L3；get_source_commit 成功路径需非-/Volumes 可发现 git 树。详见 BASELINE_GAP 及新增的 cfg(test) 用例。

---

## 6. 出口标准（Definition of Done）

### 6.1 本计划可执行判定

1. Wave 0 能在当前仓库编译出可执行的 `libra` 命令。
2. Wave 1 / Wave 2 只通过 `libra <cmd>` 驱动被测行为。
3. 默认门不要求外部凭据、真实云资源或真实公网服务；Wave 3 是显式的 GitHub live 扩展门。
4. 文档内不存在未实现的必跑 runner 或虚构命令。
5. 每个场景都有可复现的命令序列和可观察断言。
6. Wave 3 场景的 GitHub 仓库创建、查询和清理都通过 `gh` 完成。

### 6.2 默认集成阻断门

1. Wave 0 全绿。
2. Wave 1 全绿。
3. Wave 2 全绿，或失败项在同一 PR 修复。

真实 GitHub 远端 smoke 属于 Wave 3，按 §3.5 的路径映射和改动风险选择运行。真实云、publish 和其他外部服务 smoke 不属于本计划；如 release manager 另行要求，应记录在独立 release checklist，而不是加入此版本管理集成门。

---

## 7. BASELINE_GAP

以下能力不写成默认可执行步骤，只登记为后续工程任务。

### BASELINE_GAP-INTEG-001：CLI 场景 runner 基础设施已落地

- 现状：R0-R5 全部切片已落地。`tools/integration-runner/` 提供 list、check-plan（含收敛 gate）、Wave0 preflight、完整隔离执行（env_clear + SAFE_PATH + SSH_AUTH_SOCK + gitfix + gh 带 host-auth 的 ctx.gh + 原始 secret 泄漏前置检查 + 报告 §5 契约）、37/37 场景的 Rust typed 实现（含 Wave 3 的 run-live + GhRepoCleanupGuard + delete_repo scope 预检）。`run --waves 0,1,2` 与 `run-live --only live.github-create-push-clone-fetch` 均可产出完整可归档报告。check-plan 强制 yaml/MD/矩阵/registry 一致 + 短形式收敛。
- 需要补充：可选的 plan-waves（BASELINE_GAP-INTEG-003）、并发执行、key_assertion_categories 更深语义扫描（当前结构 gate 已覆盖主要类别）。深水区语义（pull --rebase 真分叉、fetch --depth 真实远端对等、更多 pack corpus、--force-with-lease、warning=9 确定源）仍按 GAP-005/009 跟踪，不属于 runner 基础设施。
- 约束：runner 始终黑盒驱动 `target/debug/libra`（或 --binary）；不得注册到根 Cargo / tests/INDEX；写盘先过 §3.6 脱敏；任何变更必须使 check-plan + 对应 run/run-live 全绿。

### BASELINE_GAP-INTEG-002：CLI 场景清单自动校验不足

- 现状：`check-plan` 已加载 yaml，并对比 runner 已实现 key，产出"文档已定义但 runner 未实现"的 id 列表（当前 0 个）。runner 已拆成 `registry.rs` + `runner/` + `scenarios/*.rs`，三方一致性 gate 继续由 **INTEG-008** 覆盖。**断言类别覆盖启发式已落地**：`check-plan` 现在对每个已实现场景扫描其 `key_assertion_categories` 中的 source-verifiable 类别（见 §2.4 列表）是否在 `scenarios/<id>.rs` 留有断言信号，缺失即失败；现有 37 个场景已逐一补强到声明即断言（config/* 补 `--json` envelope；branch/restore/stash/merge-smoke/clean/open 补负向 `LBR-` 路径；stash 补 `fsck`）。
- 需要补充：把 advisory 类别（`global_db_isolation` / `vault_isolation` / `intentional_difference`）从「运行时隔离/语义保证」升级为可机器校验的更强证据（例如解析场景实际断言的 ref/文件/JSON 字段），而非仅信号子串匹配。
- 约束：`check-plan` 可由 CI 显式执行，但不得进入 `cargo test --all` 默认测试集，不写入 `tests/INDEX.md`。三方一致性（yaml ↔ MD ↔ 矩阵）由 INTEG-008 负责。Agent 实现的 scenario fn 必须实际触发其 yaml 条目声明的 categories。

### BASELINE_GAP-INTEG-003：Path -> Wave 自动选择工具未落地

- 现状：§3.5 仍靠作者手动对照。
- 需要补充：独立 Rust runner 的 `plan-waves` 子命令读取改动路径，输出建议 CLI wave 集合。
- 约束：该命令只输出版本管理 CLI wave；不得引入交互界面、agent runtime、provider、publish 或云服务 wave。

### BASELINE_GAP-INTEG-004：GitHub live 场景 runner 与清理保护已落地

- 现状：已落地。`run-live` 子命令 + gh 预检（auth + delete_repo scope 探针）+ ctx.gh（host env，不清空）+ GhRepoCleanupGuard（Drop 兜底 + disarm on explicit success delete）+ 原始 secret 泄漏前置阻断 + 报告中 cleanup 状态记录。`live.github-create-push-clone-fetch` 已注册并实现完整步骤 + gh api 比对 + json 断言 + 隔离。无 delete 权限时 preflight 直接 skip（不创建仓库），满足“若无删除权限不启动”。
- 约束：Wave 3 仅按需（触达远端语义时）；runner 绝不输出 token；cleanup 失败时报告 `cleanup_required` 并保留 run_root。

### BASELINE_GAP-INTEG-005：版本管理命令黑盒场景覆盖不完整

- 现状：§2.3 矩阵已建立，并已为 tag、merge/rebase/cherry-pick/revert、grep/blame/describe/shortlog、clean/rm/mv/lfs、reflog/symbolic-ref、verify-pack 添加独立黑盒场景和参数表；`cli.cross-cutting-flags` 已覆盖成功 JSON envelope + 错误 JSON（`ok:false` + `LBR-*`）的基本形态。
- 需要补充：继续细化未纳入默认闭环的深水区：`pull --rebase` 真分叉冲突路径、LFS 远端 lock API、更多 pack corpus 的 `index-pack`/`verify-pack` 深度 fixture、`open` JSON 无副作用行为是否足够代表真实 open；**故意差异的正向断言**（push 拒绝本地文件 remote、symbolic-ref 仅 HEAD 等）需在对应场景中显式存在并随矩阵更新。
- 约束：任何新增场景必须是可在本机无密钥确定性复现的 `libra <cmd>` 黑盒；不得引入 live AI/cloud。
- 跟踪：§2.3 矩阵 + 对应 Wave 场景 + PR Test Plan 清单。

### BASELINE_GAP-INTEG-008：集成计划一致性检查已落地基础版

- 现状：兼容矩阵与 Code UI docs 在 `tests/compat/matrix_alignment.rs`。`check-plan` 加载 yaml，扫描 `docs/development/integration-scenarios/<id>.md`，交叉验证 §2.3 矩阵与 `scenario_registry()`（当前 37/37 已实现）。CI 在 compat-offline-core 显式运行 `check-plan` 以及 `run --waves 0,1,2`（默认黑盒执行门）。**断言强化模式校验已落地基础版**：`check-plan` 现在对每个已实现场景启发式校验其 source-verifiable `key_assertion_categories`（JSON envelope / fsck / negative LBR- / conflict / gitfix_isolation / gh_lifecycle / cleanup_guard / file_exists）是否在 `scenarios/<id>.rs` 留有断言信号（落实 §8.4 第 9、11 项的 fn 级证据），缺失即失败。
- 需要补充：quarantine / exit-code 细节检查，以及 advisory 类别（global_db_isolation / vault_isolation / intentional_difference）的更深语义校验。计划未来 CI 显式运行 check-plan 作为兼容门之一。
- 约束：不得为此新建 `scripts/` 目录，不得把该检查加入根 `Cargo.toml` 或 `tests/INDEX.md`。新增场景必须同时编辑 yaml + MD（短形式）+ runner `scenario_registry()` 数组，否则 `check-plan` 会失败或 run 会 skip。Agent 任务必须把 yaml 当 list of record。

### BASELINE_GAP-INTEG-009：深水区远端语义与全局 flag 边界

- 现状：本轮已补 force-push、`fetch --all`、`pull --rebase`（无冲突重放）、sha256 端到端、全局 flag 集中断言，并定义了 `clone --depth` + `fetch --deepen` 本地 Git fixture 场景（`cli.fetch-depth-local`）。以下仍未纳入默认确定性闭环：
  - **真实远端 shallow 对等性**：`cli.fetch-depth-local` 覆盖本地路径 `clone --depth` 与 `fetch --deepen` 目标语义；如果当前代码在该场景失败，应修 shallow 对象闭包，而不是改测试绕过。shallow 语义在真实 GitHub 远端上的对等性仍建议在 Wave 3 验证。
  - **`pull --rebase` 真分叉 + 冲突续跑**：当前只覆盖不同文件的无冲突重放；普通 `merge`/`rebase` 冲突续跑已由 `cli.merge-conflict-continue` / `cli.rebase-conflict-continue` 覆盖，但 `pull --rebase` 驱动的远端分叉冲突仍属深水区。
  - **`push --force-with-lease` / `push --atomic` 真实远端成功路径**：当前 CLI surface 已存在；默认闭环覆盖本地 file remote fail-closed，后续可在 Wave 3 live GitHub 场景补成功路径与远端 ref 比对。
  - **`--exit-code-on-warning` 退出码 9**：缺少确定性 warning 触发源，`cli.cross-cutting-flags` 暂不强行断言；需先识别一个无密钥、可复现的 warning 路径（或在 runner 中以受控方式注入）。
- 约束：以上每项落地时必须是本机无密钥可复现的 `libra <cmd>` 黑盒；shallow/分叉冲突若依赖真实远端则归入 Wave 3。
- 跟踪：§2.3 矩阵 Remote/Cross-cutting 行 + 对应 Wave 场景。

### BASELINE_GAP-INTEG-010：libra 外部工具（git/ssh）PATH 依赖未被 runner 强制

- 现状：§3.3.0 已记录 libra 在 `config import`、`init --from-git-repository`、Wave 3 SSH 等路径 fork/exec 系统 `git`/`ssh`，而 §3.3.1 收窄 PATH。当前靠 Wave 0 的 `command -v git` 预检与 `libra()` 的 `SAFE_PATH` 追加，但尚无 runner 在每次执行前强制校验，也未对“git 不可达导致的 libra 假失败”自动归类为环境 skip。
- 需要补充：runner 在 preflight 阶段解析 git/ssh 路径并写入隔离 PATH；对依赖 git/ssh 的场景，当工具缺失时输出 `env-skip: <scenario> (git not found on isolated PATH)` 而非 `fail`，与 libra 行为失败区分。
- 约束：不得为了让场景通过而把开发者整段 `$PATH` 灌入 wrapper；只追加 git/ssh 的真实目录。

---

## 8. PR / Review 协议

### 8.1 PR 描述必须包含 `## Test Plan`

```text
## Test Plan
- Binary: target/debug/libra
- New CLI scenarios:      cli.<scenario-id>
- Modified CLI scenarios: cli.<scenario-id>
- Deleted CLI scenarios:  cli.<scenario-id>
- Live GitHub scenarios:  live.github-create-push-clone-fetch | skipped: <reason>
- Waves run locally: 0, 1, 2
- Results: <pass>/<fail>/<skip>/<env-skip>   (来自 report.json.totals)
- Failures: <none> | cli.<id>: "<失败命令首行> → <stderr 首行>"   (来自 failures.md)
- Wave 3 GitHub cleanup: deleted <owner/repo> | cleanup_required <owner/repo> | not_run
- Commit SHA at run time: <sha>
```

测试引用统一用 `cli.<scenario-id>` / `live.github-*` 加完整命令。Cargo 测试目标名仅可作为开发期辅助信息，不能替代 CLI 场景结果。结果汇总取自 §5 输出契约：`Results` 行抄 `report.json.totals`，`Failures` 行抄 `failures.md`（每个 fail 一句话：失败命令 + stderr 首行），让 reviewer 不必展开日志即可判断失败性质。

**PR 必须额外声明**：
- 新增/修改的 CLI 场景均严格遵循 §3.3.1 规范模板 + §3.6 安全自检清单（每条 `libra` 调用有 `env -i` 白名单 wrapper）。
- 已同步更新 §2.3 覆盖矩阵（若触达新命令或 compat 语义）。
- Wave 3 场景已通过 gh 清理验证。

### 8.2 Reviewer 行为约束

1. 若覆盖不足，优先指出缺失的用户可执行版本管理行为和建议 CLI 场景；不要把界面/agent/provider/publish 覆盖要求塞回本计划。**必须对照 §2.3 矩阵和 COMPATIBILITY.md 检查**。
2. 报告失败时附 `commit_sha`、wave、场景 ID、完整 `libra` invocation、执行目录、日志 head/tail。
3. 怀疑 flake 时先要求用同一 `libra` 二进制和同一复现命令连续重跑；连续失败 2 次再开 flaky issue。
4. Wave 3 相关改动必须检查 `gh` 仓库生命周期：创建命令、远端 URL 来源、`gh api` 断言、`gh repo delete` 清理状态和日志脱敏。
5. **安全审查强制项**：检查所有新代码块是否符合 §3.3.1 模板与 §3.6 清单；违规直接要求修改后重审。特别关注 HOME/ vault / secret 日志 / GitHub cleanup。**必须确认 git 相关场景使用了 `gitfix()` 而非裸 `git`**。
6. **Agent 契约审查**：`--json`/`--machine` 场景必须同时覆盖成功 envelope（`ok:true` + `data`）与失败 envelope（`ok:false` + `LBR-*` 码）；故意差异行为必须有正向断言。

### 8.3 Flake 隔离清单

新增/维护：`tests/flaky_quarantine.toml` 或后续 CLI runner 对应的 quarantine 文件。

```toml
[[entries]]
scenario = "cli.<scenario-id>"
reason = "<一句话>"
issue = "<URL>"
last_seen_commit = "<sha>"
quarantined_at = "<YYYY-MM-DD>"
```

- 每次加入 quarantine，必须同时开 issue。
- 修复后必须从 quarantine 移除并在 PR 描述说明。
- quarantine 校验应确保每条 `scenario` 能解析到现有 CLI 场景。

### 8.4 本计划自检（独立 Rust 工具）

集成计划一致性检查（BASELINE_GAP-INTEG-008）应落地为独立 Rust runner/tool 的 `check-plan` 子命令。CI 可以显式运行该工具，但它**不注册到根 `Cargo.toml [[test]]`，不进入 `cargo test --all` 默认测试集，不写入 `tests/INDEX.md`，不新建 `scripts/`**。该检查应逐步覆盖：

1. 本计划默认 Wave 中不包含 Cargo `--test <name>` 集成门。
2. 本计划里所有 `--features <flag>` 出现在 `Cargo.toml [features]`。
3. 本计划没有把明确排除的测试类别写进默认 Wave 0/1/2。
4. CLI runner 落地后，本计划里所有 `cli.<scenario-id>` 都被 runner registry 覆盖（runner 内部 key 必须与 integration-scenarios.yaml 一致）。
5. GitHub live runner 落地后，本计划里所有 `live.github-*` 场景都被 runner registry 覆盖，并校验包含 `gh repo create` 与 `gh repo delete`。
6. quarantine 文件里每条 CLI / live 场景 ID 可解析为现有场景（必须在 yaml 里存在）。
7. `integration-scenarios.yaml` 与 `integration-scenarios/<id>.md`、§2.3 矩阵引用的场景 ID 三者一致（check-plan 核心职责）。
8. yaml 里 gh_required=true 的只出现在 Wave 3；requires_git 场景在 runner preflight 能正确处理 env-skip。
9. **错误 JSON 契约**：关键失败路径在 `--json`/`--machine` 下必须产出 `ok:false` + `LBR-*` 稳定码 + category/hints 的可解析 envelope（已在 `cli.cross-cutting-flags` 基线覆盖）。
10. **故意差异防护**：COMPATIBILITY.md 中 `intentionally-different` 条目在对应场景中有正向断言（非仅文档声明）。
11. **git fixture 隔离**：所有使用 `git` 的场景都定义并调用 `gitfix()` 包装（与 `libra()` 对称），无裸 `git` 调用。

---

## 9. 维护规则

1. **新增命令或修改公共表面**（`src/cli.rs` / `src/command/*.rs`）：必须同步更新 §2.3 覆盖矩阵，并在相应 Wave 补充至少一个 `cli.<cmd>-smoke` 黑盒场景（含参数表 + 负向用例），全部使用 §3.3.1 规范模板。**改动既有 Git 兼容命令时**，先在 [`integration-scenarios/README.md` 的「命令 → 场景映射」](integration-scenarios/README.md#命令--场景映射command--scenario-map) 查到该命令的 owner 场景，同步更新对应 `<id>.md` + yaml + `tools/integration-runner/src/scenarios/<file>` + `docs/commands/<cmd>.md`，再跑 `check-plan` + `run --only <owner-id>`。新增命令必须在该映射表新增一行——不得让任何 Git 兼容命令无 owner 场景。
2. 新增版本管理集成测试时，必须把 `libra <cmd>` 场景补到本计划相应 Wave，并在 CLI runner 落地后同步 runner 清单。
3. 删除/重命名场景 ID 时，必须同步更新 `integration-scenarios.yaml`、`integration-scenarios/<id>.md`（及 README 索引）、§2.3 矩阵、runner `registry.rs` + `scenarios/*.rs`、quarantine 文件。新增场景必须先在 yaml 登记，再新增对应 `<id>.md` 与 runner 实现。
4. 新增默认阻断测试必须能在本机无密钥、无外部账号、无交互界面的环境中确定性运行。
5. 未实现能力必须用 `BASELINE_GAP-*` 标记，不允许写成默认可执行步骤。
6. 若某测试需要真实网络、真实云资源或外部凭据，不得加入本计划的默认 wave。
7. 需要 GitHub 真实远端的版本管理测试必须进入 Wave 3，仓库创建、查询、API 断言和删除必须使用 `gh`。
8. 所有示例代码块与 runner 实现必须通过 §3.6 安全自检清单；CI / 人工 review 发现违规时阻断合并。
9. §2.3 矩阵、COMPATIBILITY.md 与 `src/cli.rs` 三者必须保持一致；改动任一者需运行 `cargo test --test compat_matrix_alignment`（顶层命令漂移检查）并更新本计划。
10. `integration-scenarios.yaml` 是场景存在性的唯一登记点；任何对场景列表、wave 归属、gh_required 的修改必须先编辑 yaml，再同步 `integration-scenarios/<id>.md` 与 runner 实现。`check-plan` 强制 yaml ↔ 拆分 MD ↔ registry 一致。
