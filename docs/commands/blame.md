# `libra blame`

按行追溯文件内容的最后引入提交。

## Human Output

human 模式保持：

```text
abc12345 (Author Name     2026-03-30 10:00:00 +0800 1) line content
```

`-L` 支持：

- `10`
- `10,20`
- `10,+5`

## JSON Output

```json
{
  "ok": true,
  "command": "blame",
  "data": {
    "file": "tracked.txt",
    "revision": "abc123...",
    "lines": [
      {
        "line_number": 1,
        "short_hash": "abc12345",
        "hash": "abc123...",
        "author": "Test User",
        "date": "1711766400",
        "content": "tracked"
      }
    ]
  }
}
```

## Errors

- 无效 revision / 文件不存在：`LBR-CLI-003`
- 无效 `-L` 范围：`LBR-CLI-002`
- commit/object 读取失败：`LBR-REPO-002`
