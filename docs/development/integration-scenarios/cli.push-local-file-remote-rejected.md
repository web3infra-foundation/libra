### `cli.push-local-file-remote-rejected`

目的：验证 `push` 对本地 file remote 的故意差异：本地路径 remote 可用于 `clone`/`fetch`/`pull` fixture，但 `push` 当前只支持网络 remote，必须拒绝本地 file remote。真实 push/refspec/tag/force/mirror 成功路径放到 Wave 3 GitHub 场景。

最小步骤：

```bash
SCENARIO="cli.push-local-file-remote-rejected"
REMOTE_DIR="$RUN_ROOT/fixtures/$SCENARIO/remote.git"
WORK_DIR="$RUN_ROOT/repos/$SCENARIO/work"
mkdir -p "$(dirname "$REMOTE_DIR")" "$(dirname "$WORK_DIR")"
# (prelude provides libra() -- converged short form, long wrapper removed)


libra init --bare "$REMOTE_DIR"
libra init "$WORK_DIR"
cd "$WORK_DIR"
libra config set user.name "Libra Push Rejection Test"
libra config set user.email "push-reject@example.invalid"
printf 'push\n' > push.txt
libra add push.txt
libra commit -m "test: push rejection base"
libra remote add origin "$REMOTE_DIR"
libra remote set-url --push origin "$REMOTE_DIR"
libra remote get-url --all origin

expect_local_push_rejected() {
  name="$1"
  shift
  set +e
  libra --json=compact push "$@" >"$name.out" 2>"$name.err"
  status=$?
  set -e
  test "$status" -ne 0
  python3 - "$name.err" <<'PY'
import json, sys
raw = open(sys.argv[1]).read().strip()
payload = json.loads(raw)
assert payload["ok"] is False
assert payload["error_code"] == "LBR-CLI-003"
assert "local file" in payload["message"] or "local file repositories" in payload["message"]
PY
}

expect_local_push_rejected push-main origin main
expect_local_push_rejected push-dry-run --dry-run origin main
expect_local_push_rejected push-force --force origin main
expect_local_push_rejected push-tags --tags origin
expect_local_push_rejected push-mirror --mirror --dry-run origin
```

断言：本地 file remote 已存在且可作为 remote URL 存储；`push origin main`、`push --dry-run origin main`、`push --force origin main`、`push --tags origin`、`push --mirror --dry-run origin` 都必须非 0 退出；`--json=compact` 的 stderr 错误 envelope 必须包含 `ok:false`、`error_code == "LBR-CLI-003"` 和本地 file remote 不支持的可操作提示；失败不得写入 remote refs 或修改本地 HEAD。

补充可执行断言：
- 每个本地 file remote push 失败后执行 `libra fsck --connectivity-only`，确认本地源仓库仍健康。
- `libra --json remote get-url --all origin` 仍能返回本地路径，证明失败点是 push 传输策略而非 remote 配置丢失。
- 若未来实现支持本地 file remote push，必须把本场景改成正向闭环，并同步更新 COMPATIBILITY.md / declined note。

