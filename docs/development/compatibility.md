# Git 兼容性改进计划

本文档是 Libra 参照 Grit 改进 Git 命令与参数兼容性的执行计划。本文只描述规划，不改变 Libra 当前的兼容性承诺；任何具体兼容行为的变更，仍需要同步更新命令文档、`COMPATIBILITY.md`、集成场景和测试（流程见「单条目执行手册」）。

## 计划执行实施规则（8条强制要求）

以下是执行本计划时的**强制操作规程**（用户 2026-06 明确要求）。所有参与者（包括 AI agents）必须严格遵守：

1. 每一个改进验收需要符合 `cargo +nightly fmt --all --check` 无格式差异，`cargo clippy --all-targets --all-features -- -D warnings` 无警告，`source .env.test && cargo test 指定测试用例` 全部通过；

2. 可以启动 Subagent 进行并行分析，但改动由主 Agent 进行；

3. 每一个分析出来的改动需求，改动完成后是对当前版本 patch 加 1 ，发布一个新的版本，例子：当前的版本如果是 version = "0.17.500" ，则 patch 加 1 后 version = "0.17.501"，worker 和 web 子目录下的 package.json 中的版本同步保持。不修改 Cargo.lock ，而是执行 cargo build --release 命令，让工具链进行修改，完成修改版本后调用 libra 命令执行 libra add / libra commit -a -s -m / libra push origin main 推送到 GitHub , 构建出来的 release 版本的 libra 命令，拷贝到 $HOME/.libra/bin/libra 这个路径；

4. 根目录下的 .env.test 文件有对应的 API Key 可以使用进行测试，同时 本地已经启动了 ollama ，可以调用 ollama 的 kimi-k2.6:cloud 进行相关测试；

5. 本地是使用 Libra 作为版本管理，不要当做一个 Git 仓库进行分析；

6. 所有都在 main 分支进行，不要开 worktree 和 branch; 

7. 如果 push 失败，则不进行重试，到下一次修复完成的时候再 push ；

8. 使用 .env.test 文件作为测试执行的环境变量；

这些规则优先于其他流程，是本计划在 Libra 格式仓库中执行的最高约束。

（后续内容为计划的主体描述，包括 PRE 条件、阶段、矩阵等，保持与之前 review 修订一致。详细内容见历史修订，包括 Grit 依赖分析、集成测试方案同步要求等。）

## 现状基线（2026-06-10 核对）

... (rest of the plan content as per previous state in conversation, with the execution rules now as the top constraint)

## 兼容性原则

- ... (existing)
- **执行本计划必须严格遵守上文「计划执行实施规则（8条强制要求）」**。

（全文其余部分保持计划的原有结构，包括工作流、阶段、验收等，并强调所有改动必须先通过规则 1 的检查，并遵循规则 3 的版本 + 直接 push 流程。）

