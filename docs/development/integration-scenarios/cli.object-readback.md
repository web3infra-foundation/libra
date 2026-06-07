### `cli.object-readback`

目的：验证通过 CLI 写入的 commit/tree/blob/ref 能通过 CLI plumbing 和 history inspection 命令读回，覆盖 `rev-parse`、`rev-list`、`show`、`show-ref`、`cat-file`、`hash-object`、`fsck`。

最小步骤：

```bash
# Short converged form.
SCENARIO="cli.object-readback"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"

libra init object-repo
cd object-repo

libra config set user.name "Libra Object Test"
libra config set user.email "object@example.invalid"
mkdir -p docs src
printf 'object root\n' > README.md
printf 'object docs\n' > docs/guide.md
printf 'fn main() {}\n' > src/main.rs
libra add README.md docs/guide.md src/main.rs
libra commit -m "test: object readback"

HEAD_ID="$(libra rev-parse HEAD)"
libra rev-parse --short HEAD
libra rev-parse --show-toplevel
libra rev-list HEAD
libra show --no-patch HEAD
libra show --stat HEAD
libra show HEAD:docs/guide.md
libra show-ref --head
libra show-ref --heads
libra cat-file -t "$HEAD_ID"
libra cat-file -s "$HEAD_ID"
libra cat-file -p "$HEAD_ID"
libra cat-file -e "$HEAD_ID"

printf 'loose blob\n' > loose.txt
BLOB_ID="$(libra hash-object -w loose.txt)"
libra cat-file -t "$BLOB_ID"
libra cat-file -p "$BLOB_ID"
printf 'stdin blob\n' | libra hash-object --stdin
printf 'README.md\ndocs/guide.md\n' | libra hash-object --stdin-paths

printf 'rev-list second\n' > docs/rev-list.md
libra add docs/rev-list.md
libra commit -m "test: rev-list second"
SECOND_ID="$(libra rev-parse HEAD)"
libra rev-list -n 1 HEAD
libra rev-list --skip 1 HEAD
libra rev-list "$HEAD_ID..HEAD"
libra rev-list HEAD "^$HEAD_ID"
libra rev-list --count HEAD
libra rev-list --parents -n 1 HEAD
libra rev-list --timestamp -n 1 HEAD
libra --json rev-list --count HEAD

libra fsck
libra fsck --connectivity-only
libra fsck "$HEAD_ID"
```

负向步骤：

```bash
cd "$RUN_DIR/object-repo"
! libra rev-parse no-such-revision
! libra show HEAD:no-such-path
! libra cat-file -p no-such-object
! libra hash-object missing-file.txt
! libra fsck no-such-object
```

断言：`rev-parse HEAD` 输出可传递给 `cat-file`、`fsck` 等后续命令；`rev-list HEAD` 至少包含当前提交，`-n`/`--skip`/`--count`/`--parents`/`--timestamp` 与 `A..B`/`^A` 范围过滤输出符合两提交 fixture；`show --no-patch` / `show --stat` 能读回 commit 元数据和变更统计；`show HEAD:<path>` 输出内容必须与提交前文件内容一致；`show-ref --head` / `--heads` 能列出 HEAD 和本地分支；`cat-file -t/-s/-p/-e` 分别返回类型、大小、内容和存在性；`hash-object -w` 写入的 loose blob 可由 `cat-file` 读回；`hash-object --stdin` / `--stdin-paths` 可计算输入内容或路径列表；`fsck` 和 `fsck --connectivity-only` 在健康仓库中退出码为 0；缺失 revision、path、object 或 file 必须失败且不写入新对象。

补充可执行断言（plumbing 场景重点）：
- `libra --json cat-file -p $HEAD_ID` 必须 `ok:true` 且 data 中的 commit 结构包含 `object_type == "commit"`、`tree`、`parents[]`、`message`。
- `libra --json rev-list HEAD` / `libra --json rev-list --count HEAD` 返回 `data.commits[]` 与 `data.total`，每个 commit 元素为 hash 字符串。
- `libra rev-list "$HEAD_ID..HEAD"` 与 `libra rev-list HEAD "^$HEAD_ID"` 只返回第二个提交；`--parents -n 1` 同时包含第二个提交和父提交；`--count HEAD` 输出 `2`。
- 所有对象操作后 `libra fsck` 必须通过；写入 blob 后 `libra --json cat-file -t $BLOB_ID` 验证类型为 "blob"。
- 负向 cat-file / rev-parse 错误必须返回 LBR- 码（通过 JSON error envelope 或 stderr 捕获）。
