# Git 兼容性改进计划

本文档是 Libra 参照 Grit 改进 Git 命令与参数兼容性的执行计划。本文只描述规划，不改变 Libra 当前的兼容性承诺；任何具体兼容行为的变更，仍需要同步更新命令文档、`COMPATIBILITY.md`、集成场景和测试（流程见「单条目执行手册」）。

## 计划执行实施规则（12条强制要求）

以下是执行本计划时的**强制操作规程**（用户 2026-06 明确要求）。所有参与者（包括 AI agents）必须严格遵守：

0. 出现任何其他的内部服务错误，也不暂停 goal 的任务，可以设置暂停 10 分钟，重试到任务重新开始；

1. 每一个改进验收需要符合 `cargo +nightly fmt --all --check` 无格式差异，`LIBRA_SKIP_WEB_BUILD=1 cargo clippy --all-targets --all-features -- -D warnings` 无警告，`source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test 指定测试用例` 全部通过；涉及 Web embed 的改动不得设置 `LIBRA_SKIP_WEB_BUILD=1`，必须按 Web embed check 单独验证；

2. 可以启动 Subagent 进行并行分析，但改动由主 Agent 进行；

3. 每一个分析出来的改动需求，改动完成后是对当前版本 patch 加 1 ，发布一个新的版本，例子：当前的版本如果是 version = "0.17.500" ，则 patch 加 1 后 version = "0.17.501"，worker 和 web 子目录下的 package.json 中的版本同步保持。不修改 Cargo.lock ，而是执行 cargo build --release 命令，让工具链进行修改，完成修改版本后调用 libra 命令执行 libra add / libra commit -a -s -m / libra push origin main 推送到 GitHub , 构建出来的 release 版本的 libra 命令，拷贝到 $HOME/.libra/bin/libra 这个路径；

4. 根目录下的 .env.test 文件有对应的 API Key 可以使用进行测试，同时 本地已经启动了 ollama ，可以调用 ollama 的 kimi-k2.6:cloud 进行相关测试；

5. 本地是使用 Libra 作为版本管理，不要当做一个 Git 仓库进行分析；

6. 所有都在 main 分支进行，不要开 worktree 和 branch;

7. 如果 push 失败，则不进行重试，到下一次修复完成的时候再 push ；

8. 使用 .env.test 文件作为测试执行的环境变量；

9. 如果一次改进只是修改了文档，并没有修改代码，不进行提交，只有到有修改代码了再一起按照 3 的要求进行提交；

10. 对于文档需要和代码的实现核对，核对落地的部分是否完整，不能仅通过文档确定未实现的部分;

11. 使用系统目录下的 libra 命令。对于本计划执行中的所有版本管理操作（add、commit、push、status、branch 等），必须使用系统目录下安装的 libra 命令（例如 $HOME/.libra/bin/libra 或 PATH 中解析到的系统 libra），而非本地构建的 target/debug 或 target/release 下的二进制（除非规则 3 明确要求构建 release 后拷贝覆盖系统路径）。

这些规则优先于其他流程，是本计划在 Libra 格式仓库中执行的最高约束。

**注意（rule 12 交叉核对结果）**：本文档当前为精简 stub（执行规则 + 占位）。详细的 PRE 条件、阶段（C1-C9）、矩阵、Grit 参考、集成测试方案、declined 证据等活文档已分散到：
- `docs/improvement/compatibility/README.md`（C1-C9 全景 + 路线图）
- `docs/improvement/compatibility/declined.md` + `governance.md`
- 根 `COMPATIBILITY.md`
- `docs/development/integration-test-plan.md` + `integration-scenarios.yaml` + `tools/integration-runner/`
- `tests/compat/`（所有 guard 已在 Cargo.toml [[test]] 注册，`tests/compat/README.md` 有准确 inventory 表）

