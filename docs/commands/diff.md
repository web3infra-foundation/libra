# `libra diff`

比较 `HEAD`、index、工作区或两个 revision 之间的差异。

## Human Output

支持：

- 默认 unified diff
- `--name-only`
- `--name-status`
- `--numstat`
- `--stat`

`--output <file>` 会把 human 输出写入文件；`--json` 时忽略该标志，始终写 stdout。

## JSON Output

```json
{
  "ok": true,
  "command": "diff",
  "data": {
    "old_ref": "index",
    "new_ref": "working tree",
    "files": [
      {
        "path": "tracked.txt",
        "status": "modified",
        "insertions": 1,
        "deletions": 0,
        "hunks": [
          {
            "old_start": 1,
            "old_lines": 1,
            "new_start": 1,
            "new_lines": 2,
            "lines": [" tracked", "+updated"]
          }
        ]
      }
    ],
    "total_insertions": 1,
    "total_deletions": 0,
    "files_changed": 1
  }
}
```

## Errors

- 无效 revision：`LBR-CLI-003`
- index / object 读取失败：`LBR-REPO-002`
- 文件读取失败：`LBR-IO-001`
- 输出文件写入失败：`LBR-IO-002`
