### `cli.restore-reset-diff`

目的：覆盖 `diff`、`restore`、`reset` 的工作区修改、staged 修改、路径级恢复、冲突重渲染、overlay 删除语义、HEAD 移动和输出格式。

最小步骤：

```bash
# Short converged form.
SCENARIO="cli.restore-reset-diff"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"

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
libra diff --raw
libra diff -w -U0
! libra diff --exit-code
libra add src/app.txt
libra diff --staged
libra diff --staged --name-status
libra restore --staged src/app.txt
libra status --short
libra restore --worktree src/app.txt
grep 'one' src/app.txt
printf 'pathspec file\n' > src/app.txt
printf 'src/app.txt\n' > restore-paths.txt
libra --json restore --pathspec-from-file=restore-paths.txt
printf 'pathspec nul\n' > src/app.txt
printf 'src/app.txt\0' > restore-paths-nul.txt
libra --json restore --pathspec-from-file=restore-paths-nul.txt --pathspec-file-nul

printf 'two\n' > src/app.txt
libra add src/app.txt
libra reset HEAD -- src/app.txt
libra status --short
libra add src/app.txt
printf 'src/app.txt\n' > reset-paths.txt
libra --json reset --pathspec-from-file=reset-paths.txt
libra add src/app.txt
printf 'src/app.txt\0' > reset-paths-nul.txt
libra --json reset --pathspec-from-file=reset-paths-nul.txt --pathspec-file-nul
libra add src/app.txt
libra --json reset --no-refresh HEAD
libra add src/app.txt
libra commit -m "test: restore second"
SECOND_COMMIT="$(libra rev-parse HEAD)"
libra diff --old HEAD~1 --new "$SECOND_COMMIT" --numstat
libra reset --soft HEAD~1
libra status --short
libra reset --mixed HEAD
libra restore --worktree src/app.txt
grep 'one' src/app.txt

printf 'keep\n' > keep.txt
libra add keep.txt
libra commit -m "test: keep reset target"
printf 'local keep\n' > src/app.txt
libra --json reset --keep HEAD~1
grep 'local keep' src/app.txt
libra reset --hard HEAD

printf 'merge\n' > merge.txt
libra add merge.txt
libra commit -m "test: merge reset target"
libra --json reset --merge HEAD~1
test ! -e merge.txt

printf 'overlay\n' > overlay.txt
libra add overlay.txt
libra commit -m "test: overlay restore target"
libra --json restore --source HEAD~1 --overlay overlay.txt
grep 'overlay' overlay.txt
libra --json restore --source HEAD~1 overlay.txt
test ! -e overlay.txt
libra reset --hard HEAD
# 显式 --no-overlay 与默认行为一致：source 中不存在的路径被删除
libra --json restore --source HEAD~1 --no-overlay overlay.txt
test ! -e overlay.txt
libra reset --hard HEAD

printf 'l1\nl2\nl3\nl4\n' > orig.txt
libra add orig.txt
libra commit -m "test: rename source"
rm orig.txt
printf 'l1\nl2\nl3\nCHANGED\n' > new.txt
libra diff -M70 --name-status
libra reset --hard HEAD

printf 'three\n' > src/app.txt
libra add src/app.txt
libra commit -m "test: restore third"
libra reset --hard HEAD~1
grep 'one' src/app.txt

cd "$RUN_DIR"
libra init restore-conflict
cd restore-conflict
libra config set user.name "Libra Restore Test"
libra config set user.email "restore@example.invalid"
printf 'base\n' > tracked.txt
libra add tracked.txt
libra commit -m "test: conflict base"
libra switch -c feature
printf 'feature\n' > tracked.txt
libra add tracked.txt
libra commit -m "test: feature conflict"
libra switch main
printf 'main\n' > tracked.txt
libra add tracked.txt
libra commit -m "test: main conflict"
! libra merge feature
grep '<<<<<<<' tracked.txt
! libra --json restore tracked.txt
libra --json restore --ignore-unmerged --source HEAD tracked.txt
libra --json restore --ours tracked.txt
grep 'main' tracked.txt
! libra restore tracked.txt
libra --json restore --theirs tracked.txt
grep 'feature' tracked.txt
libra --json restore --merge tracked.txt
grep '<<<<<<<' tracked.txt
libra --json restore --conflict=diff3 tracked.txt
grep '||||||| base' tracked.txt
printf 'resolved\n' > tracked.txt
libra add tracked.txt
libra merge --continue
libra fsck --connectivity-only
```

