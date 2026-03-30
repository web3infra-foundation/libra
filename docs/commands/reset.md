# `libra reset`

移动 `HEAD`，并根据模式重置 index 或工作区。

## Human Output

- 全量 reset：`HEAD is now at abc1234 <subject>`
- pathspec reset：

```text
Unstaged changes after reset:
M	path/to/file
```

## JSON Output

```json
{
  "ok": true,
  "command": "reset",
  "data": {
    "mode": "hard",
    "commit": "abc123...",
    "short_commit": "abc1234",
    "subject": "base",
    "previous_commit": "def456...",
    "files_unstaged": 0,
    "files_restored": 1,
    "pathspecs": []
  }
}
```

`pathspecs` 非空时表示本次仅对指定路径执行 reset。

## Errors

- 无效 revision：`LBR-CLI-003`
- `--soft` 与 pathspec 组合：`LBR-CLI-002`
- index / object store 损坏：`LBR-REPO-002`
- 写入 index / 工作区失败：`LBR-IO-002`
