### `cli.branch-switch-checkout`

目的：覆盖 `branch`、`switch`、`checkout` 的分支创建、切换、detached HEAD、兼容 alias、分支重命名/删除和路径恢复行为。

最小步骤：

```bash
# Converged short form.
SCENARIO="cli.branch-switch-checkout"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"

libra init branch-repo
cd branch-repo

libra config set user.name "Libra Branch Test"
libra config set user.email "branch@example.invalid"
printf 'base\n' > base.txt
libra add base.txt
libra commit -m "test: branch base"

libra branch --show-current
libra branch feature/cli-smoke
libra branch feature/from-main main
libra branch --list
libra switch feature/cli-smoke
printf 'feature\n' > feature.txt
libra add feature.txt
libra commit -m "test: feature branch"
libra checkout main
libra checkout -b compat-checkout
libra checkout main
libra switch -c switch-created main
libra switch main

BASE_COMMIT="$(libra rev-parse HEAD)"
libra switch --detach "$BASE_COMMIT"
libra rev-parse --abbrev-ref HEAD
libra switch main

libra branch -m feature/from-main feature/renamed
libra branch -d feature/renamed
libra branch -D feature/cli-smoke

printf 'dirty\n' > base.txt
libra checkout -- base.txt
grep 'base' base.txt
libra branch

# Verify branch list JSON output
libra --json branch --list >branch-list.json
python3 -c "import json; d=json.load(open('branch-list.json')); assert d['ok'] is True; assert isinstance(d['data'].get('branches'), list)"
```

负向步骤：

```bash
cd "$RUN_DIR/branch-repo"
! libra branch "bad branch"
! libra switch no-such-branch
! libra checkout no-such-branch
! libra branch -d no-such-branch
```

断言：`branch --show-current` 输出当前分支；从 HEAD 和指定 base 创建分支成功；`switch` / `checkout` 都能切换到已存在分支；`checkout -b` 与 `switch -c` 都能创建并切换分支；detached HEAD 下 `rev-parse --abbrev-ref HEAD` 输出 detached 语义或 `HEAD`；`branch -m` 后旧名消失、新名可列出；安全删除已合并分支成功，强制删除未合并分支成功；`checkout -- <path>` 能恢复工作区文件；非法分支名、缺失分支或缺失删除目标必须非 0 退出并保留现有分支状态。

补充可执行断言：
- 关键分支操作后 `libra --json branch --list` 解析验证新分支出现。
- detached 后 `libra symbolic-ref HEAD` 必须失败（或输出 "HEAD" 且非 ref），这是 Libra/Git 符号引用限制的验证点。
- `libra --json switch main` 成功后验证 `ok:true`。
- 所有分支操作后 `libra fsck` 通过；删除分支后 `libra --json show-ref --heads` 的 `data.entries[]` 不再包含已删分支。

