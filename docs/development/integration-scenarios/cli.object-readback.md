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
libra show-ref --head --no-head
libra show-ref --heads
libra show-ref --branches
libra show-ref --hash --heads
libra show-ref --abbrev=12 --heads
libra show-ref --hash=12 --heads
libra show-ref --no-hash --heads
libra show-ref --abbrev=12 --no-abbrev --heads
libra --json show-ref --abbrev=12 --heads
libra show-ref --verify refs/heads/main
libra show-ref --verify HEAD
libra show-ref --exists refs/heads/main
libra show-ref --verify --no-verify main
libra show-ref --exists --no-exists refs/heads/main
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
libra hash-object --no-filters loose.txt
printf 'loose blob\n' | libra hash-object --stdin
printf 'loose blob\n' | libra --json hash-object --stdin --path loose.txt
! printf 'loose blob\n' | libra hash-object --stdin --path loose.txt --no-filters
! libra hash-object -t bogus loose.txt

printf 'rev-list second\n' > docs/rev-list.md
libra add docs/rev-list.md
libra config user.name "Rev List Committer"
libra config user.email rev-list-committer@example.com
libra commit -m "test: rev-list second" --author "Rev List Author <rev-list@example.com>" --no-verify
libra rev-list HEAD
libra rev-list HEAD HEAD~1
libra rev-list HEAD~1..HEAD
libra rev-list ^HEAD~1 HEAD
libra rev-list HEAD~1...HEAD
libra rev-list --count HEAD
libra rev-list -n 1 HEAD
libra rev-list --skip 1 --max-count 1 HEAD
libra rev-list --count --since 0 HEAD
libra rev-list --count --after 0 HEAD
libra rev-list --count --until 0 HEAD
libra rev-list --count --before 0 HEAD
libra rev-list --count --min-parents 1 --no-min-parents HEAD
libra rev-list --count --max-parents 0 --no-max-parents HEAD
libra rev-list --count --first-parent HEAD
libra rev-list --author rev-list@example.com HEAD
libra rev-list --count --author missing-author HEAD
libra rev-list --committer rev-list-committer@example.com HEAD
libra rev-list --count --committer missing-committer HEAD
libra rev-list --grep "rev-list second" HEAD
libra rev-list --grep "object readback" --grep "rev-list second" HEAD
libra rev-list --count --grep "REV-LIST SECOND" HEAD
libra --json rev-list HEAD
LATEST_HEAD="$(libra rev-parse HEAD)"
libra fsck
libra fsck --connectivity-only
libra fsck "$HEAD_ID"
libra tag -m "release fixture" v1.0
libra tag v1-light
libra show-ref --branches --no-branches
libra show-ref --tags --no-tags
libra show-ref --dereference --tags v1.0
libra show-ref --dereference --no-dereference --tags v1.0
libra for-each-ref --points-at "$LATEST_HEAD" --format='%(refname) %(objecttype)'
libra --json for-each-ref --points-at "$LATEST_HEAD"
! libra cat-file -t deadbeef
```

关键断言：

- `rev-parse`、`show`、`show-ref`、`for-each-ref`、`cat-file`、`hash-object`（含 `--path` / `--no-filters` 兼容入口）、`rev-list`、`fsck` 当前正向路径可用。
- `rev-list --count` 输出过滤后的提交数量；`rev-list -n` 限制输出行数；`rev-list --skip --max-count` 可跳过当前 HEAD 后定位父提交；`--since` / `--after` 与 `--until` / `--before` 时间过滤可观察；multi revision、`A..B`、`^A`、`A...B`、`--first-parent`、`--author`、`--committer`、`--grep` 和 parent bound reset aliases 均有正向断言；重复 `--grep` 按 OR 匹配，默认大小写敏感。
- `show-ref --branches` 与 `--heads` 输出一致；`--no-branches` / `--no-tags` reset aliases 恢复默认 branch+tag 范围；`show-ref --abbrev=12` / `--hash=12` 输出 HEAD 的 12 位前缀；`--no-abbrev` 恢复完整哈希，`--no-hash` 按 Git 行为作为 hash-only alias；`show-ref --dereference` 对 annotated tag 输出 `refs/tags/<name>^{}` peeled 行，`--no-dereference` 取消 peeled 行；`--no-head`、`--no-verify`、`--no-exists` 可恢复对应默认行为；`show-ref --verify` 只接受完整 refname / `HEAD`；`show-ref --exists` 成功静默，缺失 ref 失败。
- `for-each-ref --points-at` 对 branch、lightweight tag 和 annotated tag peeled target 的过滤可观察；`--json` 返回标准 envelope。
- 缺失 revision/object 和非法 hash-object 类型必须失败。
- `ls-files`、高级 `for-each-ref --contains/--merged`、高级 `rev-parse` 和 `rev-list` path/cherry-pick traversal filters 不属于当前场景正向覆盖。
