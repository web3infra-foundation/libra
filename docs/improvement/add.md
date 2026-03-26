## Add 命令改进详细计划

> 最后编写时间：2026-03-26

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#第七批全局层面改进贯穿所有命令)。

### 已完成前置条件与当前代码状态

`config`、`init`、`clone` 的主改造已在当前代码库落地。`add` 改进建立在这些现状之上，同时依赖 `status` 命令的核心差异计算逻辑（`changes_to_be_staged_split_safe()` 等）。

**已确认落地的基线：**

- `config_kv` 后端已落地；`add` 不涉及 config 读写，无需迁移
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架已可用
- `StableErrorCode` 体系已有 16 个错误码
- `CliError` 支持 `.with_hint()`、`.with_stable_code()`、`.with_exit_code()`
- `execute_safe(args, _output)` 入口已存在（`add.rs:121`），接受 `OutputConfig` 但当前未使用
- `AddError` 枚举已定义 10 个变体（`add.rs:71-93`），覆盖主要错误场景
- `AddError → CliError` 映射（`add.rs:95-104`）目前仅对 `PathspecNotMatched` 提供 hint，其余全部落入通用 `CliError::fatal()`
- `--dry-run` 已实现（`add.rs:63`），但输出格式为裸 `println!("add: {}")`，无结构化支持
- `--verbose` 已实现（`add.rs:55`），在 add/refresh 各阶段输出裸 `println!()`
- `--force` 已实现（`add.rs:59`），允许暂存被 `.libraignore` 忽略的文件
- `--refresh` 已实现（`add.rs:51`），仅更新索引元数据不暂存新文件
- `finish_ignored()` 在忽略文件被拦截时返回 `CliError::failure()` + hint

**基于当前代码的 Review 结论（add 仍需改进的部分）：**

- **无 JSON / machine 输出**：`execute_safe()` 接受 `OutputConfig` 参数但完全未使用（变量名为 `_output`）；成功路径无任何输出，`--dry-run` 和 `--verbose` 使用裸 `println!()`
- **成功时沉默**：审计报告核心发现"成功时沉默"在 `add` 上完全成立——暂存 50 个文件后不输出任何确认信息
- **无 `StableErrorCode`**：`AddError → CliError` 映射全部使用通用 `fatal()`，没有显式错误码
- **`--dry-run` 输出不可机读**：`println!("add: {}")` 无法被 Agent 解析
- **warning 语义与全局输出框架冲突**：`finish_ignored()` 当前直接返回 `CliError::failure()`；即使改成 success warning，也必须接入共享 warning tracker（`record_warning()` / `emit_warning()`），而不是让 `add` 私有地产生 `LBR-WARN-001`
- **`--verbose` 使用裸 `println!()`**：不经过 `OutputConfig`，在 `--json` / `--quiet` 模式下会泄漏到 stdout
- **`--ignore-errors` 的结构化结果不完整**：当前计划只有成功文件列表，没有字段承载“哪些路径失败且被继续跳过”，无法让 Agent 精确判断部分成功

### 目标与非目标

**本批目标：**
- 消除 `add` 成功路径的沉默，human 模式下输出变更文件摘要
- 为 `add` 补齐结构化输出（`--json` / `--machine`），返回被暂存的文件列表
- 将 `AddError → CliError` 映射改为显式 `StableErrorCode`
- `--dry-run` 输出可被 Agent 消费（JSON 模式下返回结构化预览）
- `--verbose` 输出经过 `OutputConfig` 管控，不污染 JSON 流
- 被忽略文件和 `--ignore-errors` 的部分失败按 warning 处理，并接入共享 `--exit-code-on-warning` 语义

**本批非目标：**
- **不改变 add 的核心暂存逻辑**。`add_a_file()`、`validate_pathspecs()`、`filter_candidates()` 等函数的行为不变
- **不引入交互式暂存**（`git add -p` / `--interactive`）。这是独立特性，不在本批范围
- **不改变 LFS 处理逻辑**。`gen_blob_from_file()` 保持现有 LFS pointer 生成行为
- **不在 JSON 中暴露 blob hash**。blob hash 是内部实现细节，Agent 不需要

### 设计原则

