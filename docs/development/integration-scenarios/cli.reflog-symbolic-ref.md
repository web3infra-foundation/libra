### `cli.reflog-symbolic-ref`

目的：覆盖 `reflog` 与 `symbolic-ref` 的用户可观察 ref 日志和符号引用行为。

最小步骤：

```bash
# Short converged form.
SCENARIO="cli.reflog-symbolic-ref"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"

libra init ref-log-repo
cd ref-log-repo
libra config set user.name "Libra Reflog Test"
libra config set user.email "reflog@example.invalid"
printf 'one\n' > ref.txt
libra add ref.txt
libra commit -m "test: reflog one"
libra branch feature/ref-log
libra switch feature/ref-log
printf 'two\n' >> ref.txt
libra add ref.txt
libra commit -m "test: reflog two"

libra reflog show
libra reflog show HEAD
libra reflog show --stat
libra reflog show --pretty oneline
libra reflog exists HEAD
libra --json reflog expire --all --dry-run --expire=all
libra symbolic-ref HEAD
libra symbolic-ref --short HEAD
libra symbolic-ref HEAD refs/heads/main
libra symbolic-ref --short HEAD
libra symbolic-ref HEAD refs/heads/feature/ref-log
```

负向步骤：

```bash
cd "$RUN_DIR/ref-log-repo"
! libra reflog show refs/heads/no-such-branch
! libra reflog exists refs/heads/no-such-branch
! libra --json reflog expire
! libra symbolic-ref refs/heads/bad
! libra symbolic-ref HEAD refs/tags/not-a-branch
```

断言：`reflog show` 能观察 commit、branch switch 或 HEAD 更新记录；`--stat` / `--pretty oneline` 输出可用于脚本断言；`reflog exists HEAD` 可用于脚本探测；`reflog expire --all --dry-run --expire=all` 必须返回 `reflog.expire` JSON envelope 且不写入；无 ref 且无 `--all` 的 `reflog expire` 必须返回 `LBR-CLI-002`（Libra intentional-difference：Git 静默 no-op）；`symbolic-ref HEAD` 和 `--short` 输出当前分支；`symbolic-ref HEAD refs/heads/<branch>` 可切换 HEAD 的符号目标并被后续读取观察；`reflog exists` 对缺失 ref 必须失败，非 HEAD 名称和非法 symbolic-ref 目标必须失败。注意 `reflog show <missing>` 当前可能返回空列表而非失败，不能作为负向断言，只能断言输出为空或 `count=0`。

补充可执行断言：
- `libra --json reflog show` 验证 `ok:true`，且 entries 中至少包含 "commit:" 或 "checkout:" 条目，并包含本场景创建的提交消息。
- `libra --json reflog expire --all --dry-run --expire=all` 验证 `ok:true`、`command == "reflog.expire"`；无参 `expire` 的错误 JSON 验证 `error_code == "LBR-CLI-002"`。
- `libra --json symbolic-ref HEAD` 验证 `ok:true`，且 data 中的 ref 输出为 "refs/heads/..."。
- 非法 symbolic-ref 目标的失败必须包含稳定错误（LBR- 或 "not a branch" 类消息）。
- 操作前后 `libra --json show-ref --heads` 验证 `data.entries[]` 一致性（无意外丢失）。
