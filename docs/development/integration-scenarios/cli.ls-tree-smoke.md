### `cli.ls-tree-smoke`

目的：覆盖 `ls-tree` 当前未公开的 Git 兼容 plumbing 命令状态，确保它不会被误当作已发布 CLI；runner 创建 tree fixture 后断言 `libra ls-tree` 返回标准 unknown-command JSON 错误，并在场景末尾验证仓库健康。

最小步骤：

```bash
SCENARIO="cli.ls-tree-smoke"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
# Short converged (prelude)
libra init ls-tree-repo
cd ls-tree-repo
libra config user.name "Libra Integration"
libra config user.email integration@example.invalid
printf 'base\n' >tracked.txt
libra add tracked.txt
libra commit -m "initial" --no-verify
mkdir -p src/nested
printf 'deep\n' >src/nested/deep.txt
test -f src/nested/deep.txt

! libra --json ls-tree HEAD
libra fsck --connectivity-only
```

断言：`ls-tree` 当前未注册为顶层命令；`libra --json ls-tree HEAD` 必须非 0 退出，JSON 错误码为 `LBR-CLI-001`；场景结束后 `libra fsck --connectivity-only` 通过。

补充可执行断言：
- `libra --json ls-tree HEAD` 必须失败。
- JSON 错误码必须是 `LBR-CLI-001`。
- 操作后 `libra fsck --connectivity-only` 通过。
