### `cli.config-scopes`

目的：覆盖 `--local`、`--global`、`--system` scope flags。

最小步骤：

```bash
# Prelude (RUN_ROOT/SAFE_PATH/libra()/gitfix()) copied once at top of run per converged form (§3.3.1 and "手动执行 prelude").
SCENARIO="cli.config-scopes"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"

mkdir -p isolated-home
libra init scope-a
libra init scope-b

cd "$RUN_DIR/scope-a"
libra config --local set test.scope local-a
libra config --global set test.scope global-value
libra config --local get test.scope
libra config --global get test.scope

cd "$RUN_DIR/scope-b"
libra config --global get test.scope
! libra config --local get test.scope
! libra config --system list
```

断言：local key 只在当前 repo 可见；global key 在隔离 HOME 下跨 repo 可见；`--system` 当前为移除/拒绝路径，必须非 0 退出并给出不支持或不可用的明确错误；场景不得写入真实用户全局配置。

补充可执行断言：
- 使用隔离 global DB 验证 `--global set` 后在另一个 repo 中 `libra config --global get` 可见，而 `--local` 不可见。
- `! libra config --system list` 的 stderr 必须包含 "不支持" / "system" 或对应 LBR- 错误标识。
- 操作后用隔离 HOME + global DB 再次 `libra config --global list` 验证只有本场景设置的 global key，无其他污染。

