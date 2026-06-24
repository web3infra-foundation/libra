# `libra commit`

从已暂存更改创建新提交。

**别名：** `ci`

## 概要

```
libra commit [OPTIONS] -m <MESSAGE>
libra commit [OPTIONS] -F <FILE>
libra commit --amend [--no-edit]
```

## 说明

`libra commit` 从已暂存更改创建新提交，构建 tree 和 commit 对象，验证消息（包括可选的 conventional commit 格式，以及通过 vault 进行 GPG 签名），并更新 HEAD 和 refs。

该命令读取索引以确定哪些文件已暂存，构造与暂存内容匹配的 tree 对象层级，使用提供的消息和 author/committer 元数据创建 commit 对象，并推进当前分支 ref。启用 vault signing 时，提交会自动进行 GPG 签名。除非用 `--no-verify` 绕过，pre-commit 和 commit-msg hooks 会被执行。

## 选项

### `-m, --message <MESSAGE>`

使用给定消息作为提交消息。除非使用 `--no-edit`（搭配 `--amend`）或提供 `-F`，否则必需。

```bash
libra commit -m "Add new feature"
```

### `-F, --file <FILE>`

从给定文件读取提交消息。在未使用 `--no-edit` 时与 `-m` 互斥。

```bash
libra commit -F message.txt
```

### `--amend`

通过创建新提交替换当前分支 tip。新提交拥有与被替换提交相同的父提交。不能 amend merge commits（有多个父提交的提交）。

```bash
libra commit --amend
libra commit --amend -m "Updated message"
```

### `--no-edit`

与 `--amend` 一起使用时，复用原提交消息，不提示修改。与 `-m` 和 `-F` 冲突。

```bash
libra commit --amend --no-edit
```

### `--conventional`

根据 Conventional Commits 规范（https://www.conventionalcommits.org）验证提交消息。消息必须匹配模式 `<type>[optional scope]: <description>`。验证失败时会报错。

```bash
libra commit -m "feat: add login" --conventional
libra commit -m "fix(auth): handle expired tokens" --conventional
```

### `-a, --all`

提交前自动暂存已修改或已删除的已跟踪文件。等价于在 `libra commit` 前运行 `libra add -u`。不会添加新的未跟踪文件。

```bash
libra commit -a -m "Fix typo"
```

### `-s, --signoff`

使用 committer 身份在提交消息末尾添加 `Signed-off-by` trailer。

```bash
libra commit -s -m "Add feature"
```

### `--allow-empty`

允许创建没有更改的提交（相对父提交为空 diff）。适合触发 CI 或标记里程碑。

```bash
libra commit --allow-empty -m "Trigger CI"
```

### `--disable-pre`

只跳过 pre-commit hook。commit-msg hook 仍会运行。

```bash
libra commit --disable-pre -m "Quick fix"
```

### `--no-verify`

跳过所有 pre-commit 和 commit-msg hooks/validations。与 Git 的 `--no-verify` 行为一致。

```bash
libra commit --no-verify -m "WIP: work in progress"
```

### `--author <AUTHOR>`

覆盖提交作者。必须使用标准 `A U Thor <author@example.com>` 格式。

```bash
libra commit --author "Jane Doe <jane@example.com>" -m "Patch"
```

### `--status` / `--no-status`

`--status` 把工作树状态以 `#` 注释行注入提交消息编辑器模板（Git 默认显示；Libra 默认省略，故用 `--status` 主动开启）。由于是注释行，消息 cleanup 会将其剥离——仅供参考，不进入最终提交消息。未打开编辑器时（例如带 `-m`）无效果。在保留注释行的 cleanup 模式下也会省略（`--cleanup=verbatim` 与 `--cleanup=whitespace`，除非 `-v` 强制 scissors），从而绝不泄漏进消息；仅当打开编辑器且生效的 cleanup 会剥离注释时才注入。`--no-status`（默认）不含 status 段。两者构成 last-wins 切换。

```bash
libra commit --status          # 打开编辑器，模板中含注释化的状态
libra commit --no-status -m "message"
```

## 常用命令

```bash
libra commit -m "Add new feature"
libra commit -m "feat: add login" --conventional
libra commit --amend
libra commit --amend --no-edit
libra commit -a -m "Fix typo"
libra commit -F message.txt
libra commit -s -m "Add feature"
libra commit --allow-empty -m "Trigger CI"
libra commit --json -m "Add feature"
```

## 人类可读输出

默认人类模式将提交摘要写到 `stdout`。

普通提交：

```text
[main abc1234] Add new feature
 2 files changed (new: 1, modified: 1, deleted: 0)
```

Root commit：

