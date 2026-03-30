# `libra show`

显示 commit、tag、tree、blob，或 `REV:path` 指向的 blob。

## Human Output

human 模式沿用现有展示：

- commit：header + 可选 patch / stat / name-only
- annotated tag：tag 元数据后继续展示目标对象
- tree：tree entry 列表
- blob：文本内容或 binary 摘要

## JSON Output

`data.type` 决定 schema：

- `commit`
- `tag`
- `tree`
- `blob`

示例：

```json
{
  "ok": true,
  "command": "show",
  "data": {
    "type": "commit",
    "hash": "abc123...",
    "short_hash": "abc1234",
    "subject": "base",
    "files": [
      { "path": "tracked.txt", "status": "added" }
    ]
  }
}
```

## Errors

- bad revision / path 不存在：`LBR-CLI-003`
- 对象读取失败：`LBR-REPO-002`
