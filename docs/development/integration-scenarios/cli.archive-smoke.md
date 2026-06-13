### `cli.archive-smoke`

目的：覆盖 `archive` 当前未公开的 Git 兼容命令状态，确保它不会被误当作已发布 CLI；runner 断言 `libra archive` 返回标准 unknown-command JSON 错误，并在场景末尾验证仓库健康。

最小步骤：

```bash
SCENARIO="cli.archive-smoke"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
# (prelude libra - short converged)

libra init repo
cd repo
libra config user.name "Libra Integration"
libra config user.email "integration@example.invalid"
printf 'base\n' > tracked.txt
libra add tracked.txt
libra commit -m "test: archive fixtures" --no-verify

! libra --json archive --output "$RUN_DIR/release.tar" --prefix release/
libra fsck --connectivity-only
```

断言：`archive` 当前未注册为顶层命令；`libra --json archive ...` 必须非 0 退出，JSON 错误码为 `LBR-CLI-001`；操作后 `libra fsck --connectivity-only` 通过。

补充可执行断言：
- `libra --json archive --output ...` 必须失败。
- JSON 错误码必须是 `LBR-CLI-001`。
- 操作后 `libra fsck --connectivity-only` 通过。
