# `libra ls-remote`

列出远程仓库通告的引用，不下载对象，也不更新本地引用。

```bash
libra ls-remote [OPTIONS] <repository> [patterns...]
```

在 Libra 仓库内运行时，`<repository>` 可以是已配置的远程名称，也可以是 URL，或本地 Git/Libra 仓库路径。

## 选项

| 标志 | 说明 | 示例 |
|------|-------------|---------|
| `--heads` | 只显示 `refs/heads/*` 分支引用 | `libra ls-remote --heads origin` |
| `-t`, `--tags` | 只显示 `refs/tags/*` 标签引用 | `libra ls-remote --tags origin` |
| `--refs` | 省略 `HEAD` 和以 `^{}` 结尾的 peeled 标签引用 | `libra ls-remote --refs origin` |
| `--symref` | 在解析后的引用行前打印 `HEAD` 等符号引用的目标 | `libra ls-remote --symref origin` |
| `--get-url` | 解析并打印已配置 URL，不联系远端 | `libra ls-remote --get-url origin` |
| `--exit-code` | discovery 成功但没有匹配引用时以状态码 2 退出 | `libra ls-remote --exit-code origin main` |
| `--sort <KEY>` | 按 `refname`、`-refname`、`version:refname` 或 `-version:refname` 排序 | `libra ls-remote --sort=version:refname --tags origin` |
| `patterns...` | 匹配完整引用名或尾部路径组件；`*` 和 `?` 遵循 Git 风格 glob 行为，并且可以匹配 `/` | `libra ls-remote origin main 'refs/heads/*'` |

## 人类可读输出

每个匹配引用按如下格式打印：

```text
<object-id>	<refname>
```

使用 `--symref` 时，符号引用会打印在解析后的引用行之前：

```text
ref: refs/heads/main	HEAD
<object-id>	HEAD
```

示例：

```text
4f3c2d1a...	HEAD
4f3c2d1a...	refs/heads/main
```

## JSON 输出

使用 `--json` 时，输出使用标准命令信封：

```json
{
  "ok": true,
  "command": "ls-remote",
  "data": {
    "remote": "origin",
    "url": "https://example.com/repo.git",
    "heads_only": false,
    "tags_only": false,
    "refs_only": false,
    "symref": false,
    "get_url": false,
    "exit_code": false,
    "sort": null,
    "patterns": [],
    "entries": [
      {
        "hash": "4f3c2d1a...",
        "refname": "refs/heads/main"
      }
    ]
  }
}
```

## 示例

```bash
# 列出具名远程的所有引用
libra ls-remote origin

# 直接列出 URL 的所有引用（不需要注册远程）
libra ls-remote https://example.com/repo.git

# 限制为匹配模式的分支
libra ls-remote --heads origin main

# 解析配置的远端 URL，不做 discovery
libra ls-remote --get-url origin

# 显示远端 HEAD 的符号引用目标
libra ls-remote --symref origin

# 按版本感知 refname 顺序排序标签
libra ls-remote --sort=version:refname --tags origin

# 面向代理的结构化 JSON 信封，仅标签
libra --json ls-remote --tags origin
```

`libra ls-remote --help` 会渲染同一横幅，因此文档和 CLI 表面保持同步（跨命令 `--help` EXAMPLES 推出，见 `docs/development/commands/_general.md` 条目 B）。

## 说明

- `ls-remote` 只执行协议发现（对本地 Git 仓库等价于 `git-upload-pack --advertise-refs`）。
- 它不会写入对象、远程跟踪引用、配置或工作树文件。
- `--heads` 和 `--tags` 可以组合使用，以同时显示分支和标签引用，同时排除 `HEAD`。
- `--symref` 会在可从远端广告或 HEAD 对象 id 推断时报告 `HEAD` 的符号引用目标。
- `--get-url` 在协议 discovery 之前退出，并打印与 remote 诊断一致的脱敏 URL。
- `--exit-code` 是脚本信号：无匹配引用时返回状态码 2，不渲染错误。
