### `cli.tag-basic`

目的：覆盖 `tag` 创建（轻量/`-m` 附注/`-F` 文件消息/`-a -m` 显式附注）、列表与 `-n` 注释摘要、`--points-at` / `--contains` / `--merged` / `--sort` 过滤排序、强制更新、批量删除、ref 指向和 describe 依赖的 tag 可见性。

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
printf 'release v0.3.0\n' > release.txt
libra tag -F release.txt v0.3.0
libra tag -a -m "annotated via -a" v0.6.0
libra tag -l
libra tag -l -n 1
libra tag --points-at HEAD
libra tag --contains HEAD
libra tag --merged HEAD --sort=-refname
libra rev-parse v0.1.0
libra describe --tags --always HEAD
libra --json tag -l >tags.json
python3 -c "import json; d=json.load(open('tags.json')); assert d['ok'] is True; assert isinstance(d['data'].get('tags'), list)"

printf 'tag update\n' >> tag.txt
libra add tag.txt
libra commit -m "test: tag update"
libra tag -f v0.1.0
test "$(libra rev-parse v0.1.0)" != "$BASE_COMMIT"
libra tag -d v0.2.0 v0.3.0
libra --json tag -d v0.1.0 missing-tag >batch-delete.json || test "$?" -eq 128
python3 -c "import json; d=json.load(open('batch-delete.json')); assert d['ok'] is True; assert d['data']['deleted'] == ['v0.1.0']; assert d['data']['failed'][0]['name'] == 'missing-tag'"
! libra rev-parse v0.1.0
```

负向步骤：

```bash
cd "$RUN_DIR/tag-repo"
! libra tag v0.4.0 v0.5.0
! libra tag -d no-such-tag
```

断言：轻量 tag、`-m` annotated tag、`-F` 文件消息 tag 与 `-a -m` annotated tag 均可创建并被 `rev-parse` / list 观察；`tag -l` / `tag -l -n 1` 可观察 tag 名称和注释摘要（输出须包含 `-m` 与 `-a -m` 的消息首行）；`--points-at` / `--contains` / `--merged` / `--sort=-refname` 可执行并包含预期 tag；`describe --tags --always` 能使用可达 tag 描述 HEAD；`tag -f` 可更新现有 tag 指向（新提交 != BASE）；批量 `tag -d` 删除后原名不可解析；JSON 批量删除的部分失败在 stdout 返回 `deleted`/`failed`，进程退出 128；多 name 创建和删除缺失 tag 必须非 0 退出且不影响已有 tag。

补充可执行断言（使用 `libra()` + python）：
- `libra --json tag -l` 必须返回 `ok:true`，且 `data.tags[]` 包含 v0.2.0。
- `libra --json tag -d v0.1.0 missing-tag` 必须返回 `ok:true` 的 batch-delete 数据，且命令退出 128。
- 负向错误必须包含稳定错误信息或 LBR- 码（通过 stderr 捕获验证）。
- 操作后 `libra fsck --connectivity-only` 必须成功（0 退出）。
- 全局 DB 隔离：本场景操作后，用隔离的全局 DB 执行 `libra config --global list` 不得看到本场景的 user.name（除非显式 --global）。