```text
[main (root-commit) abc1234] Initial commit
 1 file changed (new: 1, modified: 0, deleted: 0)
```

`--quiet` 会抑制所有 `stdout` 输出。

## 结构化输出

`libra commit` 支持全局 `--json` 和 `--machine` 标志。

- `--json` 向 `stdout` 写入一个成功信封
- `--machine` 以紧凑单行 JSON 写入相同 schema
- 两者都会抑制 hook stdout/stderr（通过 pipe 而不是继承）
- 成功时 `stderr` 保持干净

示例：

```json
{
  "ok": true,
  "command": "commit",
  "data": {
    "head": "main",
    "branch": "main",
    "commit": "abc1234def5678901234567890abcdef12345678",
    "short_id": "abc1234",
    "subject": "Add new feature",
    "root_commit": false,
    "amend": false,
    "files_changed": {
      "total": 2,
      "new": 1,
      "modified": 1,
      "deleted": 0
    },
    "signoff": false,
    "conventional": null,
    "signed": true
  }
}
```

Root commit：

```json
{
  "ok": true,
  "command": "commit",
  "data": {
    "head": "main",
    "branch": "main",
    "commit": "abc1234def5678901234567890abcdef12345678",
    "short_id": "abc1234",
    "subject": "Initial commit",
    "root_commit": true,
    "amend": false,
    "files_changed": {
      "total": 1,
      "new": 1,
      "modified": 0,
      "deleted": 0
    },
    "signoff": false,
    "conventional": null,
    "signed": true
  }
}
```

Amend：

```json
{
  "ok": true,
  "command": "commit",
  "data": {
    "head": "main",
    "branch": "main",
    "commit": "def5678abc1234901234567890abcdef12345678",
    "short_id": "def5678",
    "subject": "Amended message",
    "root_commit": false,
    "amend": true,
    "files_changed": {
      "total": 1,
      "new": 0,
      "modified": 1,
      "deleted": 0
    },
    "signoff": false,
    "conventional": null,
    "signed": true
  }
}
```

### Schema 说明

- `head` 是分支名，或为保持向后兼容而使用的 `"detached"`
- HEAD detached 时 `branch` 为 `null`；否则为 `Some(name)`
- 传递 `--conventional` 且验证成功时，`conventional` 为 `true`；未请求时为 `null`
- 启用 vault signing 且提交已 GPG 签名时，`signed` 为 `true`
- `-s` / `--signoff` 追加 `Signed-off-by` trailer 时，`signoff` 为 `true`

## 设计理由

### `--conventional` conventional commits 标志

Git 没有内置提交消息格式验证；团队依赖 commitlint、husky 或 CI 检查等外部工具来强制 Conventional Commits。Libra 在 commit 命令中直接提供一等 `--conventional` 验证。这有两个目的：（1）在提交时立即反馈，而不是在 CI 中延迟反馈；（2）让以编程方式生成提交消息的 AI 代理无需外部工具即可验证输出。该标志是 opt-in 而非强制，以尊重使用不同提交消息约定的团队。

### 默认 vault signing，而不是手动 GPG 设置

在 Git 中，提交签名需要配置 `user.signingkey`、`gpg.program` 和 `commit.gpgsign`，这是多数开发者会跳过的多步流程。Libra 的 vault 在仓库初始化时自动生成并管理 PGP 签名密钥，因此提交默认零配置签名。这让签名提交成为常态而非例外，提升整个生态的供应链安全。不想签名的用户可以用 `libra config vault.signing false` 禁用。

### `--disable-pre` 标志

`--disable-pre` 只跳过 pre-commit hook，但仍运行 commit-msg hook。这比 Git 的 `--no-verify` 更细粒度，后者会跳过所有 hooks。用例是开发者信任提交消息验证（例如通过 commit-msg hook 做 conventional commit 检查），但在快速迭代中想跳过昂贵的 pre-commit 检查（例如完整测试套件、大型 linter 运行）。这种关注点分离是有意的：提交消息是永久记录的一部分，即使快速迭代时也应被验证。

### 用 `--no-verify` 跳过 hooks

当需要绕过所有 hook 验证时（例如紧急修复、WIP commits），`--no-verify` 会跳过 pre-commit 和 commit-msg hooks。这与 Git 的行为和命名约定一致。选择该标志名是为了 Git 兼容性，让从 Git 切换的开发者无需学习新标志名。

## 参数对比：Libra vs Git vs jj