负向步骤：

```bash
cd "$RUN_DIR/restore-repo"
! libra restore --source no-such-revision src/app.txt
! libra reset no-such-revision
! libra reset --hard no-such-rev
! libra diff --old no-such-revision --new HEAD
! libra diff --algorithm myers tracked.txt
```

正向补充步骤（diff 输出重定向与算法选择）：

```bash
cd "$RUN_DIR/restore-repo"
printf 'diff output probe\n' > tracked.txt
libra diff --output diff-out.patch tracked.txt
grep '@@' diff-out.patch
libra diff --algorithm=histogram tracked.txt | grep 'diff output probe'
rm diff-out.patch
libra reset --hard HEAD
```

断言：unstaged diff、staged diff、pathspec、name-only、name-status、numstat、stat、raw、`-w -U0` 和 `--exit-code` 输出/退码都能反映同一修改；`-M70 --name-status` 能识别 75% 相似的重命名；`restore --staged` 只取消暂存，不丢弃工作区修改；`restore --worktree` 与 `restore --pathspec-from-file=<file>`/`--pathspec-file-nul` 恢复工作区内容；`restore --overlay` 保留 source 中不存在的已追踪文件，默认 no-overlay 删除该工作区文件；冲突阶段 `restore --ours`/`--theirs` 只写工作区且 index 保持 unmerged，`restore --merge`/`--conflict=diff3` 能重建冲突标记，plain restore over unmerged path 以 128 阻断，`--ignore-unmerged` 跳过该路径；路径级 `reset HEAD -- <path>` 与 `reset --pathspec-from-file=<file>`/`--pathspec-file-nul` 只影响 index；`reset --no-refresh` 被接受并保持 mixed reset 语义；`reset --soft` 保留 index/工作区变化，`reset --mixed` 重置 index，`reset --hard` 重置 HEAD/index/工作区；`reset --keep` 保留与目标差异不重叠的本地修改，`reset --merge` 能安全移动 HEAD 并删除目标树外的干净路径；显式 `restore --no-overlay` 与默认 no-overlay 语义一致；`restore --source <坏修订>` 以 `failed to resolve checkout source` 失败且不改写工作区；`diff --output <file>` 把补丁写入文件、stdout 不含 hunk，`--algorithm=histogram` 输出与默认一致、非 histogram 算法明确报错 `not supported yet`；无效 revision 必须失败且不改变当前 HEAD（含 `reset --hard`）。

补充可执行断言：
- `libra --json diff --staged` 和 `libra --json diff` 必须返回结构化数据（files 或 changes）。
- 关键 reset/restore 后 `libra --json status` 验证状态正确（staged / unstaged）。
- 每次重置后 `libra fsck --connectivity-only` 通过。
- `libra --json reset --hard HEAD~1` 成功后验证 HEAD 回退且工作区文件恢复。
- `libra --json reset --pathspec-from-file=reset-paths.txt` 返回 `command=="reset"` 且 `files_unstaged==1`；`--pathspec-file-nul` 路径也返回结构化 reset JSON。
- `libra --json reset --no-refresh HEAD` 成功返回结构化 reset JSON，证明该兼容 no-op flag 可被脚本传入。
- `libra --json reset --keep HEAD~1` 返回 `mode=="keep"` 且保留 `src/app.txt` 的本地修改；`libra --json reset --merge HEAD~1` 返回 `mode=="merge"` 且删除目标树外的 `merge.txt`。
- `libra --json restore --pathspec-from-file=restore-paths.txt` 与 `--pathspec-file-nul` 都返回结构化 restore JSON 并恢复工作区内容；`restore --overlay` 保留 source 中不存在的 `overlay.txt`，默认 no-overlay 删除该文件。
- 真实 merge conflict 后，`libra --json restore tracked.txt` 退出 128 且 stderr 含 `is unmerged`；`restore --ignore-unmerged` 返回结构化 JSON 且不改写冲突文件；`restore --ours` 写入 ours 内容，`restore --theirs` 写入 theirs 内容；`restore --merge` 重建 `<<<<<<<`/`>>>>>>>` 标记，`restore --conflict=diff3` 包含 `||||||| base`。
- 负向 `libra restore --source no-such-rev` 必须非 0，stderr 包含错误路径或 LBR- 标识。
