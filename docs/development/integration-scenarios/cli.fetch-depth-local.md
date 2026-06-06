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
# (prelude provides libra for this scenario - converged)
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

