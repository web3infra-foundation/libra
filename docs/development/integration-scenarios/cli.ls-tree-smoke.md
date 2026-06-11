### `cli.ls-tree-smoke`

目的：覆盖 `ls-tree` 新增 Git 兼容 plumbing 命令的最小黑盒行为，验证 commit/tree 内容可读、目录过滤递归、显示参数矩阵（`-t`/`-l`/`-z`/`--name-only`/`--name-status`/`--object-only`/`--abbrev[=N]`）、JSON envelope、负向路径错误和仓库健康。

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
printf 'root\n' >README.md
printf 'lib\n' >src/lib.rs
printf 'deep\n' >src/nested/deep.txt
test -f src/nested/deep.txt
libra add README.md src/lib.rs src/nested/deep.txt
libra commit -m "test: ls-tree fixture" --no-verify
libra ls-tree HEAD
libra ls-tree -r HEAD src
libra ls-tree -d -r HEAD src
libra ls-tree -r HEAD
libra ls-tree -t -r HEAD
libra ls-tree -l HEAD
libra ls-tree --name-only HEAD
libra ls-tree --name-status -r HEAD
libra ls-tree --object-only HEAD
libra ls-tree --abbrev=7 HEAD
libra ls-tree --abbrev --object-only HEAD
libra ls-tree -z --name-only HEAD | od -c
libra --json ls-tree -r HEAD src >ls-tree.json
libra fsck
```

负向步骤：

```bash
cd "$RUN_DIR/ls-tree-repo"
! libra ls-tree HEAD missing
```

断言：默认输出包含 root tree 的 `README.md` 与 `src`；`-r HEAD src` 输出 `src/lib.rs` 和 `src/nested/deep.txt`；`-d -r HEAD src` 输出 `src` 和 `src/nested` 目录项但不输出 blob；`-r HEAD` 不含 ` tree ` 行而 `-t -r HEAD` 同时输出 tree 行（`\tsrc`、`\tsrc/nested`）与 blob 行；`-l` 为 blob 追加大小列（README.md 为 ` 5\t`）、tree 显示 ` -\t`；`--name-only HEAD` 精确输出 `README.md`、`src`、`tracked.txt` 三行路径、`--name-status -r HEAD` 作为别名精确输出四条递归路径；`--object-only` 仅输出裸 oid（含 README.md 的 blob oid，不含路径与 `blob` 字样）；`--abbrev=7` 与裸 `--abbrev` 均把 oid 截断为 7 个 hex 字符且不再出现完整 oid；`-z --name-only` 输出以 NUL 结尾（`README.md\0src\0`）且不含换行；`--json` 返回 `ok:true` 且命令名为 `ls-tree`；缺失路径必须非 0 退出并报告可诊断错误；场景结束后 `libra fsck` 通过。