交叉核对确认（subagent + 主 agent 工具验证）：
- 兼容 guard 存在且注册完整（`compat_matrix_alignment` 等 20+ 个，`Cargo.toml:166-256` + `tests/compat/README.md` 表匹配）。
- 集成方案（yaml + runner + check-plan + Command→Scenario Map + §2.3）与代码一致，无覆盖缺口。
- `.libra_attributes`、SQLite sequencer 表（`cherry_pick_state` 等迁移）、declined 证据锚点均与 src/ 实现匹配。
- 已知不准确点已在本节更新（原 "parameter_matrix_alignment" guard 不存在；主 guard 是 `compat_matrix_alignment`；Grit 仅为可选补充，无大量树内 artifact）。
- 未来对本文档的任何编辑都必须重新执行 rule 12 交叉核对（读 src/cli.rs、COMPATIBILITY.md、runner registry、scenarios/*.rs、相关迁移等），并运行 `cargo test --test compat_matrix_alignment` + `cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan`。

（以下为**已恢复的原执行计划详细内容**。内容基于最初的完整计划文本，结合交叉核对结果进行了必要的事实更新和说明，以确保与当前代码基线一致。）

## 计划状态

| 项 | 值 |
|----|----|
| 状态 | 规划中 — 前置条件 PRE-1～PRE-4 完成前，不得进入批量实现；只允许做事实核对、矩阵建模、拒绝项登记和守护测试建设 |
| 最后核对代码现状日期 | 2026-06-11（subagent 复核：compat guard 23 个、declined D1–D10、39 个集成场景、`compatibility-matrix.yaml` 仍未建、`StableErrorCode`/`LBR-UNSUPPORTED-001`、`cherry_pick_state`/`revert_sequence` 迁移、`.libra_attributes` 仅承载 LFS，均与本文一致） |
| Grit 参考版本 | **可选补充**（见 PRE-1 修订；主要参考为公开的 upstream Git 文档与 git.git `t/` 测试套件；Grit 如可用则作为额外测试思路来源，但不作为前置阻塞项）。已核对实体：`/run/media/eli/data/gitbutler/grit` @ `166c45534319cb280e7a026336a8a06c53cea30f`（2026-06-09，workspace `0.3.99`，1,605 个 per-test TOML，~46k 测试 ~91% 通过） |
| 参数矩阵（规范来源） | 待建：`docs/development/compatibility-matrix.yaml`（见 PRE-2；建成前以 `COMPATIBILITY.md`、`src/cli.rs`、`docs/development/integration-scenarios/_parameter-tables.md` 和本文种子清单联合核对；不得把本文占位摘要当作完整规范） |

## 对标分析：Grit 经验与 Libra 选型差异（2026-06-10）

> 本节基于对 GitButler Grit（`/run/media/eli/data/gitbutler/grit`，commit `166c45534319cb280e7a026336a8a06c53cea30f` 2026-06-09，2026-06-11 重新核对）的架构、测试策略和兼容性模型的实地分析，明确 Libra 应借鉴什么、应避免什么。

### Grit 的核心经验

Grit 是从头重实现 Git 的 Rust 项目（workspace 版本 `0.3.99`，169 个命令实现），其兼容性策略可概括为**"以上游测试套件结果为主真相源、手工文档划定范围边界"的混合模型**：

1. **手工范围边界 + 自动测试状态的混合模型**（2026-06-11 复核修正，原稿表述需更正）。Grit 确实没有 `COMPATIBILITY.md` 式 feature-by-feature 的四 tier 人工分类表，但它**并非"纯测试真相源、无任何人工文档"**：范围边界由手工文档 `docs/v1-scope.md`（in-scope / out-of-scope 两列表）与 `docs/manpage-parity.md`（11 个 plumbing 命令的行为核对清单，当前多为未填模板）划定；`data/tests/**/*.toml` 共 1,605 个、每测试文件一份，是**自动生成的测试状态缓存**（字段 `tests_total` / `passed_last` / `failing` / `fully_passing` / `status`），其中仅 `in_scope` 是手工覆盖字段。换言之 Grit 的行为真相是"运行上游 `t/` 套件的结果"（约 46,364 个测试、~91% 通过、1,303 个文件全绿），TOML 只是机器维护的状态镜像，HTML dashboard 由 Python 脚本据其生成（opt-in）。
2. **行为等价优先于语义优雅**。Grit 的设计哲学是 *"Where Git's behavior is surprising, Grit reproduces the surprise."* 这意味着在其范围内不依赖人工判断某个 Git 行为是否"合理"，而是以测试通过作为兼容的度量。
3. **性能回归防护制度化（但与正确性闸门解耦）**。`bench/OPTIMIZATION.md` 的机制是两条独立轨道：(a) `bench/run-everyday.sh` 用 `hyperfine` 跨 S/M/L/H 四种仓库规模量化耗时；(b) 任何优化后，相关 `t/` 家族重跑 + `data/tests/` 通过总数（`sum(passed_last)`）不得下降作为**正确性闸门**。性能数字本身不进测试断言，而是用通过数防止"为提速牺牲正确性"。
4. **范围文档极简但明确，且显式承认 partial**（2026-06-11 复核修正）。`docs/v1-scope.md` 用 in-scope / out-of-scope 两列清晰划定边界，**并单列"已在范围内但尚未全绿"的 partial 区**（submodule 标 🟡；`t1092-sparse-checkout-compatibility` 64/106；`t4202-log` 90/149；`t5616-partial-clone` 44/47）。关键差异：Grit 的 partial 是**客观的测试通过率**（自动跟踪），不是主观的"待决策"中间态——它确实没有 Libra `evaluate` 那种悬而未决、需要人工定夺的分类，但"partial 中间态"本身是存在的。
5. **最小外部依赖与显式上下文**。workspace 默认 `unsafe_code = "forbid"`、clippy `unwrap_used` / `expect_used` = `deny`；唯一例外是 `grit-lib` 为 `git_date` 的 libc FFI（对标 Git `date.c`）显式 `allow` unsafe。时间/环境通过 `std::time` / `std::env` 显式读取而非隐藏全局状态。注意 Grit 另 vendor 了整棵上游 Git C 源码（629 个 `.c`，仅作参考与少量 crypto FFI 绑定）——Libra 不应复用该 C 树（与 PRE-1 一致）。

### Libra 与 Grit 的根本选型差异

| 维度 | Grit | Libra | 差异原因 |
|------|------|-------|----------|
| **产品目标** | 成为 Git 的 drop-in 替代 | AI-native VCS，选择性兼容 Git | Libra 的一等目标是 AI 工作流、SQLite-backed refs、Vault signing、云存储和结构化输出，不是 Git 等价物 |
| **存储架构** | 复刻 Git 对象模型（loose + pack + reftable） | SQLite 为 refs/HEAD/reflog/sequencer 事实源 | Libra 的存储边界是产品核心差异，不可妥协 |
| **兼容性声明方式** | 上游 `t/` 套件通过率（自动）+ `v1-scope.md` 手工 in/out-of-scope 边界 | 四 tier 人工矩阵（supported/partial/unsupported/intentionally-different） | Libra 需要显式管理用户预期，因为大量 intentional difference（如 `.libra_attributes` 替代 `.gitattributes`）无法通过"测试是否通过"来表达；且 Libra 无法整体运行上游 `t/`，矩阵天然更依赖人工 |
| **测试策略** | 直接跑上游 Git `t/` 测试套件（~46k 测试，~91% 通过） | 自建 integration-runner + 选择性映射 upstream 用例 | Libra 不能跑完整 Git `t/`（大量测试假设 `.git/` 布局和 Git 存储语义），只能提取代表性用例 |
| **hook/filter 策略** | 复刻 Git hooks / clean / smudge（在 v1 in-scope 内） | 明确拒绝 stock Git hooks；LFS 使用内置 pointer 管理 | Libra 的安全模型要求 fail-closed，避免任意用户脚本执行 |
| **范围边界重叠度** | out-of-scope 含 interactive UX（add -p / rebase -i）、复杂 HTTP 认证、fsmonitor、send-email/shell/scalar 等 | 非目标含 submodule/subtree/sparse-checkout/patch UI/interactive rebase/server-side 等 | 两者的非目标**高度重叠**——"drop-in 替代 vs 选择性兼容"的对比应弱化；真正的根本差异是存储架构与 AI-native 目标，而非是否愿意划边界 |

### Libra 应借鉴 Grit 的具体要点

1. **降低人工矩阵权重，提升自动化测试的权威地位**。
   - `COMPATIBILITY.md` 和 `compatibility-matrix.yaml` 是**管理工具**，不是**真相源**。
   - 当矩阵与自动化测试结果冲突时，以测试为准；矩阵必须被修正。
   - 每个 `supported` 或 `enhance` 行在标记 `done` 时，必须附带 `test_evidence`（通过的测试/场景 ID）。

2. **引入 per-parameter 状态跟踪，而非仅依赖命令级 tier**。
   - Grit 的 per-test TOML 可对应到 Libra 的 per-parameter YAML 行：每行不仅记录 `action`，还记录**最后验证通过的测试证据**和**验证日期**。
   - 这防止矩阵在代码演变后变成"僵尸状态"（标注 supported 但实际测试已失效）。
   - **重要限定（2026-06-11 复核）**：这是类比而非等价。Grit 能以"测试通过=兼容"作为近乎唯一的度量，是因为它直接运行 ~46k 个上游 `t/` 测试作为 ground truth，并用自动生成的 TOML 缓存其状态；Libra 无法整体运行 Git `t/`（大量用例假设 `.git/` 布局与 Git 存储语义），只能把代表性用例提取进自建 integration-runner。因此 Libra 的矩阵**天然更依赖人工维护**，"测试即真相源"在 Libra 落地为：每行绑定一个 integration-runner 可执行场景 + `test_evidence` + `last_verified`，并由 guard / check-plan 自动刷新，而**不是**靠移植成千上万条上游测试来逼近 Grit 的自动度量。

3. **建立性能 no-regression gate 的等价机制**。
   - **Grit 的实证教训（2026-06-11 复核）**：Grit 在 1 万文件级仓库上比 C Git 慢约 50×–1030×（最差 `grep` 1031×、`stash` 1030×、`merge` 945×），根因是在循环内**逐文件重复加载 `.gitattributes` 与 config 级联**；其修复计划（`bench/OPTIMIZATION.md` P1–P5）以缓存为主，且每个缓存必须在相关 mutation（config 写入、`.gitattributes` 变更）上失效。Libra 在 Phase 5 实现 attributes/config 读取时应**从一开始就避免 per-file 重复解析**，并以 config/attributes 测试家族作为缓存失效的守护。
   - **Grit 的 gate 机制是解耦的**：性能用 `hyperfine` 单独量化（不进测试断言），正确性用"测试通过总数不下降"作闸门。Libra 不应把耗时断言与正确性测试混为一谈。
   - **Libra 的对应做法（二选一或并用）**：既可在 integration-runner 增加**性能断言切片**（大仓库 `status` 超时上限、`log` 流式输出内存上限），也可建立独立 bench 脚本并在矩阵 `performance_note` 中声明输入上限/超时/流式策略。任何影响大历史、大文件、batch、regex、network、pathspec、config、attributes、ignore、filter 的改动，必须在 PR 中附带可复核的性能边界证据。

4. **拒绝悬而未决的"待决策"中间态**。
   - Grit 的 partial 是客观测试通过率，没有 Libra `evaluate` 那种**需要人工定夺、可能无限期停留**的分类；因此 Libra 的 `evaluate` 条目必须有明确的决策期限（`decision_deadline`）和责任人（`decision_owner`），到期未决默认 `reject`。
   - Libra 的 `partial` tier 必须具体到"哪些 flag 已支持、哪些未支持"，不能作为笼统的免责条款——这与 Grit 用通过率精确表达 partial 是同一精神。

5. **不 chasing Git 的每一个 surprised behavior（但对比应弱化）**。
   - Grit 在其 v1 in-scope 范围内 reproduce Git 的 surprise，但它**同样**在 `v1-scope.md` 中把 interactive UX（add -p / rebase -i）、复杂 HTTP 认证、fsmonitor、send-email/shell/scalar/daemon 等大类划为 out-of-scope——这些与 Libra 的非目标高度重叠。
   - 因此"Libra 才有权声明 intentional difference"是过度表述：两者都划边界。Libra 真正不同于 Grit 的是**存储架构（SQLite-backed refs/HEAD/reflog/sequencer）与 AI-native 目标**，而非"是否愿意拒绝某些 Git 行为"。
   - Libra 的目标是"足够兼容以支持常见脚本和工具链"，对 Git 的 edge-case surprise 有权声明 intentional difference，只要该差异在 `COMPATIBILITY.md`、命令文档和 runner 断言中三方一致。

6. **机器生成兼容性状态视图，避免矩阵漂移（Grit dashboard 的等价物）**。
    - Grit 由 `data/tests/` 的 TOML 自动生成 HTML dashboard + SVG 徽章（`scripts/generate-dashboard-from-test-files.py`，opt-in、只读），使"哪些命令/测试未绿"始终可见，无需人工巡检。
    - Libra 的等价物（低成本、可作 Phase 0 增项）：由 `compatibility-matrix.yaml` + integration-runner 最近一次运行结果，生成一个只读状态视图，或在 `check-plan` 输出中汇总 `status` / `last_verified` / `test_evidence` 覆盖率与缺口。这让"矩阵声明与自动化测试不一致"在 CI 中显式暴露，是 PRE-2 guard (f)/(i) 的自然延伸，落实了"测试即真相源"原则。

### Grit 再分析补充：对 Libra 方案的多维评估（2026-06-11）

本节基于对 `/run/media/eli/data/gitbutler/grit` 的实际读取补充评估，重点参考：`AGENTS.md`、`TESTING.md`、`docs/v1-scope.md`、`docs/manpage-parity.md`、`bench/OPTIMIZATION.md`、`grit-lib/README.md`、`grit-protocol/src/{upload_pack,receive_pack}.rs`、`data/tests/**/*.toml`。结论是：Libra 当前方案方向正确，但必须把"评估意见"固化为机器矩阵字段和 CI 守护，否则容易变成一份无法驱动实现的长文档。

| 维度 | Grit 观察 | 对 Libra 方案的评估 | 本文档已纳入的改进要求 |
|------|-----------|--------------------|------------------------|
| 合理性 | Grit 明确目标是通过 upstream Git test suite；`docs/v1-scope.md` 另行划定 v1 in/out-of-scope。 | Libra 不是 drop-in Git，不能照搬 Grit 的"测试通过率=兼容"模型；以 Git 文档/测试为主、Grit 为补充是合理的。 | PRE-1 保持 Grit optional，不因 Grit 不可用阻塞；每个测试来源必须标注 `git.git:` 或 `grit:`。 |
| 可行性 | Grit 拥有完整 `tests/` harness 与 per-file TOML 自动状态树；Libra 现有 runner 更轻量。 | 全量迁移 Git `t/` 不可行；选择性抽样 + owner scenario 可行。 | 单条目流程要求至少一条端到端测试 + 一个 Git/Grit 代表性用例，不要求整文件迁移。 |
| 完整性 | Grit 的 `docs/manpage-parity.md` 使用命令级 checklist，状态保守；`data/tests/` 覆盖面广但仍需 scope 文档解释。 | Libra 的命令级 `COMPATIBILITY.md` 不足以证明参数级完整性。 | PRE-2 要求 `compatibility-matrix.yaml` 从 `src/cli.rs`、命令文档、Git docs、runner 场景联合生成，并禁止 `unclassified` 条目进入 Phase 1。 |
| 安全性 | Grit v1 scope 包含 hooks、GPG/SSH signing、server receive-pack 等高风险 Git 表面；protocol crate 会 spawn `grit upload-pack/receive-pack` 子进程并容忍部分非零退出。 | Libra 的安全边界应更严格：不桥接 stock hooks、不引入外部 GPG、不公开 server-side Git hosting，不把子进程协议封装当作默认方案。 | 高风险安全门新增 protocol/subprocess、hook/filter、external-command 约束；服务端命令继续 non-goal。 |
| 功能正确性与接口兼容性 | Grit 追求 Git surprising behavior 复现，且测试里允许 `test_expect_failure` 表示已知缺口。 | Libra 只能声明具体 command/flag 的兼容等级，不能泛称 Git-compatible；`accepted-no-op` 必须单列。 | PRE-2 增加 `compat_contract` 字段；验收要求 human、JSON、exit code 和 docs 三方一致。 |
| 数据流与控制流正确性 | Grit 复刻 `.git` 文件布局和 server protocol 流程；Libra 的事实源是 SQLite refs/HEAD/reflog/sequencer。 | Libra 不得为兼容 Git 测试引入 `.git/refs`、`.git/sequencer`、packed-refs 作为事实源。 | 架构不变量明确 refs/sequencer/index/object/network 的事实源、事务/锁和失败出口。 |
| 性能与效率 | Grit `bench/OPTIMIZATION.md` 显示 per-file 重复加载 attributes/config 会导致 50x-1000x 级退化。 | Libra 一旦扩展 attributes/config/pathspec/filter，必须从设计期建立 per-command 缓存和边界测试。 | `performance_note` 必填范围扩大到 pathspec/config/attributes/filter；大仓库条目必须说明缓存失效策略。 |
| 可靠性与容错性 | Grit TOML 状态写入使用 temp file + rename；每个测试文件独占状态，减少并发冲突。 | Libra 矩阵/状态视图也应避免人工手改状态漂移和并发写冲突。 | 状态视图只读生成；`last_verified` / `test_evidence` 由 guard/check-plan 校验，不由叙述性文档单独背书。 |
| 兼容性与互操作性 | Grit 同时实现客户端与部分服务端 smart protocol；v1 scope 承认真实环境中部分测试由 host git 参与。 | Libra remote 互操作必须区分本地 fixture、真实 remote、host tool 依赖；不能把 host Git/GH 行为误判为 Libra 能力。 | Remote 阶段要求 Wave 3、日志脱敏、host-tool 依赖标注；local file remote push 继续 fail-closed。 |
| 可扩展性与可维护性 | Grit 将核心逻辑放入 `grit-lib`，CLI 薄封装；测试状态自动生成 dashboard。 | Libra 应保持 command handler 薄边界和机器可读兼容状态，避免文档矩阵僵化。 | PRE-2 schema 增加 `source_of_truth`、`compat_contract`；Phase 0 产出机器状态视图或 check-plan 汇总。 |
| 合规性与标准符合性 | Grit 以 Git docs/test suite 为标准，但也明确 out-of-scope 与 known partial。 | Libra 的合规表述必须是"选择性兼容 + 明确差异"，尤其涉及 hooks、filters、GPG、server commands。 | 非目标、declined、COMPATIBILITY、命令文档、runner 断言必须一致；禁止宣传完全 Git 兼容。 |

### 本次评估形成的新增硬性约束

1. **兼容契约必须可枚举**：每个矩阵行必须说明该 flag/command 的契约是 `git-compatible`、`libra-extension`、`intentional-difference`、`accepted-no-op`、`explicitly-rejected` 之一；不能只写 `supported`。
2. **来源优先级必须固定**：冲突时按 `Libra implementation tests` → `integration-runner` → `COMPATIBILITY.md`/命令文档 → upstream Git docs/tests → Grit 补充的顺序裁决；Grit 不覆盖 Libra 明确差异。
3. **server-side 与 subprocess 默认拒绝**：任何公开 `upload-pack`、`receive-pack`、`http-backend`、daemon-style hosting，或通过子进程桥接 Git/protocol/filter/hook 的方案，默认 `risk=high` 且 `action=reject`，除非单独 RFC 证明必要性、安全边界、超时、环境清理和失败语义。
4. **性能门前移到设计期**：pathspec、attributes、config、ignore、regex、history traversal、batch object、network 类条目必须在实现前写明缓存/流式/分页/超时策略；不得先实现再补性能说明。
5. **状态视图不得手工维护**：类似 Grit `data/tests/` 的状态只能由测试/runner/check-plan 刷新；人工文档只能维护范围、理由和风险，不维护"最后通过"事实。

## 本轮再评估结论（2026-06-11）

结论：方案方向合理，但原稿仍有三类会影响落地的风险，必须先收敛后再实现大批量兼容项。

| 维度 | 结论 | 必须修正/执行的控制点 |
|------|------|------------------------|
| 合理性 | 合理。以 upstream Git 文档与 git.git `t/` 为主参考，Grit 降级为可选补充，符合可验证来源优先原则。 | 不再把 Grit 缺失作为阻塞；所有行为差异以 `COMPATIBILITY.md` + declined 登记簿为准。 |
| 可行性 | 有条件可行。现有命令矩阵、runner、错误码、SQLite sequencer、集成场景基建可复用。 | Phase 0 只能先建机器矩阵和 guard；不能直接按本文省略表格开工。 |
| 完整性 | 顶层命令覆盖较完整，但参数级内容仍是种子/摘要，不能证明无遗漏。 | `compatibility-matrix.yaml` 必须从 `src/cli.rs` clap 定义、`COMPATIBILITY.md`、命令文档和 Git 文档联合生成初稿，并记录未分类项。 |
| 安全性 | 基本方向安全，但 attributes/filter、hooks、local file remote、server-side command 是高风险边界。 | 默认 fail-closed；新增任何会执行外部命令、读取用户配置、访问网络或写远端的兼容项，必须有独立威胁分析和负向测试。 |
| 功能正确性与接口兼容 | 通过单条目流程可控，但必须避免"接受 flag 但静默 no-op"的新行为。 | `partial`/`intentionally-different` 必须在 help、命令文档、`COMPATIBILITY.md` 和 JSON/错误 envelope 中一致。 |
| 数据流与控制流 | SQLite-backed refs/HEAD/reflog/sequencer 是正确边界。 | 禁止引入 `.git/refs`、`.git/sequencer`、packed-refs 作为事实来源；所有状态变更必须经过现有事务/锁路径。 |
| 性能与效率 | 分阶段、小切片可控，但**缺乏性能基准与回归防护机制**。Grit 的 no-regression gate（测试通过数不可降）证明性能优化必须与正确性测试挂钩。 | 每个参数至少 1 条端到端测试 + 1 条代表性 upstream/Grit 用例；**大历史/大文件/batch/regex/network/pathspec/config/attributes/ignore/filter 类条目必须在矩阵中标注 `performance_note`**（输入上限、超时、流式策略、缓存失效）；runner 增加性能断言切片；不得在单 PR 迁移整个 Git `t/` 文件。 |
| 可靠性与容错 | runner 和 stable error code 可支撑可靠回归。 | 拒绝项统一 `LBR-UNSUPPORTED-001`；写操作测试必须覆盖失败不落半状态、重复执行或清理路径。 |
| 兼容性与互操作 | 网络 remote、object/pack、porcelain 常用路径优先是正确排序。 | 真实远端语义触达 `clone/fetch/pull/push/remote/ls-remote` 时必须追加 Wave 3；本地 file remote push 保持拒绝。 |
| 可扩展性与可维护 | 机器矩阵 + owner scenario + check-plan 是正确维护模型，但**人工矩阵权重过高**，存在矩阵与代码实现漂移的风险。Grit 用**自动生成的 per-test TOML 状态缓存 + 手工 `v1-scope.md` 边界**的混合模型缓解此问题——其 TOML 由运行上游 `t/` 套件自动刷新，人工只维护范围边界；Libra 无法整体运行 `t/`，矩阵更依赖人工，因此**机器化刷新与漂移检测更不可或缺**。 | `compatibility-matrix.yaml` schema 必须稳定；每行绑定 owner 场景、declined_ref、测试来源和状态；**新增 `test_evidence` 和 `last_verified` 字段**，由 guard / check-plan 自动刷新（参见「应借鉴」第 6 点的状态视图）；确保矩阵不是"僵尸状态"；当矩阵与自动化测试冲突时，以测试为准修正矩阵。 |
| 合规性与标准 | upstream Git 作为规范来源，Libra 差异显式化，符合项目治理。 | 文档不得声明"Git 完全兼容"；只能声明具体 command/flag 的 tier 与证据。 |

因此，本计划的首要改进不是立即实现更多 Git flag，而是先把"参数级事实来源、状态机、守护测试、风险门"产品化。只有当单个条目满足本文的入口条件和验证门时，才允许进入代码实现。

## 现状基线（2026-06-10 核对）

本计划建立在以下已存在的基建之上；执行时复用它们，不另起炉灶：

- **命令级矩阵已存在**：`COMPATIBILITY.md` 已是四 tier 命令级矩阵（`## Top-level commands` 表，注释中含 flag 级状态），并由 `tests/compat/matrix_alignment.rs` 守护与 `src/cli.rs::Commands` 的对齐。本计划的参数级矩阵是它的细化补充，不是替代。
- **拒绝/延后登记簿已存在**：`docs/improvement/compatibility/declined.md` 是拒绝与延后决策的正式登记簿（D1 submodule、D2 local file remote push、D3 Git hooks bridge、D5 Git LFS `.gitattributes` filters、D8/D9 `stash create/store`、D10 sparse-checkout 等，每项含代码证据锚点与重启条件）。本计划所有「明确拒绝」「保持差异」条目必须与该登记簿对账（PRE-4）。
- **集成场景基建已存在**：39 个场景（`docs/development/integration-scenarios.yaml` + `tools/integration-runner/src/scenarios/`，一一对应），Command → Scenario Map 覆盖约 54 个 Git 兼容命令；跨场景 flag 矩阵已存在于 `docs/development/integration-scenarios/_parameter-tables.md`。
- **错误码基建已存在**：`StableErrorCode`（`src/utils/error.rs`，`LBR-<DOMAIN>-<NNN>`）+ `docs/error-codes.md` + `tests/compat/error_codes_doc_sync.rs` 同步守护。已有泛用 `LBR-UNSUPPORTED-001`，以及「解析后明确拒绝」的实现先例（bisect `--` pathspec 拒绝，`src/cli.rs` 中 `CliError::command_usage(...).with_hint(...)` 模式）。
- **sequencer 状态已落 SQLite**：`cherry_pick_state`（`sql/migrations/2026060401_cherry_pick_state.sql`）与 `revert_sequence`（`2026060801`）两表已存在。计划中「status 报告 cherry-pick in-progress」的存储前置条件已满足。
- **`.libra_attributes` 现状**：目前仅承载 LFS track/untrack（`src/utils/lfs.rs`、`src/command/lfs.rs`）。Phase 5 的 attributes/filter 兼容是在该文件格式之上的扩展，不是新文件。

