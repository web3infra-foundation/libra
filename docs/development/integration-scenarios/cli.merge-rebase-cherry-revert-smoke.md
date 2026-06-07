### `cli.merge-rebase-cherry-revert-smoke`

目的：覆盖 `merge`（fast-forward 与三方无冲突 merge）、`rebase`、`cherry-pick`、`revert` 的最小可观察闭环，以及 `--continue` / `--abort` 无会话失败路径。

最小步骤：

```bash
SCENARIO="cli.merge-rebase-cherry-revert-smoke"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
# (prelude provides libra() -- converged short form, long wrapper removed)

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
libra switch rebase-topic
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

断言：fast-forward merge 后 HEAD 等于目标提交；三方无冲突 merge 产生可观察 merge 结果并保留双方文件；`rebase main` 把 topic 提交重放到新 base 且文件存在；`cherry-pick <commit>` 在当前分支生成等价修改；`revert <commit>` 创建反向提交并撤销目标修改；缺失目标、无 merge/rebase 会话的 continue/abort 和非法 commit 必须失败且不破坏当前分支。

补充可执行断言：
- 每次主要操作后执行 `libra fsck --connectivity-only` 必须 0 退出。
- `libra --json log -n 1` 验证 merge commit 有 2 个 parent（对于非 ff merge）。
- 负向步骤必须产生非 0 退出，且 stderr 包含 "not a" / "no such" 或 LBR- 相关错误标识（通过捕获验证）。
- `libra --json show-ref --heads` 验证 `data.entries[]` 中的分支状态在 rebase/cherry 后一致。

