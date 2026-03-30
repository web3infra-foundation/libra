# `libra tag`

创建、列出或删除 tag。

## Human Output

- `libra tag -l`：打印 tag 列表
- `libra tag -d v1.0`：`Deleted tag 'v1.0'`
- 默认 create 路径沿用现有 human 展示逻辑

## JSON Output

`--json` / `--machine` 使用 `action` 区分操作：

```json
{
  "ok": true,
  "command": "tag",
  "data": {
    "action": "create",
    "name": "v1.0",
    "hash": "abc123...",
    "tag_type": "lightweight",
    "message": null
  }
}
```

`action=list` 时返回 `tags` 数组；`action=delete` 时返回 `name` 和 `hash`。

## Errors

- 重复创建：`LBR-CONFLICT-002`
- tag 不存在：`LBR-CLI-003`
- 删除写入失败：`LBR-IO-002`
