# `libra log`

显示提交历史。human 模式保留现有 `--oneline`、`--graph`、`--pretty`、`--stat`、`--patch` 等表现。

## JSON Output

`--json` / `--machine` 返回过滤后的结构化提交列表：

```json
{
  "ok": true,
  "command": "log",
  "data": {
    "commits": [
      {
        "hash": "abc123...",
        "short_hash": "abc1234",
        "author_name": "Test User",
        "author_email": "test@example.com",
        "author_date": "2026-03-30T10:00:00+08:00",
        "committer_name": "Test User",
        "committer_email": "test@example.com",
        "committer_date": "2026-03-30T10:00:00+08:00",
        "subject": "base",
        "body": "",
        "parents": [],
        "refs": ["HEAD -> main"],
        "files": [
          { "path": "tracked.txt", "status": "added" }
        ]
      }
    ],
    "total": null
  }
}
```

说明：

- `-n` 对 JSON 同样生效
- `--graph`、`--pretty`、`--oneline` 在 JSON 模式下不改变 schema
- `files` 始终是结构化变更摘要，不包含 patch

## Errors

- 空分支 / 空 HEAD：`LBR-REPO-003`
- 无效日期参数：`LBR-CLI-002`
