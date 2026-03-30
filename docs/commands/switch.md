# `libra switch`

切换分支、创建并切换新分支，或切换到 detached HEAD。

## Human Output

- 切换分支：`Switched to branch 'main'`
- 新建并切换：`Switched to a new branch 'feature'`
- detached：`HEAD is now at abc1234`
- `--track` 会先输出 upstream 建立信息，再输出切换结果

## JSON Output

`--json` / `--machine` 返回：

```json
{
  "ok": true,
  "command": "switch",
  "data": {
    "previous_branch": "main",
    "previous_commit": "abc123...",
    "branch": "feature",
    "commit": "abc123...",
    "created": true,
    "detached": false,
    "tracking": null
  }
}
```

`tracking` 仅在 `--track` 时存在，包含 `remote` 和 `remote_branch`。

## Errors

- 不存在的分支 / revision：`LBR-CLI-003`
- 工作区不干净：`LBR-REPO-003`
- 更新 HEAD 或恢复工作区失败：`LBR-IO-002`
