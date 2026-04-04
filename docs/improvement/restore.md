# Restore 命令���进详细计划

## 所属批次

第四批：暂存与撤���命令（P1 ��致性修复）

## 已完成前置���件与当前代码状态

### 当前代码已���备
- `RestoreError` typed enum（7 变体）：`ResolveSource`、`ReferenceNotCommit`、`ReadIndex`、`ReadObject`、`InvalidPathEncoding`、`WriteWorktree`、`LfsDownload`
- `execute_checked()` + `execute_checked_typed()` 双路径实现
- pathspec 过滤、LFS 支持、worktree/staged 目标
- `checkout` 命令通过 `execute_checked_typed()` 复用 restore 逻辑

### 当前代码缺失
- **无 `StableErrorCode` 映射**：`RestoreError` 通过 `CliError::fatal(e.to_string())` 兜底
- **无 JSON/machine 输出**：`OutputConfig` 参数被忽略（`_output`）
- **无确认消息**：操作完全静默
- **无 `run_restore()` / `render_restore_output()` 分层**
- **无 `RestoreOutput` 结构化输出类型**
- **双路径代码重复**：`execute_checked()` 和 `execute_checked_typed()` 逻辑相似
- **测试仅覆盖错误路径**：无正向 restore 操作测试

## 改进内容

### 特性 1：`RestoreError` → `StableErrorCode` 映射 + `From<RestoreError> for CliError`

**变更范围**：`src/command/restore.rs`

| RestoreError 变体 | StableErrorCode | hint |
|-------------------|-----------------|------|
| `ResolveSource` | `CliInvalidTarget` | "check that the source ref exists" |
| `ReferenceNotCommit` | `CliInvalidTarget` | "only commit references can be used as restore source" |
| `ReadIndex` | `IoReadFailed` | — |
| `ReadObject` | `IoReadFailed` | — |
| `InvalidPathEncoding` | `CliInvalidArguments` | — |
| `WriteWorktree` | `IoWriteFailed` | — |
| `LfsDownload` | `NetworkUnavailable` | "check LFS server availability" |

### 特性 2：`run_restore()` + `render_restore_output()` 分层

- `run_restore(args) -> Result<RestoreOutput, RestoreError>`：纯业务逻辑
- `render_restore_output(result, output) -> CliResult<()>`：JSON/human/quiet 渲染
- 消除 `execute_checked()` / `execute_checked_typed()` 重复

### 特性 3：`RestoreOutput` 结构化输出 + 确认消息

```rust
#[derive(Debug, Clone, Serialize)]
pub struct RestoreOutput {
    pub source: Option<String>,
    pub worktree: bool,
    pub staged: bool,
    pub restored_files: Vec<String>,
    pub deleted_files: Vec<String>,
}
```

human 模式输出确认消息：`"Updated N paths from {source}"`

### 特性 4：`--help` EXAMPLES

### 特性 5：补充测试

新增正向 restore 测试：
- `test_restore_worktree_from_index`
- `test_restore_staged_from_head`
- `test_restore_json_output`
- `test_restore_confirm_message`

## 验证方式

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警��
3. `cargo test restore_test` 全部通过
4. `libra restore --json --source HEAD file.txt` 输出合法 JSON