1. **执行层与渲染层拆分**：`execute_safe()` 调用纯执行入口收集结果，再根据 `OutputConfig` 渲染 human / JSON / machine
2. **成功时必须确认**：human 模式下至少输出一行摘要告知用户暂存了什么
3. **`--verbose` 和 `--dry-run` 的输出受 `OutputConfig` 管控**：`--json` 下不产生 human 格式的逐文件输出
4. **`--quiet` 仅抑制标准 stdout**：与全局语义一致；warning / error 仍可写 stderr
5. **错误码显式映射**：每个 `AddError` 变体都有确定的 `StableErrorCode`
6. **被忽略文件和部分失败是 warning，不是 fatal**：默认 exit `0`；仅当用户显式传入 `--exit-code-on-warning` 时才由全局 CLI 层转换为 exit `9`
7. **路径显示基准与 `status` 保持一致**：`AddOutput` 中的文件路径沿用当前 `status --json` 现有契约，使用"相对于当前工作目录"的显示路径，而不是仓库根相对路径
   > **注意**：这与 `init` / `clone` 的 `path` 字段（绝对路径）不同。`add` 和 `status` 面向工作树内的文件操作，相对路径对 Agent 更友好；`init` / `clone` 面向仓库位置，绝对路径更明确。

### 特性 1：执行层与渲染层拆分

**当前问题：** `execute_safe()` 直接执行暂存逻辑并在内部使用裸 `println!()` 输出 `--verbose` / `--dry-run` 信息。成功路径不返回任何结构化结果。`execute()` 是一个 fire-and-forget 包装。

**修正后的方案：**

- 新增纯执行入口 `run_add(args) -> Result<AddOutput, AddError>`，不做任何输出
- `execute_safe()` 调用 `run_add()` 后根据 `OutputConfig` 渲染
- `--verbose` 的逐文件输出改为收集到 `AddOutput` 中，由渲染层决定是否/如何显示
- `--dry-run` 的预览结果同样收集到 `AddOutput`
- ignored / partial-failure warning 由执行层收集到 `AddOutput`，渲染层再决定 human 提示和是否记录 warning

**`AddOutput` 结构：**

```rust
#[derive(Debug, Clone, Serialize)]
pub struct AddFailure {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddOutput {
    /// 实际暂存（或 dry-run 预览）的文件，按操作类型分组
    pub added: Vec<String>,       // 新文件
    pub modified: Vec<String>,    // 修改的文件
    pub removed: Vec<String>,     // 删除的文件（tracked file 不存在于工作树）
    pub refreshed: Vec<String>,   // --refresh 模式下刷新元数据的文件
    /// 被 .libraignore 忽略的路径（仅当 pathspec 匹配到忽略文件时填充）
    pub ignored: Vec<String>,
    /// `--ignore-errors` 下被跳过的失败路径；空数组表示无部分失败
    pub failed: Vec<AddFailure>,
    /// 是否为 dry-run 模式
    pub dry_run: bool,
}
```

> **路径基准约束：** `added` / `modified` / `removed` / `refreshed` / `ignored` / `failed[*].path` 全部使用“相对于当前工作目录”的显示路径，与 `status --json` 的现有路径语义保持一致；本批**不**切换到仓库根相对路径，避免形成新的跨命令不一致或对现有 `status` JSON consumer 的心智偏差。

**渲染规则：**

| 模式 | stdout | stderr |
|------|--------|--------|
| human（默认） | 摘要行（如 `add 3 files (2 new, 1 modified)`） | `ignored` / `failed` 的 warning block（如果存在） |
| human + `--verbose` | 逐文件列表 + 摘要行 | `ignored` / `failed` 的 warning block（如果存在） |
| human + `--dry-run` | 逐文件列表（前缀 `add:` / `remove:` / `refresh:`） | `ignored` 的 warning block（如果存在） |
| `--quiet` | 无 | 仅保留 `ignored` / `failed` warning；无 warning 时完全静默 |
| `--json` / `--machine` | JSON envelope | 无额外 human warning 文本；但 `ignored` / `failed` 仍会记录到 warning tracker，供 `--exit-code-on-warning` 使用 |
| `--json` + `--dry-run` | JSON envelope（`dry_run: true`） | 同上 |

**human 模式摘要格式：**

