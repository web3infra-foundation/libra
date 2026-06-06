### `cli.gc-smoke`

目的：覆盖 `gc` 对不可达 loose object 的黑盒清理路径，确保 dry-run 不删除对象、正式 `--prune=now` 删除不可达对象，并保持仓库连通性可由 `fsck` 验证。

最小步骤：

```bash
SCENARIO="cli.gc-smoke"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
# (prelude libra - short converged)

libra init repo
cd repo
libra config set user.name "Libra GC Test"
libra config set user.email "gc@example.invalid"
printf 'tracked\n' > tracked.txt
libra add tracked.txt
libra commit -m "test: gc base" --no-verify

printf 'gc unreachable blob\n' > unreachable.txt
OID="$(libra hash-object -w unreachable.txt)"
libra cat-file -t "$OID" | grep '^blob$'

libra --json gc --dry-run --prune=now >gc-dry-run.json
python3 -c "import json; d=json.load(open('gc-dry-run.json')); assert d['ok'] is True; assert d['data']"
libra cat-file -t "$OID" | grep '^blob$'

libra gc --prune=now
! libra cat-file -t "$OID"
libra fsck --connectivity-only
```

断言：`hash-object -w` 生成一个未被任何 ref、index 或 reflog 保护的 loose blob；`gc --dry-run --prune=now` 必须返回成功 JSON envelope 且不能删除该对象；随后 `gc --prune=now` 必须删除该不可达对象；删除后 `cat-file -t <oid>` 必须非 0，并输出 `LBR-*` 或 object-not-found 文本；最后 `fsck --connectivity-only` 必须通过。

补充可执行断言：
- `libra --json gc --dry-run --prune=now` 必须 `ok:true` 且带 `data`。
- dry-run 后 `libra cat-file -t "$OID"` 仍输出 `blob`。
- 正式 prune 后 `libra cat-file -t "$OID"` 必须失败，stderr/stdout 包含 `LBR-*` 或 `object not found`。
- 操作后 `libra fsck --connectivity-only` 通过。