| 参数 / 标志 | Git | jj | Libra |
|---|---|---|---|
| 带消息提交 | `git commit -m "msg"` | `jj commit -m "msg"` | `libra commit -m "msg"` |
| 从文件提交 | `git commit -F file` | N/A | `libra commit -F file` |
| Amend 上次提交 | `git commit --amend` | `jj describe`（编辑工作副本提交） | `libra commit --amend` |
| Amend 且不编辑 | `git commit --amend --no-edit` | `jj describe --no-edit` | `libra commit --amend --no-edit` |
| 自动暂存已跟踪 | `git commit -a` | N/A（自动跟踪） | `libra commit -a` |
| 允许空提交 | `git commit --allow-empty` | `jj commit --allow-empty` | `libra commit --allow-empty` |
| Signoff trailer | `git commit -s` / `--signoff` | N/A | `libra commit -s` / `--signoff` |
| GPG 签名提交 | `git commit -S`（手动 GPG） | N/A（无签名） | 自动（vault-backed） |
| 覆盖 author | `git commit --author="..."` | N/A | `libra commit --author="..."` |
| Conventional 检查 | 外部工具（commitlint） | N/A | `libra commit --conventional` |
| 只跳过 pre-commit | N/A | N/A | `libra commit --disable-pre` |
| 跳过所有 hooks | `git commit --no-verify` | N/A | `libra commit --no-verify` |
| Fixup commit | `git commit --fixup=<commit>` | N/A | N/A |
| Squash commit | `git commit --squash=<commit>` | `jj squash` | N/A |
| 交互式消息 | `git commit`（打开编辑器） | `jj commit`（打开编辑器） | N/A（需要通过 -m 或 -F 提供消息） |
| 编辑器中 verbose diff | `git commit -v` | N/A | N/A |
| 重置作者日期 | `git commit --reset-author` | N/A | N/A |
| Cleanup 模式 | `git commit --cleanup=<mode>` | N/A | N/A |
| Trailer | `git commit --trailer="..."` | N/A | N/A |
| 结构化 JSON 输出 | N/A | N/A | `--json` / `--machine` |
| 错误提示 | 最少 | 最少 | 每种错误类型都有可操作提示 |

## 错误处理

每个 `CommitError` 变体都会映射到显式 `StableErrorCode`。

| 场景 | 错误码 | 退出码 | 提示 |
|----------|-----------|------|------|
| 索引损坏 | `LBR-REPO-002` | 128 | "the index file may be corrupted; try 'libra status' to verify" |
| 无法保存索引 | `LBR-IO-002` | 128 | -- |
| 无内容可提交（干净） | `LBR-REPO-003` | 128 | "use 'libra add' to stage changes" |
| 无内容可提交（无已跟踪文件） | `LBR-REPO-003` | 128 | "create/copy files and use 'libra add' to track" |
| 缺少 author 身份 | `LBR-AUTH-001` | 128 | "run 'libra config user.name ...' and 'libra config user.email ...'" |
| 没有可 amend 的提交 | `LBR-REPO-003` | 128 | "create a commit before using --amend" |
| Amend merge commit | `LBR-REPO-003` | 128 | "create a new commit instead of amending a merge commit" |
| 无效 author 格式 | `LBR-CLI-002` | 129 | "expected format: 'Name <email>'" |
| 无法读取消息文件 | `LBR-IO-001` | 128 | -- |
| 空提交消息 | `LBR-REPO-003` | 128 | "use -m to provide a commit message" |
| Tree 创建失败 | `LBR-INTERNAL-001` | 128 | Issues URL |
| 对象存储失败 | `LBR-IO-002` | 128 | -- |
| 父提交缺失 | `LBR-REPO-002` | 128 | "the parent commit is missing or corrupted" |
| HEAD 更新失败 | `LBR-IO-002` | 128 | -- |
| Pre-commit hook 失败 | `LBR-REPO-003` | 128 | "use --no-verify to bypass the hook" |
| Conventional commit 无效 | `LBR-CLI-002` | 129 | "see https://www.conventionalcommits.org for format rules" |
| Vault signing 失败 | `LBR-AUTH-001` | 128 | "check vault configuration with 'libra config --list'" |
| Auto-stage 失败 | `LBR-IO-001` | 128 | -- |
| 暂存更改计算 | `LBR-REPO-002` | 128 | "failed to compute staged changes" |

## 兼容性说明

- Libra 不打开编辑器进行交互式消息编写；始终需要 `-m` 或 `-F`（`--amend --no-edit` 除外）
- jj 没有带暂存的传统 `commit` 命令；`jj commit` 会完成 working copy commit
- 不支持 `--fixup` 和 `--squash`；使用 `libra rebase -i` 进行提交重组
- Vault signing 替代 Git 的 `commit.gpgsign` 和 `user.signingkey` 配置
- 不支持用于剥离注释的 `--cleanup` 模式；消息按原样使用