```text
# 单文件
add 'src/main.rs' (new file)

# 多文件
add 3 files (2 new, 1 modified)

# --verbose 模式（摘要前逐行列出）
add(new): src/main.rs
add(modified): src/lib.rs
add(new): README.md
add 3 files (2 new, 1 modified)

# --dry-run 模式
add: src/main.rs
add: src/lib.rs
remove: old_file.rs
(dry run, no files were staged)

# --refresh 模式
refreshed 2 files

# 无变更
nothing to add
```

**`--verbose` 在 `--json` 模式下的行为：** `--verbose` 不影响 JSON 输出——JSON 始终包含完整文件列表（等价于 verbose），因此 `--verbose` 标志在 JSON 模式下是 no-op。

**warning-only 场景：**

- 当所有请求路径都被忽略，或 `--ignore-errors` 下所有候选文件都失败时，human 模式**不额外输出成功摘要**，只在 stderr 输出 warning block；JSON / machine 仍返回 `ok: true` 的结构化结果，便于 Agent 判断“命令完成，但没有任何路径成功加入索引”

### 特性 2：JSON 输出设计

**成功输出结构：**

```json
{
  "ok": true,
  "command": "add",
  "data": {
    "added": ["src/new_file.rs"],
    "modified": ["src/main.rs", "src/lib.rs"],
    "removed": [],
    "refreshed": [],
    "ignored": [],
    "failed": [],
    "dry_run": false
  }
}
```

**`--dry-run --json`：**

```json
{
  "ok": true,
  "command": "add",
  "data": {
    "added": ["src/new_file.rs"],
    "modified": ["src/main.rs"],
    "removed": ["old_file.rs"],
    "refreshed": [],
    "ignored": [],
    "failed": [],
    "dry_run": true
  }
}
```

**`--refresh --json`：**

```json
{
  "ok": true,
  "command": "add",
  "data": {
    "added": [],
    "modified": [],
    "removed": [],
    "refreshed": ["src/main.rs", "src/lib.rs"],
    "ignored": [],
    "failed": [],
    "dry_run": false
  }
}
```

**忽略文件 warning（pathspec 匹配到被忽略文件）：**

```json
{
  "ok": true,
  "command": "add",
  "data": {
    "added": ["src/main.rs"],
    "modified": [],
    "removed": [],
    "refreshed": [],
    "ignored": ["build/output.o", "build/cache.bin"],
    "failed": [],
    "dry_run": false
  }
}
```

**`--ignore-errors --json`（部分成功 + 部分失败）：**

```json
{
  "ok": true,
  "command": "add",
  "data": {
    "added": ["src/main.rs"],
    "modified": [],
    "removed": [],
    "refreshed": [],
    "ignored": [],
    "failed": [
      {
        "path": "broken.txt",
        "message": "failed to create index entry for 'broken.txt': <detail>"
      }
    ],
    "dry_run": false
  }
}
```

> **注**：`ignored` 或 `failed` 非空时 JSON `ok` 仍为 `true`。这两类都是 warning 而非 fatal。human 模式下 warning 只写 stderr；JSON / machine 模式不额外输出 human warning 文本，但必须记录 warning 状态，以便顶层 CLI 在 `--exit-code-on-warning` 下返回 exit `9`。

**无变更场景：**

```json
{
  "ok": true,
  "command": "add",
  "data": {
    "added": [],
    "modified": [],
    "removed": [],
    "refreshed": [],
    "ignored": [],
    "failed": [],
    "dry_run": false
  }
}
```

**明确不纳入 JSON 契约的字段：**

- `blob_hash` / `object_id` — 内部实现细节
- `file_size` / `mode` — 可通过 `libra status --json` 获取
- `lfs_tracked` — 可通过 `libra lfs ls-files` 获取

### 特性 3：错误处理与 StableErrorCode

**`AddError → CliError` 显式映射：**

