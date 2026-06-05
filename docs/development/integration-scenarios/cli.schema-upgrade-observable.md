### `cli.schema-upgrade-observable`

目的：验证新建仓库的 SQLite schema 可被 CLI 正常使用。

最小步骤：

```bash
SCENARIO="cli.schema-upgrade-observable"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
# (prelude provides libra() -- converged short form, long wrapper removed)

libra init schema-repo
cd schema-repo

libra db status
libra db --json status >db-status.json
python3 -c "import json; d=json.load(open('db-status.json')); assert d['ok'] is True; assert 'current_version' in d['data']; assert 'latest_version' in d['data']; assert 'state' in d['data']"
libra db upgrade
libra db status

libra config set user.name "Libra Schema Test"
libra config set user.email "schema@example.invalid"
printf 'schema\n' > schema.txt
libra add schema.txt
libra commit -m "test: schema usable after status"
libra log --oneline -n 1
libra fsck --connectivity-only
```

负向步骤：

```bash
cd "$RUN_ROOT/repos"
mkdir not-a-repo
cd not-a-repo
! libra db status
! libra db upgrade
```

断言：`db status` 只读取 schema 状态并退出码为 0；`db --json status` 输出 current/latest/state 等结构化字段或等价 schema 状态；`db upgrade` 对已是当前版本的仓库应成功且幂等；升级/状态检查后提交闭环和 `fsck --connectivity-only` 不触发 migration 或 schema 错误；非仓库目录中的 `db status` / `db upgrade` 必须失败并提示缺少 Libra 仓库。

补充可执行断言：
- `libra --json db status` 必须 `ok:true`，`data.current_version == data.latest_version` 且 `data.state` 为兼容状态。
- 非仓库目录执行 `libra db status` 必须非 0，stderr 包含 "not a libra repository" 或 LBR-REPO-001。
- 操作后 `libra fsck --connectivity-only` 必须 0 退出。
- 验证 schema 升级幂等：连续两次 `libra db upgrade` 均成功且无副作用。