## 前置条件（阻塞 Phase 0）

### PRE-1：建立命令/参数/测试参考来源（Grit 降级为可选补充）

- **主要参考来源**（强制）：upstream Git 官方文档（git-scm.com/docs） + git.git 仓库的 `t/` 测试目录（公开可 clone，文件名如 `t0001-init.sh`、`t4202-log.sh` 等均为标准 Git 测试套件命名）。这些是命令行为、flag 边界、exit code、输出格式的权威输入。
- **Grit 角色**（可选补充，非阻塞）：Grit（GitButler 项目的 Grit）如可用，可作为额外测试用例思路和边界分析的补充来源。推荐位置：与 gitmono 平行的 `gitbutler/grit`（当前发现路径示例 `/run/media/eli/data/gitbutler/grit` @166c45534 2026-06-09），或通过 `LIBRA_GRIT_PATH` 环境变量指定。仅使用其 `grit/` / `grit-lib/` 下的命令实现、测试数据（t*.sh + data/tests/*.toml）、以及依赖结构分析；**不要**尝试复用其 vendored `git/` C 树或 libgit-sys 绑定。所有使用必须在兼容矩阵 `grit_tests` 字段中标注 commit + 来源。不存在时不影响计划推进（主参考为公开 upstream Git）。
- **动作**：
  1. 在「初始测试套件映射」表中，为每一行标注主要来源（`git.git t/` 或 `Grit` 补充）。
  2. 对引用的 Git `t/` 文件，执行存在性抽样校验（至少确认 git.git 可公开 clone 且对应文件存在于最近 tag/HEAD）。
  3. 如 Grit 可用，在「计划状态」表可选登记其 commit（格式：`Grit (optional): <url-or-path>@<hash> <date>`）；未提供时填写 "unavailable / supplemental only"。
  4. 所有后续矩阵 `grit_tests` 字段的迁移状态记录，必须同时注明来源（`git.git:<path>` 或 `grit:<path>`）。
- **验收**：「初始测试套件映射」表每行有明确来源标注；PRE-1 不因 Grit 缺失而阻塞；PRE-2 yaml 建成后，来源信息进入 `grit_tests` / `tests` 字段的结构化记录。

### PRE-2：参数矩阵的存储格式与守护

- 矩阵落地为机器可读文件 `docs/development/compatibility-matrix.yaml`。本文「参数级改进矩阵」表格是它的种子；yaml 建成后，本文表格降级为非规范性摘要并在表前注明。
- 每行 schema 建议包含以下字段：

| 字段 | 含义 |
|------|------|
| `command` / `flag` | 命令与参数（命令级条目 flag 留空） |
| `action` | `implement` / `enhance` / `reject` / `intentional-diff` / `evaluate` |
| `priority` | P0–P3 |
| `phase` | 0–5（归属实现阶段） |
| `status` | `planned` / `in-progress` / `done` / `blocked`（blocked 必须填原因） |
| `declined_ref` | declined.md 的 D 编号；`reject` / `intentional-diff` 行必填 |
| `owner_scenario` | Command → Scenario Map 中的 owner 场景 id（如 `cli.commit-smoke`） |
| `tests` | 目标/已落地测试位置 |
| `grit_tests` | 参考的 Grit 测试文件 + 迁移状态（`pending` / `ported` / `covered-by-existing` / `rejected-non-goal` / `intentional-difference`） |
| `git_tests` | 参考的 upstream Git `t/` 文件 + 迁移状态；当 `grit_tests` 不适用时本字段仍必填 |
| `risk` | `low` / `medium` / `high`；触达 refs/index/object/network/filter/hook/secret 的条目默认不低于 `medium` |
| `compat_contract` | `git-compatible` / `libra-extension` / `intentional-difference` / `accepted-no-op` / `explicitly-rejected`；用户可见行必填，且必须与 `COMPATIBILITY.md` 和命令文档一致 |
| `source_of_truth` | 本行裁决依据，按优先级列出 `libra-test` / `integration-runner` / `compat-docs` / `git-docs` / `git-tests` / `grit-supplement`；`grit-supplement` 不得单独作为实现依据 |
| `data_flow` | 该 flag 读写的事实源（worktree/index/objects/SQLite refs/config/vault/network/stdout-stderr） |
| `control_flow` | 入口 parser、handler、事务/锁、错误出口、JSON/human render 路径摘要 |
| `test_evidence` | 验证该条目行为的**测试真相源**：`tests/command/<file>::<fn>`、`tools/integration-runner` 场景 ID、或 `git.git t/<file>` 行号；`status=done` 时必填 |
| `performance_note` | 性能边界说明：输入大小上限、超时阈值、流式/分页策略、内存上限、缓存与失效策略；对大历史/大文件/batch/regex/network/archive/filter/pathspec/config/attributes/ignore 类条目必填 |
| `decision_deadline` | `evaluate` 行专用：决策截止日期（ISO 8601）；到期未收敛则按默认规则处理 |
| `decision_owner` | `evaluate` 行专用：负责收敛决策的维护者或团队标识 |
| `last_verified` | 本条目的矩阵状态最后一次与代码/测试核对通过的日期（ISO 8601）；由 guard 或 check-plan 自动更新 |
| `notes` | 备注 |

- 新增或扩展 compat guard。推荐新建 `tests/compat/parameter_matrix_alignment.rs`；如维护者选择复用 `tests/compat/matrix_alignment.rs`，必须在测试名中清楚区分参数矩阵检查。guard 至少校验：
  - (a) 矩阵中命令名 ⊆ `src/cli.rs::Commands` 或根 `COMPATIBILITY.md` 的"intentionally absent"表；
  - (b) `action` 为 `reject`/`intentional-diff` 的行 `declined_ref` 非空且在 declined.md 中存在；
  - (c) `action`/`priority`/`phase`/`status`/`risk` 枚举值合法；
  - (c2) 用户可见行 `compat_contract` 非空且枚举值合法；`action=reject` 必须对应 `explicitly-rejected`，`action=intentional-diff` 必须对应 `intentional-difference`；
  - (c3) `source_of_truth` 至少包含一个 Libra 侧证据源（`libra-test`、`integration-runner` 或 `compat-docs`），不得只包含 `grit-supplement`；
  - (d) 用户可见行为变更行必须有 `owner_scenario`；
  - (e) `git_tests` 或 `grit_tests` 至少一个有来源或明确 `rejected-non-goal`；
  - (f) `status=done` 的行 `test_evidence` 非空且指向已存在的测试或场景；
  - (g) `risk=high` 或涉及大历史/大文件/batch/regex/network/archive/filter/pathspec/config/attributes/ignore 的行 `performance_note` 非空；
  - (h) `action=evaluate` 的行必须有 `decision_deadline` 和 `decision_owner`，且 `decision_deadline` 不得早于矩阵创建日期；
  - (i) `last_verified` 日期不得早于矩阵创建日期，且对 `status=done` 的行不得早于最近一个相关 PR 的合并日期（由 CI 或 check-plan 自动更新）。
- 如果新建 guard，按仓库惯例在 `Cargo.toml` 注册 `[[test]]` 并在 `tests/compat/README.md` 加 inventory 行；如果扩展既有 guard，只更新对应 README 行，不新增虚构测试目标。
- **验收**：yaml 存在且覆盖本文全部参数级条目；guard 随 `cargo test --all` 进 CI 并常绿。

### PRE-3：拒绝错误码决策（已定案）

- **决策（review 确认并记录）**：**统一复用 `LBR-UNSUPPORTED-001`**（不新增 variant）。理由：已有先例（bisect `--` pathspec 拒绝），避免 error code 膨胀；所有拒绝类 flag 共用同一个稳定码，便于用户和脚本统一处理"Libra 明确不支持的 Git 表面"。
- **文案模板（强制）**：
  - stderr human：`<flag> is not supported in Libra: <一句话原因>。`（例如 `rebase -i/--interactive is not supported in Libra: interactive rebase todo-editor workflow is out of scope.`）
  - 必须通过 `.with_hint(...)` 附加：指向替代（若有）、`docs/improvement/compatibility/declined.md#Dn`、或建议使用 Libra-native 能力。
  - JSON/`--machine` envelope：仍使用标准 `{ "ok": false, "error": { "code": "LBR-UNSUPPORTED-001", "message": "..." } }`；`data` 字段可选携带 `flag`、`suggested_alternative` 等结构化信息（由 `CliError` 机制提供）。
- 拒绝行为实现参考：`src/cli.rs` 中 `CliError::command_usage(...).with_hint(...)` + 在 command handler 早期返回的模式。
- **验收**：本节已回写决策；PRE-4 对账时所有 `reject` 行必须填 `declined_ref`；Phase 0 所有 P0 拒绝测试必须统一断言 `LBR-UNSUPPORTED-001` + 非零 exit + stderr 包含模板片段 + `--json` envelope 含该 code。新增/修改 `StableErrorCode` 时仍必须同步 `docs/error-codes.md`（guard 强制）。

### PRE-4：与 declined.md 登记簿对账

- 把本文所有「明确拒绝」「保持差异」行与 `docs/improvement/compatibility/declined.md` 互相对账：已有 D 编号的在矩阵中填 `declined_ref`；没有的新增 D 条目（含证据锚点、理由、重启条件）。
- **验收**：PRE-2 guard 的 (b) 项检查通过；declined.md 无与本计划矛盾的条目。

## 目标

- 优先增强 Libra 已经暴露的用户可见 Git 命令。
- 选择性新增少量高价值 Git plumbing 命令，用于支持脚本、测试和工具链，而不是把 Libra 变成完整 Git 重实现。
- 保留 Libra 的产品边界：AI-native 工作流、SQLite-backed refs、Vault-backed secrets/signing、云存储和结构化输出仍是一等设计目标。
- 将公开的 upstream Git 文档与 `t/` 测试套件作为命令与参数发现、边界行为分析、上游测试优先级排序的主要参考；Grit（如可用）仅作为可选的补充测试思路来源（详见 PRE-1 修订版）。

## 非目标

以下 Git 功能和命令族明确不在本兼容计划范围内：

| 范围 | 决策 | 原因 |
|------|------|------|
| `submodule` | 不实现 | 产品边界。Libra 不承载 Git 的嵌套仓库工作流。已有 submodule 相关参数应继续明确拒绝或记录为 unsupported。 |
| `subtree` | 不实现 | 产品边界。它不是 Libra 期望支持的仓库组合模型。 |
| `sparse-checkout` | 不实现 | 产品边界和存储/index 边界。Libra 不提供 Git sparse-checkout 工作流；相关命令和参数应继续明确拒绝或记录为 unsupported。 |
| Git hooks (`.git/hooks`, `core.hooksPath`) | 不实现 | Libra 不桥接 stock Git hooks。替代方案是 Libra-native hooks：`.libra/hooks/*.sh` 或 `.libra/hooks/*.ps1`，以及 Libra agent/automation hooks。 |
| Git patch-selection UI | 不实现 | 不实现 `add -p`、`commit -p`、`checkout -p`、`restore -p`、`reset -p`、`stash -p` 的 Git prompt/hunk-selection UI。未来如需补丁选择，应作为 Libra-native TUI/agent surface 设计。 |
| Git interactive rebase | 不实现 | 不实现 `rebase -i`、`--interactive`、`--edit-todo` 的 Git todo-editor workflow。Libra 继续支持非交互 linear rebase、autosquash 和结构化 rebase 状态。 |
| `daemon` | 不实现 | Git daemon 属于 server-side 行为，不属于 Libra CLI 兼容目标。 |
| `shell` | 不实现 | server/admin shell surface 不在 Libra 范围内。 |
| `scalar` | 不实现 | Scalar 风格仓库管理不是 Libra 的目标。 |
| `send-email` | 不实现 | 邮件补丁工作流不在 Libra 产品重点内。 |
| Server-side Git commands | 不实现 | 包括 server-side `upload-pack`、`receive-pack`、`http-backend`、daemon-style hosting 以及相关 admin/helper surface。Libra 可以保留支持 `clone`/`fetch`/`push` 的内部 protocol client/encoder，但不应公开 server-side Git hosting 命令。 |

当这些能力以参数形式出现在已支持命令上时，推荐行为是用稳定的 Libra 错误码明确拒绝，并在文档中说明这是有意边界。

## 兼容性原则

- 优先增强现有 Libra 命令，再考虑新增命令。
- 优先处理高频 porcelain 命令，再处理低层 plumbing 命令。
- 对有意差异保持显式记录，避免静默近似 Git 行为。
- 保留全局 `--json` 和 `--machine` 行为；结构化输出不能破坏 Git-compatible human output。
- refs 和 reflog 继续以 SQLite 为事实来源，不引入 flat `.git/refs` 或 packed-refs 作为 Libra 的主存储。
- 签名继续使用 Vault-backed 机制，不把外部 GnuPG 作为核心依赖。
- 公开的 upstream Git 文档与 `t/` 测试套件是参数覆盖和测试思路的主要参考；Grit（如可用）仅作为可选补充（见 PRE-1 修订版）。
- **所有的改动都需要同时更新集成测试方案**（见上文「计划执行实施规则」第 1 条的详细定义）。这是本计划所有实现工作的硬性约束，与 AGENTS.md 中 Git 兼容命令变更的同步义务一致。
- 不新增隐式外部程序执行路径。任何涉及 editor、pager、filter、hook、credential helper、ssh、git、gh、browser launcher 的行为都必须显式记录触发条件、环境变量继承范围、脱敏规则和非 TTY/`--json`/`--machine` 下的拒绝或降级策略。
- 不新增静默兼容。一个 Git flag 若尚不能正确实现，只能明确拒绝、标记 accepted-no-op 且文档化、或保持不暴露；不得为了"看起来兼容"接受后无提示忽略。
- **测试即真相源**。`COMPATIBILITY.md` 和 `compatibility-matrix.yaml` 是管理预期的人造工具，不是行为真相。当矩阵声明与自动化测试结果冲突时，以自动化测试为准修正矩阵。每个标记为 `supported` 或 `enhance` 的条目在 `status=done` 时必须附带 `test_evidence`（具体通过的测试名、场景 ID 或 upstream Git `t/` 行号），防止矩阵在代码迭代后变成"僵尸状态"。
- **来源裁决顺序固定**。当 Libra 现有实现、integration-runner、`COMPATIBILITY.md`/命令文档、upstream Git 文档/测试、Grit 补充材料之间出现冲突时，按 `Libra implementation tests` → `integration-runner` → `COMPATIBILITY.md`/命令文档 → upstream Git docs/tests → Grit 补充的顺序处理。Grit 只能提示测试思路或边界案例，不能推翻 Libra 已登记的 intentional difference 或 declined 决策。

## 架构与数据流控制流不变量

这些不变量用于评估每个参数条目的功能正确性、接口兼容性和安全性；违反任一项都必须先写设计说明再实现。

| 领域 | 不变量 | 验证方式 |
|------|--------|----------|
| CLI 入口 | 新增/变更公开 flag 必须从 `src/cli.rs` clap grammar 进入，并接入命令 examples/help。 | `cargo test --test compat_matrix_alignment`；命令 help 抽样；命令文档同步。 |
| 输出 | Human stdout/stderr 与 `--json`/`--machine` 结构化输出都要定义；错误码稳定。 | command tests 断言 human 片段 + JSON envelope；拒绝项断言 `LBR-UNSUPPORTED-001`。 |
| Refs/HEAD/reflog | 仍以 `.libra/libra.db` 为事实来源；不读写 `.git/refs` 或 packed-refs。 | 代码 review 检查路径访问；相关场景运行 `show-ref`/`reflog`/`fsck`。 |
| Index/worktree | 写工作区和 index 的命令必须先完成冲突/覆盖检查，再进入写入；失败时不留下半更新 index。 | 负向测试覆盖 dirty/unmerged/missing path；失败后重复 `status`/`fsck`。 |
| Objects/pack | 对象写入必须保持 hash-kind pinning；sha1/sha256 行为不可混淆。 | sha1 与 sha256 场景分别验证 `hash-object`/`cat-file`/`fsck`。 |
| Sequencer | merge/rebase/cherry-pick/revert 状态继续使用 SQLite 表或现有状态模型；不引入 Git 文件 sequencer。 | 冲突续跑场景断言 `status` repo_state 与 continue/abort/skip 行为。 |
| Network | 真实远端语义只能在明确 remote 类型与能力协商后执行；本地 file remote push 继续 fail-closed。 | 本地拒绝场景 + Wave 3 live 场景；日志脱敏自检。 |
| Protocol subprocess | Git protocol helper 不得通过公开子命令或未受控子进程桥接绕过 Libra handler、事务、锁和输出 envelope。 | review 检查 `Command::new`/外部 helper；协议帧测试覆盖 stdout/stderr 分离、非零退出、超时、env 清理。 |
| Secrets | Vault、credential、token 不得进入 stdout/stderr、JSON、panic、debug log 或 runner artifact。 | no_secret_leak 类断言；review 检查 tracing 字段。 |

## 高风险兼容项安全门

以下类别默认 `risk=high`，不得只按普通 flag 流程落地：

| 类别 | 风险 | 允许推进的最低条件 |
|------|------|--------------------|
| clean/smudge filters、textconv、diff drivers、archive attributes | 可执行任意外部命令、读取敏感文件、产生非确定性对象内容。 | 独立 RFC；默认禁用；白名单/超时/大小上限/环境清理；`--json`/非 TTY 行为定义；拒绝测试。 |
| hooks bridge、`core.hooksPath`、`.git/hooks` | 执行用户脚本并扩大供应链攻击面。 | 保持拒绝，除非 agent hooks 统一方案完成并重新评估 D3。 |
| local file remote push、server-side Git 命令 | 并发写入、锁、权限和数据损坏风险。 | 保持拒绝；重启需 RFC 描述锁与原子性。 |
| protocol subprocess bridge（如公开 `upload-pack`/`receive-pack`/`http-backend` 或 spawn 外部 Git/Grit helper） | 子进程环境继承、stderr/stdout 协议混流、非零退出容忍、超时和资源上限错误都可能造成协议不一致或信息泄露。 | 默认拒绝；如确需内部使用，必须是非公开实现细节，设置固定 argv、清理 env、超时/kill、stderr 脱敏、协议帧边界测试，并证明不会绕过 SQLite refs/事务边界。 |
| interactive patch/rebase UI | TTY 状态机复杂，难以自动化验证，易破坏 agent 驱动。 | 保持拒绝；未来只能作为 Libra-native TUI/agent surface 重新设计。 |
| external GnuPG/keyring | 与 vault-backed signing 冲突，密钥边界不清。 | 保持 vault-backed；不得引入外部 GnuPG 作为核心依赖。 |

## 工作流

### 1. 命令与参数盘点

前置：PRE-1（修订版：公开 Git 来源已建立，Grit 如可用则登记补充信息）、PRE-2（矩阵格式与 guard 已定）。

- 从公开 upstream Git 文档 + `src/cli.rs::Commands` + `COMPATIBILITY.md` 提取当前 Git/Libra 命令与参数基线（`matrix_alignment.rs` 已保证 cli 与 COMPATIBILITY 一致）。
- **可选**：若 Grit 可用，从其 `README.md` / `src/commands/*.rs` 提取补充的命令/flag 发现与边界测试思路。
- 建立三方参考：标准 Git 行为、Libra 当前状态、可选 Grit 覆盖建议。
- 对 Libra 已存在的命令，逐项对比上游 Git 文档中的参数；Grit 提供的额外用例仅用于丰富测试选择，不强制对齐其实现状态。
- 将每个缺失或不同的参数按 PRE-2 schema 的 `action` 字段归类（`implement` / `enhance` / `reject` / `intentional-diff` / `evaluate`），并填入 `phase`、`priority`、`declined_ref`。

交付物：`docs/development/compatibility-matrix.yaml`（PRE-2 定义的格式），由参数矩阵 guard 守护（可新建 `parameter_matrix_alignment`，也可扩展现有 compat guard），从本文档链接。本文「参数级改进矩阵」各表作为种子数据导入，导入时按「与代码现状的已知校正」一节修正。盘点时**必须**同时从 `src/cli.rs` + `COMPATIBILITY.md` 提取 Libra 现状（Grit 清单仅作参考对比，不得作为 Libra 必须对齐的规范）。

### 2. 现有 Porcelain 命令增强

第一阶段聚焦 Libra 已经暴露且用户高频使用的命令。

优先级不是"补齐 Git 全部参数"，而是按用户频率、风险和现有架构契合度排序：

| 命令族 | 优先处理 | 谨慎/拒绝边界 | 主要 owner 场景 |
|--------|----------|---------------|----------------|
| commit/status/add | message/editor/dry-run/porcelain/pathspec-from-file 等已暴露能力的正确性与文档一致性；status repo_state 补足。 | patch-selection UI、外部 GnuPG、未建模 index bit（如 intent-to-add）不得静默模拟。 | `cli.commit-status-log`、`cli.cross-cutting-flags` |
| restore/checkout/reset | path restore、conflict stage、bulk pathspec、dirty/unmerged 安全拒绝。 | `-p` patch UI、Git C-quoting 长尾、覆盖未检查路径。 | `cli.restore-reset-diff`、`cli.branch-switch-checkout` |
| clean/rm/mv | fail-closed 删除语义、dry-run、nested repo 保护、JSON failed 列表。 | 删除类命令必须优先数据安全；不得为了 Git parity 降低默认保护。 | `cli.clean-rm-mv-lfs-basic` |
| stash/worktree | stash untracked/ignored/index 行为、worktree intentional differences 回归防护。 | worktree 不得被宣传为 Git-style branch isolation；stash create/store 仍按 D8/D9 延后。 | `cli.stash-bisect-worktree` |

### 3. Branch、History 和 Diff 增强

本阶段只推进可由现有 object/ref/index 模型稳定支持的 history 查询与 diff 输出。

| 命令族 | 可推进方向 | 风险控制 |
|--------|------------|----------|
| branch/tag/show-ref/for-each-ref 类 | sort/filter/format 子集，基于 SQLite refs 和对象元数据实现。 | 格式 mini-language 必须分阶段；未知 atom fail-closed，不输出错误数据。 |
| log/show/shortlog/describe | 多 root、路径过滤、pretty format 子集、rename follow 的已知限制文档化。 | 大历史遍历需要上限/流式输出；`--json` schema 只做 additive 扩展。 |
| diff/grep/blame | rename/word diff/context/pathspec 能力补强。 | regex、binary、大文件、颜色输出和 no-index 路径必须有大小/路径安全边界。 |

### 4. Merge 与 Sequencer 增强

Sequencer 类命令的正确性优先级高于 flag 数量。任何增强都必须证明冲突、继续、跳过、退出、abort 的状态转移完整。

| 状态 | 允许动作 | 必须保持的控制流 |
|------|----------|------------------|
| clean | start merge/rebase/cherry-pick/revert | 创建状态前完成 dirty/unmerged 检查；失败不写状态。 |
| in-progress | continue/skip/abort/quit/status | 从 SQLite 状态恢复；每个出口清理或保留状态的规则明确。 |
| conflict | restore conflict files/status JSON/human hint | 冲突标记、index 状态和 repo_state 一致；禁止自动覆盖用户解决结果。 |

`rebase --exec`、custom merge drivers、rerere 等会执行外部命令或引入复杂状态缓存，默认不进 P1；进入前需独立设计。

### 5. Remote 命令增强

Remote 相关改动必须区分本地确定性场景、真实远端互操作和明确拒绝项。

| 类别 | 默认测试门 | 说明 |
|------|------------|------|
| 本地 remote 可模拟语义 | `run --only cli.clone-*`、`cli.fetch-depth-local` 等 | clone/fetch/pull 的对象闭包、tag auto-follow、shallow 边界可在本地 fixture 验证。 |
| 真实远端语义 | Wave 3 `live.github-*` | 触达 push/fetch/pull/remote/ls-remote 真实协议差异时必须跑；需 `gh` 清理和脱敏。 |
| 明确拒绝 | 负向场景 | local file remote push、submodule recurse、server-side commands 继续 fail-closed。 |

### 6. 选择性新增高价值 Plumbing 命令

只有当 plumbing 能支持脚本、测试或现有 porcelain 命令兼容性时才新增。

P1 候选：`ls-files`、`ls-tree`、`write-tree`、`read-tree`、`update-index`、`update-ref`、`for-each-ref`、`check-ref-format`。

新增 plumbing 的接受条件：

| 条件 | 要求 |
|------|------|
| 明确调用方 | 至少一个现有 porcelain、integration-runner 断言或外部脚本兼容场景需要它。 |
| 事务安全 | `update-ref`、`read-tree`、`update-index` 等写命令必须使用现有 SQLite/index 事务边界。 |
| 输出契约 | Human 输出尽量 Git-compatible；`--json`/`--machine` 为 additive Libra envelope。 |
| 场景登记 | 新命令必须新增 Command → Scenario Map 行和 `cli.<cmd>-smoke`。 |

### 7. Object、Pack、Attributes 与 Filter 兼容

- 将 `cat-file` 向 `--batch-command` 等扩展。
- `.libra_attributes` 当前仅是 Libra LFS tracking 事实源。任何向 diff/archive/textconv/CRLF/filter 扩展的设计，必须先定义它与 `.gitattributes` 的优先级、冲突处理和迁移路径；不得默认读取 `.gitattributes` 后执行外部 filter。
- clean/smudge filter、textconv 和 diff driver 兼容默认归入高风险类别；除非安全 RFC 通过，否则应明确拒绝并记录为 intentional difference。
- pack/object 增强必须覆盖 sha1/sha256、loose/pack、缺失对象、promisor/shallow 边界和 `fsck` 可观测性。

### 8. 输出、退出码与错误信息 Parity

- 建立高优先级命令的 Git/Grit/Libra exit-code matrix；Grit 不可用时使用 Git 文档与 git.git `t/` 结果。
- 每一个 intentional difference 都必须记录到 `COMPATIBILITY.md` 和命令文档。
- 接受但 no-op 的 flag 必须显式列为 `accepted-no-op` 或在 notes 中说明，不得混入 `supported`。
- 对脚本用户，稳定错误码优先于 prose；对人工用户，错误信息必须包含失败对象、路径/ref/remote 和可执行修复建议。

### 9. 单条目执行手册（每个矩阵条目的标准流程）

每个矩阵条目按下列顺序落地。PR 粒度：一个参数、或同一命令内强相关的小参数簇一个 PR（目标 ~100–300 行）；不要把多个命令的参数改动捆在一个 PR。

1. **矩阵**：把 `compatibility-matrix.yaml` 对应行置 `in-progress`；`evaluate` 行必须先按「待决策条目」一节收敛为四种终态之一才能开工。
2. **实现**：`src/cli.rs`（flag 定义）+ `src/command/<cmd>.rs`。拒绝类条目使用 PRE-3 定案的错误码与文案模板。
3. **契约文档**：`COMPATIBILITY.md` 行内 flag 注释 → `docs/commands/<cmd>.md` → 若新增 `StableErrorCode` variant，同步 `docs/error-codes.md`。
4. **测试**：`tests/command/` 下相关测试。按 action 分类（implement/enhance、reject 五件套、intentional-diff 等）。
5. **集成场景与计划矩阵（强制）**：用户可见 Git 兼容行为变化时，找到 owner 场景，同步 `docs/development/integration-scenarios/<id>.md` + `integration-scenarios.yaml` + `tools/integration-runner/src/scenarios/<id>.rs`；**必须**同步 `docs/development/integration-test-plan.md` §2.3 版本管理命令黑盒覆盖矩阵。
6. **验证**（全部必须绿）：`cargo +nightly fmt --all --check`、`LIBRA_SKIP_WEB_BUILD=1 cargo clippy --all-targets --all-features -- -D warnings`、`LIBRA_SKIP_WEB_BUILD=1 cargo test --all`（含各种 compat guard）、`cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan`、`cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --only <owner-ids>`。触达真实远端语义时追加对应 `run-live --only <ids>`。
7. **收尾**：矩阵行置 `done`，回填 `tests`、`git_tests`、`grit_tests`、`test_evidence`、`last_verified`、`compat_contract` 与 `source_of_truth`；若测试结果与矩阵声明冲突，先修正矩阵再合并实现。

Grit 和 upstream Git tests 作为测试选择输入；覆盖非目标的测试必须在矩阵中记为 `rejected-non-goal` 并转换成拒绝行为测试，不得静默跳过。

新增命令（Phase 4 plumbing）比新增 flag 多出以下义务：Command → Scenario Map 新增一行 + 至少一个 `cli.<cmd>-smoke` 场景；`COMPATIBILITY.md` 主表新增一行；`docs/commands/<cmd>.md` 新页（含 Examples）；`src/cli.rs` 中 `<CMD>_EXAMPLES` 常量接入 `after_help`（三个 guard 强制）。

## Grit / Git 测试补全目标

（前置 PRE-1。upstream Git `t/` 是主参考；Grit 的 `tests/t*.sh` 和 `data/tests/**/*.toml` 仅在可用时作为补充输入。目标不是直接运行全部测试，而是按分类把用例转换为 Libra 测试或拒绝行为测试。）

### 测试分类规则

| 分类 | 含义 | Libra 目标 |
|------|------|------------|
| 适用 | 测试覆盖 Libra 已支持或本文档计划支持的命令/参数。 | 转换为 Libra command test 或 integration-runner scenario。 |
| 拒绝行为 | 测试覆盖已确认 non-goal 或明确拒绝的参数。 | 不实现 Git 行为；新增稳定错误码、stderr/stdout 和 exit code 测试。 |
| 保持差异 | 测试覆盖 Libra intentional difference。 | 增加差异行为测试，确保不会被误改成 Git 行为。 |
| 依赖新增 plumbing | 测试依赖 `ls-files` 等计划新增命令。 | 绑定到对应 plumbing 阶段。 |
| 跳过 | 测试覆盖 submodule、server-side Git 等非目标。 | 在矩阵中记录为 non-goal，不进入实现 backlog。 |

### 初始测试套件映射（来源已修订）

Phase 0 应在 `compatibility-matrix.yaml` 中记录第一批测试来源，而不是在本文维护不可校验的长表。初始来源至少覆盖下列 Git `t/` 文件族，并按命令映射到 owner 场景；Grit 可用时只追加补充行。

| 来源族 | 覆盖方向 | 矩阵记录要求 |
|--------|----------|--------------|
| `t0001-*`、`t1400-*` | init、refs、symbolic-ref、show-ref 基础行为 | 标注 hash-kind/ref storage 差异；SQLite refs 不实现的行为填 declined/int-diff。 |
| `t2200-*`、`t2070-*` | add、checkout、restore、pathspec | patch UI、sparse、特殊 quoting 必须分类，不得静默跳过。 |
| `t3903-*`、`t7500-*` | stash、commit message/editor/cleanup | editor/TTY 与 `--json`/非 TTY 安全边界必填。 |
| `t3200-*`、`t3300-*` | branch、notes、tag/ref 查询 | sort/format atom 子集和未知 atom fail-closed 行为必填。 |
| `t4202-*`、`t4013-*`、`t7810-*` | log、diff、grep 输出与过滤 | 大历史、大文件、regex dialect 和 binary 行为标风险。 |
| `t3400-*`、`t3500-*`、`t7600-*` | rebase、cherry-pick、merge/revert sequencer | 状态机、conflict、continue/abort/skip 路径必须有黑盒场景。 |
| `t5500-*`、`t5510-*`、`t5520-*`、`t5600-*` | clone、fetch、pull、push、remote | 区分本地 fixture、Wave 3 live、明确拒绝；token 脱敏必填。 |
| `t1006-*`、`t1007-*`、`t1010-*`、`t5000-*`、`t5300-*` | cat-file、hash-object、tree/index/archive/pack plumbing | sha1/sha256、loose/pack、缺失对象、属性/filter 高风险分类必填。 |

### 测试补全执行要求（review 修订版）

- **聚焦 + 代表性原则（review 强制）**：每实现/补强一个参数级目标，**至少**提供一条端到端行为测试 + 至少一个直接来自 Git `t/` 或 Grit 的代表性用例。不得要求在同一 PR 内迁移整个测试文件的所有合理用例。允许使用 `covered-by-existing`。
- 对 `明确拒绝` 的参数，必须补充五件套测试。
- 每个被分类的测试来源文件都应在后续矩阵中记录迁移状态。

## 待决策条目与决策规则

矩阵中的 `evaluate` 与 `明确拒绝或实现` 都是未完成的决策，不能带入实现阶段。规则：
- 每个该类条目在进入其所属 Phase 前，必须收敛为 `implement` / `enhance` / `reject` / `intentional-diff` 之一。
- **决策期限**：`evaluate` 行必须在矩阵中填写 `decision_deadline`（默认自条目创建起 14 天内）和 `decision_owner`（明确的责任人标识）。
- **默认决策规则**：到期未收敛的条目，自动按 `reject` 处理，登记为新的 declined 条目（含重启条件），并同步更新 `COMPATIBILITY.md` 和命令文档。不得因为"还未决定"而让 `evaluate` 条目无限期停留在矩阵中。
- 决策清单与截止点（add -N、stash create/store、commit --porcelain、clone/fetch --upload-pack、push --force-if-includes、rebase --empty=ask、cherry-pick --rerere-autoupdate、merge drivers、cat-file 小写 -z、log -L / gc 等）。

## 参数级改进矩阵

> 本节表格是 `docs/development/compatibility-matrix.yaml` 的**种子数据**（见 PRE-2）。yaml 建成后以 yaml 为规范来源，本节降级为非规范性摘要。

### 与代码现状的已知校正（2026-06-10 核对）

| 条目 | 当前校正 |
|------|----------|
| `parameter_matrix_alignment` | 当前仓库未发现同名 guard；本文只能要求新增或扩展 guard，不能把它当作既有事实。 |
| status cherry-pick in-progress | SQLite `cherry_pick_state` 前提已满足；是否输出到 status 仍需逐项核对实现和测试。 |
| `stash create` / `stash store` | 已在 declined.md 登记为 D8/D9，默认延后；实现前必须重启决策。 |
| `clean -i` | 已实现 Libra-native interactive selection loop，属于 intentionally-different，不应再列为缺失。 |
| `clone --sparse` / `sparse-checkout` | 已登记 D10，默认延后；不得作为普通 P1 实现项。 |
| `.libra_attributes` | 当前承载 LFS track/untrack；filter/diff/archive attributes 扩展仍是高风险未来项。 |
| local file remote push | D2 明确拒绝；任何 remote 计划不得把它列为待实现缺口。 |
| accepted no-op flags | 已存在若干 accepted/no-op 或 reserved/no-op 行为（如部分 command notes 所述）；新增时必须在矩阵中显式分类，避免误标 supported。 |

### Porcelain：工作区与提交 / 分支、历史与差异 / Sequencer 与合并 / Remote Client / Plumbing 与对象命令

本节不再引用"历史完整版本"作为事实来源。Phase 0 必须把下列种子项导入 `compatibility-matrix.yaml`，导入时逐项核对代码、命令文档、`COMPATIBILITY.md` 和 declined 登记簿。

| 组 | P0/P1 种子项 | 默认动作 |
|----|--------------|----------|
| Working tree | `status` repo_state/cherry-pick 可见性、pathspec 过滤；`add --pathspec-from-file` 行为边界；`restore`/`checkout` conflict stage；`reset --pathspec-from-file`；`clean` 删除保护。 | `enhance` 或 `evaluate`，删除/覆盖类默认 `risk=high`。 |
| Commit | editor/template/cleanup/dry-run/porcelain/fixup/autosquash 文档与实现一致性；`--allow-empty-message`、patch UI、external GnuPG。 | 已暴露能力用 `enhance`；patch UI/GnuPG 按 non-goal 拒绝或 intentional-diff。 |
| History/diff | `log --all/--branches/--tags/--reverse/-L`、pretty format 长尾、`diff --cc`、attributes diff drivers、grep regex dialect。 | 先 `evaluate`；大历史/regex/binary 默认 `risk=medium`。 |
| Branch/ref | `branch --sort/--format`、`for-each-ref`、`update-ref`、symbolic-ref 限制。 | SQLite refs 兼容项可 `implement`；flat ref 依赖项拒绝。 |
| Sequencer | `merge` drivers/custom strategy、`rebase --exec`/interactive、`cherry-pick --strategy`/rerere、`revert` cleanup/signing。 | interactive/external-command 类默认拒绝或延后；状态机类需独立测试。 |
| Remote | `clone --template/-c/--upload-pack`、`fetch --server-option/--upload-pack`、`push` lease/signed/options、`remote` group/update 长尾。 | 真实协议能力先 `evaluate`；local file push/submodule recurse 保持拒绝。 |
| Object/plumbing | `cat-file --batch-command`、`hash-object --path/--no-filters`、`archive` pathspec/export-ignore、`verify-pack` corpus、`ls-files`/`ls-tree` 等。 | 支持测试/脚本的 plumbing 优先；filter/attribute 执行类默认 high risk。 |

## 阶段计划

每个阶段有明确的入口与出口条件；出口条件不满足不进入下一阶段（允许并行准备下一阶段的决策项）。阶段内每个条目按「单条目执行手册」（§9）落地。

### Phase 0: 盘点与边界保护

**入口**：PRE-1～PRE-4 全部完成。

- 产出 `compatibility-matrix.yaml`（Git / optional-Grit / Libra 命令与参数三方参考矩阵），全部条目完成 `action`/`phase`/`priority` 分类。
- 在矩阵中标记 non-goals 并填 `declined_ref`。
- 为 unsupported submodule、subtree、sparse-checkout、Git hooks、Git patch-selection UI、Git interactive rebase 和 server-side command surfaces 新增或更新文档。
- 落地 P0 拒绝行为测试批次（使用 PRE-3 定案的错误码），覆盖本文矩阵中全部 P0 `明确拒绝` 行（至少包括 add/commit/... 的 -p/--patch、rebase -i 等）。
- 落地 P0 `保持差异` 锁定测试。

**出口**：矩阵无 `unclassified` 条目；参数矩阵 guard 进 CI；上述 P0 拒绝/差异测试全部落地并绿；declined.md 对账完成。

若选择扩展既有 guard 而非新建 `parameter_matrix_alignment`，出口条件改为：参数矩阵检查已进入 CI，且测试名/README 明确说明覆盖 `compatibility-matrix.yaml`。

### Phase 1: 现有 Porcelain 参数 Parity

**入口**：Phase 0 出口达成；add -N、stash create/store、commit --porcelain 三项决策完成。

- 改进 `commit`、`status`、`add`、`restore`、`checkout`、`reset`、`clean` 和 `stash`（按矩阵中 phase=1 的行，P1 先于 P2）。
- 删除/覆盖类行为优先补负向测试；交互/editor 类行为优先补非 TTY、`--json` 和 `--machine` 下的拒绝或降级路径。

**出口**：矩阵中 phase=1 行全部 `done` 或 `blocked`；相关 owner 场景更新并通过 `run --only`。

### Phase 2: History / Diff / Branch 查询增强

**入口**：Phase 0 出口达成；目标行均已分类为 `implement` / `enhance` / `reject` / `intentional-diff`。

- 改进 `branch` / `tag` / `show-ref` / history / diff / grep / blame / describe / shortlog 的查询类 flag。
- 先做只读输出与过滤；涉及重写 refs 或工作区的条目回到对应写命令阶段。

**出口**：phase=2 行全部 `done` 或带原因 `blocked`；大历史/大文件/regex 边界测试落地；相关 owner 场景 `run --only` 通过。

### Phase 3: Remote Client 互操作增强

**入口**：Phase 0 出口达成；真实远端需求已标明是否需要 Wave 3。

- 改进 `clone` / `fetch` / `pull` / `push` / `remote` / `ls-remote` 的协议能力、能力协商和错误报告。
- 本地 fixture 覆盖对象闭包、tag、shallow、refspec；真实远端行为只通过 Wave 3 验证。

**出口**：本地场景和必要 Wave 3 场景通过；日志脱敏自检通过；local file remote push、submodule recurse 等拒绝项仍有负向断言。

### Phase 4: 选择性 Plumbing

**入口**：至少一个现有 porcelain、runner 断言或外部脚本兼容需求证明该 plumbing 有调用方。

- 分批新增 `ls-files`、`ls-tree`、`write-tree`、`read-tree`、`update-index`、`update-ref`、`for-each-ref`、`check-ref-format` 等高价值 plumbing。
- 写类 plumbing 必须先证明事务、锁和失败回滚路径。

**出口**：每个新增命令都有 `COMPATIBILITY.md` 行、命令文档、examples、owner scenario 和 smoke 场景；compat guard 与 check-plan 通过。

### Phase 5: Attributes / Filters 与长尾兼容

**入口**：高风险安全门通过；`.libra_attributes` 与 `.gitattributes` 的优先级和迁移路径已形成 RFC。

- 只推进不会执行外部命令、不会改变对象内容可重复性的 attributes 子集。
- clean/smudge、textconv、diff drivers 默认继续拒绝，除非独立 RFC、sandbox/timeout/env 清理和负向测试全部到位。

**出口**：所有 high-risk 条目有安全说明和拒绝/实现测试；文档明确与 Git `.gitattributes` / Git LFS filter 的互操作边界。

## 交互式功能边界

- 不实现 `add -p` 等 Git patch-selection UI。
- 不实现 `rebase -i` 的 Git todo-editor workflow。
- `clean -i` 保留为 Libra-native interactive selection loop，并明确标记为 `intentionally-different`。
- Editor-based message editing 继续支持。
- 在 `--json`、`--machine` 或非 TTY 场景下，任何隐式 editor/prompt 都应拒绝。

## 验收标准

全部标准可由命令或评审客观检查：

1. **矩阵完整性**：`compatibility-matrix.yaml` 覆盖本文全部参数级种子项且每行有 `action`/`phase`/`status`/`risk`/`compat_contract`/`source_of_truth`/`owner_scenario`（用户可见行为）/来源字段；任何 `evaluate` 行不得带入其所属实现阶段；参数矩阵 guard 在 CI 常绿。
2. **非目标锁定**：非目标表中每个命令族至少有一条拒绝行为测试（invocation → 稳定错误码 + exit code + stderr + `--json` envelope 断言），且在 declined.md 有对应 D 条目。
3. **架构不回退**：全程不引入 flat `.git/refs` / packed-refs、`.git/sequencer` 文件兼容、外部 GnuPG 依赖；refs/reflog/sequencer 状态仍以 SQLite 为事实来源；`--json`/`--machine` envelope 在所有改动命令上保持兼容。
4. **场景覆盖**：本计划新增的每个用户可见兼容行为都能在 `integration-scenarios.yaml` 中指出 owner 场景；`check-plan` 常绿。
5. **测试迁移可追溯**：每个被分类的测试来源（upstream Git `t/` 或 Grit 补充）在矩阵 `git_tests` / `grit_tests` 字段中有迁移状态和来源标注，无静默遗漏。
6. **每阶段出口条件留痕**：阶段切换时，矩阵中该阶段 `blocked` 行均有原因与目标阶段，本文档「计划状态」表更新当前阶段。
7. **Review 新增要求 1 满足（「所有的改动都需要同时更新集成测试方案」）**：每次相关 PR 必须同时提交完整的集成测试方案更新（yaml/MD/runner/registry/§2.3 矩阵），`check-plan` 全绿，且 PR 描述中显式说明同步内容。
8. **Review 新增要求 2 满足**：所有 `reject` / `intentional-diff`（至少 P0 批次）均有至少一个可通过 integration-runner 黑盒执行的拒绝/差异断言路径；`run --only` 执行日志或场景代码中留有可验证信号。
9. **安全门满足**：所有 `risk=high` 条目有独立安全说明；涉及外部命令、hooks、filters、网络、secret 的变更有 fail-closed 负向测试和日志脱敏检查。
10. **数据流/控制流正确**：每个写操作条目在矩阵中标明读写事实源、事务/锁边界、失败出口；测试覆盖失败后 `status`/`fsck` 或等价健康检查。
11. **性能边界明确**：大历史、大文件、batch、regex、network、archive/filter、pathspec、config、attributes、ignore 类条目有输入大小、超时、流式输出、分页策略或缓存失效策略；测试至少覆盖一个边界条件。
12. **互操作边界明确**：触达真实远端协议的条目有 Wave 3 计划；触达 Git intentional difference 的条目在 `COMPATIBILITY.md`、命令文档和 integration-runner 断言中三方一致。
13. **测试证据完整性**：矩阵中 `status=done` 的每一行都有非空的 `test_evidence`，指向真实存在的测试或场景；guard 能校验 `test_evidence` 的可达性。
14. **性能边界可验证**：所有 `risk=high` 或大历史/大文件/batch/regex/network/archive/filter/pathspec/config/attributes/ignore 类条目都有非空的 `performance_note`，且 runner 或命令测试中至少覆盖一个边界条件（如输入上限、超时、流式输出、缓存失效）。
15. **兼容契约一致性**：每个用户可见行的 `compat_contract` 必须能在 `COMPATIBILITY.md`、命令文档和测试断言中找到同义表述；`accepted-no-op` 必须有显式文档说明，`explicitly-rejected` 必须断言 `LBR-UNSUPPORTED-001` 或登记的稳定错误码。
16. **协议/子进程边界可审计**：新增任何 `Command::new`、外部 helper、protocol bridge 或 server-side Git surface 时，必须在矩阵中标 `risk=high` 并附安全说明；若属于公开 CLI 表面，默认拒绝并登记 declined，除非 RFC 已批准。

（执行本计划时必须同时遵守上文全部 0-11 条规则以及新增的 13-16 项验收标准。）
