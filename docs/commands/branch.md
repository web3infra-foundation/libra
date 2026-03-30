# `libra branch`

创建、删除、重命名、查看或列出分支。

## Human Output

- list：打印分支列表
- safe delete：`Deleted branch feature (was abc123...)`
- rename：`Renamed branch 'old' to 'new'`
- `--show-current`：打印当前分支名，detached 时打印 `HEAD detached at <hash>`

## JSON Output

`--json` / `--machine` 使用 `action` 区分：

```json
{
  "ok": true,
  "command": "branch",
  "data": {
    "action": "create",
    "name": "feature",
    "commit": "abc123..."
  }
}
```

支持的 action：

- `list`: `branches`
- `create`: `name`, `commit`
- `delete`: `name`, `commit`, `force`
- `rename`: `old_name`, `new_name`
- `set-upstream`: `branch`, `upstream`
- `show-current`: `name`, `detached`, `commit`

## Errors

- 无效起点 / 不存在分支：`LBR-CLI-003`
- 当前分支不可删 / detached HEAD：`LBR-REPO-003`
- locked / already exists：`LBR-CONFLICT-002`
- 写 refs 失败：`LBR-IO-002`
