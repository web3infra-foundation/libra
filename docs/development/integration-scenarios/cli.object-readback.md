### `cli.object-readback`

目的：验证当前 CLI 写入的 commit/blob/ref 能通过已注册 plumbing/history-inspection 命令读回。

最小步骤：

```bash
SCENARIO="cli.object-readback"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"

libra init object-repo
cd object-repo
libra config user.name "Libra Object Test"
libra config user.email "object@example.invalid"
mkdir -p docs
printf 'object root\n' > README.md
printf 'object docs\n' > docs/guide.md
libra add README.md docs/guide.md
libra commit -m "test: object readback" --no-verify

HEAD_ID="$(libra rev-parse HEAD)"
libra rev-parse --short HEAD
libra rev-parse --show-toplevel
libra --json rev-parse HEAD
! libra rev-parse no-such-revision

libra show --no-patch HEAD
libra show HEAD:docs/guide.md
libra --json show HEAD
libra show-ref --head
libra show-ref --heads
libra show-ref --hash --heads
libra show-ref --abbrev=12 --heads
libra show-ref --hash=12 --heads
libra --json show-ref --abbrev=12 --heads
libra show-ref --verify refs/heads/main
libra show-ref --verify HEAD
libra show-ref --exists refs/heads/main
! libra show-ref --verify main
! libra show-ref --exists refs/heads/missing

libra cat-file -t "$HEAD_ID"
libra cat-file -s "$HEAD_ID"
libra cat-file -p "$HEAD_ID"
libra cat-file -e "$HEAD_ID"
printf 'loose blob\n' > loose.txt
BLOB_ID="$(libra hash-object -w loose.txt)"
libra cat-file -t "$BLOB_ID"
libra cat-file -p "$BLOB_ID"
libra show "$BLOB_ID"
libra --json hash-object loose.txt
printf 'loose blob\n' | libra hash-object --stdin
! libra hash-object -t bogus loose.txt

printf 'rev-list second\n' > docs/rev-list.md
libra add docs/rev-list.md
libra commit -m "test: rev-list second" --no-verify
libra rev-list HEAD
libra rev-list --count HEAD
libra rev-list -n 1 HEAD
libra rev-list --skip 1 --max-count 1 HEAD
libra --json rev-list HEAD
libra fsck
libra fsck --connectivity-only
libra fsck "$HEAD_ID"
libra tag -m "release fixture" v1.0
libra show-ref --dereference --tags v1.0
! libra cat-file -t deadbeef
```

关键断言：

- `rev-parse`、`show`、`show-ref`、`cat-file`、`hash-object`、`rev-list`、`fsck` 当前正向路径可用。
- `rev-list --count` 输出过滤后的提交数量；`rev-list -n` 限制输出行数；`rev-list --skip --max-count` 可跳过当前 HEAD 后定位父提交。
- `show-ref --abbrev=12` / `--hash=12` 输出 HEAD 的 12 位前缀；`show-ref --dereference` 对 annotated tag 输出 `refs/tags/<name>^{}` peeled 行；`show-ref --verify` 只接受完整 refname / `HEAD`；`show-ref --exists` 成功静默，缺失 ref 失败。
- 缺失 revision/object 和非法 hash-object 类型必须失败。
- `for-each-ref`、`ls-files`、高级 `rev-parse`/`rev-list` 过滤不属于当前场景正向覆盖。
