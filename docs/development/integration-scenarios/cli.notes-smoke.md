### `cli.notes-smoke`

目的：记录当前 `notes` 命令尚未注册的状态，防止集成方案把 notes 功能误判为已实现。

最小步骤：

```bash
SCENARIO="cli.notes-smoke"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"

libra init repo
cd repo
libra config user.name "Libra Integration"
libra config user.email "integration@example.invalid"
printf 'base\n' > tracked.txt
libra add tracked.txt
libra commit -m "initial" --no-verify

! libra --json notes add -m "first note"
libra fsck --connectivity-only
```

关键断言：

- `notes` 返回稳定 JSON unknown-command 错误。
- 失败不破坏仓库连通性。