| AddError 变体 | StableErrorCode | 退出码 | hint |
|--------------|-----------------|--------|------|
| `NotInRepo` | `RepoNotFound` | 128 | `run 'libra init' to create a repository` |
| `PathspecNotMatched` | `CliInvalidTarget` | 129 | `check the path and try again` + `use 'libra status' to inspect tracked and untracked files` |
| `PathOutsideRepo` | `CliInvalidTarget` | 129 | `all paths must be within the repository working tree` |
| `IndexLoad` | `RepoCorrupt` | 128 | `the index file may be corrupted; try 'libra status' to verify` |
| `IndexSave` | `IoWriteFailed` | 128 | 无 |
| `RefreshFailed` | `IoReadFailed` | 128 | 无 |
| `CreateIndexEntry` | `IoWriteFailed` | 128 | 无 |
| `InvalidPathEncoding` | `CliInvalidTarget` | 129 | `path contains non-UTF-8 characters` |
| `Workdir` | `RepoNotFound` / `IoReadFailed` | 128 | 根据 `ErrorKind::NotFound` 区分 |
| `Status` | `RepoCorrupt` | 128 | `failed to compute working tree status` |

**被忽略文件的 warning 处理：**

当前 `finish_ignored()` 返回 `CliError::failure()`，退出码为 `1`。改为：

- 如果有文件被成功暂存 + 部分 pathspec 命中忽略文件：**不**返回错误，将忽略信息放入 `AddOutput.ignored`，human 模式在 stderr 输出 warning
- 如果所有 pathspec 都命中忽略文件且无文件被暂存：仍返回 `Ok(AddOutput)`，仅包含 `ignored`；human 模式只输出 warning block；JSON / machine 返回 `ok: true`
- warning 不直接通过 `CliError::WarningEmitted` 返回。统一由渲染层在存在 `ignored` / `failed` 时调用共享 warning 记录逻辑，让顶层 CLI 按 `--exit-code-on-warning` 决定是否转成 exit `9`

**`--ignore-errors` 的 warning 处理：**

- `--ignore-errors` 仅吞掉**逐文件**失败；`IndexLoad` / `IndexSave` / `Status` 等全局失败仍然立刻返回 fatal error
- 被跳过的逐文件失败写入 `AddOutput.failed`
- human 模式在 stderr 输出 warning block；JSON / machine 仅通过 `failed` 字段暴露，不再混入 human 文本
- 只要存在 `ignored` 或 `failed`，都必须调用共享 warning tracker，确保与 clone / config 等命令的 `--exit-code-on-warning` 语义一致

**"Nothing specified" 场景改进：**

当前无 pathspec 且无 `-A` / `-u` / `--refresh` 时，`execute_safe()` 直接 `eprintln!()` 后 `return Ok(())`。改为：
- 返回 `CliError::command_usage()` + `StableErrorCode::CliInvalidArguments`
- hint: `maybe you wanted to say 'libra add .'?`
- 退出码 `129`

### 特性 4：Human 成功确认消息

**当前问题：** `libra add src/main.rs` 成功后无任何输出。审计报告核心发现"成功时沉默"。

**修正后的方案（仅在 human 模式、非 `--quiet` 时输出到 stdout）：**

单文件暂存：
```text
add 'src/main.rs' (new file)
```

多文件暂存：
```text
add 3 files (2 new, 1 modified)
```

删除暂存（tracked file 从工作树删除后 `libra add` 更新索引）：
```text
add 2 files (1 modified, 1 removed)
```

无变更（pathspec 匹配到的文件均未变更）：
```text
nothing to add
```

被忽略文件 warning（部分文件暂存成功 + 部分被忽略）：
```text
stdout:
add 2 files (2 new)

stderr:
warning: the following paths are ignored by one of your .libraignore files:
build/output.o
hint: use -f if you really want to add them.
hint: use 'libra restore --staged <file>' to unstage if needed
```

**`--verbose` 模式（逐文件 + 摘要）：**
```text
add(new): src/new_file.rs
add(modified): src/main.rs
add 2 files (1 new, 1 modified)
```

> **注**：`--verbose` 的逐文件格式保持与当前代码一致（`add(new):` / `add(modified):` / `removed:`），仅从裸 `println!()` 改为经过 `OutputConfig` 管控。

### 特性 5：Cross-Cutting Improvements 在 add 中的具体落地

