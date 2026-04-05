# Stash 命令改进详细计划

## 所属批次

第四批：暂存与撤销命令（P1 一致性修复）

## 已完成前置条件与当前代码状态

### 当前代码已具备
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架
- 稳定错误码体系 18 个错误码（`StableErrorCode`）
- `CliError` 带 `with_stable_code()` / `with_hint()` 链式 API
- `Stash` 子命令枚举（`cli.rs`）：`Push`、`Pop`、`List`、`Apply`、`Drop`
- 子命令各自有基础功能实现（push/pop/list/apply/drop）
- `update_stash_ref()` reflog 写入、`merge_trees()` 三方合并
- `has_stash()` / `get_stash_num()` 辅助查询

### 当前代码缺失
- **无 `StashError` typed enum**：所有函数返回 `Result<(), String>`
- **无 `StableErrorCode` 映射**：错误通过 `CliError::from_legacy_string()` 兜底
- **无 JSON/machine 输出**：`OutputConfig` 参数被忽略（`_output`）
- **无 `run_stash()` / `render_stash_output()` 分层**：业务逻辑与输出混合
- **无 `StashOutput` 结构化输出类型**
- **无测试文件**：`tests/command/stash_test.rs` 不存在

## 改进内容

### 特性 1：`StashError` typed enum + `StableErrorCode` 映射

**变更范围**：`src/command/stash.rs`

引入 `StashError` 枚举替换所有 `Result<(), String>` 返回值：

| 变体 | 触发条件 | StableErrorCode |
|------|---------|-----------------|
| `NotInRepo` | `require_repo()` 失败 | `RepoNotFound` |
| `NoInitialCommit` | push 时无 HEAD | `RepoStateInvalid` |
| `NoStashFound` | apply/pop/drop 时无 stash | `CliInvalidTarget` |
| `InvalidStashRef(String)` | stash 引用格式错误 | `CliInvalidArguments` |
| `StashNotExist { index }` | 指定索引不存在 | `CliInvalidTarget` |
| `MergeConflict(Vec<String>)` | apply 时冲突 | `ConflictUnresolved` |
| `ReadObject(String)` | 对象读取失败 | `IoReadFailed` |
| `WriteObject(String)` | 对象写入失败 | `IoWriteFailed` |
| `IndexSave(String)` | 索引保存失败 | `IoWriteFailed` |
| `ResetFailed(String)` | hard reset 失败 | `IoWriteFailed` |

### 特性 2：`run_stash()` + `render_stash_output()` 执行/渲染分层

**变更范围**：`src/command/stash.rs`

- `run_stash(cmd, output) -> Result<StashOutput, StashError>`：纯业务逻辑
- `render_stash_output(result, output) -> CliResult<()>`：JSON/human/quiet 渲染
- `execute_safe()` 简化为 `run_stash()` → `render_stash_output()` 管线

### 特性 3：`StashOutput` 结构化输出

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "action")]
pub enum StashOutput {
    Noop { message: String },
    Push { message: String, stash_id: String },
    Pop { index: usize, stash_id: String },
    Apply { index: usize, stash_id: String, branch: String },
    Drop { index: usize, stash_id: String },
    List { entries: Vec<StashListEntry> },
}

#[derive(Debug, Clone, Serialize)]
pub struct StashListEntry {
    pub index: usize,
    pub message: String,
    pub stash_id: String,
}
```

### 特性 4：`--help` EXAMPLES

添加 after_help EXAMPLES 段到 cli.rs 的 Stash 子命令。

### 特性 5：集成测试

新增 `tests/command/stash_test.rs`：
- `test_stash_push_and_pop`
- `test_stash_list`
- `test_stash_apply`
- `test_stash_drop`
- `test_stash_json_output`
- `test_stash_outside_repo_returns_fatal_128`
- `test_stash_no_changes`

## 验证方式

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `cargo test stash_test` 全部通过
4. `libra stash push --json` 输出合法 JSON
5. `libra stash list --json` 输出合法 JSON
