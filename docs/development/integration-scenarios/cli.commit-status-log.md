### `cli.commit-status-log`

目的：覆盖 `status`、`add`、`commit`、`log` 的最小提交闭环，以及脚本常用输出格式、自动暂存、消息来源和失败路径。

最小步骤：

```bash
# Prelude copied once at top of run (see "手动执行 prelude" or §3.3.1). Short form per convergence.
SCENARIO="cli.commit-status-log"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"

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
libra log --stat -n 3

printf 'from file\n' > message.txt
printf 'tracked\n' > tracked.txt
libra add tracked.txt
libra commit -F message.txt --signoff

printf 'tracked update\n' >> tracked.txt
libra commit -a -m "test: auto stage tracked update"
libra commit --allow-empty -m "test: empty marker"
libra commit --amend --no-edit

mv tracked.txt renamed.txt
libra add -A
libra status --short
libra status --porcelain v2
libra status --porcelain v2 -z
libra status -z -s
libra --json status
libra commit -m "rename tracked" --no-verify
libra log --follow --oneline renamed.txt
libra log --follow --name-status renamed.txt
libra --json log --follow renamed.txt

printf 'target\n' > type-target.txt
libra add type-target.txt
libra commit -m "test: add type target" --no-verify
rm type-target.txt
ln -s renamed.txt type-target.txt
libra status --porcelain v2
mkdir -p scratch
printf 'untracked\n' > scratch/note.txt
libra config status.showUntrackedFiles no
libra status --short
libra status --short --untracked-files=all
libra config status.branch true
libra status --short
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

断言：`add --dry-run` 不写入 index；`add` 后 `status --porcelain` 能看到 staged 文件；`commit -m` / `commit -F` / `commit -a` / `commit --allow-empty` / `commit --amend --no-edit` 均按预期创建或更新提交；`status --exit-code` 在干净工作区退出码为 0、在 dirty 工作区非 0；`log --oneline`、`log --name-status --grep --author`、`log --stat` 能观察到对应提交、作者、消息和文件变化；`log --follow renamed.txt` 能穿过 `tracked.txt -> renamed.txt` rename 看到初始提交；缺少 staged change 或 conventional 校验失败必须非 0 且不产生新提交。

补充可执行断言（本场景为基础，推荐所有后续场景复用模式）：
- 每次 commit 后立即 `libra --json status` + python 断言 `ok:true` 且 data 反映干净或 dirty 状态。
- `libra --json log -n 3` 验证 `data.commits[]` 非空，commit 包含 hash/subject 或等价消息字段，且作者匹配配置的 user.name。
- 关键 commit 后执行 `libra fsck --connectivity-only` 必须 0 退出。
- 负向 conventional commit 失败的 stderr 必须包含 "conventional" 或对应 LBR- 错误码（通过 `2>&1 | cat` 捕获验证）。
- `libra --json commit --allow-empty -m "json empty"` 成功后验证 envelope + 新 commit 在 `libra --json log -n 1` 中出现。
- 已暂存 rename 后，`status --short` 必须包含 `R  tracked.txt -> renamed.txt`，`status --porcelain v2` 必须包含 `2 R  ... R100 renamed.txt<TAB>tracked.txt`，`libra --json status` 的 `data.renames[]` 必须包含 `from=tracked.txt`、`to=renamed.txt`、`score=100`。
- 已暂存 rename 后，`status --porcelain v2 -z` 必须以 NUL 分隔 `renamed.txt` 和 `tracked.txt`，不得保留 TAB；`status -z -s` 必须包含 `R  renamed.txt<NUL>tracked.txt<NUL>`（先新路径后旧路径）。
- rename 提交后，`log --follow --oneline renamed.txt` 必须同时包含 `rename tracked` 和 `initial`；`log --follow --name-status renamed.txt` 必须在 human 输出中包含 `R100<TAB>tracked.txt<TAB>renamed.txt`；`--json log --follow renamed.txt` 必须保持 log JSON envelope 成功。
- `status.showUntrackedFiles=no` 必须默认隐藏未跟踪目录，显式 `--untracked-files=all` 必须覆盖配置并显示子文件；`status.branch=true` 必须让 `status --short` 输出 `## main` 分支头。
- 文件类型变化后，`status --porcelain v2` 必须为该路径输出 `T` 状态列。
- 操作全程使用隔离的 `LIBRA_CONFIG_GLOBAL_DB`，结束后用该 DB 执行 `libra config list --global` 不得残留本场景的临时 key。