| ID | 改进 | add 中的具体落地 |
|----|------|-----------------|
| **A** | 退出码 `0/128/129` | 参数错误（无 pathspec 且无 mode flag、pathspec 不匹配、路径越界）→ exit `129`；运行时错误（索引损坏、I/O 失败）→ exit `128`；成功 → exit `0`；warning-only / partial-warning 默认仍 exit `0`，仅 `--exit-code-on-warning` 时由顶层 CLI 转为 exit `9`（与 `clone` / `config` 等命令的全局 `--exit-code-on-warning` 语义一致）|
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | **不适用**——`add` 的参数是 pathspec（文件路径），不是 enum 值，无法做有意义的 fuzzy match。路径不匹配时已有 `PathspecNotMatched` + hint 指向 `libra status` |
| **G** | Issues URL | 仅在 `IndexLoad` / `IndexSave` 错误且内部原因为非预期的 GitError 时输出 Issues URL。其他错误（路径不匹配、编码错误）不输出——这些是用户可自行修复的问题 |

### `--help` EXAMPLES 段

```text
EXAMPLES:
    libra add .                        Stage all changes in current directory
    libra add src/main.rs              Stage a specific file
    libra add src/ tests/              Stage multiple paths
    libra add -A                       Stage all changes (adds, modifies, removes)
    libra add -u                       Update tracked files only (no new files)
    libra add --dry-run .              Preview what would be staged
    libra add -f ignored_file.log      Force-add an ignored file
    libra add --refresh                Refresh index metadata without staging
```

### 全部场景结构化 Output 设计（`--json` / `--machine`）

所有结构化输出遵循统一信封格式，通过 `emit_json_data()` 输出到 stdout。错误 JSON 通过 `CliError` 输出到 stderr。`--machine` 与 `--json` 使用同一 schema，仅格式化方式不同（紧凑单行）。

**路径兼容性约束：** 所有成功字段中的路径继续使用“相对于当前工作目录”的显示形式，与现有 `status --json` 行为一致；新增 `add --json` 不单独发明 repo-root 相对路径规则。

#### 成功 envelope

```json
{
  "ok": true,
  "command": "add",
  "data": { ... }
}
```

#### `libra add src/main.rs --json`

```json
{
  "ok": true,
  "command": "add",
  "data": {
    "added": ["src/main.rs"],
    "modified": [],
    "removed": [],
    "refreshed": [],
    "ignored": [],
    "failed": [],
    "dry_run": false
  }
}
```

#### `libra add --dry-run . --json`

```json
{
  "ok": true,
  "command": "add",
  "data": {
    "added": ["src/new_file.rs"],
    "modified": ["src/main.rs"],
    "removed": ["old_file.rs"],
    "refreshed": [],
    "ignored": [],
    "failed": [],
    "dry_run": true
  }
}
```

#### 错误 JSON：pathspec 不匹配

```json
{
  "ok": false,
  "error_code": "LBR-CLI-003",
  "category": "cli",
  "exit_code": 129,
  "message": "pathspec 'nonexistent.rs' did not match any files",
  "hints": [
    "check the path and try again.",
    "use 'libra status' to inspect tracked and untracked files."
  ]
}
```

#### 错误 JSON：路径越界

```json
{
  "ok": false,
  "error_code": "LBR-CLI-003",
  "category": "cli",
  "exit_code": 129,
  "message": "'../../outside' is outside repository at '/Users/eli/projects/my-repo'",
  "hints": [
    "all paths must be within the repository working tree"
  ]
}
```

#### 错误 JSON：未指定 pathspec

```json
{
  "ok": false,
  "error_code": "LBR-CLI-002",
  "category": "cli",
  "exit_code": 129,
  "message": "nothing specified, nothing added",
  "hints": [
    "maybe you wanted to say 'libra add .'?"
  ]
}
```

### 测试要求

#### `tests/command/add_test.rs`（核心执行路径扩展）

- **（已有）** 基础暂存：单文件、多文件、`-A`、`-u`、`--dry-run`、`--force`、空文件、子目录
- **（新增）`run_add()` 分类结果**：验证 `added` / `modified` / `removed` / `refreshed` / `ignored` / `failed` 分类准确，不依赖 stdout/stderr 文本
- **（新增）warning-only 执行路径**：全部 pathspec 被忽略时返回 `Ok(AddOutput)`，且 staged 列表为空、`ignored` 非空
- **（新增）`--ignore-errors` 部分成功**：部分文件失败时 `failed` 非空，但成功文件仍被写入索引
- **（新增）无变更**：所有匹配文件均未修改时返回空结果，不写索引脏状态

#### `tests/command/add_cli_test.rs`（二进制输出与退出码扩展）

