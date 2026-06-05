### `cli.grep-blame-describe-shortlog`

目的：覆盖 history inspection 剩余命令：`grep`、`blame`、`describe`、`shortlog` 的常用参数和失败路径。

最小步骤：

```bash
SCENARIO="cli.grep-blame-describe-shortlog"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
# (prelude provides libra() -- converged short form, long wrapper removed)

libra init inspect-repo
cd inspect-repo
libra config set user.name "Libra Inspect Test"
libra config set user.email "inspect@example.invalid"
mkdir -p docs src
printf 'Alpha\nBeta\n' > docs/guide.txt
printf 'fn main() { println!("alpha"); }\n' > src/main.rs
libra add docs/guide.txt src/main.rs
libra commit -m "feat: add inspect files"
libra tag -m "inspect release" v1.0.0
printf 'Gamma\n' >> docs/guide.txt
libra add docs/guide.txt
libra commit -m "fix: update guide"

libra grep Alpha docs
libra grep -F 'println!("alpha")' src
libra grep -i gamma docs/guide.txt
libra grep -n -e Alpha -e Gamma docs/guide.txt
libra grep -c Alpha docs/guide.txt
libra grep -l alpha src
libra grep --tree HEAD~1 Alpha docs/guide.txt
printf 'Gamma\n' > patterns.txt
libra grep -f patterns.txt docs/guide.txt
libra blame docs/guide.txt
libra blame -L 1,2 docs/guide.txt HEAD
libra describe --tags HEAD
libra describe --always --abbrev 12 HEAD
libra shortlog
libra shortlog -s
libra shortlog -n

# Verify JSON outputs for AI Agent readability
libra --json grep Alpha docs >grep.json
python3 -c "import json; d=json.load(open('grep.json')); assert d['ok'] is True; assert 'matches' in d['data'] or isinstance(d['data'].get('matches'), list)"
libra --json blame docs/guide.txt >blame.json
python3 -c "import json; d=json.load(open('blame.json')); assert d['ok'] is True; assert 'lines' in d['data'] or isinstance(d['data'].get('lines'), list)"
libra --json describe --tags HEAD >describe.json
python3 -c "import json; d=json.load(open('describe.json')); assert d['ok'] is True; assert 'resolved_commit' in d['data'] or 'result' in d['data']"
libra --json shortlog >shortlog.json
python3 -c "import json; d=json.load(open('shortlog.json')); assert d['ok'] is True; assert 'authors' in d['data'] or isinstance(d['data'].get('authors'), list)"
```

负向步骤：

```bash
cd "$RUN_DIR/inspect-repo"
! libra grep no-such-pattern docs/guide.txt
! libra grep --tree no-such-revision Alpha docs/guide.txt
! libra blame -L bad docs/guide.txt
! libra blame missing.txt
! libra describe no-such-revision
```

断言：`grep` 可在工作区、指定 pathspec、pattern file 和历史 tree 中匹配内容，`-F` / `-i` / `-n` / `-c` / `-l` 输出可用于脚本断言；`blame` 输出每行作者和提交信息，`-L` 限制行范围；`describe --tags` 使用可达 tag，`--always --abbrev` 在需要时输出短 hash；`shortlog` 默认、summary 和排序模式都能按作者汇总；无匹配 grep、非法 revision、非法 blame 范围、缺失文件必须失败且不改变仓库。

补充可执行断言：
- `libra --json grep Alpha docs` 必须 `ok:true` 且 `data.matches[]` 可解析。
- `libra --json blame -L 1,1 docs/guide.txt` 验证结构包含 author / commit 信息。
- `libra --json describe --tags` 成功且包含 tag 信息。
- `libra --json shortlog` 返回按作者汇总的结构。
- 负向 `libra grep` 无匹配 或 `libra blame` 非法范围必须非 0，stderr 包含可识别错误（可选 LBR-）。

