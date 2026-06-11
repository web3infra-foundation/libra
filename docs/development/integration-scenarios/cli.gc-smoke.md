### `cli.gc-smoke`

目的：覆盖 `gc` 与同族维护命令 `prune` 对不可达 loose object 的黑盒清理路径，确保 dry-run 与 `--no-prune` 不删除对象、正式 `gc --prune=now` / `prune` 删除不可达对象、兼容性标志 `--auto`/`--aggressive`/`--force` 成功且 warnings 可见、`prune --expire` 仅过期早于截止时间的对象、位置参数 `<head>` 作为额外可达性根保留对象，并保持仓库连通性可由 `fsck` 验证。

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

libra gc --no-prune                                 # 关闭修剪：真实（非 dry-run）gc 也不得删除不可达对象
libra cat-file -t "$OID" | grep '^blob$'

libra gc --prune=now
! libra cat-file -t "$OID"
libra fsck --connectivity-only

# 兼容性标志：--auto / --aggressive 为接受但不改变清理行为的 no-op（warnings 在 JSON data 中可见），
# --force 在上一次 gc 刚结束（锁可用、无残留）时也必须成功
libra --json gc --auto >gc-auto.json
python3 -c "import json; d=json.load(open('gc-auto.json')); assert d['ok'] is True; assert any('--auto is accepted for compatibility' in w for w in d['data']['warnings'])"
libra --json gc --aggressive >gc-aggressive.json
python3 -c "import json; d=json.load(open('gc-aggressive.json')); assert d['ok'] is True; assert any('does not repack' in w for w in d['data']['warnings'])"
libra fsck --connectivity-only
libra --json gc --force >gc-force.json
python3 -c "import json; d=json.load(open('gc-force.json')); assert d['ok'] is True; assert any('gc lock was available' in w for w in d['data']['warnings'])"

# 同族命令 `libra prune`：构造新的不可达 blob 后验证三条路径
printf 'prune unreachable blob\n' > prune-me.txt
POID="$(libra hash-object -w prune-me.txt)"
libra --json prune --dry-run >prune-dry-run.json
python3 -c "import json; d=json.load(open('prune-dry-run.json')); assert d['ok'] is True; assert d['data']"
libra cat-file -t "$POID" | grep '^blob$'         # dry-run 不删除
libra prune --expire=2000-01-01                     # 仅过期早于截止时间的对象
libra cat-file -t "$POID" | grep '^blob$'         # 新对象被保留
libra prune -v                                      # 默认策略删除全部不可达对象
! libra cat-file -t "$POID"

# 位置参数 <head>：作为额外可达性根，保留其可达对象；同批其余不可达对象仍被修剪
printf 'prune keep blob\n' > prune-keep.txt
KOID="$(libra hash-object -w prune-keep.txt)"
printf 'prune drop blob\n' > prune-drop.txt
DOID="$(libra hash-object -w prune-drop.txt)"
libra --json prune "$KOID" >prune-head.json
python3 -c "import json; d=json.load(open('prune-head.json')); assert d['ok'] is True; assert any(o['object_id']=='$DOID' for o in d['data']['objects'])"
libra cat-file -t "$KOID" | grep '^blob$'           # head 指定的对象被保留
! libra cat-file -t "$DOID"                          # 其余不可达对象被修剪
libra fsck --connectivity-only
```

断言：`hash-object -w` 生成一个未被任何 ref、index 或 reflog 保护的 loose blob；`gc --dry-run --prune=now` 必须返回成功 JSON envelope 且不能删除该对象；随后 `gc --prune=now` 必须删除该不可达对象；删除后 `cat-file -t <oid>` 必须非 0，并输出 `LBR-*` 或 object-not-found 文本；`prune --dry-run` 必须返回成功 JSON envelope（`data.objects` 列出该 OID）且不删除对象；`prune --expire=2000-01-01` 必须保留刚写入的新对象；`prune -v` 必须删除该不可达对象；最后 `fsck --connectivity-only` 必须通过。

补充可执行断言：
- `libra --json gc --dry-run --prune=now` 必须 `ok:true` 且带 `data`。
- dry-run 后 `libra cat-file -t "$OID"` 仍输出 `blob`。
- 正式 prune 后 `libra cat-file -t "$OID"` 必须失败，stderr/stdout 包含 `LBR-*` 或 `object not found`。
- `libra --json prune --dry-run` 必须 `ok:true`，`data` 内 `objects` 含目标 OID，且对象仍可被 `cat-file -t` 读取。
- `libra prune --expire=2000-01-01` 之后目标对象仍输出 `blob`（远早于其 mtime，未过期）。
- `libra prune -v` 之后 `libra cat-file -t "$POID"` 必须失败，stderr/stdout 包含 `LBR-*` 或 `object not found`。
- 操作后 `libra fsck --connectivity-only` 通过。