- **（已有）** pathspec 不匹配、忽略文件、损坏索引等 CLI 级回归
- **（新增）成功摘要输出**：`libra add src/main.rs` 后 stdout 包含 `add` 和文件名
- **（新增）stdout/stderr 分离**：部分忽略场景下摘要只出现在 stdout，warning 只出现在 stderr
- **（新增）warning-only human 输出**：ignored-only 场景下 stdout 为空，stderr 仅包含 warning / hint
- **（新增）`--quiet` 静默**：无 warning 的成功路径下 stdout 和 stderr 均为空；有 warning 时仅 stderr 保留 warning
- **（新增）`--verbose` 受控输出**：human 模式下 `--verbose` 输出逐文件列表 + 摘要；`--json` + `--verbose` 不产生 human 格式输出
- **（新增）"Nothing specified" 退出码**：无 pathspec 且无 mode flag 时退出码为 `129`
- **（新增）`--exit-code-on-warning`**：ignored-only 和 `--ignore-errors` 部分失败场景默认 exit `0`，加上 `--exit-code-on-warning` 后 exit `9`

#### `tests/command/add_json_test.rs`（JSON schema 稳定性，新增文件）

- **schema 完整性**：验证 `--json` 输出中每个字段的类型和存在性：
  - `added` / `modified` / `removed` / `refreshed` / `ignored` 是 string 数组
  - `failed` 是 object 数组，元素包含 `path`（string）和 `message`（string）
  - `dry_run` 是 bool
  - 所有路径字段都遵循“相对于当前工作目录”的显示规则，不含前导 `/`
- **`--dry-run --json`**：`dry_run == true`，实际索引未被修改（后续 `status` 仍显示未暂存）
- **`--refresh --json`**：`refreshed` 数组非空，`added` / `modified` / `removed` 均为空
- **`-A --json`**：所有变更文件出现在对应数组中
- **`-u --json`**：新文件不出现在任何数组中（仅更新已跟踪文件）
- **`--force --json`**：被忽略的文件出现在 `added` 中，`ignored` 为空
- **`--force --dry-run --json`**：被忽略的文件出现在 `added` 中，`ignored` 为空，且索引未被修改
- **warning-only JSON**：ignored-only 场景返回 `ok == true`，stdout 只有 envelope，stderr 无额外 human warning 文本
- **`--ignore-errors --json`**：`failed` 非空，且成功暂存的文件仍出现在对应数组中
- **子目录调用路径基准**：在仓库子目录执行 `libra add --json ../path`，返回路径相对于当前子目录，而不是仓库根
- **`--machine add`**：stdout 按 `\n` 分割后恰好 1 行非空行，可被 `serde_json::from_str()` 解析为与 `--json` 相同的 schema

#### CLI 错误码验证（放入 `tests/command/add_cli_test.rs`）

- `PathspecNotMatched` 返回 `LBR-CLI-003`
- `PathOutsideRepo` 返回 `LBR-CLI-003`
- 仓库外执行返回 `LBR-REPO-001`
- "Nothing specified" 返回 `LBR-CLI-002`
- ignored-only / partial-warning 场景默认不返回 fatal 错误码；仅 `--exit-code-on-warning` 时 exit `9`

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入改进范围的执行路径，都必须有对应的集成测试覆盖

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/add.rs` | **重构** | 拆分执行层（`run_add()` → `AddOutput`）与渲染层；补充 `failed` 结构化字段；`--verbose` / `--dry-run` 输出收集到 `AddOutput`；warning 接入共享 warning tracker；`AddError → CliError` 显式 `StableErrorCode` 映射；"Nothing specified" 改为 `CliError::command_usage()` |
| `src/command/status.rs` | **无改动** | `changes_to_be_staged_split_safe()` 等已满足 add 的需求 |
| `tests/command/add_test.rs` | **扩展** | 新增执行层分类结果、warning-only、partial success、无变更场景 |
| `tests/command/add_cli_test.rs` | **扩展** | 新增成功摘要、stdout/stderr 分离、quiet、verbose、warning exit code 场景以及 CLI 错误码验证 |
| `tests/command/add_json_test.rs` | **新增** | JSON schema 完整性和稳定性验证 |
| `tests/command/mod.rs` | **修改** | 注册新增的测试文件 |
