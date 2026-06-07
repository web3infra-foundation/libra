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
# (prelude provides libra - MD converged for this Rust scenario)

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

cd "$RUN_ROOT/repos/$SCENARIO"
libra clone --origin upstream --no-checkout "$REMOTE_DIR" no-checkout
cd no-checkout
libra config get remote.upstream.url
test ! -f README.md

cd "$RUN_ROOT/repos/$SCENARIO"
libra clone --jobs 2 "$REMOTE_DIR" jobs-clone
libra clone --reference "$CLONE_DIR" "$REMOTE_DIR" reference-clone
cd reference-clone
libra fsck --connectivity-only

cd "$RUN_ROOT/repos/$SCENARIO"
libra clone --local --no-hardlinks "$REMOTE_DIR" local-copy
libra clone --shared "$REMOTE_DIR" shared-copy
cd shared-copy
libra fsck --connectivity-only

cd "$RUN_ROOT/repos/$SCENARIO"
libra --json clone "$REMOTE_DIR" clone-json
```

负向步骤：

```bash
cd "$RUN_ROOT/repos/$SCENARIO/clone"
! libra fetch origin no-such-branch
! libra pull --ff-only origin no-such-branch
! libra clone "$RUN_ROOT/fixtures/$SCENARIO/missing.git" "$RUN_ROOT/repos/$SCENARIO/missing-clone"

# Verify fetch/pull JSON output format
cd "$RUN_DIR"
cd "$RUN_ROOT/repos/$SCENARIO/clone-json"
libra --json fetch origin >fetch.json
python3 -c "import json; d=json.load(open('fetch.json')); assert d['ok'] is True; assert 'data' in d"
libra --json pull --ff-only origin main >pull.json
python3 -c "import json; d=json.load(open('pull.json')); assert d['ok'] is True; assert 'data' in d"
```

断言：隔离 `gitfix()` 创建的本地 Git 仓库可作为 clone/fetch/pull remote；`remote add`、`remote -v`、`remote get-url` 能观察本地路径 URL；`ls-remote` 可看到 `refs/heads/main`；普通 clone 后文件和 log 可见；Git fixture 新提交后，clone 仓库通过 `fetch`、`fetch --all` 和 `pull --ff-only` 能看到新增提交；**`pull --rebase` 把 clone 端本地提交重放到 upstream 新提交之上——`README.md` 含 upstream 的 `third`，本地 `clone-local.txt` 仍在**；`clone --bare` 生成 Libra bare 布局（可观察到 `libra.db`）；`clone --single-branch -b main` 只检出指定分支；`--origin upstream --no-checkout` 写入 upstream remote 且不物化工作树；`--jobs 2` 被接受；`--reference` / `--local --no-hardlinks` / `--shared` 在本地 remote 上可完成并通过 fsck；缺失 remote 或缺失 ref 必须非 0 退出且不创建半成品仓库或损坏当前 clone。

补充可执行断言：
- `libra --json clone "$REMOTE_DIR" clone-json` 成功后 `ok:true`，并验证 `libra --json log -n 1` 结构。
- 每次 fetch/pull 后 `libra fsck --connectivity-only` 通过。
- `libra --json ls-remote --heads` 返回结构化 refs 列表。
- 负向 `libra fetch origin no-such` 必须非 0，stderr 包含 "couldn't find remote ref" 或对应 LBR-NET 错误。
- 验证 `pull --rebase` 成功后，本地提交历史被重放（通过 `libra --json log -n 5` 的 `data.commits[]` 顺序观察）。
