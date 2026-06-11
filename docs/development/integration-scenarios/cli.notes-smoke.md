### `cli.notes-smoke`

目的：覆盖 `libra notes`（提交注释，对应 `git notes`）的黑盒闭环——`add`/`list`/`show`/`remove`、显式对象定位（`add <object>`/`list <object>`/`show <object>`）、消息来源 `-m`/`-F`/`-f`、`--ref` 自定义 notes ref、JSON envelope，以及负向路径（未带 `-f` 重复添加、删除后再 show、空消息）。Libra 的 `refs/notes/*` 仅本地，不随 push/fetch/clone 传输。

最小步骤：

```bash
SCENARIO="cli.notes-smoke"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
# (prelude libra - short converged)

libra init repo
cd repo
libra config set user.name "Libra Notes Test"
libra config set user.email "notes@example.invalid"
printf 'base\n' > tracked.txt
libra add tracked.txt
libra commit -m "test: notes base" --no-verify
BASE="$(libra rev-parse HEAD)"

# 默认对象 = HEAD：add / show / list
libra notes add -m "first note"
libra notes show | grep "first note"
libra --json notes show
libra --json notes list           # data 中 annotated_object 含 BASE

# 未带 -f 重复添加必须失败；-f 覆盖
! libra notes add -m "second note"
libra notes add -f -m "overwritten note"
libra notes show | grep "overwritten note"

# 显式对象的 show / list（对象作用域）
libra notes show "$BASE" | grep "overwritten note"
libra --json notes list "$BASE"   # data 中 annotated_object == BASE

# -F 从文件读取消息，附加到第二个提交
printf 'file note body\n' > note.txt
printf 'base\nmore\n' > tracked.txt
libra add tracked.txt
libra commit -m "test: notes second" --no-verify
libra notes add -F note.txt
libra notes show | grep "file note body"

# 自定义 notes ref 与默认 refs/notes/commits 隔离；add 显式指定对象
libra notes --ref refs/notes/review add -m "review note" "$BASE"
libra --json notes --ref refs/notes/review list   # data 中 annotated_object 含 BASE

# 删除 HEAD 注释后 show 必须 not-found；显式对象删除仍可用
libra notes remove
! libra notes show
libra notes remove "$BASE"

# 空消息必须被拒绝
! libra notes add -m "" "$BASE"

libra fsck --connectivity-only
```

断言：`notes add -m` 为 HEAD 写入注释，`notes show` 输出该文本；`--json notes show` / `--json notes list` 必须 `ok:true` 且带 `data`，list 的 `data` 中 `annotated_object` 含被注释 commit 的完整 hash；未带 `-f` 对已有注释的对象再次 `add` 必须失败（`already exists` / `LBR-*`），`-f` 覆盖后 `show` 输出新文本；显式对象 `notes show "$BASE"` 输出该对象的注释文本，对象作用域 `--json notes list "$BASE"` 必须 `ok:true` 且 `data` 含 BASE 的完整 hash；`-F note.txt` 把文件内容写入第二个提交的注释；`--ref refs/notes/review` 的注释独立于默认 ref，且 `add` 可显式指定对象 `"$BASE"`（review ref 的 list 输出含 BASE）；`notes remove` 删除 HEAD 注释后 `notes show` 必须失败（not-found / `LBR-*`），显式对象 `notes remove "$BASE"` 仍成功；空消息 `notes add -m ""` 必须以用法错误失败（`empty` / `LBR-*`）；最后 `fsck --connectivity-only` 必须通过。

补充可执行断言：
- `libra --json notes show` 与 `libra --json notes list` 必须 `ok:true` 且带 `data`。
- `libra --json notes list` 的 `data` 必须包含被注释提交的完整 hash（`annotated_object`）。
- 对象作用域 `libra --json notes list "$BASE"` 与 review ref 的 `--json notes --ref refs/notes/review list` 输出均必须含 BASE 完整 hash。
- 未带 `-f` 的重复 `add`、删除后 `show`、空消息 `add` 三条负向命令必须退出非 0，stderr/stdout 含 `LBR-*` 或对应文本（`already` / `note` / `empty`）。
- 操作后 `libra fsck --connectivity-only` 通过。
