### `cli.gc-smoke`

目的：覆盖已公开的 `gc` 与同族维护命令 `prune` 基础 dry-run 行为，确保它们能在含 unreachable loose blob 的仓库中返回成功 JSON envelope；runner 同时验证 `maintenance run --dry-run --task gc` 入口仍返回成功 JSON envelope，并在场景末尾验证仓库健康。

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

libra --json gc --dry-run >gc.json
python3 -c "import json; d=json.load(open('gc.json')); assert d['ok'] is True; assert d['data']['dry_run'] is True"
libra --json prune --dry-run >prune.json
python3 -c "import json; d=json.load(open('prune.json')); assert d['ok'] is True; assert d['data']['dry_run'] is True"
libra --json maintenance run --dry-run --task gc >maintenance-gc.json
python3 -c "import json; d=json.load(open('maintenance-gc.json')); assert d['ok'] is True"
libra fsck --connectivity-only
```

断言：`gc` 和 `prune` 当前已注册为顶层命令；`libra --json gc --dry-run` 与 `libra --json prune --dry-run` 必须 0 退出并返回 `ok:true` 的 JSON envelope；`maintenance run --dry-run --task gc` 必须成功；操作后 `libra fsck --connectivity-only` 通过。

补充可执行断言：
- `libra --json gc --dry-run` 必须成功，且 `data.dry_run` 为 `true`。
- `libra --json prune --dry-run` 必须成功，且 `data.dry_run` 为 `true`。
- `libra --json maintenance run --dry-run --task gc` 必须 `ok:true`。
- 操作后 `libra fsck --connectivity-only` 通过。
