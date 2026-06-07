### `cli.restore-reset-diff`

目的：覆盖 `diff`、`restore`、`reset` 的工作区修改、staged 修改、路径级恢复、HEAD 移动和输出格式。

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
```

负向步骤：

```bash
cd "$RUN_DIR/restore-repo"
! libra restore --source no-such-revision src/app.txt
! libra reset no-such-revision
! libra diff --old no-such-revision --new HEAD
```

断言：unstaged diff、staged diff、pathspec、name-only、name-status、numstat、stat、raw、`-w -U0` 和 `--exit-code` 输出/退码都能反映同一修改；`-M70 --name-status` 能识别 75% 相似的重命名；`restore --staged` 只取消暂存，不丢弃工作区修改；`restore --worktree` 恢复工作区内容；路径级 `reset HEAD -- <path>` 与 `reset --pathspec-from-file=<file>`/`--pathspec-file-nul` 只影响 index；`reset --no-refresh` 被接受并保持 mixed reset 语义；`reset --soft` 保留 index/工作区变化，`reset --mixed` 重置 index，`reset --hard` 重置 HEAD/index/工作区；无效 revision 必须失败且不改变当前 HEAD。

补充可执行断言：
- `libra --json diff --staged` 和 `libra --json diff` 必须返回结构化数据（files 或 changes）。
- 关键 reset/restore 后 `libra --json status` 验证状态正确（staged / unstaged）。
- 每次重置后 `libra fsck --connectivity-only` 通过。
- `libra --json reset --hard HEAD~1` 成功后验证 HEAD 回退且工作区文件恢复。
- `libra --json reset --pathspec-from-file=reset-paths.txt` 返回 `command=="reset"` 且 `files_unstaged==1`；`--pathspec-file-nul` 路径也返回结构化 reset JSON。
- `libra --json reset --no-refresh HEAD` 成功返回结构化 reset JSON，证明该兼容 no-op flag 可被脚本传入。
- 负向 `libra restore --source no-such-rev` 必须非 0，stderr 包含错误路径或 LBR- 标识。
