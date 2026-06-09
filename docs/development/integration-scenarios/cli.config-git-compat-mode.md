### `cli.config-git-compat-mode`

目的：集中覆盖 `ConfigArgs` 上的 Git 兼容隐藏 flag 与位置参数翻译路径。

最小步骤：

```bash
# Short converged.
SCENARIO="cli.config-git-compat-mode"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"

libra init config-repo
cd config-repo
libra config set --add remote.origin.fetch "+refs/heads/*:refs/remotes/origin/*"
libra config set --add remote.origin.fetch "+refs/tags/*:refs/tags/*"

libra config user.compat value-from-positional
libra config --get user.compat
libra config --add user.compat second-value
libra config --get-all user.compat
libra config --get-regexp '^user\\.'
libra config --list
libra config -l
libra config --list --show-origin
libra config --unset user.compat
libra config --unset-all remote.origin.fetch
libra config --get -d fallback missing.compat
libra config --get --default fallback-long missing.compat.long
```

负向步骤：

```bash
! libra config --default fallback user.bad-default value
! libra config init value
! libra config --import user.name
```

断言：位置参数 `key valuepattern` 的默认模式等价于 set；`--get` / `--get-all` / `--get-regexp` / `--list` / `-l` / `--show-origin` / `--add` / `--unset` / `--unset-all` / `-d` / `--default` 均至少有一个直接 invocation 覆盖；`--default` 只能与 get 类模式组合；不含 section 的 key 非 0 退出并对 `init` / `clone` 给出“这是顶层命令”的提示。`--import` 的正向导入路径依赖系统 `git`，由 `cli.config-import-path-edit` 覆盖；本场景只保留 `--import <key>` 的参数拒绝路径，避免把普通 compat 场景误标为 `requires_git`。

补充可执行断言：
- `libra --json config --get user.compat` 必须 `ok:true`，且 `data.value == "value-from-positional"`。
- `libra --json config --get-all user.compat` 必须返回 `data.entries[]`，且包含 `value-from-positional` 与 `second-value`。
- `libra --json config --list --show-origin` 必须返回 `data.entries[]`，每条包含 key/value 与 origin 或 scope 字段。
- `libra config --get --default fallback-long missing.compat.long` 必须输出 fallback-long 且退出码为 0。
- 负向 `--default` 非 get 模式、`config init value`、`--import user.name` 均必须非 0，stderr 包含可识别错误文本或 LBR- 稳定码。

