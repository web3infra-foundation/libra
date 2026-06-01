# Libra 版本管理集成测试计划

> 目标：把集成测试计划收敛到“编译后的 `libra` 命令在真实临时仓库中执行版本管理功能”的黑盒测试。测试对象是 `target/debug/libra` 或 release 构建产物，而不是 Rust 单元测试、Cargo `--test` 目标或直接调用内部模块。
> 原则：默认 Wave 0/1/2 只列当前仓库真实可执行、可在本机确定性复现的 CLI 功能场景；需要真实远端互操作时，使用独立的 Wave 3 GitHub live 场景。GitHub 仓库创建、查询和清理统一通过 `gh` 命令完成。交互界面、agent runtime、provider、publish 和真实云服务不属于本计划。

---

## 0. TL;DR

**默认阻断门**：Wave 0 编译产物可用 + Wave 1 CLI 核心版本管理场景全绿 + Wave 2 CLI 兼容/存储场景全绿。

**GitHub 真实远端门**：当改动触达 `clone`、`fetch`、`pull`、`push`、`remote`、`ls-remote` 或协议层真实远端语义时，额外执行 Wave 3。Wave 3 必须用 `gh` 创建临时 GitHub 仓库，并用 `gh` 查询和删除该仓库；`libra` 只作为被测 VCS 命令访问该远端。

**测试引用规范**：

- 场景级：`cli.config-basic-kv`、`cli.config-git-compat-mode`、`cli.init-basic`、`cli.init-branch-and-format-options`、`cli.commit-status-log`。
- GitHub live 场景级：`live.github-create-push-clone-fetch`。
- 命令级：引用完整 `libra <subcommand>` 调用、退出码、关键 stdout/stderr 断言和执行目录。
- 不用 Cargo 测试目标名作为本计划的唯一引用；本计划关心用户可执行的 `libra` 行为。

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

**人工执行可行性指引**（runner 落地前）：
- **最小冒烟子集**（人工快速验证，~15-20 分钟）：Wave 0 全 + `cli.init-basic`、`cli.config-basic-kv`、`cli.commit-status-log`、`cli.branch-switch-checkout`、`cli.cross-cutting-flags`、`cli.object-readback`、`cli.stash-bisect-worktree`、`cli.clone-fetch-pull-local`（本地 remote）。
- **完整矩阵**：仅在 PR 改动触达对应命令组、或准备提交前由 runner / 长时间手工执行。
- 所有执行必须严格使用 §3.3.1 的 `libra()`（含 `TMPDIR` + 智能 `SAFE_PATH`）；第 4 章内联 wrapper 若与 §3.3.1 不一致，以 §3.3.1 为准并必须先修正文档/runner。

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
| 集成计划自检脚本 | 缺口 | 集成计划场景清单的自动一致性校验仍未落地（一直未实现，非脚本移除）；见 BASELINE_GAP-INTEG-008 |
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
| Working Tree | status, add, rm, mv, restore, clean, stash, lfs, worktree | supported/partial/int-diff | 优秀（本地确定性命令全） | cli.commit-status-log, cli.restore-reset-diff, cli.stash-bisect-worktree, cli.clean-rm-mv-lfs-basic | LFS 远端 lock API 不进默认 Wave |
| History | log, shortlog, show, show-ref, ls-remote, diff, grep, blame, describe | supported | 优秀（inspection 全） | cli.commit-status-log, cli.object-readback, cli.grep-blame-describe-shortlog | 真实远端 refs 见 Wave3 |
| Branching | commit, branch, switch, checkout, tag, merge, rebase, reset, cherry-pick, revert | supported/partial | 优秀（核心闭环全） | cli.branch-switch-checkout, cli.restore-reset-diff, cli.commit-status-log, cli.tag-basic, cli.merge-rebase-cherry-revert-smoke, cli.merge-conflict-continue, cli.rebase-conflict-continue | merge/rebase 冲突续跑成功路径已有独立场景 |
| Remote | remote, fetch, pull, push, open | supported/partial | 良好（本地 Git clone/fetch/pull + 本地 file remote push 拒绝 + GitHub live push 闭环 + `clone --depth` 本地 shallow 实现目标，open 无副作用 smoke） | cli.clone-*, cli.push-local-file-remote-rejected, cli.open-smoke, cli.fetch-depth-local, live.github-create-push-clone-fetch | `fetch --depth` 补充语义与 `pull --rebase` 真分叉冲突路径仍属深水区；真实 push 语义只在 Wave3 |
| Maintenance | db, fsck, cat-file, hash-object, verify-pack, rev-parse, rev-list, symbolic-ref, reflog, bisect, index-pack | supported/partial/int-diff | 良好（index-pack 除外） | cli.schema-*, cli.object-readback, cli.sha256-object-readback, cli.verify-pack-smoke, cli.stash-bisect-worktree, cli.reflog-symbolic-ref | index-pack 为隐藏内部命令，仅在 verify-pack 场景的 fixture 生成中作为辅助命令使用；sha256 端到端读写已独立覆盖 |
| Cross-cutting | --json/--machine/--quiet/--color/--progress/--exit-code-on-warning | supported | 良好（独立场景集中断言全局 flag 语义；warning=9 仍按 gap 跟踪） | cli.cross-cutting-flags | 详见下方「跨命令标志」 |
| AI/Cloud | code*, automation, cloud, publish, agent*, hooks | intentionally-different | 显式排除（见 2.2） | — | hooks 为兼容隐藏命令，由专属测试覆盖 |

**剩余覆盖缺口（BASELINE_GAP-INTEG-005）**：本次计划已补齐 tag、merge/rebase/cherry-pick/revert、merge/rebase 冲突续跑成功路径、grep/blame/describe/shortlog、clean/rm/mv/lfs、本地 reflog/symbolic-ref、verify-pack 的独立场景 + 参数表；本轮改进又补齐 **本地 file remote push 拒绝（`cli.push-local-file-remote-rejected`）**、**GitHub live push（`push --dry-run` / `push -u` / refspec / `--tags` / delete / `--force` / `--mirror`）**、**`fetch --all`**、**`pull --rebase`**、**sha256 端到端对象读写（`cli.sha256-object-readback`）** 与 **全局 flag 集中断言（`cli.cross-cutting-flags`）**。仍需后续细化（按风险排序）：`pull --rebase` 真分叉路径、`fetch --depth` 补充语义、LFS 远端 lock API、更多 pack corpus 的 `index-pack`/`verify-pack` 深度 fixture、以及 `open` 的 JSON 无副作用行为是否足够覆盖真实系统 open。`cli.fetch-depth-local` 已定义本地 Git fixture 的 `clone --depth` 目标断言；若当前实现返回 `LBR-REPO-002 object not found`，应作为 shallow clone 对象闭包缺陷处理。注意 `push` 当前有 `--force`/`-f`、`--tags`、`--mirror`，但**无 `--force-with-lease`**；`fetch` 当前只有 `--all`/`--depth`、**无 `--prune`/`--tags`**——本矩阵只登记已存在的 flag，避免引用不存在的参数。新增命令到 `src/cli.rs` 时必须同步更新本矩阵并至少添加一个 `cli.<cmd>-smoke` 场景。

**跨命令标志（Cross-cutting）**：`--json`/`--machine`/`--quiet`/`--color`/`--progress`/`--exit-code-on-warning` 是全局 flag（定义在 `src/cli.rs` 的 `Cli` 根结构，对所有子命令生效）。本轮改进新增 `cli.cross-cutting-flags` 场景集中断言其语义（JSON envelope 形态、`--machine` 蕴含 ndjson+quiet+no-pager+color=never、`--quiet` 抑制 stdout、无 warning 时 `--exit-code-on-warning` 不改变退出码、`--color=never`/`NO_COLOR`），不再依赖各功能场景顺带覆盖。确定性 warning 触发源尚未固化，warning 时退出码 9 按 BASELINE_GAP-INTEG-009 跟踪，不能在默认 Wave 中硬断言。

**故意差异回归防护（Intentional Differences Regression Guards）**：COMPATIBILITY.md 明确标注的 `intentionally-different` 行为必须有**正向断言**防止悄悄对齐 Git：
- `worktree remove` 默认保留目录（不隐式数据丢失）——已在 `cli.stash-bisect-worktree` 断言 `test -d`。
- `push` 拒绝本地文件 remote（仅支持 `git@` / `https` 等网络 remote）——已由 `cli.push-local-file-remote-rejected` 显式断言 `LBR-CLI-003` / "local file repositories is not supported"。
- `symbolic-ref` 仅支持 HEAD（其他符号引用因 SQLite 存储被拒绝）。
- 这些必须出现在对应场景的负向步骤或专用小节中；新增故意差异时必须同步矩阵备注 + 断言。

**与 tests/INDEX.md 关系**：Cargo 集成测试（Wave 1/2）提供 L1 确定性保障；本计划的黑盒 CLI 场景是用户视角的补充门。未来应通过一个集成计划一致性检查（自包含 Rust 测试或 CI 步骤，仿照 `tests/compat/matrix_alignment.rs` 的去脚本化做法，而非新建 `scripts/`）保持引用一致；该检查未落地前按 BASELINE_GAP-INTEG-008 记录为未自动校验。

**断言强化标准（Assertion Strengthening Standard）**：为确保 Agent 可解析性、安全隔离验证和 Git 兼容性，所有场景最终应逐步纳入以下可执行断言模式（已在 `cli.commit-status-log`、`cli.cross-cutting-flags`、`cli.tag-basic` 等场景示范）：
- 成功路径：至少一个 `--json`（或 `--machine`）调用 + `python3 -c "import json; d=...; assert d['ok'] is True; assert 'data' in d"`（ndjson 场景用逐行解析）。
- 错误路径：负向命令必须非 0 退出，stderr 捕获验证包含 `LBR-` 稳定码或特定可操作错误文本（例如 "not a libra repository"、"no such"）。
- 状态一致性：关键 mutating 操作后执行 `libra fsck --connectivity-only`（0 退出）或 `libra --json show-ref --heads` 验证 refs 健康。
- 隔离验证：涉及 config/vault/global 的场景，操作后用隔离 `LIBRA_CONFIG_GLOBAL_DB` 或 HOME 执行 `libra config --global list` 验证无本场景残留（或显式检查文件不存在）。
- 故意差异：COMPATIBILITY.md 中的 intentionally-different 行为必须有正面 `test` / `grep` / JSON 断言（例如 worktree remove 后目录仍存在、push 拒绝本地文件 remote）。
- 冲突/历史场景：冲突标记（`<<<<<<<`）、`libra --json status` 中 `data.merge_state.conflicted_paths[]` 非空、--continue 后该字段缺失或为空必须可脚本断言；不得引用 Git-only 顶层 `ls-files`。
- 所有断言必须在 `libra()` 包装下执行，且使用 `$RUN_ROOT` 下的文件/输出，避免依赖主机状态。

本标准随 runner 落地将逐步自动化。当前 PR 贡献新场景时至少应包含 JSON envelope + 1 个 LBR- 错误验证 + fsck。

**本轮系统化补充断言已直接更新以下场景**（示范 + 强化）：
- `cli.commit-status-log`（基础闭环 + JSON + fsck + LBR + 隔离）
- `cli.tag-basic`、`cli.branch-switch-checkout`、`cli.object-readback`
- `cli.merge-rebase-cherry-revert-smoke`、`cli.merge-conflict-continue`
- `cli.reflog-symbolic-ref`、`cli.clean-rm-mv-lfs-basic`
- `cli.cross-cutting-flags`（先前已强化错误 JSON）

