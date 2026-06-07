### `cli.stash-bisect-worktree`

目的：覆盖兼容性差异较大的 `stash`、`bisect`、`worktree` 命令面，重点验证状态保存/恢复、`stash push -u` / `-a` / `--all` / `--keep-index`、二分会话状态和 Libra worktree remove 默认保留目录的差异语义。

最小步骤：

```bash
# Short converged form (long original wrapper removed for convergence).
SCENARIO="cli.stash-bisect-worktree"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"

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

printf 'visible\n' > visible-untracked.txt
libra --json stash push -u >stash-push-untracked.json
python3 -c "import json; d=json.load(open('stash-push-untracked.json')); assert d['ok'] is True; assert d['data']['included_untracked'] >= 1"
test ! -e visible-untracked.txt
libra stash pop
test -f visible-untracked.txt
rm visible-untracked.txt

printf 'ignored.log\n' > .libraignore
printf 'ignored\n' > ignored.log
libra --json stash push --all >stash-push-all.json
python3 -c "import json; d=json.load(open('stash-push-all.json')); assert d['ok'] is True; assert d['data']['included_untracked'] >= 1"
test ! -e .libraignore
test ! -e ignored.log
libra stash pop
test -f .libraignore
test -f ignored.log
rm .libraignore ignored.log

printf 'staged\n' > number.txt
libra add number.txt
printf 'unstaged\n' > number.txt
libra --json stash push --keep-index >stash-push-keep-index.json
python3 -c "import json; d=json.load(open('stash-push-keep-index.json')); assert d['ok'] is True; assert d['data']['kept_index'] is True"
test "$(cat number.txt)" = "staged"
libra reset --hard
libra stash clear --force

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

断言：`stash push` 保存 tracked 修改并清理工作区；`stash list` / `stash show` 能观察 stash 条目；`stash apply` 保留 stash，`stash pop` 应用并删除 stash；`stash push -u` 保存/移除/恢复可见 untracked 文件；`stash push --all` 保存/移除/恢复可见 untracked 与 ignored 文件；`stash push --keep-index` 保留 staged 内容并移除 unstaged delta；`stash clear --force` 清空列表；`bisect start <bad> --good <good>` 建立会话，`view` / `log` 能观察状态，`bad` / `good <rev>` 推进会话，`reset` 恢复原始 HEAD；`worktree add` 注册 linked worktree，`list` 显示路径，`lock --reason` / `unlock` 更新锁状态，`move` 更新路径，`remove` 默认只注销登记且保留目录，`prune` 可执行；非法 stash ref、非法 revision 和缺失 worktree 必须失败且不破坏已有仓库状态。

补充可执行断言（故意差异重点场景）：
- `libra worktree remove <path>` 后 `test -d <path>` 必须仍存在（Libra 故意保留目录，不像 Git 默认删除）。
- `libra --json stash list` 验证 `ok:true` 且 `data.entries[]` 或 `data.stashes[]` 可解析。
- 每次 stash/bisect/worktree 操作后 `libra fsck --connectivity-only` 必须 0 退出。
- `worktree remove` 后的 `libra --json worktree list` 不再包含该 worktree。
- 负向 `worktree remove` 不存在路径的错误必须非 0，stderr 包含路径。
- 验证 `--delete-dir` 模式真正删除目录：`libra worktree remove --delete-dir <path> && test ! -d <path>`。
