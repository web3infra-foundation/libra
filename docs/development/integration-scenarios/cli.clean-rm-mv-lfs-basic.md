### `cli.clean-rm-mv-lfs-basic`

目的：覆盖工作树管理剩余命令 `clean`、`rm`、`mv` 和本地确定性的 `lfs track/untrack/ls-files` 行为；远端 LFS lock API 不进入默认 Wave。

最小步骤：

```bash
SCENARIO="cli.clean-rm-mv-lfs-basic"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
# (prelude provides libra() -- converged short form, long wrapper removed)

libra init worktree-tools-repo
cd worktree-tools-repo
libra config set user.name "Libra Worktree Tools Test"
libra config set user.email "worktree-tools@example.invalid"
mkdir -p docs assets tmp ignored
printf 'keep\n' > docs/keep.txt
printf 'move\n' > docs/move.txt
printf 'dry\n' > docs/dry.txt
printf 'verbose\n' > docs/verbose.txt
printf 'json\n' > docs/json.txt
printf 'remove\n' > docs/remove.txt
libra add docs/keep.txt docs/move.txt docs/dry.txt docs/verbose.txt docs/json.txt docs/remove.txt
libra commit -m "test: worktree tools base"

libra mv docs/move.txt docs/moved.txt
libra mv -n docs/dry.txt docs/dry-moved.txt
test -f docs/dry.txt
test ! -e docs/dry-moved.txt
libra mv -v docs/verbose.txt docs/verbose-moved.txt
libra --json mv --sparse docs/json.txt docs/json-moved.txt
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

断言：`mv` 同时更新工作区路径和 index 状态；`mv -n` 打印 dry-run 两行且不移动文件；`mv -v` 只打印实际 rename；`libra --json mv --sparse` 返回 `ok:true` 且 `--sparse` 不进入 `MvOutput` JSON；`rm` 删除 tracked 文件并可提交；`clean -n` 不删除、`clean -f` 删除文件、`clean -fd` 删除目录、`clean -fX` 只删除 ignored 文件；`lfs track` 写入 `.libra_attributes`，无参数可列出 pattern；tracked 大文件提交后可由 `lfs ls-files` 三种格式观察；`lfs untrack` 移除 pattern；缺少 `-f/-n`、互斥 clean flag、缺失 rm/mv 源必须失败；`lfs lock` 在无远端 LFS 服务/认证时必须失败且不得泄露凭据。`lfs untrack` 对缺失 pattern 当前可能是幂等空删除，不作为负向断言。

补充可执行断言：
- `libra --json lfs ls-files` 返回 `ok:true`；无 LFS tracked 文件时 `data.files` 可缺失（当前 `LfsOutput.files` 为空会被省略），有 tracked 文件时 `data.files[]` 必须可解析。
- `libra --json mv --sparse` 返回 `ok:true`，`data` 字段集合不包含 `sparse`。
- 验证 `.libra_attributes` 内容包含 `*.bin`（`grep` 或 `cat` 后 python 检查）。
- `libra --json status --porcelain` 在 mv/rm 后可解析且显示正确 staged 状态。
- 操作后 `libra fsck --connectivity-only` 通过。
- 全局隔离：本场景的 `.libraignore` 和 LFS pattern 不得通过隔离 HOME 的全局 config 泄露到其他场景。
