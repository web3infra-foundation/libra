# Merge 命令改进详细计划

> 最后编写时间：2026-05-15

本文记录第 32 批中 `merge` 的当前落地状态与后续边界。`merge` 与 `rebase` 同批跟踪，但两者复杂度不同：`merge` 先以 fast-forward-only 成功路径完成结构化输出与执行/渲染拆分；`rebase` 仍需要单独拆解 legacy 状态机。

## 当前落地状态

已落地：

- `execute_safe(args, output)` 通过 `run_merge()` + `render_merge_output()` 拆分执行层和渲染层。
- `run_merge_for_pull()` 继续作为 `pull` 复用的内部 helper，保持 `run_<cmd>_for_<delegator>()` 命名约定。
- `--json` 与 `--machine` 支持 fast-forward 和 already-up-to-date 成功结果。
- fast-forward、already-up-to-date、remote ref JSON、machine single-line 和 non-fast-forward JSON error envelope 均有命令级回归测试。
- merge-owned CLI 路径把 non-fast-forward 映射为 `LBR-CONFLICT-002`，保持既有 `Not possible to fast-forward merge, try merge manually` 文案。
- 生产路径不再使用 `unreachable!()` 处理空 HEAD 分支。

当前成功 JSON 形态：

```json
{
  "ok": true,
  "command": "merge",
  "data": {
    "strategy": "fast-forward",
    "old_commit": "abc1234...",
    "commit": "def5678...",
    "files_changed": 1,
    "up_to_date": false
  }
}
```

`already-up-to-date` 时 `commit` 为 `null`，`files_changed` 为 `0`，`up_to_date` 为 `true`。

## Schema 兼容边界

`data.files_changed` 当前是数字。虽然跨命令约定中统计字段倾向使用对象形态，但该字段已由 `merge` 的命令文档和测试发布为数字，不能在本批中改类型。

后续如果需要更详细的文件统计，只能 additive 增加新字段，例如 `file_stats`，不能改变 `files_changed` 的字段名、类型或语义。

## 本批不做的事项

- 不实现三方 merge、merge commit、recursive/ort 策略或冲突状态。
- 不支持 `--no-ff`、`--squash`、`--abort`、`--continue`、octopus merge。
- 不改变 `pull` 复用 `run_merge_for_pull()` 的错误语义。
- 不把 `merge` 与 `rebase` 抽成共用执行层。
- 不在本文件内定义 rebase 状态机；rebase 作为下一切片独立收口。

## 后续工作

- 如需支持真正三方 merge，先设计冲突状态、index 写入与 abort/continue 契约，再扩展 `MergeOutput`。
- 如需补全详细统计，新增兼容字段，不修改 `files_changed`。
- `rebase` 仍需拆成 `run_rebase()` / `render_rebase_output()` / `RebaseOutput` / `RebaseError`，并修复 legacy 路径中打印错误但可能不传递失败状态的问题。