其余场景（config/*、init/* 变体、push-local-file-remote-rejected、clone 系列、schema、verify-pack、sha256、open、Wave 3 live.github-* 等）请对照本标准逐一补充相同模式的断言。目标：每个场景的“断言”部分最终都包含可直接在 `libra()` 下执行的 python/shell 检查，而非仅描述性文字。

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

通过标准：`target/debug/libra` 存在且可执行；`--version` 和 `--help` 退出码为 0；格式、lint 通过；**`git` 与 `ssh` 可在 §3.3.1 收窄后的 `PATH` 中解析到**（否则按 §3.3.0 追加其所在目录，或把依赖 git/ssh 的场景标记 skip 并记录原因，而不是当作 libra 行为失败）；`compat_matrix_alignment` 测试通过（兼容矩阵与 Code UI docs 一致性的去脚本化检查）；集成计划场景清单的自动一致性校验仍未落地，按 BASELINE_GAP-INTEG-008 记录为缺口而不是假装已验证。

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

> **关于 §4 的内联 `libra()` 包装**：第 4 章保留了若干可复制的 `libra()` 包装函数；它们必须与本模板语义一致：`PATH` 只能来自 `SAFE_PATH` 或安全默认值，必须显式注入 `TMPDIR="$RUN_ROOT/tmp"`，并且必须继续使用 `env -i` 白名单环境。runner 落地后应把这些副本收敛到同一份 prelude（见 BASELINE_GAP-INTEG-001），避免 N 份副本各自漂移。

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
- [ ] 远端 URL 来自 `gh repo view --json sshUrl`（或有记录的 HTTPS 认证源）
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
| `tests/**` | 对应 wave | 只在其影响 CLI 场景或辅助脚本时纳入 |
| `src/command/{clone,fetch,pull,push,remote,ls_remote}.rs` | 1, 2 | 3，若行为需要真实 GitHub 远端确认 |
| `src/command/{lfs,fsck,cat_file,verify_pack,symbolic_ref,shortlog,describe,open}.rs` | 1 | 2 |

---

## 4. 执行波次

Wave 1 覆盖单仓库、无网络、无外部服务的核心版本管理闭环。

### `cli.config-basic-kv`

目的：覆盖 `config set/get/list/unset` 子命令、位置参数 `key`、位置参数 `value`，以及默认 local scope。

最小步骤：

```bash
SCENARIO="cli.config-basic-kv"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init config-repo
cd config-repo

libra config set user.name "Libra Config Test"
libra config get user.name
libra config list
libra config unset user.name
! libra config get user.name
libra config get --default fallback user.name
```

断言：`set` 后 `get` 输出设置值；`list` 包含 `user.name=Libra Config Test` 或等价 key/value 输出；`unset` 后普通 `get` 按缺失语义非 0 或无值，带 `--default` 返回 fallback。

补充可执行断言（config 家族基础模式）：
- `libra --json config get user.name` 必须返回 `ok:true`，且 `data.value == "Libra Config Test"`。
- `libra --json config list` 解析验证 `data.entries[]` 或等价结构包含本场景设置的 key。
- unset 后 `libra config get --default fallback user.name` 必须输出 fallback 且退出码 0。
- 整个场景操作后，用隔离 `LIBRA_CONFIG_GLOBAL_DB` 执行 `libra config --global list` 不得残留本场景的 user.name（严格隔离验证）。
- 负向 `libra config get 不存在的key` 必须非 0，可选捕获 stderr 验证错误文本或 LBR- 码。

### `cli.config-scopes`

目的：覆盖 `--local`、`--global`、`--system` scope flags。

最小步骤：

```bash
SCENARIO="cli.config-scopes"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
mkdir -p isolated-home
libra init scope-a
libra init scope-b

cd "$RUN_DIR/scope-a"
libra config --local set test.scope local-a
libra config --global set test.scope global-value
libra config --local get test.scope
libra config --global get test.scope

cd "$RUN_DIR/scope-b"
libra config --global get test.scope
! libra config --local get test.scope
! libra config --system list
```

断言：local key 只在当前 repo 可见；global key 在隔离 HOME 下跨 repo 可见；`--system` 当前为移除/拒绝路径，必须非 0 退出并给出不支持或不可用的明确错误；场景不得写入真实用户全局配置。

补充可执行断言：
- 使用隔离 global DB 验证 `--global set` 后在另一个 repo 中 `libra config --global get` 可见，而 `--local` 不可见。
- `! libra config --system list` 的 stderr 必须包含 "不支持" / "system" 或对应 LBR- 错误标识。
- 操作后用隔离 HOME + global DB 再次 `libra config --global list` 验证只有本场景设置的 global key，无其他污染。

### `cli.config-set-input-and-encryption`

目的：覆盖 `set` 子命令的 `--add`、`--encrypt`、`--plaintext`、`--stdin` 参数，以及敏感 key 的保护输入行为。

最小步骤：

```bash
cd "$RUN_DIR/config-repo"

libra config set --add remote.origin.fetch "+refs/heads/*:refs/remotes/origin/*"
libra config set --add remote.origin.fetch "+refs/tags/*:refs/tags/*"
libra config get --all remote.origin.fetch

printf 'stdin-value\n' | libra config set --stdin custom.stdin
libra config get custom.stdin

libra config set --encrypt custom.secret "s3cr3t"
libra config get custom.secret
libra config get --reveal custom.secret

libra config set --plaintext custom.plain "plain-value"
libra config get custom.plain
```

负向步骤：

```bash
cd "$RUN_DIR/config-repo"
! libra config set --encrypt --plaintext custom.bad value
! libra config set --stdin custom.bad value
! libra config set --plaintext vault.env.TEST_SECRET value
```

断言：`--add` 允许同 key 多值，`get --all` 能看到全部值；`--stdin` 去掉末尾换行并保存；`--encrypt` 默认 `get` 不泄露明文，`get --reveal` 才输出明文；`--plaintext` 保存普通明文；互斥/非法组合必须非 0 退出且不写入坏状态。

补充可执行断言：
- `libra --json config get --all remote.origin.fetch` 必须返回 `ok:true`，且 `data.entries[]` 长度 ≥2。
- `--encrypt` 后普通 `libra --json config get custom.secret` 必须成功但不返回明文（或返回 masked）；加 `--reveal` 才返回真实值。
- `--stdin` 后验证值不带末尾换行（`libra config get | wc -l` 验证）。
- 非法组合（如 `--encrypt --plaintext`）必须非 0，且不写入任何配置。
- 操作后用隔离 global DB 验证无泄露。

### `cli.config-get-default-and-patterns`

目的：覆盖 `get` 子命令的 `--all`、`--reveal`、`--regexp`、`-d/--default`，以及 Git 兼容隐藏 flag `--get`、`--get-all`、`--get-regexp`。

最小步骤：

```bash
cd "$RUN_DIR/config-repo"

libra config set user.name "Pattern User"
libra config set user.email "pattern@example.invalid"
libra config set core.editor vim

libra config get user.name
libra config --get user.name
libra config get --default fallback missing.key
libra config get -d fallback-short missing.short
libra config get --regexp '^user\\.'
libra config --get-regexp '^user\\.'
libra config --get-all remote.origin.fetch
```

断言：普通 get 与 `--get` 输出一致；缺失 key 带 default 时退出码为 0 并输出 fallback；regexp 只输出匹配 key；`--get-all` 覆盖多值 key。隐藏 flag 是兼容 invocation 覆盖，不要求出现在 `config --help`。

补充可执行断言（Agent 非常常用）：
- `libra --json config get --default fallback missing.key` 必须 `ok:true`，且 `data.value == "fallback"`、`data.default_applied == true`。
- `libra --json config --get-regexp '^user\.'` 返回 `data.entries[]`，所有 entry 的 `key` 以 `user.` 开头。
- 普通 `libra --json config get user.name` 与 `libra --json config --get user.name` 结果等价。
- 非法 `--default` 与非 get 组合必须失败。
- 验证 `--json` 输出结构稳定（"ok", "data" 字段存在）。

### `cli.config-list-variants`

目的：覆盖 `list` 子命令的 `--name-only`、`--show-origin`、`--vault`、`--ssh-keys`、`--gpg-keys`，以及 Git 兼容 `--list` / `-l` / `--show-origin`。

最小步骤：

```bash
cd "$RUN_DIR/config-repo"

libra config list
libra config -l
libra config --list
libra config list --name-only
libra config list --show-origin
libra config --list --show-origin
libra config list --vault
libra config list --ssh-keys
libra config list --gpg-keys
```

断言：三种 list 入口均成功；`--name-only` 只输出 key 名；`--show-origin` 输出 scope/origin 信息；vault/ssh/gpg 专项列表在无记录时输出明确空状态，在已有记录时只输出公钥或 key 名称，不输出私钥、root token 或 unseal key。

补充可执行断言：
- `libra --json config list --name-only` 返回 `data.entries[]`，所有 entry 只暴露 `key`，`value` 为空或不存在。
- `libra --json config list --show-origin` 每个条目包含 origin/scope 信息。
- `libra --json config list --vault`（无 vault 记录时）成功且 data 为空或明确空状态。
- `libra config list --ssh-keys` / `--gpg-keys` 输出不得包含私钥材料。
- 操作后用隔离 global DB 验证无全局污染。

### `cli.config-unset-compat-flags`

目的：覆盖 `unset --all` 子命令参数，以及 Git 兼容隐藏 flag `--unset`、`--unset-all`。

最小步骤：

```bash
cd "$RUN_DIR/config-repo"

libra config set temp.single value
libra config --unset temp.single
! libra config get temp.single

libra config set --add temp.multi one
libra config set --add temp.multi two
libra config unset --all temp.multi
! libra config get --all temp.multi

libra config set --add temp.legacy one
libra config set --add temp.legacy two
libra config --unset-all temp.legacy
! libra config --get-all temp.legacy
```

断言：单值 unset 和 all unset 都能通过后续 get 观察到删除效果；legacy hidden flags 直接 invocation 可用，但不要求 help 展示。

补充可执行断言：
- `libra --json config set temp.single value && libra --json config --unset temp.single` 后 `libra --json config get temp.single` 必须非 0 或 data 为空。
- 多值场景：`--unset-all` 后 `--json get --all` 返回空列表。
- 验证 legacy `--unset-all` 与现代 `unset --all` 行为等价。
- 操作全程使用隔离 global DB。

### `cli.config-import-path-edit`

目的：覆盖 `import`、`path`、`edit` 子命令，以及 Git 兼容隐藏 flag `--import`。

最小步骤：

```bash
SCENARIO="cli.config-import-path-edit"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
# Dynamic git bin resolution
SAFE_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
GIT_BIN="$(command -v git || true)"
case ":$SAFE_PATH:" in *":$(dirname "${GIT_BIN:-/usr/bin/git}"):"*) ;; *)
  [ -n "$GIT_BIN" ] && SAFE_PATH="$SAFE_PATH:$(dirname "$GIT_BIN")" ;; esac

gitfix() {
  env -i PATH="$SAFE_PATH" HOME="$RUN_ROOT/home" \
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
    TMPDIR="$RUN_ROOT/tmp" LANG=C LC_ALL=C git "$@"
}
libra() {
  env -i \
    PATH="$SAFE_PATH" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
mkdir git-config-source
cd git-config-source
gitfix init
gitfix config user.name "Imported Git User"
gitfix config user.email "imported@example.invalid"

libra init libra-import-target
cd libra-import-target
libra config import
libra config get user.name
libra config get user.email
libra config path

cd "$RUN_DIR/git-config-source"
libra init libra-import-legacy
cd libra-import-legacy
libra config --import

! libra config edit
```

断言：`config import` / `--import` 从 Git config 导入当前 scope 可接受的配置项，不接受任意文件路径作为参数；`path` 输出当前 scope 的 config DB 路径且路径存在；`edit` 当前因 SQLite 存储不支持文本编辑，必须非 0 退出并提示使用 `set/unset/list`。

补充可执行断言：
- `libra --json config path` 成功且 data.path 指向 .libra/libra.db 或 global DB。
- `libra --json config import` 成功后，`libra --json config get user.name` 返回从 Git fixture 导入的值。
- `! libra config edit` 必须非 0，stderr 包含 "set/unset/list" 或等价提示。
- 验证 import 只导入当前 scope 可接受的 key。

### `cli.config-key-generation`

目的：覆盖 `generate-ssh-key --remote <NAME>` 和 `generate-gpg-key --name <NAME> --email <EMAIL> --usage <KIND>`。

最小步骤：

```bash
SCENARIO="cli.config-key-generation"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
mkdir -p keygen-home
libra init keygen-repo
cd keygen-repo
libra config set user.name "Keygen User"
libra config set user.email "keygen@example.invalid"
libra remote add origin git@example.invalid:owner/repo.git

libra config generate-ssh-key --remote origin
libra config get vault.ssh.origin.pubkey

libra config generate-gpg-key --name "Signing User" --email "signing@example.invalid" --usage signing
libra config get vault.gpg.pubkey
libra config get vault.signing

libra config generate-gpg-key --name "Encrypt User" --email "encrypt@example.invalid" --usage encrypt
libra config get vault.gpg.encrypt.pubkey
```

负向步骤：

```bash
cd "$RUN_DIR/keygen-repo"
! libra config --global generate-ssh-key --remote origin
! libra config generate-ssh-key --remote bad.name
! libra config generate-ssh-key --remote no-such-remote
! libra config --global generate-gpg-key --name Bad --email bad@example.invalid
! libra config generate-gpg-key --usage archive
```

断言：SSH key 生成要求 remote 存在且 remote 名只含 `[a-zA-Z0-9_-]`；生成后 public key 可通过 config 读取，private key 只以 vault-encrypted config key 存在且不得出现在日志中；GPG signing usage 写入 `vault.gpg.pubkey` 并启用 `vault.signing`，encrypt usage 写入 `vault.gpg.encrypt.pubkey`；global key generation 和非法 usage 必须失败且无本地副作用。

补充可执行断言（安全关键场景）：
- 生成 SSH key 后，`libra --json config get vault.ssh.origin.pubkey` 必须 `ok:true` 且包含公钥内容。
- 生成 GPG signing key 后，`libra --json config get vault.signing` 必须显示启用状态。
- 验证 private key 绝不泄露：`libra config list --vault` 输出中不得出现私钥材料（仅 pubkey 或 key 名称）。
- 负向 `--global generate-ssh-key` 必须非 0，且错误提示隔离要求。
- 非法 usage（如 archive）必须失败，stderr 包含 "usage" 相关错误或 LBR- 码。
- 操作全程使用隔离 HOME + global DB，结束后验证真实用户 vault 未被触碰（通过检查隔离环境外无新 key）。

### `cli.config-git-compat-mode`

目的：集中覆盖 `ConfigArgs` 上的 Git 兼容隐藏 flag 与位置参数翻译路径。

最小步骤：

```bash
cd "$RUN_DIR/config-repo"

libra config user.compat value-from-positional
libra config --get user.compat
libra config --add user.compat second-value
libra config --get-all user.compat
libra config --get-regexp '^user\\.'
libra config --list
libra config -l
libra config --list --show-origin
libra config --unset user.compat
libra config --unset-all remote.origin.fetch
libra config --get -d fallback missing.compat
libra config --get --default fallback-long missing.compat.long
libra config --import
```

负向步骤：

```bash
! libra config --default fallback user.bad-default value
! libra config init value
! libra config --import user.name
```

断言：位置参数 `key valuepattern` 的默认模式等价于 set；`--get` / `--get-all` / `--get-regexp` / `--list` / `-l` / `--show-origin` / `--add` / `--unset` / `--unset-all` / `--import` / `-d` / `--default` 均至少有一个直接 invocation 覆盖；`--default` 只能与 get 类模式组合；不含 section 的 key 非 0 退出并对 `init` / `clone` 给出“这是顶层命令”的提示。

补充可执行断言：
- `libra --json config --get user.compat` 必须 `ok:true`，且 `data.value == "value-from-positional"`。
- `libra --json config --get-all user.compat` 必须返回 `data.entries[]`，且包含 `value-from-positional` 与 `second-value`。
- `libra --json config --list --show-origin` 必须返回 `data.entries[]`，每条包含 key/value 与 origin 或 scope 字段。
- `libra config --get --default fallback-long missing.compat.long` 必须输出 fallback-long 且退出码为 0。
- 负向 `--default` 非 get 模式、`config init value`、`--import user.name` 均必须非 0，stderr 包含可识别错误文本或 LBR- 稳定码。

### `cli.init-basic`

目的：验证 `libra init` 的默认初始化路径创建可用普通仓库，并作为所有 init 参数矩阵的最小基线。

最小步骤：

```bash
SCENARIO="cli.init-basic"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}

libra init repo
test -f repo/.libra/libra.db
test -d repo/.libra/objects
cd repo
libra status
libra db status
libra fsck --connectivity-only
```

负向步骤：

```bash
cd "$RUN_DIR"
! libra init repo
```

断言：初始化命令退出码为 0；`.libra/libra.db` 和对象目录存在；`status`、`db status`、`fsck --connectivity-only` 可在新仓库中执行；重复初始化同一路径必须非 0 或明确提示已有仓库，且不得破坏既有 `.libra` 布局。

补充可执行断言：
- `libra --json status` 必须返回 `ok:true`，且 `data.head.type == "branch"`、`data.head.name` 指向初始分支。
- `libra --json db status` 必须返回 `ok:true`，且 `data.current_version` / `data.latest_version` / `data.state` 可解析。
- 重复 init 失败时 stderr 必须包含已有仓库/目标路径相关错误或 LBR- 稳定码。

### `cli.init-directory-and-quiet`

目的：覆盖位置参数 `DIRECTORY`、短参数 `-q` 和长参数 `--quiet`。

最小步骤：

```bash
SCENARIO="cli.init-directory-and-quiet"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}

libra init nested/repo
test -f nested/repo/.libra/libra.db
test -d nested/repo/.libra/objects
cd nested/repo
libra status

cd "$RUN_DIR"
libra init -q quiet-short >quiet-short.out 2>quiet-short.err
libra init --quiet quiet-long >quiet-long.out 2>quiet-long.err
test -f quiet-short/.libra/libra.db
test -f quiet-long/.libra/libra.db
```

断言：`DIRECTORY` 可创建不存在的嵌套目录；`-q` / `--quiet` 退出码为 0；quiet 模式不输出普通初始化 banner，但错误仍应写入 stderr；quiet 仓库进入目录后 `status` 可执行。

补充可执行断言：
- `libra --json init -q quiet-json-repo` 成功（ok:true），且 `test -f quiet-json-repo/.libra/libra.db`。
- quiet 模式下 stdout 为空（或极小），但 stderr 可包含初始化信息。
- 操作后 `libra fsck --connectivity-only` 在 quiet 仓库中通过。
- 所有 init 使用隔离 LIBRA_CONFIG_GLOBAL_DB，结束后验证无全局污染。

### `cli.init-branch-and-format-options`

目的：覆盖 `-b <branch>`、`--initial-branch <branch>`、`--object-format <format>` 和 `--ref-format <format>`。

最小步骤：

```bash
SCENARIO="cli.init-branch-and-format-options"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init -b develop init-branch-short
cd init-branch-short
libra branch
libra status

cd "$RUN_DIR"
libra init --initial-branch trunk init-branch-long
cd init-branch-long
libra branch

cd "$RUN_DIR"
libra init --object-format sha1 object-sha1
cd object-sha1
libra config get core.objectformat

cd "$RUN_DIR"
libra init --object-format sha256 object-sha256
cd object-sha256
libra config get core.objectformat

cd "$RUN_DIR"
libra init --ref-format strict ref-strict
cd ref-strict
libra config get core.initrefformat

cd "$RUN_DIR"
libra init --ref-format filesystem ref-filesystem
cd ref-filesystem
libra config get core.initrefformat
```

负向步骤：

```bash
cd "$RUN_DIR"
! libra init --object-format sha265 bad-object-format
! libra init --ref-format unknown bad-ref-format
! libra init -b "bad branch" bad-branch-name
```

断言：短/长 initial branch 参数都能通过 `branch` 或等价公开命令观察到初始分支；`core.objectformat` 分别为 `sha1` / `sha256`；`core.initrefformat` 分别为 `strict` / `filesystem`；非法 object/ref format 或非法分支名必须非 0 退出，并给出可理解的参数错误或修复提示。

补充可执行断言（对象格式与 ref 格式关键）：
- `libra --json config get core.objectformat` 在 sha256 仓库中验证值为 "sha256"。
- `libra --json init --object-format sha256 sha256-json` 成功后用 `libra --json cat-file -p HEAD` 验证对象 ID 格式（64 位 hex）。
- 非法 `--object-format sha265` 的错误必须非 0，且包含 "unsupported object format" 或 LBR- 相关标识（捕获 stderr 验证）。
- 所有 init 后立即 `libra fsck --connectivity-only` 通过。

### `cli.init-bare-and-shared`

目的：覆盖 `--bare` 与 `--shared <MODE>`。

最小步骤：

```bash
SCENARIO="cli.init-bare-and-shared"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init --bare bare-repo
test -f bare-repo/libra.db
test -d bare-repo/objects
test ! -e bare-repo/.libra
cd bare-repo
! libra status

cd "$RUN_DIR"
libra init --shared false shared-false
libra init --shared true shared-true
libra init --shared umask shared-umask
libra init --shared group shared-group
libra init --shared all shared-all
libra init --shared world shared-world
libra init --shared everybody shared-everybody
libra init --shared 0770 shared-octal
```

负向步骤：

```bash
cd "$RUN_DIR"
! libra init --shared invalid shared-invalid
! libra init --shared 8888 shared-bad-octal
```

断言：bare 仓库把 `libra.db` 和 `objects` 放在目标目录本身，不创建普通工作区 `.libra/`；普通工作区命令在 bare 仓库中应按当前 CLI 语义失败或提示不适用；所有支持的 shared mode 退出码为 0；非法 shared mode 非 0 退出并列出支持值。Unix 平台可补充检查 shared 仓库文件权限；跨平台默认只要求 CLI 可观察仓库状态正确。

补充可执行断言：
- bare repo 后 `test -f bare-repo/libra.db && test ! -e bare-repo/.libra`。
- 在 bare repo 中 `libra status` 必须非 0。
- 所有合法 --shared 模式创建后，`libra db --json status` 成功，且 `data.current_version` / `data.latest_version` 可解析。
- 非法 --shared 值（invalid、8888）的错误必须非 0，且 stderr 列出支持的 mode。
- 操作后在 shared 仓库执行 `libra fsck --connectivity-only` 通过。

### `cli.init-template`

目的：覆盖 `--template <template-directory>`。

最小步骤：

```bash
SCENARIO="cli.init-template"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
mkdir -p template/info template/hooks template/custom
printf 'ignored-by-template\n' > template/info/exclude
printf '#!/bin/sh\nexit 0\n' > template/hooks/pre-commit.sh
printf 'sentinel\n' > template/custom/sentinel.txt

libra init --template template templated-repo
test -f templated-repo/.libra/info/exclude
test -f templated-repo/.libra/hooks/pre-commit.sh
test -f templated-repo/.libra/custom/sentinel.txt
cd templated-repo
libra status
```

负向步骤：

```bash
cd "$RUN_DIR"
! libra init --template missing-template bad-template-repo
```

断言：模板目录内容被复制到目标仓库的 Libra 存储根；模板不会阻止 `objects/pack`、`objects/info`、`libra.db` 等必要布局创建；不存在或非目录 template 路径必须失败并在错误中标明路径。

补充可执行断言：
- 模板中的文件（exclude、pre-commit.sh、sentinel.txt）必须出现在 `templated-repo/.libra/` 对应位置。
- `libra --json init --template template templated-json` 成功后验证 `ok:true`。
- 缺失 template 目录时错误必须非 0，stderr 包含路径。
- 转换后的仓库 `libra fsck --connectivity-only` 通过。

### `cli.init-from-git-repository`

目的：覆盖 `--from-git-repository <path>`，验证本地 Git 仓库转换为 Libra 仓库的 CLI 可观察行为。

最小步骤：

```bash
SCENARIO="cli.init-from-git-repository"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
# Dynamic git bin resolution
SAFE_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
GIT_BIN="$(command -v git || true)"
case ":$SAFE_PATH:" in *":$(dirname "${GIT_BIN:-/usr/bin/git}"):"*) ;; *)
  [ -n "$GIT_BIN" ] && SAFE_PATH="$SAFE_PATH:$(dirname "$GIT_BIN")" ;; esac

gitfix() {
  env -i PATH="$SAFE_PATH" HOME="$RUN_ROOT/home" \
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
    TMPDIR="$RUN_ROOT/tmp" LANG=C LC_ALL=C git "$@"
}
libra() {
  env -i \
    PATH="$SAFE_PATH" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
mkdir git-source
cd git-source
gitfix init
gitfix config user.name "Git Fixture"
gitfix config user.email "git-fixture@example.invalid"
printf 'from git\n' > README.md
gitfix add README.md
gitfix commit -m "fixture: initial"

cd "$RUN_DIR"
libra init --from-git-repository git-source converted
cd converted
libra status
libra log --oneline
test -f README.md
```

负向步骤：

```bash
cd "$RUN_DIR"
! libra init --from-git-repository missing-source converted-missing
```

断言：转换后的 Libra 仓库可执行 `status` 和 `log`；至少一个来自 Git fixture 的文件、提交或 ref 可通过 `libra` 命令观察；缺失 source 路径非 0 退出并提示有效 Git 仓库要求。这里的 Git 仓库只作为本地 fixture，不进入 GitHub live 语义。

补充可执行断言：
- 转换后 `libra --json status` 和 `libra --json log -n 1` 均 `ok:true`，且 `data.commits[]` 非空。
- `test -f converted/README.md` 且内容与 Git fixture 一致。
- 转换后的仓库 `libra fsck --connectivity-only` 通过。
- 缺失 source 时错误必须非 0，包含 "valid Git repository" 或等价提示。
- 使用 gitfix() 创建的 fixture 必须严格隔离（无主机 GIT_* 污染）。

### `cli.init-vault`

目的：覆盖 `--vault <bool>`，并验证默认 vault 行为与显式关闭行为。

最小步骤：

```bash
SCENARIO="cli.init-vault"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
mkdir -p home-vault home-no-vault

libra init --vault true vault-repo
cd vault-repo
test -f .libra/vault.db
libra config get vault.signing

cd "$RUN_DIR"
libra init --vault false no-vault-repo
cd no-vault-repo
test ! -f .libra/vault.db
libra config get vault.signing
```

断言：`--vault true` 创建 repo-local `vault.db` 并使 `vault.signing` 可通过 `config get` 观察；`--vault false` 不创建 `vault.db`，`vault.signing` 为关闭值；场景必须隔离 `HOME`，不得读写开发者真实 `~/.libra/vault-keys`。

补充可执行断言（安全关键）：
- `--vault true` 后 `test -f .libra/vault.db` 且 `libra --json config get vault.signing` 成功。
- `--vault false` 后 `test ! -f .libra/vault.db`。
- 使用隔离 HOME 执行后，验证真实 `~/.libra/vault-keys`（或 global vault）未被创建/修改。
- `libra --json config get vault.signing` 在 false 情况下返回关闭值。
- 操作后 `libra fsck` 通过。

### `libra init` 参数覆盖表

| 参数 | 场景 ID | 关键断言 |
|---|---|---|
| `DIRECTORY` | `cli.init-directory-and-quiet` | 目标目录和 `.libra/libra.db` 被创建 |
| `-q` / `--quiet` | `cli.init-directory-and-quiet` | 成功但不输出普通 banner |
| `-b` / `--initial-branch` | `cli.init-branch-and-format-options` | 初始分支可通过公开命令观察 |
| `--object-format` | `cli.init-branch-and-format-options` | `core.objectformat` 为 `sha1` / `sha256`，非法值失败 |
| `--ref-format` | `cli.init-branch-and-format-options` | `core.initrefformat` 为 `strict` / `filesystem`，非法值失败 |
| `--bare` | `cli.init-bare-and-shared` | 存储根为目标目录本身，无普通 `.libra/` 工作区布局 |
| `--shared` | `cli.init-bare-and-shared` | 支持值成功，非法值失败并提示支持值 |
| `--template` | `cli.init-template` | 模板内容复制到 Libra 存储根，缺失路径失败 |
| `--from-git-repository` | `cli.init-from-git-repository` | 本地 Git fixture 的文件/提交/ref 可通过 Libra CLI 观察 |
| `--vault` | `cli.init-vault` | `vault.db` 与 `vault.signing` 状态符合显式 bool |

### `cli.commit-status-log`

目的：覆盖 `status`、`add`、`commit`、`log` 的最小提交闭环，以及脚本常用输出格式、自动暂存、消息来源和失败路径。

最小步骤：

```bash
SCENARIO="cli.commit-status-log"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init history-repo
cd history-repo

libra config set user.name "Libra Test"
libra config set user.email "libra-test@example.invalid"

printf 'hello\n' > hello.txt
libra status --short
libra add --dry-run hello.txt
libra add hello.txt
libra status --porcelain
libra commit -m "test: initial commit"
libra status --exit-code
libra log --oneline
libra log -n 1 --name-status --grep "initial" --author "Libra Test"

printf 'from file\n' > message.txt
printf 'tracked\n' > tracked.txt
libra add tracked.txt
libra commit -F message.txt --signoff

printf 'tracked update\n' >> tracked.txt
libra commit -a -m "test: auto stage tracked update"
libra commit --allow-empty -m "test: empty marker"
libra commit --amend --no-edit
libra log --stat -n 3
```

负向步骤：

```bash
cd "$RUN_DIR/history-repo"
! libra commit -m "test: no staged changes"
! libra commit --conventional -m "not conventional"

printf 'dirty\n' > dirty.txt
! libra status --exit-code
rm dirty.txt
```

断言：`add --dry-run` 不写入 index；`add` 后 `status --porcelain` 能看到 staged 文件；`commit -m` / `commit -F` / `commit -a` / `commit --allow-empty` / `commit --amend --no-edit` 均按预期创建或更新提交；`status --exit-code` 在干净工作区退出码为 0、在 dirty 工作区非 0；`log --oneline`、`log --name-status`、`log --stat` 能观察到对应提交、作者、消息和文件变化；缺少 staged change 或 conventional 校验失败必须非 0 且不产生新提交。

补充可执行断言（本场景为基础，推荐所有后续场景复用模式）：
- 每次 commit 后立即 `libra --json status` + python 断言 `ok:true` 且 data 反映干净或 dirty 状态。
- `libra --json log -n 3` 验证 `data.commits[]` 非空，commit 包含 hash/subject 或等价消息字段，且作者匹配配置的 user.name。
- 关键 commit 后执行 `libra fsck --connectivity-only` 必须 0 退出。
- 负向 conventional commit 失败的 stderr 必须包含 "conventional" 或对应 LBR- 错误码（通过 `2>&1 | cat` 捕获验证）。
- `libra --json commit --allow-empty -m "json empty"` 成功后验证 envelope + 新 commit 在 `libra --json log -n 1` 中出现。
- 操作全程使用隔离的 `LIBRA_CONFIG_GLOBAL_DB`，结束后用该 DB 执行 `libra config list --global` 不得残留本场景的临时 key。

### `libra status/add/commit/log` 参数覆盖表

| 参数或子命令 | 场景 ID | 关键断言 |
|---|---|---|
| `status` | `cli.commit-status-log` | 默认状态可执行，干净/dirty 状态可观察 |
| `status --short` | `cli.commit-status-log` | untracked 或 staged path 以短格式出现 |
| `status --porcelain` | `cli.commit-status-log` | 输出适合脚本断言的机器可读状态 |
| `status --exit-code` | `cli.commit-status-log` | 干净为 0，dirty 为非 0 |
| `add <pathspec>` | `cli.commit-status-log` | 指定文件被加入 index 并可由 status 观察 |
| `add --dry-run` | `cli.commit-status-log` | 预览输出不改变 index |
| `commit -m` | `cli.commit-status-log` | 提交消息进入 log |
| `commit -F` | `cli.commit-status-log` | 从文件读取提交消息 |
| `commit -a` | `cli.commit-status-log` | 已跟踪文件修改被自动暂存并提交 |
| `commit --allow-empty` | `cli.commit-status-log` | 空提交成功并出现在 log 中 |
| `commit --amend --no-edit` | `cli.commit-status-log` | 最后一个提交被替换且消息复用 |
| `commit --conventional` | `cli.commit-status-log` | 非 conventional 消息失败且不写入提交 |
| `commit --signoff` | `cli.commit-status-log` | 提交消息包含 Signed-off-by trailer |
| `log --oneline` | `cli.commit-status-log` | 输出短 hash 和提交主题 |
| `log -n` | `cli.commit-status-log` | 输出数量受限制 |
| `log --author` / `--grep` | `cli.commit-status-log` | 只返回匹配作者或消息的提交 |
| `log --name-status` / `--stat` | `cli.commit-status-log` | 文件变化摘要可观察 |

### `cli.branch-switch-checkout`

目的：覆盖 `branch`、`switch`、`checkout` 的分支创建、切换、detached HEAD、兼容 alias、分支重命名/删除和路径恢复行为。

最小步骤：

```bash
SCENARIO="cli.branch-switch-checkout"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init branch-repo
cd branch-repo

libra config set user.name "Libra Branch Test"
libra config set user.email "branch@example.invalid"
printf 'base\n' > base.txt
libra add base.txt
libra commit -m "test: branch base"

libra branch --show-current
libra branch feature/cli-smoke
libra branch feature/from-main main
libra branch --list
libra switch feature/cli-smoke
printf 'feature\n' > feature.txt
libra add feature.txt
libra commit -m "test: feature branch"
libra checkout main
libra checkout -b compat-checkout
libra checkout main
libra switch -c switch-created main
libra switch main

BASE_COMMIT="$(libra rev-parse HEAD)"
libra switch --detach "$BASE_COMMIT"
libra rev-parse --abbrev-ref HEAD
libra switch main

libra branch -m feature/from-main feature/renamed
libra branch -d feature/renamed
libra branch -D feature/cli-smoke

printf 'dirty\n' > base.txt
libra checkout -- base.txt
grep 'base' base.txt
libra branch

# Verify branch list JSON output
libra --json branch --list >branch-list.json
python3 -c "import json; d=json.load(open('branch-list.json')); assert d['ok'] is True; assert isinstance(d['data'].get('branches'), list)"
```

负向步骤：

```bash
cd "$RUN_DIR/branch-repo"
! libra branch "bad branch"
! libra switch no-such-branch
! libra checkout no-such-branch
! libra branch -d no-such-branch
```

断言：`branch --show-current` 输出当前分支；从 HEAD 和指定 base 创建分支成功；`switch` / `checkout` 都能切换到已存在分支；`checkout -b` 与 `switch -c` 都能创建并切换分支；detached HEAD 下 `rev-parse --abbrev-ref HEAD` 输出 detached 语义或 `HEAD`；`branch -m` 后旧名消失、新名可列出；安全删除已合并分支成功，强制删除未合并分支成功；`checkout -- <path>` 能恢复工作区文件；非法分支名、缺失分支或缺失删除目标必须非 0 退出并保留现有分支状态。

补充可执行断言：
- 关键分支操作后 `libra --json branch --list` 解析验证新分支出现。
- detached 后 `libra symbolic-ref HEAD` 必须失败（或输出 "HEAD" 且非 ref），这是 Libra/Git 符号引用限制的验证点。
- `libra --json switch main` 成功后验证 `ok:true`。
- 所有分支操作后 `libra fsck` 通过；删除分支后 `libra --json show-ref --heads` 的 `data.entries[]` 不再包含已删分支。

### `libra branch/switch/checkout` 参数覆盖表

| 参数或子命令 | 场景 ID | 关键断言 |
|---|---|---|
| `branch <name>` | `cli.branch-switch-checkout` | 从 HEAD 创建本地分支 |
| `branch <name> <commit>` | `cli.branch-switch-checkout` | 从指定 base 创建分支 |
| `branch --list` | `cli.branch-switch-checkout` | 已创建分支可列出 |
| `branch --show-current` | `cli.branch-switch-checkout` | 当前分支名可观察 |
| `branch -m <old> <new>` | `cli.branch-switch-checkout` | 分支重命名后新名可用、旧名不可用 |
| `branch -d` / `branch -D` | `cli.branch-switch-checkout` | 安全删除和强制删除路径均覆盖 |
| `switch <branch>` | `cli.branch-switch-checkout` | 切换到现有分支 |
| `switch -c <branch> <start>` | `cli.branch-switch-checkout` | 创建并切换到新分支 |
| `switch --detach <commit>` | `cli.branch-switch-checkout` | HEAD 进入 detached 状态 |
| `checkout <branch>` | `cli.branch-switch-checkout` | 兼容分支切换路径可用 |
| `checkout -b <branch>` | `cli.branch-switch-checkout` | 兼容创建并切换路径可用 |
| `checkout -- <pathspec>` | `cli.branch-switch-checkout` | 路径恢复行为可观察 |

### `cli.restore-reset-diff`

目的：覆盖 `diff`、`restore`、`reset` 的工作区修改、staged 修改、路径级恢复、HEAD 移动和输出格式。

最小步骤：

```bash
SCENARIO="cli.restore-reset-diff"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init restore-repo
cd restore-repo

libra config set user.name "Libra Restore Test"
libra config set user.email "restore@example.invalid"
mkdir -p src
printf 'one\n' > src/app.txt
libra add src/app.txt
libra commit -m "test: restore base"

printf 'two\n' > src/app.txt
libra diff src/app.txt
libra diff --name-only
libra diff --stat
libra add src/app.txt
libra diff --staged
libra diff --staged --name-status
libra restore --staged src/app.txt
libra status --short
libra restore --worktree src/app.txt
grep 'one' src/app.txt

printf 'two\n' > src/app.txt
libra add src/app.txt
libra reset HEAD -- src/app.txt
libra status --short
libra add src/app.txt
libra commit -m "test: restore second"
SECOND_COMMIT="$(libra rev-parse HEAD)"
libra diff --old HEAD~1 --new "$SECOND_COMMIT" --numstat
libra reset --soft HEAD~1
libra status --short
libra reset --mixed HEAD
libra restore --worktree src/app.txt
grep 'one' src/app.txt

printf 'three\n' > src/app.txt
libra add src/app.txt
libra commit -m "test: restore third"
libra reset --hard HEAD~1
grep 'one' src/app.txt
```

负向步骤：

```bash
cd "$RUN_DIR/restore-repo"
! libra restore --source no-such-revision src/app.txt
! libra reset no-such-revision
! libra diff --old no-such-revision --new HEAD
```

断言：unstaged diff、staged diff、name-only、name-status、numstat 和 stat 输出都能反映同一修改；`restore --staged` 只取消暂存，不丢弃工作区修改；`restore --worktree` 恢复工作区内容；路径级 `reset HEAD -- <path>` 只影响 index；`reset --soft` 保留 index/工作区变化，`reset --mixed` 重置 index，`reset --hard` 重置 HEAD/index/工作区；无效 revision 必须失败且不改变当前 HEAD。

补充可执行断言：
- `libra --json diff --staged` 和 `libra --json diff` 必须返回结构化数据（files 或 changes）。
- 关键 reset/restore 后 `libra --json status` 验证状态正确（staged / unstaged）。
- 每次重置后 `libra fsck --connectivity-only` 通过。
- `libra --json reset --hard HEAD~1` 成功后验证 HEAD 回退且工作区文件恢复。
- 负向 `libra restore --source no-such-rev` 必须非 0，stderr 包含错误路径或 LBR- 标识。

### `libra diff/restore/reset` 参数覆盖表

| 参数或子命令 | 场景 ID | 关键断言 |
|---|---|---|
| `diff <pathspec>` | `cli.restore-reset-diff` | unstaged 工作区修改可见 |
| `diff --staged` | `cli.restore-reset-diff` | staged 修改可见 |
| `diff --old --new` | `cli.restore-reset-diff` | 两个 revision 间差异可见 |
| `diff --name-only` / `--name-status` | `cli.restore-reset-diff` | 文件名和状态摘要可用于脚本断言 |
| `diff --stat` / `--numstat` | `cli.restore-reset-diff` | 文件级统计输出可见 |
| `restore --staged <path>` | `cli.restore-reset-diff` | index 恢复到 HEAD，工作区保持修改 |
| `restore --worktree <path>` | `cli.restore-reset-diff` | 工作区文件恢复到 index 或 source 内容 |
| `restore --source <rev>` | `cli.restore-reset-diff` | source revision 不存在时失败且不改写文件 |
| `reset HEAD -- <path>` | `cli.restore-reset-diff` | 路径级 reset 只取消暂存 |
| `reset --soft` | `cli.restore-reset-diff` | 只移动 HEAD，保留 index/工作区 |
| `reset --mixed` | `cli.restore-reset-diff` | 移动 HEAD 并重置 index |
| `reset --hard` | `cli.restore-reset-diff` | HEAD、index、工作区全部回到目标 revision |

### `cli.stash-bisect-worktree`

目的：覆盖兼容性差异较大的 `stash`、`bisect`、`worktree` 命令面，重点验证状态保存/恢复、二分会话状态和 Libra worktree remove 默认保留目录的差异语义。

最小步骤：

```bash
SCENARIO="cli.stash-bisect-worktree"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init workflow-repo
cd workflow-repo

libra config set user.name "Libra Workflow Test"
libra config set user.email "workflow@example.invalid"
printf '0\n' > number.txt
libra add number.txt
libra commit -m "test: workflow base"

printf 'stash change\n' >> number.txt
libra stash push -m "wip: tracked change"
libra stash list
libra stash show
libra stash apply
libra status --short
libra restore --worktree number.txt
libra stash pop
libra status --short

# Test stash branch (checkout new branch and apply stash)
printf 'stash branch change\n' >> number.txt
libra stash push -m "wip: stash branch"
libra stash branch stash-branch-test
libra branch --show-current | grep -q 'stash-branch-test'
libra switch main
libra branch -D stash-branch-test

libra stash clear --force
libra stash list

GOOD_COMMIT="$(libra rev-parse HEAD)"
printf '1\n' > number.txt
libra add number.txt
libra commit -m "test: bisect middle"
printf '2\n' > number.txt
libra add number.txt
libra commit -m "test: bisect bad"
BAD_COMMIT="$(libra rev-parse HEAD)"
libra bisect start "$BAD_COMMIT" --good "$GOOD_COMMIT"
libra bisect view
libra bisect bad
libra bisect good "$GOOD_COMMIT"
libra bisect log
libra bisect reset

# Test bisect skip
libra bisect start "$BAD_COMMIT" --good "$GOOD_COMMIT"
libra bisect skip
libra bisect reset

libra worktree add "$RUN_ROOT/repos/workflow-linked"
libra worktree list
libra worktree lock "$RUN_ROOT/repos/workflow-linked" --reason "integration smoke"
libra worktree list
libra worktree unlock "$RUN_ROOT/repos/workflow-linked"
libra worktree move "$RUN_ROOT/repos/workflow-linked" "$RUN_ROOT/repos/workflow-moved"
libra worktree remove "$RUN_ROOT/repos/workflow-moved"
test -d "$RUN_ROOT/repos/workflow-moved"
libra worktree prune

# Test worktree remove --delete-dir
libra worktree add "$RUN_ROOT/repos/workflow-to-delete"
libra worktree remove --delete-dir "$RUN_ROOT/repos/workflow-to-delete"
test ! -d "$RUN_ROOT/repos/workflow-to-delete"

# Verify JSON outputs for AI Agent readability
libra --json stash list >stash-list.json
python3 -c "import json; d=json.load(open('stash-list.json')); assert d['ok'] is True; assert isinstance(d['data'].get('entries') or d['data'].get('stashes') or [], list)"
libra --json worktree list >worktree-list.json
python3 -c "import json; d=json.load(open('worktree-list.json')); assert d['ok'] is True; assert isinstance(d['data'].get('worktrees') or d['data'].get('entries') or [], list)"
```

负向步骤：

```bash
cd "$RUN_DIR/workflow-repo"
! libra stash pop stash@{999}
! libra bisect bad no-such-revision
! libra worktree remove "$RUN_ROOT/repos/no-such-worktree"
```

断言：`stash push` 保存 tracked 修改并清理工作区；`stash list` / `stash show` 能观察 stash 条目；`stash apply` 保留 stash，`stash pop` 应用并删除 stash；`stash clear --force` 清空列表；`bisect start <bad> --good <good>` 建立会话，`view` / `log` 能观察状态，`bad` / `good <rev>` 推进会话，`reset` 恢复原始 HEAD；`worktree add` 注册 linked worktree，`list` 显示路径，`lock --reason` / `unlock` 更新锁状态，`move` 更新路径，`remove` 默认只注销登记且保留目录，`prune` 可执行；非法 stash ref、非法 revision 和缺失 worktree 必须失败且不破坏已有仓库状态。

补充可执行断言（故意差异重点场景）：
- `libra worktree remove <path>` 后 `test -d <path>` 必须仍存在（Libra 故意保留目录，不像 Git 默认删除）。
- `libra --json stash list` 验证 `ok:true` 且 `data.entries[]` 或 `data.stashes[]` 可解析。
- 每次 stash/bisect/worktree 操作后 `libra fsck --connectivity-only` 必须 0 退出。
- `worktree remove` 后的 `libra --json worktree list` 不再包含该 worktree。
- 负向 `worktree remove` 不存在路径的错误必须非 0，stderr 包含路径。
- 验证 `--delete-dir` 模式真正删除目录：`libra worktree remove --delete-dir <path> && test ! -d <path>`。

### `libra stash/bisect/worktree` 参数覆盖表

| 参数或子命令 | 场景 ID | 关键断言 |
|---|---|---|
| `stash push -m` | `cli.stash-bisect-worktree` | tracked 修改被保存，消息可在列表中观察 |
| `stash list` / `stash show` | `cli.stash-bisect-worktree` | stash 条目和文件级摘要可观察 |
| `stash apply` | `cli.stash-bisect-worktree` | 修改恢复但 stash 条目保留 |
| `stash pop` | `cli.stash-bisect-worktree` | 修改恢复且 stash 条目删除 |
| `stash clear --force` | `cli.stash-bisect-worktree` | 非交互清空 stash 列表 |
| `bisect start <bad> --good <good>` | `cli.stash-bisect-worktree` | 二分边界可初始化 |
| `bisect bad` / `bisect good <rev>` | `cli.stash-bisect-worktree` | 会话状态推进并可由 log/view 观察 |
| `bisect log` / `bisect view` | `cli.stash-bisect-worktree` | 当前会话和候选状态可输出 |
| `bisect reset` | `cli.stash-bisect-worktree` | 结束会话并恢复原 HEAD |
| `worktree add <path>` | `cli.stash-bisect-worktree` | linked worktree 被创建并登记 |
| `worktree list` | `cli.stash-bisect-worktree` | 主 worktree 和 linked worktree 均可列出 |
| `worktree lock --reason` / `unlock` | `cli.stash-bisect-worktree` | 锁状态和 reason 可观察并可解除 |
| `worktree move <src> <dest>` | `cli.stash-bisect-worktree` | 登记路径和目录路径同步移动 |
| `worktree remove <path>` | `cli.stash-bisect-worktree` | 默认注销登记但保留目录 |
| `worktree prune` | `cli.stash-bisect-worktree` | 清理 stale 登记路径可执行 |

### `cli.tag-basic`

目的：覆盖 `tag` 创建（轻量/附注）、列表、强制更新、删除、ref 指向和 describe 依赖的 tag 可见性。

最小步骤：

```bash
SCENARIO="cli.tag-basic"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init tag-repo
cd tag-repo
libra config set user.name "Libra Tag Test"
libra config set user.email "tag@example.invalid"
printf 'tag base\n' > tag.txt
libra add tag.txt
libra commit -m "test: tag base"
BASE_COMMIT="$(libra rev-parse HEAD)"

libra tag v0.1.0
libra tag -m "release v0.2.0" v0.2.0
libra tag -l
libra tag -l -n 1
libra rev-parse v0.1.0
libra describe --tags --always HEAD
libra --json tag -l >tags.json
python3 -c "import json; d=json.load(open('tags.json')); assert d['ok'] is True; assert isinstance(d['data'].get('tags'), list)"

printf 'tag update\n' >> tag.txt
libra add tag.txt
libra commit -m "test: tag update"
libra tag -f v0.1.0
test "$(libra rev-parse v0.1.0)" != "$BASE_COMMIT"
libra tag -d v0.1.0
! libra rev-parse v0.1.0
```

负向步骤：

```bash
cd "$RUN_DIR/tag-repo"
! libra tag
! libra tag v0.2.0
! libra tag -d no-such-tag
```

断言：轻量 tag 与 annotated tag 均可创建并被 `rev-parse` 解析；`tag -l` / `tag -l -n` 可观察 tag 名称和注释摘要；`describe --tags --always` 能使用可达 tag 描述 HEAD；`tag -f` 可更新现有 tag 指向（新提交 != BASE）；`tag -d` 删除后原名不可解析；缺少 tag 名、重复创建和删除缺失 tag 必须非 0 退出且不影响已有 tag。

补充可执行断言（使用 `libra()` + python）：
- `libra --json tag -l` 必须返回 `ok:true`，且 `data.tags[]` 包含 v0.2.0。
- 负向错误必须包含稳定错误信息或 LBR- 码（通过 stderr 捕获验证）。
- 操作后 `libra fsck --connectivity-only` 必须成功（0 退出）。
- 全局 DB 隔离：本场景操作后，用隔离的全局 DB 执行 `libra config --global list` 不得看到本场景的 user.name（除非显式 --global）。

### `cli.merge-rebase-cherry-revert-smoke`

目的：覆盖 `merge`（fast-forward 与三方无冲突 merge）、`rebase`、`cherry-pick`、`revert` 的最小可观察闭环，以及 `--continue` / `--abort` 无会话失败路径。

最小步骤：

```bash
SCENARIO="cli.merge-rebase-cherry-revert-smoke"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init history-edit-repo
cd history-edit-repo
libra config set user.name "Libra History Edit Test"
libra config set user.email "history-edit@example.invalid"

printf 'base\n' > base.txt
libra add base.txt
libra commit -m "test: history-edit base"

libra branch ff-target
libra switch ff-target
printf 'ff\n' > ff.txt
libra add ff.txt
libra commit -m "test: fast-forward target"
FF_COMMIT="$(libra rev-parse HEAD)"
libra switch main
libra merge ff-target
test "$(libra rev-parse HEAD)" = "$FF_COMMIT"

libra branch merge-side main
libra switch merge-side
printf 'side\n' > side.txt
libra add side.txt
libra commit -m "test: merge side"
libra switch main
printf 'main\n' > main.txt
libra add main.txt
libra commit -m "test: merge main"
libra merge merge-side
libra log --oneline -n 1
test -f side.txt

libra branch rebase-topic main~1
libra switch rebase-topic
printf 'rebase\n' > rebase.txt
libra add rebase.txt
libra commit -m "test: rebase topic"
libra switch topic
libra rebase main
libra log --oneline -n 1
test -f rebase.txt

libra switch main
libra branch pick-source
libra switch pick-source
printf 'pick\n' > pick.txt
libra add pick.txt
libra commit -m "test: cherry source"
PICK_COMMIT="$(libra rev-parse HEAD)"
libra switch main
libra cherry-pick "$PICK_COMMIT"
test -f pick.txt

REVERT_TARGET="$(libra rev-parse HEAD)"
libra revert "$REVERT_TARGET"
test ! -f pick.txt
```

负向步骤：

```bash
cd "$RUN_DIR/history-edit-repo"
! libra merge no-such-branch
! libra merge --continue
! libra merge --abort
! libra rebase no-such-branch
! libra rebase --continue
! libra cherry-pick no-such-commit
! libra revert no-such-commit
```

断言：fast-forward merge 后 HEAD 等于目标提交；三方无冲突 merge 产生可观察 merge 结果并保留双方文件；`rebase main` 把 topic 提交重放到新 base 且文件存在；`cherry-pick <commit>` 在当前分支生成等价修改；`revert <commit>` 创建反向提交并移除被 revert 的文件；缺失目标、无 merge/rebase 会话的 continue/abort 和非法 commit 必须失败且不破坏当前分支。

补充可执行断言：
- 每次主要操作后执行 `libra fsck --connectivity-only` 必须 0 退出。
- `libra --json log -n 1` 验证 merge commit 有 2 个 parent（对于非 ff merge）。
- 负向步骤必须产生非 0 退出，且 stderr 包含 "not a" / "no such" 或 LBR- 相关错误标识（通过捕获验证）。
- `libra --json show-ref --heads` 验证 `data.entries[]` 中的分支状态在 rebase/cherry 后一致。

### `cli.merge-conflict-continue`

目的：覆盖 `merge` 产生冲突后的 `--continue` / `--abort` 成功路径。

最小步骤：

```bash
SCENARIO="cli.merge-conflict-continue"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init merge-conflict-repo
cd merge-conflict-repo
libra config set user.name "Libra Merge Conflict Test"
libra config set user.email "merge-conflict@example.invalid"

printf 'base line\n' > shared.txt
libra add shared.txt
libra commit -m "test: merge conflict base"

libra branch side
libra switch side
printf 'side change\n' > shared.txt
libra add shared.txt
libra commit -m "test: side change"

libra switch main
printf 'main change\n' > shared.txt
libra add shared.txt
libra commit -m "test: main change"

set +e
libra merge side >merge-conflict.out 2>merge-conflict.err
MERGE_STATUS=$?
set -e
test "$MERGE_STATUS" -ne 0
grep '<<<<<<<' shared.txt
printf 'resolved merge\n' > shared.txt
libra add shared.txt
libra merge --continue
libra log --oneline -n 1
grep 'resolved merge' shared.txt
```

负向步骤：

```bash
cd "$RUN_DIR/merge-conflict-repo"
! libra merge --continue
! libra merge --abort
```

断言：`merge side` 在同一文件产生冲突，工作区出现冲突标记；解决冲突并 `add` 后 `merge --continue` 成功完成合并提交；合并后 `log` 可见 merge commit，`shared.txt` 内容为解决后的文本；无 merge 会话时 `merge --continue` / `merge --abort` 必须失败且不破坏当前分支状态。

补充可执行断言（merge 冲突场景）：
- `merge side` 必须以非 0 退出进入冲突状态，且 `merge-conflict.err` 包含 "conflict"、"merge" 或 LBR- 相关错误文本。
- 冲突后 `libra --json status` 必须可解析出 `data.merge_state.conflicted_paths[]`，且包含 `shared.txt`。
- `merge --continue` 成功后 `libra --json status` 显示 `data.is_clean == true`，且 `data.merge_state` 缺失或 `conflicted_paths` 为空。
- `libra fsck` 在 --continue / --abort 后必须通过。
- 负向 merge --continue 无会话时错误必须包含可识别文本（"no merge" 或 LBR- 相关）。

补充可执行断言（冲突场景核心）：
- 冲突后 `libra --json status` 必须显示 `data.merge_state.conflicted_paths[]` 非空。
- `merge --continue` 成功后 `libra --json status` 显示 index 干净（`data.is_clean == true`，无 `merge_state.conflicted_paths` 条目）。
- `libra fsck` 在 continue/abort 后必须通过。
- 负向 continue/abort 的错误必须是可识别的 "no merge in progress" 类（捕获 stderr 验证包含 "merge" 或 LBR-CONFLICT 相关）。

### `cli.rebase-conflict-continue`

目的：覆盖 `rebase` 产生冲突后的 `--continue` / `--abort` / `--skip` 成功路径。

最小步骤：

```bash
SCENARIO="cli.rebase-conflict-continue"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init rebase-conflict-repo
cd rebase-conflict-repo
libra config set user.name "Libra Rebase Conflict Test"
libra config set user.email "rebase-conflict@example.invalid"

printf 'base line\n' > shared.txt
libra add shared.txt
libra commit -m "test: rebase conflict base"

libra branch topic
libra switch topic
printf 'topic change\n' > shared.txt
libra add shared.txt
libra commit -m "test: topic change"

libra switch main
printf 'main change\n' > shared.txt
libra add shared.txt
libra commit -m "test: main change"

libra switch topic
set +e
libra rebase main >rebase-conflict.out 2>rebase-conflict.err
REBASE_STATUS=$?
set -e
test "$REBASE_STATUS" -ne 0
grep '<<<<<<<' shared.txt
printf 'resolved rebase\n' > shared.txt
libra add shared.txt
libra rebase --continue
libra log --oneline -n 1
grep 'resolved rebase' shared.txt
```

负向步骤：

```bash
cd "$RUN_DIR/rebase-conflict-repo"
! libra rebase --continue
! libra rebase --abort
```

断言：`rebase main` 在 topic 提交与 main 修改同一文件时产生冲突，工作区出现冲突标记；解决冲突并 `add` 后 `rebase --continue` 成功完成重放；重放后 `log` 可见 topic 提交在 main 之上，`shared.txt` 内容为解决后的文本；无 rebase 会话时 `rebase --continue` / `rebase --abort` 必须失败且不破坏当前分支状态。

补充可执行断言（rebase 冲突）：
- 冲突后 `libra --json status` 可解析 `data.merge_state.conflicted_paths[]`，且包含 `shared.txt`。
- `rebase --continue` 成功后 `libra --json status` 显示 `data.is_clean == true`，且 `data.merge_state` 缺失或 `conflicted_paths` 为空。
- `libra fsck` 在 rebase --continue/--abort 后通过。
- 负向 rebase --continue 无会话错误必须可识别（stderr 捕获验证 "rebase" 或 LBR-）。

### `cli.grep-blame-describe-shortlog`

目的：覆盖 history inspection 剩余命令：`grep`、`blame`、`describe`、`shortlog` 的常用参数和失败路径。

最小步骤：

```bash
SCENARIO="cli.grep-blame-describe-shortlog"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init inspect-repo
cd inspect-repo
libra config set user.name "Libra Inspect Test"
libra config set user.email "inspect@example.invalid"
mkdir -p docs src
printf 'Alpha\nBeta\n' > docs/guide.txt
printf 'fn main() { println!("alpha"); }\n' > src/main.rs
libra add docs/guide.txt src/main.rs
libra commit -m "feat: add inspect files"
libra tag -m "inspect release" v1.0.0
printf 'Gamma\n' >> docs/guide.txt
libra add docs/guide.txt
libra commit -m "fix: update guide"

libra grep Alpha docs
libra grep -F 'println!("alpha")' src
libra grep -i gamma docs/guide.txt
libra grep -n -e Alpha -e Gamma docs/guide.txt
libra grep -c Alpha docs/guide.txt
libra grep -l alpha src
libra grep --tree HEAD~1 Alpha docs/guide.txt
printf 'Gamma\n' > patterns.txt
libra grep -f patterns.txt docs/guide.txt
libra blame docs/guide.txt
libra blame -L 1,2 docs/guide.txt HEAD
libra describe --tags HEAD
libra describe --always --abbrev 12 HEAD
libra shortlog
libra shortlog -s
libra shortlog -n

# Verify JSON outputs for AI Agent readability
libra --json grep Alpha docs >grep.json
python3 -c "import json; d=json.load(open('grep.json')); assert d['ok'] is True; assert 'matches' in d['data'] or isinstance(d['data'].get('matches'), list)"
libra --json blame docs/guide.txt >blame.json
python3 -c "import json; d=json.load(open('blame.json')); assert d['ok'] is True; assert 'lines' in d['data'] or isinstance(d['data'].get('lines'), list)"
libra --json describe --tags HEAD >describe.json
python3 -c "import json; d=json.load(open('describe.json')); assert d['ok'] is True; assert 'resolved_commit' in d['data'] or 'result' in d['data']"
libra --json shortlog >shortlog.json
python3 -c "import json; d=json.load(open('shortlog.json')); assert d['ok'] is True; assert 'authors' in d['data'] or isinstance(d['data'].get('authors'), list)"
```

负向步骤：

```bash
cd "$RUN_DIR/inspect-repo"
! libra grep no-such-pattern docs/guide.txt
! libra grep --tree no-such-revision Alpha docs/guide.txt
! libra blame -L bad docs/guide.txt
! libra blame missing.txt
! libra describe no-such-revision
```

断言：`grep` 可在工作区、指定 pathspec、pattern file 和历史 tree 中匹配内容，`-F` / `-i` / `-n` / `-c` / `-l` 输出可用于脚本断言；`blame` 输出每行作者和提交信息，`-L` 限制行范围；`describe --tags` 使用可达 tag，`--always --abbrev` 在需要时输出短 hash；`shortlog` 默认、summary 和排序模式都能按作者汇总；无匹配 grep、非法 revision、非法 blame 范围、缺失文件必须失败且不改变仓库。

补充可执行断言：
- `libra --json grep Alpha docs` 必须 `ok:true` 且 `data.matches[]` 可解析。
- `libra --json blame -L 1,1 docs/guide.txt` 验证结构包含 author / commit 信息。
- `libra --json describe --tags` 成功且包含 tag 信息。
- `libra --json shortlog` 返回按作者汇总的结构。
- 负向 `libra grep` 无匹配 或 `libra blame` 非法范围必须非 0，stderr 包含可识别错误（可选 LBR-）。

### `cli.clean-rm-mv-lfs-basic`

目的：覆盖工作树管理剩余命令 `clean`、`rm`、`mv` 和本地确定性的 `lfs track/untrack/ls-files` 行为；远端 LFS lock API 不进入默认 Wave。

最小步骤：

```bash
SCENARIO="cli.clean-rm-mv-lfs-basic"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init worktree-tools-repo
cd worktree-tools-repo
libra config set user.name "Libra Worktree Tools Test"
libra config set user.email "worktree-tools@example.invalid"
mkdir -p docs assets tmp ignored
printf 'keep\n' > docs/keep.txt
printf 'move\n' > docs/move.txt
printf 'remove\n' > docs/remove.txt
libra add docs/keep.txt docs/move.txt docs/remove.txt
libra commit -m "test: worktree tools base"

libra mv docs/move.txt docs/moved.txt
libra status --short
libra commit -a -m "test: move tracked file"

libra rm docs/remove.txt
libra status --short
libra commit -m "test: remove tracked file"

printf 'scratch\n' > tmp/scratch.log
libra clean -n tmp/scratch.log
test -f tmp/scratch.log
libra clean -f tmp/scratch.log
test ! -f tmp/scratch.log
printf 'dir scratch\n' > tmp/dir-file.txt
libra clean -fd tmp
test ! -e tmp

printf '*.ignored\n' > .libraignore
printf 'ignored\n' > ignored/file.ignored
libra clean -nX
libra clean -fX
test ! -f ignored/file.ignored

libra lfs track '*.bin'
libra lfs track
printf 'large payload\n' > assets/blob.bin
libra add .libra_attributes assets/blob.bin
libra commit -m "test: lfs tracked file"
libra lfs ls-files
libra lfs ls-files --long --size
libra lfs ls-files --name-only
libra lfs untrack '*.bin'
libra lfs track
```

负向步骤：

```bash
cd "$RUN_DIR/worktree-tools-repo"
! libra clean
! libra clean -xX
! libra rm no-such-file.txt
! libra mv no-such-source.txt docs/dest.txt
! libra lfs lock assets/blob.bin
```

断言：`mv` 同时更新工作区路径和 index 状态；`rm` 删除 tracked 文件并可提交；`clean -n` 不删除、`clean -f` 删除文件、`clean -fd` 删除目录、`clean -fX` 只删除 ignored 文件；`lfs track` 写入 `.libra_attributes`，无参数可列出 pattern；tracked 大文件提交后可由 `lfs ls-files` 三种格式观察；`lfs untrack` 移除 pattern；缺少 `-f/-n`、互斥 clean flag、缺失 rm/mv 源必须失败；`lfs lock` 在无远端 LFS 服务/认证时必须失败且不得泄露凭据。`lfs untrack` 对缺失 pattern 当前可能是幂等空删除，不作为负向断言。

补充可执行断言：
- `libra --json lfs ls-files` 返回 `ok:true`；无 LFS tracked 文件时 `data.files` 可缺失（当前 `LfsOutput.files` 为空会被省略），有 tracked 文件时 `data.files[]` 必须可解析。
- 验证 `.libra_attributes` 内容包含 `*.bin`（`grep` 或 `cat` 后 python 检查）。
- `libra --json status --porcelain` 在 mv/rm 后可解析且显示正确 staged 状态。
- 操作后 `libra fsck --connectivity-only` 通过。
- 全局隔离：本场景的 `.libraignore` 和 LFS pattern 不得通过隔离 HOME 的全局 config 泄露到其他场景。

### `cli.reflog-symbolic-ref`

目的：覆盖 `reflog` 与 `symbolic-ref` 的用户可观察 ref 日志和符号引用行为。

最小步骤：

```bash
SCENARIO="cli.reflog-symbolic-ref"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init ref-log-repo
cd ref-log-repo
libra config set user.name "Libra Reflog Test"
libra config set user.email "reflog@example.invalid"
printf 'one\n' > ref.txt
libra add ref.txt
libra commit -m "test: reflog one"
libra branch feature/ref-log
libra switch feature/ref-log
printf 'two\n' >> ref.txt
libra add ref.txt
libra commit -m "test: reflog two"

libra reflog show
libra reflog show HEAD
libra reflog show --stat
libra reflog show --pretty oneline
libra reflog exists HEAD
libra symbolic-ref HEAD
libra symbolic-ref --short HEAD
libra symbolic-ref HEAD refs/heads/main
libra symbolic-ref --short HEAD
libra symbolic-ref HEAD refs/heads/feature/ref-log
```

负向步骤：

```bash
cd "$RUN_DIR/ref-log-repo"
! libra reflog show refs/heads/no-such-branch
! libra reflog exists refs/heads/no-such-branch
! libra symbolic-ref refs/heads/bad
! libra symbolic-ref HEAD refs/tags/not-a-branch
```

断言：`reflog show` 能观察 commit、branch switch 或 HEAD 更新记录；`--stat` / `--pretty oneline` 输出可用于脚本断言；`reflog exists HEAD` 可用于脚本探测；`symbolic-ref HEAD` 和 `--short` 输出当前分支；`symbolic-ref HEAD refs/heads/<branch>` 可切换 HEAD 的符号目标并被后续读取观察；`reflog exists` 对缺失 ref 必须失败，非 HEAD 名称和非法 symbolic-ref 目标必须失败。注意 `reflog show <missing>` 当前可能返回空列表而非失败，不能作为负向断言，只能断言输出为空或 `count=0`。

补充可执行断言：
- `libra --json reflog show` 验证 `ok:true`，且 entries 中至少包含 "commit:" 或 "checkout:" 条目，并包含本场景创建的提交消息。
- `libra --json symbolic-ref HEAD` 验证 `ok:true`，且 data 中的 ref 输出为 "refs/heads/..."。
- 非法 symbolic-ref 目标的失败必须包含稳定错误（LBR- 或 "not a branch" 类消息）。
- 操作前后 `libra --json show-ref --heads` 验证 `data.entries[]` 一致性（无意外丢失）。

### `cli.open-smoke`

目的：覆盖 `open` 命令的最小可观察行为，但避免默认 Wave 在 CI/headless 环境中真的打开浏览器或系统应用。

最小步骤：

```bash
SCENARIO="cli.open-smoke"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init open-repo
cd open-repo
libra remote add origin git@github.com:example/open-repo.git
libra --json open >open-default.json
libra --json open origin >open-origin.json
python3 -c "import json; d=json.load(open('open-default.json')); assert d['ok'] is True; assert d['data']['launched'] is False; assert 'web_url' in d['data']"
python3 -c "import json; d=json.load(open('open-origin.json')); assert d['ok'] is True; assert d['data']['launched'] is False; assert 'web_url' in d['data']"
```

负向步骤：

```bash
cd "$RUN_DIR/open-repo"
! libra --json open no-such-remote
```

断言：全局 `--json` 模式输出包含 `remote`、`remote_url`、`web_url` 和 `launched=false`，不启动外部程序；指定 remote 可解析托管页面 URL；缺失 remote 或不安全 URL 必须失败。默认 Wave 严禁运行会真实启动浏览器/系统应用的裸 `libra open`。

补充可执行断言：
- 已有 JSON 断言保持；额外验证 `libra --json open no-such-remote` 的错误 envelope 包含 `ok:false` + LBR- 码或 "no such remote"。
- 验证即使 remote URL 非法，`launched=false` 且无副作用（无浏览器进程）。
- 操作后 `libra fsck` 通过。

### `libra tag/history-inspection/worktree-tools/ref-log` 参数覆盖表

| 参数或子命令 | 场景 ID | 关键断言 |
|---|---|---|
| `tag <name>` / `tag -m <msg>` | `cli.tag-basic` | 轻量和 annotated tag 均可创建、列出、解析 |
| `tag -l` / `tag -l -n` / `tag -f` / `tag -d` | `cli.tag-basic` | 列表、注释摘要、强制更新和删除路径覆盖 |
| `merge <branch>` | `cli.merge-rebase-cherry-revert-smoke` | fast-forward 与三方无冲突 merge 均可观察 |
| `merge --continue` / `--abort` | `cli.merge-rebase-cherry-revert-smoke` | 无会话时明确失败；冲突续跑场景另行补充 |
| `rebase <upstream>` | `cli.merge-rebase-cherry-revert-smoke` | topic 提交重放到新 base |
| `rebase --continue` | `cli.merge-rebase-cherry-revert-smoke` | 无会话时明确失败；冲突续跑场景另行补充 |
| `cherry-pick <commit>` | `cli.merge-rebase-cherry-revert-smoke` | 指定提交修改被重放到当前分支 |
| `revert <commit>` | `cli.merge-rebase-cherry-revert-smoke` | 创建反向提交并撤销目标修改 |
| `grep` / `grep -F/-i/-n/-c/-l/-e/-f/--tree` | `cli.grep-blame-describe-shortlog` | 工作区、pathspec、pattern file 和历史 tree 搜索可观察 |
| `blame` / `blame -L` | `cli.grep-blame-describe-shortlog` | 行级作者、提交和范围限制可观察 |
| `describe --tags/--always/--abbrev` | `cli.grep-blame-describe-shortlog` | tag 描述和 hash fallback 可观察 |
| `shortlog` / `shortlog -s` / `shortlog -n` | `cli.grep-blame-describe-shortlog` | 作者汇总和排序可观察 |
| `clean -n/-f/-fd/-fX` | `cli.clean-rm-mv-lfs-basic` | dry-run、文件删除、目录删除、ignored-only 删除覆盖 |
| `rm <path>` | `cli.clean-rm-mv-lfs-basic` | tracked 文件从工作区和 index 移除 |
| `mv <src> <dst>` | `cli.clean-rm-mv-lfs-basic` | tracked 文件移动并更新 index |
| `lfs track/untrack/ls-files` | `cli.clean-rm-mv-lfs-basic` | `.libra_attributes` pattern 和 LFS tracked 文件列表可观察 |
| `reflog show` / `reflog show --stat` / `reflog exists` | `cli.reflog-symbolic-ref` | HEAD/ref 更新记录可读，exists 可脚本探测 |
| `symbolic-ref` / `symbolic-ref --short` / `symbolic-ref HEAD <target>` | `cli.reflog-symbolic-ref` | HEAD 符号引用读写可观察 |
| `--json open` | `cli.open-smoke` | 只输出 URL 和 `launched=false`，不启动外部程序 |

### `cli.cross-cutting-flags`

目的：集中覆盖 `src/cli.rs` 根结构（`Cli`）上的全局 flag —— `--json`(`-J`)/`--machine`/`--quiet`(`-q`)/`--color`/`--no-color`/`--progress`/`--exit-code-on-warning`，断言其语义本身，而不是依赖各功能场景顺带触发。本场景的内联 `libra()` 已对齐 §3.3.1 更新后的规范（含 `TMPDIR` 与 git/ssh 感知 `SAFE_PATH`），可作为其他场景收敛的样板。

最小步骤：

```bash
SCENARIO="cli.cross-cutting-flags"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
SAFE_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
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
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init flags-repo
cd flags-repo
libra config set user.name "Libra Flags Test"
libra config set user.email "flags@example.invalid"
printf 'flag\n' > flag.txt
libra add flag.txt
libra commit -m "test: flags base"

# --json / -J：stdout 是可解析 JSON envelope（需要 PATH 上的解析器时用 python3，否则仅断言非空）
libra --json status >status.json
libra -J status >status.short.json
libra --json=compact log >log.compact.json
libra --json=ndjson log >log.ndjson
python3 -c "import json; d=json.load(open('status.json')); assert d['ok'] is True; assert 'data' in d; assert 'untracked' in d['data']"
python3 -c "import json; d=json.load(open('status.short.json')); assert d['ok'] is True; assert 'data' in d"
python3 -c "import json; d=json.load(open('log.compact.json')); assert d['ok'] is True; assert isinstance(d['data'].get('commits'), list)"
python3 -c "import json; lines=[json.loads(l) for l in open('log.ndjson')]; assert len(lines) > 0; assert 'hash' in lines[0] or 'id' in lines[0]"

# --quiet：抑制主结果 stdout，但命令仍成功
libra --quiet status >quiet.out
test ! -s quiet.out

# --machine：蕴含 ndjson + no-pager + color=never + quiet
libra --machine status >machine.out

# --color=never / --no-color：stdout 不含 ANSI 转义序列
libra --color=never log >log.nocolor
libra --no-color log >log.nocolor2
! grep -q "$(printf '\033')" log.nocolor

# --progress=none：长操作不打印进度
libra --progress none status >/dev/null

# --exit-code-on-warning：无 warning 时不得改变成功命令退出码
# warning 时退出码 9 需要先固化确定性 warning 源，当前按 BASELINE_GAP-INTEG-009 跟踪。
libra --exit-code-on-warning status

# 错误 JSON 形态（Agent 关键契约）：--json 模式下失败也必须在 stderr 产出 ok:false + LBR-* 稳定码
! libra --json cat-file -p 0000000000000000000000000000000000000000 2>err.json || true
python3 -c "
import json, sys
data = open('err.json').read().strip()
if data:
    try:
        j = json.loads(data)
        assert j.get('ok') is False
        assert 'error_code' in j and j['error_code'].startswith('LBR-')
        assert 'category' in j and 'message' in j
        assert 'hints' in j or 'details' in j
    except Exception as e:
        print('JSON error envelope parse failed:', e, file=sys.stderr)
        sys.exit(1)
"
```

负向步骤：

```bash
cd "$RUN_DIR/flags-repo"
! libra --json=bogus status
! libra --color=plaid log
# 无 warning 时 --exit-code-on-warning 不应改变退出码
libra --exit-code-on-warning status
```

断言：`--json`/`-J` 输出可被 JSON 解析（或至少非空且为单一 envelope）；`--json=compact`/`=ndjson` 切换布局；`--quiet` 使主结果 stdout 为空但退出码 0；`--machine` 等价于 ndjson+no-pager+color=never+quiet 的组合（参见 `src/cli.rs` 中 `--machine` 的文档化语义）；`--color=never`/`--no-color` 去除 ANSI 转义；`--progress none` 不打印进度；`--exit-code-on-warning` 在无 warning 时退出码为 0；非法 `--json`/`--color` 值必须非 0 退出并提示可选值。warning 时退出码 9 暂不进入默认 Wave，按 BASELINE_GAP-INTEG-009 要求先识别无密钥、可复现 warning 源。

补充可执行断言（Agent 契约核心场景）：
- `libra --json status > s.json && python3 -c "import json; d=json.load(open('s.json')); assert d['ok'] is True; assert 'data' in d"`
- `libra --machine status > m.out && python3 -c "import json; [json.loads(l) for l in open('m.out')]"` （验证 ndjson 可解析）
- `libra --quiet status > q.out && test ! -s q.out`
- `libra --exit-code-on-warning status` 在无 warning 时退出码必须为 0。
- 非法 `--json=bogus` 必须非 0，且错误 envelope 包含 LBR-CLI-002 或等价。
- 验证 `--progress json` 在 JSON 模式下输出 NDJSON progress 到 stderr。
- 额外：`libra --json --exit-code-on-warning status` 在干净状态下退出码为 0；warning=9 组合行为只在 BASELINE_GAP-INTEG-009 的确定性 warning 源落地后启用。

通过标准：全部场景退出码和断言通过，无未解释 skip/fail。`merge --continue` / `rebase --continue` 的冲突续跑成功路径由 `cli.merge-conflict-continue` / `cli.rebase-conflict-continue` 覆盖；LFS 远端 lock API、真实浏览器/系统 open 行为不进入默认 Wave，必要时登记独立 follow-up。

## 4.2 Wave 2：CLI 存储、schema 与本地协议场景（必跑）

Wave 2 覆盖需要跨仓库、本地 remote 或底层存储可观察结果的功能，但仍只通过 `libra` 命令驱动。

### `cli.schema-upgrade-observable`

目的：验证新建仓库的 SQLite schema 可被 CLI 正常使用。

最小步骤：

```bash
SCENARIO="cli.schema-upgrade-observable"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init schema-repo
cd schema-repo

libra db status
libra db --json status >db-status.json
python3 -c "import json; d=json.load(open('db-status.json')); assert d['ok'] is True; assert 'current_version' in d['data']; assert 'latest_version' in d['data']; assert 'state' in d['data']"
libra db upgrade
libra db status

libra config set user.name "Libra Schema Test"
libra config set user.email "schema@example.invalid"
printf 'schema\n' > schema.txt
libra add schema.txt
libra commit -m "test: schema usable after status"
libra log --oneline -n 1
libra fsck --connectivity-only
```

负向步骤：

```bash
cd "$RUN_ROOT/repos"
mkdir not-a-repo
cd not-a-repo
! libra db status
! libra db upgrade
```

断言：`db status` 只读取 schema 状态并退出码为 0；`db --json status` 输出 current/latest/state 等结构化字段或等价 schema 状态；`db upgrade` 对已是当前版本的仓库应成功且幂等；升级/状态检查后提交闭环和 `fsck --connectivity-only` 不触发 migration 或 schema 错误；非仓库目录中的 `db status` / `db upgrade` 必须失败并提示缺少 Libra 仓库。

补充可执行断言：
- `libra --json db status` 必须 `ok:true`，`data.current_version == data.latest_version` 且 `data.state` 为兼容状态。
- 非仓库目录执行 `libra db status` 必须非 0，stderr 包含 "not a libra repository" 或 LBR-REPO-001。
- 操作后 `libra fsck --connectivity-only` 必须 0 退出。
- 验证 schema 升级幂等：连续两次 `libra db upgrade` 均成功且无副作用。

### `cli.clone-fetch-pull-local`

目的：验证本地路径 Git remote 的 `clone`、`remote`、`ls-remote`、`fetch`、`pull` 行为，不访问公网，并覆盖本地 Git 仓库互操作性。注意 `push` 当前故意拒绝本地 file remote，因此本场景通过隔离 `gitfix()` 直接推进 Git fixture，不使用 `libra push` 搭 fixture。

最小步骤：

```bash
SCENARIO="cli.clone-fetch-pull-local"
REMOTE_DIR="$RUN_ROOT/fixtures/$SCENARIO/git-source"
CLONE_DIR="$RUN_ROOT/repos/$SCENARIO/clone"
mkdir -p "$(dirname "$REMOTE_DIR")" "$(dirname "$CLONE_DIR")"
SAFE_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
GIT_BIN="$(command -v git || true)"
case ":$SAFE_PATH:" in *":$(dirname "${GIT_BIN:-/usr/bin/git}"):"*) ;; *)
  [ -n "$GIT_BIN" ] && SAFE_PATH="$SAFE_PATH:$(dirname "$GIT_BIN")" ;; esac
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
libra() {
  env -i \
    PATH="$SAFE_PATH" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}

mkdir -p "$REMOTE_DIR"
cd "$REMOTE_DIR"
gitfix init -b main
gitfix config user.name "Libra Remote Seed"
gitfix config user.email "remote-seed@example.invalid"
printf 'first\n' > README.md
gitfix add README.md
gitfix commit -m "test: seed remote"

libra ls-remote "$REMOTE_DIR"
libra ls-remote --heads "$REMOTE_DIR" main
libra clone "$REMOTE_DIR" "$CLONE_DIR"
cd "$CLONE_DIR"
libra remote -v
libra remote get-url origin
libra remote add mirror "$REMOTE_DIR"
libra remote get-url mirror
libra config set user.name "Libra Clone Local"
libra config set user.email "clone-local@example.invalid"
libra log --oneline
grep 'first' README.md

cd "$REMOTE_DIR"
printf 'second\n' >> README.md
gitfix add README.md
gitfix commit -m "test: second remote commit"

cd "$CLONE_DIR"
libra fetch origin main
libra fetch --all
libra show-ref --heads
libra pull --ff-only origin main
grep 'second' README.md

# pull --rebase：clone 端先造一个本地提交，再让 source 推进 upstream，
# rebase 把本地提交重放到 upstream 新提交之上（改不同文件，确定性无冲突）
printf 'local only\n' > clone-local.txt
libra add clone-local.txt
libra commit -m "test: clone local commit"
cd "$REMOTE_DIR"
printf 'third\n' >> README.md
gitfix add README.md
gitfix commit -m "test: third remote commit"
cd "$CLONE_DIR"
libra pull --rebase origin main
grep 'third' README.md
test -f clone-local.txt
```

补充步骤：

```bash
cd "$RUN_ROOT/repos/$SCENARIO"
libra clone --bare "$REMOTE_DIR" bare-clone.git
test -f bare-clone.git/libra.db

libra clone --single-branch -b main "$REMOTE_DIR" single-branch
cd single-branch
libra branch --show-current
```

负向步骤：

```bash
cd "$RUN_ROOT/repos/$SCENARIO/clone"
! libra fetch origin no-such-branch
! libra pull --ff-only origin no-such-branch
! libra clone "$RUN_ROOT/fixtures/$SCENARIO/missing.git" "$RUN_ROOT/repos/$SCENARIO/missing-clone"

# Verify clone/fetch/pull JSON output format
cd "$RUN_DIR"
libra --json clone "$REMOTE_DIR" "$RUN_ROOT/repos/$SCENARIO/clone-json" >clone.json
python3 -c "import json; d=json.load(open('clone.json')); assert d['ok'] is True; assert 'data' in d"
cd "$RUN_ROOT/repos/$SCENARIO/clone-json"
libra --json fetch origin >fetch.json
python3 -c "import json; d=json.load(open('fetch.json')); assert d['ok'] is True; assert 'data' in d"
libra --json pull --ff-only origin main >pull.json
python3 -c "import json; d=json.load(open('pull.json')); assert d['ok'] is True; assert 'data' in d"
```

断言：隔离 `gitfix()` 创建的本地 Git 仓库可作为 clone/fetch/pull remote；`remote add`、`remote -v`、`remote get-url` 能观察本地路径 URL；`ls-remote` 可看到 `refs/heads/main`；普通 clone 后文件和 log 可见；Git fixture 新提交后，clone 仓库通过 `fetch`、`fetch --all` 和 `pull --ff-only` 能看到新增提交；**`pull --rebase` 把 clone 端本地提交重放到 upstream 新提交之上——`README.md` 含 upstream 的 `third`，本地 `clone-local.txt` 仍在**；`clone --bare` 生成 Libra bare 布局（可观察到 `libra.db`）；`clone --single-branch -b main` 只检出指定分支；缺失 remote 或缺失 ref 必须非 0 退出且不创建半成品仓库或损坏当前 clone。

补充可执行断言：
- `libra --json clone "$REMOTE_DIR" clone-json` 成功后 `ok:true`，并验证 `libra --json log -n 1` 结构。
- 每次 fetch/pull 后 `libra fsck --connectivity-only` 通过。
- `libra --json ls-remote --heads` 返回结构化 refs 列表。
- 负向 `libra fetch origin no-such` 必须非 0，stderr 包含 "couldn't find remote ref" 或对应 LBR-NET 错误。
- 验证 `pull --rebase` 成功后，本地提交历史被重放（通过 `libra --json log -n 5` 的 `data.commits[]` 顺序观察）。

### `cli.fetch-depth-local`

目的：验证本地路径 Git source 上的 `clone --depth` shallow 基本语义。该场景不使用 `push`，因为当前 `push` 故意拒绝本地 file remote。当前实现若在本场景暴露 `LBR-REPO-002 object not found`，应记录为 shallow clone 对象闭包实现缺口，而不是把场景改回本地 push fixture。

最小步骤：

```bash
SCENARIO="cli.fetch-depth-local"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
SAFE_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
GIT_BIN="$(command -v git || true)"
case ":$SAFE_PATH:" in *":$(dirname "${GIT_BIN:-/usr/bin/git}"):"*) ;; *)
  [ -n "$GIT_BIN" ] && SAFE_PATH="$SAFE_PATH:$(dirname "$GIT_BIN")" ;; esac
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
libra() {
  env -i \
    PATH="$SAFE_PATH" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}

REMOTE_DIR="$RUN_ROOT/fixtures/$SCENARIO/git-source"
mkdir -p "$(dirname "$REMOTE_DIR")"

mkdir -p "$REMOTE_DIR"
cd "$REMOTE_DIR"
gitfix init -b main
gitfix config user.name "Libra Depth Test"
gitfix config user.email "depth@example.invalid"
printf 'first\n' > a.txt
gitfix add a.txt
gitfix commit -m "test: first"
printf 'second\n' > a.txt
gitfix add a.txt
gitfix commit -m "test: second"
printf 'third\n' > a.txt
gitfix add a.txt
gitfix commit -m "test: third"

cd "$RUN_DIR"
libra clone --depth 1 "$REMOTE_DIR" shallow-clone
cd shallow-clone
libra log --oneline | wc -l | grep -q '^1$'
test -f a.txt
grep 'third' a.txt

cd "$RUN_DIR"
libra clone --depth 2 "$REMOTE_DIR" shallow-clone-2
cd shallow-clone-2
libra log --oneline | wc -l | grep -q '^2$'
```

负向步骤：

```bash
cd "$RUN_DIR"
! libra clone --depth 0 "$REMOTE_DIR" "$RUN_ROOT/repos/$SCENARIO/bad-depth"
```

断言：`clone --depth 1` 只获取最新提交，`log` 数量为 1，但工作区文件内容是最新的；`clone --depth 2` 获取 2 个提交；非法 depth（如 0）必须非 0 退出。本地 Git fixture shallow 语义可作为基本功能验证，与真实远端的深度对等性差异另由 BASELINE_GAP-INTEG-009 跟踪。

补充可执行断言：
- `libra --json clone --depth 1 "$REMOTE_DIR" shallow1` 成功；进入 `shallow1` 后运行 `libra --json log -n 10 >log.json`，用 python 断言 `len(data.commits) == 1`。
- shallow clone 后 `libra --json rev-list HEAD` 返回 `data.total` 和 `data.commits[]`，数量与 depth 预期一致。
- 非法 `--depth 0` 错误必须非 0。
- shallow clone 后执行 `libra fsck --connectivity-only` 必须通过。

### `cli.push-local-file-remote-rejected`

目的：验证 `push` 对本地 file remote 的故意差异：本地路径 remote 可用于 `clone`/`fetch`/`pull` fixture，但 `push` 当前只支持网络 remote，必须拒绝本地 file remote。真实 push/refspec/tag/force/mirror 成功路径放到 Wave 3 GitHub 场景。

最小步骤：

```bash
SCENARIO="cli.push-local-file-remote-rejected"
REMOTE_DIR="$RUN_ROOT/fixtures/$SCENARIO/remote.git"
WORK_DIR="$RUN_ROOT/repos/$SCENARIO/work"
mkdir -p "$(dirname "$REMOTE_DIR")" "$(dirname "$WORK_DIR")"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}

libra init --bare "$REMOTE_DIR"
libra init "$WORK_DIR"
cd "$WORK_DIR"
libra config set user.name "Libra Push Rejection Test"
libra config set user.email "push-reject@example.invalid"
printf 'push\n' > push.txt
libra add push.txt
libra commit -m "test: push rejection base"
libra remote add origin "$REMOTE_DIR"
libra remote set-url --push origin "$REMOTE_DIR"
libra remote get-url --all origin

expect_local_push_rejected() {
  name="$1"
  shift
  set +e
  libra --json=compact push "$@" >"$name.out" 2>"$name.err"
  status=$?
  set -e
  test "$status" -ne 0
  python3 - "$name.err" <<'PY'
import json, sys
raw = open(sys.argv[1]).read().strip()
payload = json.loads(raw)
assert payload["ok"] is False
assert payload["error_code"] == "LBR-CLI-003"
assert "local file" in payload["message"] or "local file repositories" in payload["message"]
PY
}

expect_local_push_rejected push-main origin main
expect_local_push_rejected push-dry-run --dry-run origin main
expect_local_push_rejected push-force --force origin main
expect_local_push_rejected push-tags --tags origin
expect_local_push_rejected push-mirror --mirror --dry-run origin
```

断言：本地 file remote 已存在且可作为 remote URL 存储；`push origin main`、`push --dry-run origin main`、`push --force origin main`、`push --tags origin`、`push --mirror --dry-run origin` 都必须非 0 退出；`--json=compact` 的 stderr 错误 envelope 必须包含 `ok:false`、`error_code == "LBR-CLI-003"` 和本地 file remote 不支持的可操作提示；失败不得写入 remote refs 或修改本地 HEAD。

补充可执行断言：
- 每个本地 file remote push 失败后执行 `libra fsck --connectivity-only`，确认本地源仓库仍健康。
- `libra --json remote get-url --all origin` 仍能返回本地路径，证明失败点是 push 传输策略而非 remote 配置丢失。
- 若未来实现支持本地 file remote push，必须把本场景改成正向闭环，并同步更新 COMPATIBILITY.md / declined note。

### `cli.object-readback`

目的：验证通过 CLI 写入的 commit/tree/blob/ref 能通过 CLI plumbing 和 history inspection 命令读回，覆盖 `rev-parse`、`rev-list`、`show`、`show-ref`、`cat-file`、`hash-object`、`fsck`。

最小步骤：

```bash
SCENARIO="cli.object-readback"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
libra() {
  env -i \
    PATH="${SAFE_PATH:-/usr/bin:/bin:/usr/sbin:/sbin}" \
    USERPROFILE="$RUN_ROOT/home" \
    HOME="$RUN_ROOT/home" \
    XDG_CONFIG_HOME="$RUN_ROOT/xdg-config" \
    XDG_CACHE_HOME="$RUN_ROOT/xdg-cache" \
    TMPDIR="$RUN_ROOT/tmp" \
    LIBRA_TEST=1 \
    LIBRA_CONFIG_GLOBAL_DB="$RUN_ROOT/home/.libra/config.db" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init object-repo
cd object-repo

libra config set user.name "Libra Object Test"
libra config set user.email "object@example.invalid"
mkdir -p docs src
printf 'object root\n' > README.md
printf 'object docs\n' > docs/guide.md
printf 'fn main() {}\n' > src/main.rs
libra add README.md docs/guide.md src/main.rs
libra commit -m "test: object readback"

HEAD_ID="$(libra rev-parse HEAD)"
libra rev-parse --short HEAD
libra rev-parse --show-toplevel
libra rev-list HEAD
libra show --no-patch HEAD
libra show --stat HEAD
libra show HEAD:docs/guide.md
libra show-ref --head
libra show-ref --heads
libra cat-file -t "$HEAD_ID"
libra cat-file -s "$HEAD_ID"
libra cat-file -p "$HEAD_ID"
libra cat-file -e "$HEAD_ID"

printf 'loose blob\n' > loose.txt
BLOB_ID="$(libra hash-object -w loose.txt)"
libra cat-file -t "$BLOB_ID"
libra cat-file -p "$BLOB_ID"
printf 'stdin blob\n' | libra hash-object --stdin
printf 'README.md\ndocs/guide.md\n' | libra hash-object --stdin-paths

libra fsck
libra fsck --connectivity-only
libra fsck "$HEAD_ID"
```

负向步骤：

```bash
cd "$RUN_DIR/object-repo"
! libra rev-parse no-such-revision
! libra show HEAD:no-such-path
! libra cat-file -p no-such-object
! libra hash-object missing-file.txt
! libra fsck no-such-object
```

断言：`rev-parse HEAD` 输出可传递给 `cat-file`、`fsck` 等后续命令；`rev-list HEAD` 至少包含当前提交；`show --no-patch` / `show --stat` 能读回 commit 元数据和变更统计；`show HEAD:<path>` 输出内容必须与提交前文件内容一致；`show-ref --head` / `--heads` 能列出 HEAD 和本地分支；`cat-file -t/-s/-p/-e` 分别返回类型、大小、内容和存在性；`hash-object -w` 写入的 loose blob 可由 `cat-file` 读回；`hash-object --stdin` / `--stdin-paths` 可计算输入内容或路径列表；`fsck` 和 `fsck --connectivity-only` 在健康仓库中退出码为 0；缺失 revision、path、object 或 file 必须失败且不写入新对象。

补充可执行断言（plumbing 场景重点）：
- `libra --json cat-file -p $HEAD_ID` 必须 `ok:true` 且 data 中的 commit 结构包含 `object_type == "commit"`、`tree`、`parents[]`、`message`。
- `libra --json rev-list HEAD` 返回 `data.commits[]` 与 `data.total`，每个 commit 元素为 hash 字符串。
- 所有对象操作后 `libra fsck` 必须通过；写入 blob 后 `libra --json cat-file -t $BLOB_ID` 验证类型为 "blob"。
- 负向 cat-file / rev-parse 错误必须返回 LBR- 码（通过 JSON error envelope 或 stderr 捕获）。

### `cli.sha256-object-readback`

目的：验证 `--object-format sha256` 仓库不仅 `core.objectformat` 正确，还能走完整“提交→对象读回”闭环。这覆盖 `src/cli.rs` 的 hash-kind preflight（按仓库 `core.objectformat` 调 `set_hash_kind`）的端到端正确性；`cli.init-branch-and-format-options` 只验证了 config 键，未验证 sha256 对象真正可写可读。

最小步骤：

```bash
SCENARIO="cli.sha256-object-readback"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
SAFE_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
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
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}
libra init --object-format sha256 sha256-repo
cd sha256-repo
libra config get core.objectformat
libra config set user.name "Libra Sha256 Test"
libra config set user.email "sha256@example.invalid"
printf 'sha256 payload\n' > payload.txt
libra add payload.txt
libra commit -m "test: sha256 commit"

HEAD_ID="$(libra rev-parse HEAD)"
test "${#HEAD_ID}" -eq 64          # sha256 对象 id 为 64 位 hex（sha1 为 40 位）
libra cat-file -t "$HEAD_ID"
libra cat-file -p "$HEAD_ID"
libra show --stat HEAD
libra log --oneline -n 1
libra fsck --connectivity-only

BLOB_ID="$(libra hash-object -w payload.txt)"
test "${#BLOB_ID}" -eq 64
libra cat-file -p "$BLOB_ID"
```

断言：`core.objectformat` 为 `sha256`；commit 与 blob 的对象 id 均为 64 位 hex，证明 hash-kind preflight 正确按仓库格式 pin（而非默认 sha1）；`cat-file -t/-p`、`show --stat`、`log --oneline`、`fsck --connectivity-only`、`hash-object -w` 在 sha256 仓库全部成功且写入对象可读回；与默认 sha1 的 `cli.object-readback` 形成对照。

补充可执行断言：
- `libra --json config get core.objectformat` 验证值为 "sha256"。
- `libra --json cat-file -p HEAD` 成功且 commit ID 为 64 字符 hex。
- 写入 blob 后 `libra --json cat-file -t $BLOB_ID` 返回 "blob"。
- 全流程 `libra fsck --connectivity-only` 通过。

### `cli.verify-pack-smoke`

目的：覆盖 `verify-pack` 对 `.idx` / `.pack` 成对文件的黑盒验证，避免 Maintenance 矩阵把 pack 验证误归入 `fsck` 或 `cat-file` 覆盖。

最小步骤：

```bash
SCENARIO="cli.verify-pack-smoke"
REPO_ROOT="$PWD"   # 记录 libra 仓库根目录（Wave 0 执行目录），供后续复制 fixture
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
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
libra init pack-source
cd pack-source
libra config set user.name "Libra Pack Test"
libra config set user.email "pack@example.invalid"
printf 'pack one\n' > one.txt
printf 'pack two\n' > two.txt
libra add one.txt two.txt
libra commit -m "test: pack source"

# verify-pack 需要 pack+idx 成对输入；用仓库内固定 pack fixture 并通过隐藏
# index-pack 生成 idx，避免读取开发者真实 .git/.libra pack 目录。
mkdir -p "$RUN_ROOT/fixtures/$SCENARIO"
PACK_FILE="$RUN_ROOT/fixtures/$SCENARIO/small-sha1.pack"
PACK_IDX="$RUN_ROOT/fixtures/$SCENARIO/small-sha1.idx"
cp "$REPO_ROOT/tests/data/packs/small-sha1.pack" "$PACK_FILE"
libra index-pack "$PACK_FILE" -o "$PACK_IDX"
test -f "$PACK_IDX"
libra verify-pack "$PACK_IDX"
libra verify-pack --pack "$PACK_FILE" "$PACK_IDX"
libra verify-pack -v "$PACK_IDX"
libra verify-pack -s "$PACK_IDX"
libra --json verify-pack "$PACK_IDX" >verifypack.json
python3 -c "import json; d=json.load(open('verifypack.json')); assert d['ok'] is True; assert d['data']['verified'] is True; assert 'objects' in d['data']"
```

负向步骤：

```bash
cd "$RUN_DIR/pack-source"
! libra verify-pack "$RUN_ROOT/fixtures/$SCENARIO/missing.idx"
cp "$PACK_IDX" "$RUN_ROOT/fixtures/$SCENARIO/corrupt.idx"
printf 'corrupt' >> "$RUN_ROOT/fixtures/$SCENARIO/corrupt.idx"
! libra verify-pack "$RUN_ROOT/fixtures/$SCENARIO/corrupt.idx"
```

断言：`index-pack` 仅作为隐藏内部 fixture 生成器使用；`verify-pack` 默认从 idx sibling 推导 `.pack` 路径；`--pack` 显式路径可验证同一 pack；`-v` 输出对象 hash/offset；`-s` 输出统计摘要；`--json` 输出 `verified=true`、object count、pack/index hash 等结构化字段；缺失或损坏 idx 必须失败且错误包含受影响路径。fixture 来源固定为仓库内 `tests/data/packs/small-sha1.pack` 复制到 `$RUN_ROOT/fixtures/$SCENARIO/`，不得读取开发者真实 `.git/objects/pack` 或 `.libra/objects/pack`。

补充可执行断言：
- `libra --json verify-pack "$PACK_IDX"` 必须 `ok:true`；单 idx 时 `data.verified == true`，多 idx 时 `data.packs[].verified` 全为 true。
- 损坏 idx 场景 `libra verify-pack corrupt.idx` 必须非 0，stderr 包含路径或 corrupt 信息。
- 操作后在生成 pack 的仓库执行 `libra fsck` 通过。
- 验证 `--json` 输出包含 "objects" 数组。

### `libra db/remote/object` 覆盖表

| 参数或子命令 | 场景 ID | 关键断言 |
|---|---|---|
| `db status` / `db --json status` | `cli.schema-upgrade-observable` | schema 状态可读且结构化输出可用于断言 |
| `db upgrade` | `cli.schema-upgrade-observable` | 当前 schema 下幂等成功，非仓库失败 |
| `clone <remote> <path>` | `cli.clone-fetch-pull-local` | 本地 remote 可 clone，文件和 log 可见 |
| `clone --bare` | `cli.clone-fetch-pull-local` | bare clone 使用 bare 布局 |
| `clone --single-branch -b <branch>` | `cli.clone-fetch-pull-local` | 指定分支被检出 |
| `remote add` / `remote -v` / `remote get-url` | `cli.clone-fetch-pull-local`、`cli.push-local-file-remote-rejected` | remote URL 可写入、列出和读取 |
| `remote set-url --push` | `cli.push-local-file-remote-rejected` | push URL 可设置并由 get-url 观察 |
| `ls-remote` / `ls-remote --heads` | `cli.clone-fetch-pull-local` | 本地 remote refs 可查询 |
| `fetch <remote> <refspec>` | `cli.clone-fetch-pull-local` | fetched ref/object 可由 show-ref 或 pull 观察 |
| `fetch --all` | `cli.clone-fetch-pull-local` | 所有已配置 remote 被刷新 |
| `pull --ff-only <remote> <refspec>` | `cli.clone-fetch-pull-local` | fast-forward 后工作区包含远端新增内容 |
| `pull --rebase <remote> <refspec>` | `cli.clone-fetch-pull-local` | 本地提交重放到 upstream 新提交之上 |
| `push <local-path> ...` | `cli.push-local-file-remote-rejected` | 本地 file remote push 被拒绝并返回 LBR-CLI-003 |
| `push --dry-run` | `live.github-create-push-clone-fetch` | 真实网络 remote 上预览更新但远端 ref 不变 |
| `push -u <remote> <refspec>` | `live.github-create-push-clone-fetch` | 写入远端并设置 upstream |
| `push <src>:<dst>` | `live.github-create-push-clone-fetch` | 指定目标 ref 被创建 |
| `push --tags` | `live.github-create-push-clone-fetch` | 本地 tag refs 推送到远端 |
| `push --mirror` | `live.github-create-push-clone-fetch` | 镜像同步只作用于临时 GitHub 仓库 |
| `push --force <remote> <ref>` | `live.github-create-push-clone-fetch` | 非快进改写被普通 push 拒绝、被 --force 覆盖 |
| `push <remote> :<dst>` | `live.github-create-push-clone-fetch` | 远端 ref 被删除 |
| `rev-parse` / `rev-list` | `cli.object-readback` | revision 可解析且祖先列表可读 |
| `show` / `show <rev>:<path>` | `cli.object-readback` | commit 元数据、统计和文件内容可读回 |
| `show-ref` | `cli.object-readback`、`cli.clone-fetch-pull-local` | HEAD、heads、tags refs 可观察 |
| `cat-file -t/-s/-p/-e` | `cli.object-readback` | 对象类型、大小、内容和存在性可验证 |
| `hash-object -w` / `--stdin` / `--stdin-paths` | `cli.object-readback` | 文件、stdin 和路径列表可计算 blob id，写入对象可读回 |
| `fsck` / `fsck --connectivity-only` | `cli.object-readback`、`cli.schema-upgrade-observable` | 健康仓库完整性检查通过 |
| `verify-pack` / `verify-pack --pack` / `-v` / `-s` | `cli.verify-pack-smoke` | pack/index 成对验证、对象列表和统计输出可观察 |
| `init --object-format sha256` 端到端 | `cli.sha256-object-readback` | sha256 仓库对象 id 为 64 位 hex 且可提交/读回 |

通过标准：全部场景 green。Wave 2 只覆盖版本管理相关的 schema、本地 protocol/client 和对象读写行为；不得要求真实云凭据。

## 4.3 Wave 3：GitHub 真实远端场景（按需运行）

Wave 3 覆盖需要 GitHub 真实远端确认的 clone/fetch/pull/push/remote/ls-remote 行为。它不是默认无凭据阻断门，但一旦某次改动声明触达真实远端语义，就必须运行或给出明确 skip/block 原因。

### `live.github-create-push-clone-fetch`

目的：验证 `libra` 能和通过 `gh` 创建的 GitHub 临时仓库完成真实远端闭环。

前置条件：

1. `gh auth status --active --hostname github.com` 退出码为 0。
2. 当前账号有创建私有仓库和删除测试仓库权限；若没有删除权限，不启动场景。
3. 本机具备 Libra 访问所选远端 URL 的认证能力。默认使用 `sshUrl`，因此需要 GitHub 已配置可用 SSH key；HTTPS 只在 Libra 明确配置了可记录、可隐藏的认证来源时使用。

最小步骤：

```bash
BINARY="$(pwd)/target/debug/libra"
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
RUN_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/libra-integ-$RUN_ID.XXXXXX")"
mkdir -p "$RUN_ROOT"/{home,xdg-config,xdg-cache,repos,fixtures,logs,artifacts}
OWNER="$(gh api user --jq '.login')"
REPO="$OWNER/libra-integ-$RUN_ID"

# Dynamic git and ssh bin resolution
SAFE_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
GIT_BIN="$(command -v git || true)"
case ":$SAFE_PATH:" in *":$(dirname "${GIT_BIN:-/usr/bin/git}"):"*) ;; *)
  [ -n "$GIT_BIN" ] && SAFE_PATH="$SAFE_PATH:$(dirname "$GIT_BIN")" ;; esac
SSH_BIN="$(command -v ssh || true)"
case ":$SAFE_PATH:" in *":$(dirname "${SSH_BIN:-/usr/bin/ssh}"):"*) ;; *)
  [ -n "$SSH_BIN" ] && SAFE_PATH="$SAFE_PATH:$(dirname "$SSH_BIN")" ;; esac

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
    SSH_AUTH_SOCK="${SSH_AUTH_SOCK:-}" \
    LANG=C LC_ALL=C \
    "$BINARY" "$@"
}

gh auth status --active --hostname github.com
gh repo create "$REPO" --private --disable-issues --disable-wiki \
  --description "Temporary Libra integration test $RUN_ID"
trap 'gh repo delete "$REPO" --yes' EXIT

REMOTE_URL="$(gh repo view "$REPO" --json sshUrl --jq '.sshUrl')"
gh repo view "$REPO" --json nameWithOwner,isPrivate,isEmpty,url,sshUrl

cd "$RUN_ROOT/repos"
libra init source
cd source
libra config set user.name "Libra GitHub Integration"
libra config set user.email "libra-integration@example.invalid"
printf 'github remote\n' > README.md
libra add README.md
libra commit -m "test: github integration"
libra remote add origin "$REMOTE_URL"
libra push --dry-run origin main
libra push -u origin main

REMOTE_MAIN_SHA="$(gh api "repos/$REPO/git/ref/heads/main" --jq '.object.sha')"
test "$REMOTE_MAIN_SHA" = "$(libra rev-parse HEAD)"

libra branch feature/live main
libra switch feature/live
printf 'feature branch\n' > feature.txt
libra add feature.txt
libra commit -m "test: github feature branch"
libra push origin feature/live:feature/pushed
libra tag v-live-smoke
libra push --tags origin
gh api "repos/$REPO/git/ref/tags/v-live-smoke" --jq '.object.sha' >/dev/null
libra push origin :feature/pushed
libra push --mirror --dry-run origin
libra push --mirror origin

libra switch main
printf 'forced rewrite\n' >> README.md
libra add README.md
libra commit --amend --no-edit
FORCED_MAIN="$(libra rev-parse HEAD)"
set +e
libra push origin main >non-ff.out 2>non-ff.err
NON_FF_STATUS=$?
set -e
test "$NON_FF_STATUS" -ne 0
libra push --force origin main
test "$(gh api "repos/$REPO/git/ref/heads/main" --jq '.object.sha')" = "$FORCED_MAIN"

cd "$RUN_ROOT/repos"
libra clone "$REMOTE_URL" cloned
cd cloned
libra log --oneline
grep 'forced rewrite' README.md

cd "$RUN_ROOT/repos/source"
printf 'second commit\n' >> README.md
libra add README.md
libra commit -m "test: github second commit"
libra push origin main

cd "$RUN_ROOT/repos/cloned"
libra fetch origin
libra pull origin main
grep 'second commit' README.md
```

断言：

1. `gh repo create` 创建的是当前账号名下的临时私有仓库，`gh repo view` 可查询到 `nameWithOwner`、`isPrivate`、`sshUrl`。
2. `libra remote add`、`push --dry-run origin main`、`push -u origin main`、refspec push、tag push、delete refspec、`push --mirror --dry-run`、`push --mirror`、`push --force`、`clone`、`fetch`、`pull` 均退出码为 0。
3. `gh api repos/<owner>/<repo>/git/ref/heads/main` 能看到被推送的 `main` ref，且 normal push 在非快进 rewrite 后必须失败、`push --force` 后远端 main 才更新到 `FORCED_MAIN`。
4. clone 后 `log --oneline` 能看到首次/force 后提交；pull 后工作区能看到第二次提交内容。
5. 日志不得包含 GitHub token、PAT、SSH 私钥、`gh auth token` 输出或带明文凭据的 URL。
6. 场景结束后 `gh repo delete "$REPO" --yes` 成功；失败时报告 `cleanup_required` 并列出仓库名。

补充可执行断言（Wave 3 最高价值）：
- 关键步骤后执行 `libra --json log -n 1` 并验证 `ok:true`。
- `gh api` 返回的 sha 与本地 `libra rev-parse HEAD` 一致（initial push 与 force push 后都 capture 比对）。
- 整个运行使用完整隔离 `libra()`（含 TMPDIR + SAFE_PATH + LIBRA_TEST）。
- 强制要求 `trap 'gh repo delete ... --yes' EXIT` 且 cleanup 状态明确记录。
- 推荐验证 `libra --json show-ref --heads` 在 clone 后可解析。

通过标准：真实 GitHub 仓库创建、push、远端 ref 查询、clone、fetch/pull 和删除全部成功。若失败是认证、权限、GitHub 服务或本机网络问题，报告必须区分环境失败与 Libra 行为失败。

补充可执行断言（Wave 3 最高价值场景）：
- 每个 `libra` 操作（init、push、clone、fetch、pull）均使用完整隔离 `libra()` wrapper（含 TMPDIR + SAFE_PATH）。
- 关键步骤后执行 `libra --json log -n 1` 并验证 `ok:true` + 提交存在。
- `gh api` 查询与 `libra show-ref` 结果必须一致（至少覆盖 main、tag、删除后的 feature ref、force push 后 main）。
- 强制要求 trap + `gh repo delete --yes`，失败时明确记录 `cleanup_required`。
- 整个 Wave 3 运行日志必须通过 §3.6 脱敏自检（无 token/PAT/私钥）。
- 推荐在 runner 中捕获 `gh api` 返回的 sha 与本地 `libra rev-parse` 比对。

---

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

**自动捕获机制（让“失败命令 + 错误信息”可机器获取）**：每个场景在独立子 shell 里以 `set -Eeo pipefail` + `ERR` trap 执行；trap 用 `$BASH_COMMAND` 抓到正是失败的那条命令，配合场景 stderr 尾部即可生成上面的 `failure`：

```bash
# runner 核心：单场景执行 + 失败捕获（$BASH_COMMAND = 失败命令；! libra ... 的预期失败不触发）
run_scenario() {
  sid="$1"; fn="$2"; wave="$3"          # fn 为封装好该场景步骤的 bash 函数
  sdir="$RUN_ROOT/logs/$sid"; mkdir -p "$sdir"
  (
    set -Eeo pipefail
    trap 'rc=$?; printf "%s\n" "$BASH_COMMAND" >"'"$sdir"'/fail.cmd"; echo "$rc" >"'"$sdir"'/fail.exit"' ERR
    "$fn"
  ) >"$sdir/scenario.out" 2>"$sdir/scenario.err"
  rc=$?
  if [ "$rc" -eq 0 ]; then
    status=pass; failcmd=""; failexit=0
  else
    status=fail
    failcmd="$(cat "$sdir/fail.cmd" 2>/dev/null)"      # 失败命令
    failexit="$(cat "$sdir/fail.exit" 2>/dev/null || echo "$rc")"
  fi
  emit_ndjson "$sid" "$wave" "$status" "$failcmd" "$failexit" \
    "$(tail -n 20 "$sdir/scenario.err")"                # stderr 尾部 → stderr_tail
}
```

要点：`set -E` 让 `ERR` trap 继承进场景函数；`! libra …` 这类**预期失败**被 `set -e` 视为成功、不触发 trap，因此负向步骤不会误报；而 `test -f X` / `grep` 这类断言失败会真实触发，正是我们要记的 `fail`。

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
- `rerun-failed.txt` — 每行一个失败场景 ID，供下一轮 `runner --only "$(paste -sd, rerun-failed.txt)"` 只重跑失败项。
- 分 wave 原始日志（沿用旧契约）：`wave0-build.log` / `wave1-cli-core.log` / `wave2-cli-storage-protocol.log` / `wave3-github-live.log`（未运行写 skip/block 原因）。

### 5.6 退出码与 CI 对接

runner 进程退出码：`0` = 无 `fail`（`skip`/`env-skip` 不算失败）；`1` = 至少一个 `fail`；`2` = 前置/编译失败（Wave 0 未过）。CI 以退出码门控，以 `report.json.totals.fail == 0` 复核，并把 `failures.md` 贴进失败 job 摘要。**`env-skip` 不得让 CI 变绿掩盖问题**：当某 wave 因环境缺失被整体 `env-skip` 时，runner 退出码仍为 0 但必须在 stdout 和 `summary.md` 顶部高亮 `WARN: <wave> env-skipped (<reason>)`，由 reviewer 判断是否可接受。

报告中所有 URL / 路径 / 命令在写盘前都必须通过 §3.6 的脱敏自检；命中即标记该 run 为 `redaction_self_check: "leak-blocked"` 并拒绝归档。

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

### BASELINE_GAP-INTEG-001：CLI 场景 runner 未落地

- 现状：本计划定义了应执行的 CLI 黑盒场景，但仓库尚未提供统一 runner 来自动记录命令、退出码、输出和断言。
- 需要补充：一个本机脚本编译 `libra`，创建隔离临时仓库，按场景执行 `libra <cmd>`，并**严格产出 §5 输出契约**——L1 实时行、L2 `results.ndjson`、L3 `report.json` + `summary.md` + `failures.md` + `rerun-failed.txt`；用 §5.3 的 `set -Eeo pipefail` + `ERR` trap 机制捕获失败命令与 stderr 尾部；退出码遵循 §5.6。
- 约束：runner 只能驱动编译后的 `libra` 命令；不得把 Cargo `--test` 目标重新包装成默认集成门；所有写盘产物先过 §3.6 脱敏自检。

### BASELINE_GAP-INTEG-002：CLI 场景清单自动校验不足

- 现状：集成计划场景清单的自动一致性校验仍未落地；本计划已经转向 CLI 场景 ID，因此暂无自动校验场景清单与未来 runner 的一致性。仓库已无 `scripts/` 目录，新增校验应是自包含 Rust 测试或 CI 步骤（仿 `tests/compat/matrix_alignment.rs`），而非 `scripts/*.sh`。
- 需要补充：校验逻辑应扫描本文件中的 `cli.*` 场景 ID，确保 runner 覆盖同名场景，并确保默认 Wave 中没有 Cargo `--test` 目标。

### BASELINE_GAP-INTEG-003：Path -> Wave 自动选择脚本未落地

- 现状：§3.5 仍靠作者手动对照。
- 需要补充：一个本机脚本读取改动路径，输出建议 CLI wave 集合。
- 约束：脚本只输出版本管理 CLI wave；不得引入交互界面、agent runtime、provider、publish 或云服务 wave。

### BASELINE_GAP-INTEG-004：GitHub live 场景 runner 与清理保护未落地

- 现状：Wave 3 已定义 `gh` 驱动的 GitHub 临时仓库测试流程，但仓库尚未提供自动 runner 来统一执行 preflight、仓库创建、日志脱敏、失败保留和 `gh repo delete` 清理。
- 需要补充：一个 live runner，执行 `gh auth status`、创建临时私有仓库、运行 `live.github-create-push-clone-fetch`、用 `gh api` 断言远端 ref，并在 `EXIT` 路径强制清理。
- 约束：runner 不得输出 token，不得复用人工仓库，不得在 cleanup 能力不足时创建 GitHub 仓库。

### BASELINE_GAP-INTEG-005：版本管理命令黑盒场景覆盖不完整

- 现状：§2.3 矩阵已建立，并已为 tag、merge/rebase/cherry-pick/revert、grep/blame/describe/shortlog、clean/rm/mv/lfs、reflog/symbolic-ref、verify-pack 添加独立黑盒场景和参数表；`cli.cross-cutting-flags` 已覆盖成功 JSON envelope + 错误 JSON（`ok:false` + `LBR-*`）的基本形态。
- 需要补充：继续细化未纳入默认闭环的深水区：`pull --rebase` 真分叉冲突路径、LFS 远端 lock API、更多 pack corpus 的 `index-pack`/`verify-pack` 深度 fixture、`open` JSON 无副作用行为是否足够代表真实 open；**故意差异的正向断言**（push 拒绝本地文件 remote、symbolic-ref 仅 HEAD 等）需在对应场景中显式存在并随矩阵更新。
- 约束：任何新增场景必须是可在本机无密钥确定性复现的 `libra <cmd>` 黑盒；不得引入 live AI/cloud。
- 跟踪：§2.3 矩阵 + 对应 Wave 场景 + PR Test Plan 清单。

### BASELINE_GAP-INTEG-008：集成计划一致性检查未落地

- 现状：兼容矩阵漂移与 Code UI docs 一致性检查已**去脚本化**、落地为 `tests/compat/matrix_alignment.rs`（随 `cargo test --all` 运行；CI 另以 `cargo test --test compat_matrix_alignment` 单独 gate）。**仅剩**集成计划场景清单（`cli.*` / `live.github-*` ID ↔ 本文件/未来 runner）的自动一致性校验尚未实现；仓库已无 `scripts/` 目录。
- 需要补充：一个自包含检查（Rust 测试或 CI 步骤，**非 `scripts/*.sh`**），至少校验本文件 §2.3 矩阵与 `src/cli.rs` / `COMPATIBILITY.md` 一致（顶层命令部分已由 `compat_matrix_alignment` 覆盖）、默认 Wave 不含 Cargo `--test` 门、所有 `cli.*` / `live.github-*` 场景 ID 可被 runner 或文档解析。
- 约束：该检查未落地前，PR/Test Plan 只能把集成计划一致性标为 `not_available` 或 `blocked_by BASELINE_GAP-INTEG-008`，不得声称已通过；不得为此新建 `scripts/` 目录。

### BASELINE_GAP-INTEG-009：深水区远端语义与全局 flag 边界

- 现状：本轮已补 force-push、`fetch --all`、`pull --rebase`（无冲突重放）、sha256 端到端、全局 flag 集中断言，并定义了 `clone --depth` 本地 Git fixture 场景（`cli.fetch-depth-local`）。以下仍未纳入默认确定性闭环：
  - **`fetch --depth` 补充语义 + 真实远端 shallow**：`cli.fetch-depth-local` 覆盖本地路径 `clone --depth` 目标语义；如果当前代码在该场景失败，应修 shallow clone 对象闭包，而不是改测试绕过。`fetch --depth` 对已 shallow clone 的增量获取、以及 shallow 语义在真实 GitHub 远端上的对等性，仍建议在 Wave 3 验证。
  - **`pull --rebase` 真分叉 + 冲突续跑**：当前只覆盖不同文件的无冲突重放；普通 `merge`/`rebase` 冲突续跑已由 `cli.merge-conflict-continue` / `cli.rebase-conflict-continue` 覆盖，但 `pull --rebase` 驱动的远端分叉冲突仍属深水区。
  - **`push --force-with-lease`**：当前 `push` 已有 `--force`/`-f`、`--tags`、`--mirror`，但无 lease 安全 force；如未来新增需补场景。
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

### 8.4 本计划自检（去脚本化）

集成计划一致性检查（BASELINE_GAP-INTEG-008，落地为自包含 Rust 测试或 CI 步骤，**不新建 `scripts/`**）应逐步覆盖：

1. 本计划默认 Wave 中不包含 Cargo `--test <name>` 集成门。
2. 本计划里所有 `--features <flag>` 出现在 `Cargo.toml [features]`。
3. 本计划没有把明确排除的测试类别写进默认 Wave 0/1/2。
4. CLI runner 落地后，本计划里所有 `cli.<scenario-id>` 都被 runner 覆盖。
5. GitHub live runner 落地后，本计划里所有 `live.github-*` 场景都被 runner 覆盖，并校验包含 `gh repo create` 与 `gh repo delete`。
6. quarantine 文件里每条 CLI / live 场景 ID 可解析为现有场景。
7. **错误 JSON 契约**：关键失败路径在 `--json`/`--machine` 下必须产出 `ok:false` + `LBR-*` 稳定码 + category/hints 的可解析 envelope（已在 `cli.cross-cutting-flags` 基线覆盖）。
8. **故意差异防护**：COMPATIBILITY.md 中 `intentionally-different` 条目在对应场景中有正向断言（非仅文档声明）。
9. **git fixture 隔离**：所有使用 `git` 的场景都定义并调用 `gitfix()` 包装（与 `libra()` 对称），无裸 `git` 调用。

---

## 9. 维护规则

1. **新增命令或修改公共表面**（`src/cli.rs` / `src/command/*.rs`）：必须同步更新 §2.3 覆盖矩阵，并在相应 Wave 补充至少一个 `cli.<cmd>-smoke` 黑盒场景（含参数表 + 负向用例），全部使用 §3.3.1 规范模板。
2. 新增版本管理集成测试时，必须把 `libra <cmd>` 场景补到本计划相应 Wave，并在 CLI runner 落地后同步 runner 清单。
3. 删除/重命名场景 ID 时，必须同步更新本计划、CLI runner、集成计划一致性检查（BASELINE_GAP-INTEG-008）和 quarantine 文件。
4. 新增默认阻断测试必须能在本机无密钥、无外部账号、无交互界面的环境中确定性运行。
5. 未实现能力必须用 `BASELINE_GAP-*` 标记，不允许写成默认可执行步骤。
6. 若某测试需要真实网络、真实云资源或外部凭据，不得加入本计划的默认 wave。
7. 需要 GitHub 真实远端的版本管理测试必须进入 Wave 3，仓库创建、查询、API 断言和删除必须使用 `gh`。
8. 所有示例代码块与 runner 实现必须通过 §3.6 安全自检清单；CI / 人工 review 发现违规时阻断合并。
9. §2.3 矩阵、COMPATIBILITY.md 与 `src/cli.rs` 三者必须保持一致；改动任一者需运行 `cargo test --test compat_matrix_alignment`（顶层命令漂移检查）并更新本计划。
