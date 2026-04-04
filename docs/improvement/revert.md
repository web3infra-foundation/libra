# Revert 命令改���详细计划

## 所属批次

第四批：暂存与撤销命令（P1 一致性修复）

## 已完��前置条件与当前代��状态

### 当前��码已具备
- 基本 revert 逻辑：三方合并、反向补丁、root commit 处理
- `--no-commit` 标志
- 确认消息（"Revert commit ..."）
- detached HEAD 检测（拒绝）
- 测试覆盖：`test_basic_revert`、`test_revert_no_commit`、`test_revert_root_commit`、`test_revert_errors`

### 当前代码缺失
- **无 `RevertError` typed enum**：全部 `Result<_, String>` 返回
- **无 `StableErrorCode` 映射**
- **无 JSON/machine 输出**：`OutputConfig` 参数被忽略
- **无 `run_revert()` / `render_revert_output()` 分层**
- **无 `RevertOutput` 结构化输出类型**
- **无 `--help` EXAMPLES**

## 改进内容

### ��性 1：`RevertError` typed enum + `StableErrorCode` 映射

| 变体 | 触发条件 | StableErrorCode |
|------|---------|-----------------|
| `NotInRepo` | require_repo 失败 | `RepoNotFound` |
| `DetachedHead` | detached HEAD 状态 | `RepoStateInvalid` |
| `InvalidCommit(String)` | 无法解析提交引用 | `CliInvalidTarget` |
| `MergeCommitUnsupported` | 尝试 revert merge commit | `CliInvalidArguments` |
| `Conflict { path }` | revert 冲突 | `ConflictUnresolved` |
| `LoadObject(String)` | 对象读取失败 | `IoReadFailed` |
| `SaveObject(String)` | 对象写入失败 | `IoWriteFailed` |
| `IndexSave(String)` | 索引保存失败 | `IoWriteFailed` |
| `UpdateHead(String)` | HEAD 更新失败 | `IoWriteFailed` |

### 特性 2：`run_revert()` + `render_revert_output()` 分层

- `run_revert(args) -> Result<RevertOutput, RevertError>`
- `render_revert_output(result, output) -> CliResult<()>`

### 特性 3：`RevertOutput` 结构化输出

```rust
#[derive(Debug, Clone, Serialize)]
pub struct RevertOutput {
    pub reverted_commit: String,
    pub new_commit: Option<String>,
    pub no_commit: bool,
    pub files_changed: usize,
}
```

### 特性 4：`--help` EXAMPLES

### 特性 5：JSON 输出测试

- `test_revert_json_output`
- `test_revert_cli_error_codes`

## 验证方式

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `cargo test revert_test` 全部通过
4. `libra revert --json HEAD` 输出合法 JSON
