# Merge 命令改进详细计划

> 最后编写时间：2026-05-15

本文记录第 32 批中 `merge` 的当前落地状态与后续边界。`merge` 与 `rebase` 同批跟踪，但两者复杂度不同：`merge` 已从最初的 fast-forward-only 成功路径扩展到 C7 的 single-head three-way merge 与冲突 lifecycle；`rebase` 仍保持独立状态机。

## 当前落地状态

已落地：

- `execute_safe(args, output)` 通过 `run_merge()` + `render_merge_output()` 拆分执行层和渲染层。
- `run_merge_for_pull()` 继续作为 `pull` 复用的内部 helper，保持 `run_<cmd>_for_<delegator>()` 命名约定。
- `--json` 与 `--machine` 支持 fast-forward、already-up-to-date、clean three-way、`--continue`、`--abort` 成功结果。
- fast-forward、already-up-to-date、remote ref JSON、machine single-line、clean three-way、conflict marker、status hint、abort、continue、dirty refusal、untracked overwrite 均有命令级回归测试。
- diverged 单目标 merge 会执行三方合并：无冲突时创建双父 merge commit；有冲突时写 marker、unmerged index stage 与 Libra merge state，并返回 `LBR-CONFLICT-002`。
- 生产路径不再使用 `unreachable!()` 处理空 HEAD 分支。

当前成功 JSON 形态：

```json
{
  "ok": true,
  "command": "merge",
  "data": {
    "strategy": "three-way",
    "old_commit": "abc1234...",
    "commit": "def5678...",
    "files_changed": 2,
    "up_to_date": false,
    "parents": ["abc1234...", "fedcba9..."]
  }
}
```

`already-up-to-date` 时 `commit` 为 `null`，`files_changed` 为 `0`，`up_to_date` 为 `true`。Fast-forward 仍使用 `strategy: "fast-forward"`，且不会输出 `parents`。

## Schema 兼容边界

`data.files_changed` 当前是数字。虽然跨命令约定中统计字段倾向使用对象形态，但该字段已由 `merge` 的命令文档和测试发布为数字，不能在本批中改类型。

后续如果需要更详细的文件统计，只能 additive 增加新字段，例如 `file_stats`，不能改变 `files_changed` 的字段名、类型或语义。

## 仍不做的事项

- 不支持 octopus merge、recursive/ort 策略选择、自定义 `-X` option。
- 不支持 `--no-ff`、`--squash`、签名校验、自动编辑 merge message。
- 不把 `pull` 实现成第二套 merge state machine；`pull` 必须继续复用 `run_merge_for_pull()`。
- 不把 `merge` 与 `rebase` 抽成共用执行层。
- 不在本文件内定义 rebase 状态机；rebase 作为下一切片独立收口。

## 后续工作

- 如需补全详细统计，新增兼容字段，不修改 `files_changed`。
- 如需支持 octopus/custom strategy/squash，必须另开兼容计划并保持 `COMPATIBILITY.md` 的 `partial` 状态。
- `rebase` 的 run/render/output/error split、typed failure 传递与 mode-preserving replay 已在独立计划中收口；本文件不再把 rebase 作为 merge 后续工作。
