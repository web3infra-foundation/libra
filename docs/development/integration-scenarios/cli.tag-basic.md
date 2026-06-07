### `cli.tag-basic`

目的：覆盖 `tag` 创建（轻量/附注）、列表、强制更新、删除、ref 指向和 describe 依赖的 tag 可见性。

最小步骤：

```bash
# Short converged form.
SCENARIO="cli.tag-basic"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"

libra init tag-repo
cd tag-repo
libra config set user.name "Libra Tag Test"
libra config set user.email "tag@example.invalid"
printf 'tag base\n' > tag.txt
libra add tag.txt
libra commit -m "test: tag base"
BASE_COMMIT="$(libra rev-parse HEAD)"

libra tag v0.1.0
libra tag -m "release v0.2.0" v0.2.0
libra tag -l
libra tag -l -n 1
libra rev-parse v0.1.0
libra describe --tags --always HEAD
libra --json tag -l >tags.json
python3 -c "import json; d=json.load(open('tags.json')); assert d['ok'] is True; assert isinstance(d['data'].get('tags'), list)"

printf 'tag update\n' >> tag.txt
libra add tag.txt
libra commit -m "test: tag update"
libra tag -f v0.1.0
test "$(libra rev-parse v0.1.0)" != "$BASE_COMMIT"
libra tag -d v0.1.0
! libra rev-parse v0.1.0
```

负向步骤：

```bash
cd "$RUN_DIR/tag-repo"
! libra tag
! libra tag v0.2.0
! libra tag -d no-such-tag
```

断言：轻量 tag 与 annotated tag 均可创建并被 `rev-parse` 解析；`tag -l` / `tag -l -n` 可观察 tag 名称和注释摘要；`describe --tags --always` 能使用可达 tag 描述 HEAD；`tag -f` 可更新现有 tag 指向（新提交 != BASE）；`tag -d` 删除后原名不可解析；缺少 tag 名、重复创建和删除缺失 tag 必须非 0 退出且不影响已有 tag。

补充可执行断言（使用 `libra()` + python）：
- `libra --json tag -l` 必须返回 `ok:true`，且 `data.tags[]` 包含 v0.2.0。
- 负向错误必须包含稳定错误信息或 LBR- 码（通过 stderr 捕获验证）。
- 操作后 `libra fsck --connectivity-only` 必须成功（0 退出）。
- 全局 DB 隔离：本场景操作后，用隔离的全局 DB 执行 `libra config --global list` 不得看到本场景的 user.name（除非显式 --global）。

