# Cherry-Pick 命令改���详细计划

## ��属批次

第四批：���存与撤销命令（P1 一致性修复）

## 已完成前置条件与当前代码状态

### 当前代码已具备
- 三方合并 cherry-pick 逻辑
- `--no-commit` 标志
- 多 commit cherry-pick
- reflog 集成（`with_reflog`）
- 确认消息（"Cherry-picking commit ...", "Finished cherry-pick ..."）
- detached HEAD 检测
- 测试覆盖：basic、with-commit、multiple-commits、errors、sha256-handling

### 当前代码缺失
- **无 `CherryPickError` typed enum**：全部 `Result<_, String>` 返回
- **无 `StableErrorCode` 映射**
- **无 JSON/machine 输出**：`OutputConfig` 参数被忽略
- **无 `run_cherry_pick()` / `render_cherry_pick_output()` 分层**
- **无 `CherryPickOutput` 结构化输出类型**
- **无 `--help` EXAMPLES**

## 改进内容

### 特性 1：`CherryPickError` typed enum + `StableErrorCode` 映射

| 变体 | 触发条件 | StableErrorCode |
|------|---------|-----------------|
| `NotInRepo` | require_repo 失败 | `RepoNotFound` |
| `DetachedHead` | detached HEAD 状态 | `RepoStateInvalid` |
| `InvalidCommit(String)` | 无法解析提交引用 | `CliInvalidTarget` |
| `MergeCommitUnsupported` | cherry-pick merge commit | `CliInvalidArguments` |
| `MultipleWithNoCommit` | 多 commit + --no-commit | `CliInvalidArguments` |
| `Conflict(String)` | cherry-pick 冲突 | `ConflictUnresolved` |
| `LoadObject(String)` | 对象读取失败 | `IoReadFailed` |
| `SaveFailed(String)` | 对象/索引保存失败 | `IoWriteFailed` |
| `UpdateHead(String)` | HEAD 更新失败 | `IoWriteFailed` |

### 特��� 2：`run_cherry_pick()` + `render_cherry_pick_output()` 分层

- `run_cherry_pick(args) -> Result<CherryPickOutput, CherryPickError>`
- `render_cherry_pick_output(result, output) -> CliResult<()>`

### 特性 3：`CherryPickOutput` 结构化���出

```rust
#[derive(Debug, Clone, Serialize)]
pub struct CherryPickOutput {
    pub picked: Vec<CherryPickEntry>,
    pub no_commit: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CherryPickEntry {
    pub source_commit: String,
    pub new_commit: Option<String>,
}
```

### 特性 4：`--help` EXAMPLES

### 特性 5：JSON 输出测试

- `test_cherry_pick_json_output`
- `test_cherry_pick_cli_error_codes`

## 验证方式

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `cargo test cherry_pick_test` 全部通过
4. `libra cherry-pick --json <commit>` 输出合法 JSON
