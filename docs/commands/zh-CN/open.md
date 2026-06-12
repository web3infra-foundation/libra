# `libra open`

将远程 URL 解析为 Web URL，并可选择启动系统浏览器。

## 概要

```
libra open [<remote>]
```

## 说明

`libra open` 确定仓库可在 Web 中浏览的 URL，并在人类输出模式下用默认系统浏览器打开它。该命令接受一个可选位置参数，可以是已配置的远程名称（例如 `origin`），也可以是直接 URL。

未给出参数时，命令按以下顺序尝试：
1. 当前分支配置的 upstream 远程。
2. 名为 `origin` 的远程。
3. 第一个已配置远程（按字母顺序）。

如果解析出的 URL 使用 SSH 或 SCP 语法（`git@host:path` 或 `ssh://...`），它会自动转换为 HTTPS URL。最终 URL 会被验证，确保在传递给 OS 浏览器启动器前使用 `http://` 或 `https://`。这会防止本地文件访问、`javascript:` 或其他注入向量。

在 macOS 上，该命令使用 `open`；在 Linux 上使用 `xdg-open`；在 Windows 上使用 `cmd /C start`。

## 选项

| 标志 / 参数 | 说明 | 示例 |
|-----------------|-------------|---------|
| `<remote>` | 远程名称或直接 URL。省略时，从 tracking 配置或 `origin` 自动检测。 | `libra open origin` |
| `--json` | 向 stdout 输出结构化 JSON 信封，而不是打开浏览器（全局标志）。 | `libra open --json` |
| `--machine` | 紧凑单行 JSON，不启动浏览器（全局标志）。 | `libra open --machine` |
| `--quiet` | 抑制 stdout 上的 "Opening ..." 消息。 | `libra open --quiet` |

## 常用命令

```bash
libra open
libra open origin
libra open https://github.com/web3infra-foundation/libra
libra open --json
```

## 人类可读输出

```text
Opening https://github.com/web3infra-foundation/libra
```

`--quiet` 会抑制 `stdout`。

## 结构化输出（JSON 示例）

```json
{
  "ok": true,
  "command": "open",
  "data": {
    "remote": "origin",
    "remote_url": "git@github.com:web3infra-foundation/libra.git",
    "web_url": "https://github.com/web3infra-foundation/libra",
    "launched": false
  }
}
```

当参数是直接 URL 而不是远程名称时，`remote` 为 `null`：

```json
{
  "ok": true,
  "command": "open",
  "data": {
    "remote": null,
    "remote_url": "https://github.com/web3infra-foundation/libra",
    "web_url": "https://github.com/web3infra-foundation/libra",
    "launched": false
  }
}
```

### Schema 说明

- `remote` 是逻辑远程名称；提供直接 URL 时为 `null`
- `remote_url` 是配置中的原始 URL（或直接 URL 参数）
- `web_url` 是转换后的可浏览 HTTPS URL
- `launched` 在人类模式下成功生成浏览器进程时为 `true`
- 对于 `--json` / `--machine`，`launched` 为 `false`，因为会有意跳过浏览器启动

### URL 转换规则

| 输入格式 | 转换后输出 |
|-------------|-------------------|
| `https://github.com/user/repo.git` | `https://github.com/user/repo` |
| `http://github.com/user/repo.git` | `http://github.com/user/repo` |
| `git@github.com:user/repo.git` (SCP) | `https://github.com/user/repo` |
| `ssh://git@github.com/user/repo.git` | `https://github.com/user/repo` |
| `ssh://user@host.com:2222/repo.git` | `https://host.com/repo` |

## 设计理由

### 为什么支持直接 URL？

`libra open` 的主要用例是快速跳转到仓库的 Web 界面。有时开发者或代理会从聊天消息、issue tracker 或日志输出中拿到 URL，并希望无需先配置远程就打开它。接受直接 URL 和远程名称，使该命令成为通用的“在浏览器中打开这个仓库”工具。如果参数匹配已配置远程名称，则远程名称优先；否则它被视为字面 URL。这种双模式行为消除了常见摩擦点，同时不增加复杂度。

### 为什么不直接使用 `git web--browse`？

`git web--browse` 是一个启动浏览器的 Git 内部 helper，但有若干限制：它不会将 SSH/SCP URL 转换为 HTTPS，不验证 URL 安全性，并且需要配置 `instaweb` 或 `browse` helper。Libra 的 `open` 命令处理完整 URL 转换流水线（SCP 到 HTTPS、SSH 到 HTTPS、去除 `.git` 后缀），并在传递给 OS 启动器前验证最终 URL 使用安全 scheme。因此它无需额外配置即可处理所有常见远程 URL 格式。

### 为什么验证 URL 安全性？

当远程 URL 被转换并传递给 OS 命令（`open`、`xdg-open`、`cmd /C start`）时，如果 URL 使用 `file://`、`javascript:` 之类的 scheme，或包含 shell 元字符，就存在命令注入或非预期文件访问风险。Libra 会在启动浏览器前验证最终 URL 只使用 `http://` 或 `https://`。在 Windows 上，URL 还会被额外加引号，以防止 `cmd.exe` 元字符展开。这种纵深防御同时防护意外误配置和精心构造的远程 URL 攻击。

## 参数对比：Libra vs Git vs jj

| 功能 | Libra | Git | jj |
|---------|-------|-----|----|
| 在浏览器中打开仓库 | `libra open` | `git web--browse`（手动） | N/A |
| 打开指定远程 | `libra open origin` | N/A | N/A |
| 打开直接 URL | `libra open <url>` | N/A | N/A |
| SSH 到 HTTPS 转换 | 自动 | N/A | N/A |
| SCP 到 HTTPS 转换 | 自动 | N/A | N/A |
| URL 安全验证 | 仅 http/https | N/A | N/A |
| 结构化输出 | `--json` / `--machine` | 无 | 无 |
| 自动检测远程 | Tracking -> origin -> 第一个 | N/A | N/A |

## 错误处理

| 场景 | StableErrorCode | 退出码 | 提示 |
|----------|-----------------|------|------|
| 不在仓库中且没有显式 URL | `LBR-REPO-001` | 128 | "run this command inside a libra repository, or pass a URL" |
| 没有配置远程 | `LBR-REPO-003` | 128 | "add a remote first: 'libra remote add origin \<url>'" |
| 已配置远程但没有 URL | `LBR-REPO-003` | 128 | "configure the URL: 'libra config set remote.\<name>.url \<url>'" |
| 解析出的 URL 不安全或无效 | `LBR-CLI-003` | 129 | "pass an explicit https:// URL or configure a supported remote URL" |
| 无法读取远程配置 | `LBR-IO-001` | 128 | -- |
| 无法启动浏览器 | `LBR-IO-002` | 128 | "check that a default browser is configured" |
