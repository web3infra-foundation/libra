### `cli.open-smoke`

目的：覆盖 `open` 命令的最小可观察行为，但避免默认 Wave 在 CI/headless 环境中真的打开浏览器或系统应用。

最小步骤：

```bash
SCENARIO="cli.open-smoke"
RUN_DIR="$RUN_ROOT/repos/$SCENARIO"
mkdir -p "$RUN_DIR"
cd "$RUN_DIR"
# Short converged (prelude)
libra init open-repo
cd open-repo
libra remote add origin git@github.com:example/open-repo.git
libra --json open >open-default.json
libra --json open origin >open-origin.json
python3 -c "import json; d=json.load(open('open-default.json')); assert d['ok'] is True; assert d['data']['launched'] is False; assert 'web_url' in d['data']"
python3 -c "import json; d=json.load(open('open-origin.json')); assert d['ok'] is True; assert d['data']['launched'] is False; assert 'web_url' in d['data']"
```

负向步骤：

```bash
cd "$RUN_DIR/open-repo"
! libra --json open no-such-remote
```

断言：全局 `--json` 模式输出包含 `remote`、`remote_url`、`web_url` 和 `launched=false`，不启动外部程序；指定 remote 可解析托管页面 URL；缺失 remote 或不安全 URL 必须失败。默认 Wave 严禁运行会真实启动浏览器/系统应用的裸 `libra open`。

补充可执行断言：
- 已有 JSON 断言保持；额外验证 `libra --json open no-such-remote` 的错误 envelope 包含 `ok:false` + LBR- 码或 "no such remote"。
- 验证即使 remote URL 非法，`launched=false` 且无副作用（无浏览器进程）。
- 操作后 `libra fsck` 通过。

