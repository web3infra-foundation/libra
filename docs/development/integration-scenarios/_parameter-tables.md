# 参数覆盖表（按命令组）

### `libra init` 参数覆盖表

| 参数 | 场景 ID | 关键断言 |
|---|---|---|
| `DIRECTORY` | `cli.init-directory-and-quiet` | 目标目录和 `.libra/libra.db` 被创建 |
| `-q` / `--quiet` | `cli.init-directory-and-quiet` | 成功但不输出普通 banner |
| `-b` / `--initial-branch` | `cli.init-branch-and-format-options` | 初始分支可通过公开命令观察 |
| `--object-format` | `cli.init-branch-and-format-options` | `core.objectformat` 为 `sha1` / `sha256`，非法值失败 |
| `--ref-format` | `cli.init-branch-and-format-options` | `core.initrefformat` 为 `strict` / `filesystem`，非法值失败 |
| `--bare` | `cli.init-bare-and-shared` | 存储根为目标目录本身，无普通 `.libra/` 工作区布局 |
| `--shared` | `cli.init-bare-and-shared` | 支持值成功，非法值失败并提示支持值 |
| `--template` | `cli.init-template` | 模板内容复制到 Libra 存储根，缺失路径失败 |
| `--from-git-repository` | `cli.init-from-git-repository` | 本地 Git fixture 的文件/提交/ref 可通过 Libra CLI 观察 |
| `--vault` | `cli.init-vault` | `vault.db` 与 `vault.signing` 状态符合显式 bool |



### `libra status/add/commit/log` 参数覆盖表

| 参数或子命令 | 场景 ID | 关键断言 |
|---|---|---|
| `status` | `cli.commit-status-log` | 默认状态可执行，干净/dirty 状态可观察 |
| `status --short` | `cli.commit-status-log` | untracked 或 staged path 以短格式出现 |
| `status --porcelain` | `cli.commit-status-log` | 输出适合脚本断言的机器可读状态 |
| `status --exit-code` | `cli.commit-status-log` | 干净为 0，dirty 为非 0 |
| `add <pathspec>` | `cli.commit-status-log` | 指定文件被加入 index 并可由 status 观察 |
| `add --dry-run` | `cli.commit-status-log` | 预览输出不改变 index |
| `commit -m` | `cli.commit-status-log` | 提交消息进入 log |
| `commit -F` | `cli.commit-status-log` | 从文件读取提交消息 |
| `commit -a` | `cli.commit-status-log` | 已跟踪文件修改被自动暂存并提交 |
| `commit --allow-empty` | `cli.commit-status-log` | 空提交成功并出现在 log 中 |
| `commit --amend --no-edit` | `cli.commit-status-log` | 最后一个提交被替换且消息复用 |
| `commit --conventional` | `cli.commit-status-log` | 非 conventional 消息失败且不写入提交 |
| `commit --signoff` | `cli.commit-status-log` | 提交消息包含 Signed-off-by trailer |
| `log --oneline` | `cli.commit-status-log` | 输出短 hash 和提交主题 |
| `log -n` | `cli.commit-status-log` | 输出数量受限制 |
| `log --author` / `--grep` | `cli.commit-status-log` | 只返回匹配作者或消息的提交 |
| `log --name-status` / `--stat` | `cli.commit-status-log` | 文件变化摘要可观察 |



### `libra branch/switch/checkout` 参数覆盖表

| 参数或子命令 | 场景 ID | 关键断言 |
|---|---|---|
| `branch <name>` | `cli.branch-switch-checkout` | 从 HEAD 创建本地分支 |
| `branch <name> <commit>` | `cli.branch-switch-checkout` | 从指定 base 创建分支 |
| `branch --list` | `cli.branch-switch-checkout` | 已创建分支可列出 |
| `branch --show-current` | `cli.branch-switch-checkout` | 当前分支名可观察 |
| `branch -m <old> <new>` | `cli.branch-switch-checkout` | 分支重命名后新名可用、旧名不可用 |
| `branch -d` / `branch -D` | `cli.branch-switch-checkout` | 安全删除和强制删除路径均覆盖 |
| `switch <branch>` | `cli.branch-switch-checkout` | 切换到现有分支 |
| `switch -c <branch> <start>` | `cli.branch-switch-checkout` | 创建并切换到新分支 |
| `switch --detach <commit>` | `cli.branch-switch-checkout` | HEAD 进入 detached 状态 |
| `checkout <branch>` | `cli.branch-switch-checkout` | 兼容分支切换路径可用 |
| `checkout -b <branch>` | `cli.branch-switch-checkout` | 兼容创建并切换路径可用 |
| `checkout -- <pathspec>` | `cli.branch-switch-checkout` | 路径恢复行为可观察 |



### `libra diff/restore/reset` 参数覆盖表

| 参数或子命令 | 场景 ID | 关键断言 |
|---|---|---|
| `diff <pathspec>` | `cli.restore-reset-diff` | unstaged 工作区修改可见 |
| `diff --staged` | `cli.restore-reset-diff` | staged 修改可见 |
| `diff --old --new` | `cli.restore-reset-diff` | 两个 revision 间差异可见 |
| `diff --name-only` / `--name-status` | `cli.restore-reset-diff` | 文件名和状态摘要可用于脚本断言 |
| `diff --stat` / `--numstat` | `cli.restore-reset-diff` | 文件级统计输出可见 |
| `diff --raw` | `cli.restore-reset-diff` | raw 机器格式含 mode、abbrev object id、状态和路径 |
| mode-only `diff --raw` / `--name-status` | `cli.restore-reset-diff` | `T` typechange 输出由命令测试覆盖 |
| `diff -b` / `-w` / `--ignore-blank-lines` | `cli.restore-reset-diff` | 空白忽略路径由命令测试覆盖，runner 覆盖 `-w` |
| `diff -U<n>` / `--unified <n>` / `diff.context` | `cli.restore-reset-diff` | 可配置上下文由命令测试覆盖，runner 覆盖 `-U0` |
| `diff --exit-code` / `--quiet` | `cli.restore-reset-diff` | 有差异时语义退码为 1 |
| `diff -M<n>` / `--find-renames[=<n>]` | `cli.restore-reset-diff` | Git 风格短阈值和 name-status 重命名输出可见 |
| `diff -C<n>` / `--find-copies[=<n>]` / `--no-renames` | `cli.restore-reset-diff` | copy/禁用 rename 细节由命令测试覆盖 |
| `diff --relative[=<path>]` / `diff.noPrefix` | `cli.restore-reset-diff` | 子目录过滤和路径裁切由命令测试覆盖 |
| `diff --word-diff[=<mode>]` / `--word-diff-regex` / `diff.wordRegex` | `cli.restore-reset-diff` | word-diff 标记、regex 上限和配置由命令测试覆盖 |
| `diff -W` / `--function-context` | `cli.restore-reset-diff` | 函数上下文扩展由命令测试覆盖 |
| `restore --staged <path>` | `cli.restore-reset-diff` | index 恢复到 HEAD，工作区保持修改 |
| `restore --worktree <path>` | `cli.restore-reset-diff` | 工作区文件恢复到 index 或 source 内容 |
| `restore --source <rev>` | `cli.restore-reset-diff` | source revision 不存在时失败且不改写文件 |
| `reset HEAD -- <path>` | `cli.restore-reset-diff` | 路径级 reset 只取消暂存 |
| `reset --pathspec-from-file=<file>` / `--pathspec-file-nul` | `cli.restore-reset-diff` | file/NUL pathspec 输入只取消暂存指定路径 |
| `reset --no-refresh` | `cli.restore-reset-diff` | 兼容 no-op flag 可传入且保持 mixed reset 语义 |
| `reset --soft` | `cli.restore-reset-diff` | 只移动 HEAD，保留 index/工作区 |
| `reset --mixed` | `cli.restore-reset-diff` | 移动 HEAD 并重置 index |
| `reset --hard` | `cli.restore-reset-diff` | HEAD、index、工作区全部回到目标 revision |



### `libra stash/bisect/worktree` 参数覆盖表

| 参数或子命令 | 场景 ID | 关键断言 |
|---|---|---|
| `stash push -m` | `cli.stash-bisect-worktree` | tracked 修改被保存，消息可在列表中观察 |
| `stash list` / `stash show` | `cli.stash-bisect-worktree` | stash 条目和文件级摘要可观察 |
| `stash apply` | `cli.stash-bisect-worktree` | 修改恢复但 stash 条目保留 |
| `stash pop` | `cli.stash-bisect-worktree` | 修改恢复且 stash 条目删除 |
| `stash clear --force` | `cli.stash-bisect-worktree` | 非交互清空 stash 列表 |
| `bisect start <bad> --good <good>` | `cli.stash-bisect-worktree` | 二分边界可初始化 |
| `bisect bad` / `bisect good <rev>` | `cli.stash-bisect-worktree` | 会话状态推进并可由 log/view 观察 |
| `bisect log` / `bisect view` | `cli.stash-bisect-worktree` | 当前会话和候选状态可输出 |
| `bisect reset` | `cli.stash-bisect-worktree` | 结束会话并恢复原 HEAD |
| `worktree add <path>` | `cli.stash-bisect-worktree` | linked worktree 被创建并登记 |
| `worktree list` | `cli.stash-bisect-worktree` | 主 worktree 和 linked worktree 均可列出 |
| `worktree lock --reason` / `unlock` | `cli.stash-bisect-worktree` | 锁状态和 reason 可观察并可解除 |
| `worktree move <src> <dest>` | `cli.stash-bisect-worktree` | 登记路径和目录路径同步移动 |
| `worktree remove <path>` | `cli.stash-bisect-worktree` | 默认注销登记但保留目录 |
| `worktree prune` | `cli.stash-bisect-worktree` | 清理 stale 登记路径可执行 |



### `libra tag/history-inspection/worktree-tools/ref-log` 参数覆盖表

| 参数或子命令 | 场景 ID | 关键断言 |
|---|---|---|
| `tag <name>` / `tag -m <msg>` | `cli.tag-basic` | 轻量和 annotated tag 均可创建、列出、解析 |
| `tag -l` / `tag -l -n` / `tag -f` / `tag -d` | `cli.tag-basic` | 列表、注释摘要、强制更新和删除路径覆盖 |
| `merge <branch>` | `cli.merge-rebase-cherry-revert-smoke` | fast-forward 与三方无冲突 merge 均可观察 |
| `merge --find-renames[=<n>]` | `cli.merge-rebase-cherry-revert-smoke` | 相似度阈值控制 rename+edit 是否自动合并 |
| `merge --squash --continue` | `cli.merge-rebase-cherry-revert-smoke` | 与 lifecycle action 组合必须被拒绝 |
| `merge --continue` / `--abort` | `cli.merge-rebase-cherry-revert-smoke` | 无会话时明确失败；冲突续跑场景另行补充 |
| `rebase <upstream>` | `cli.merge-rebase-cherry-revert-smoke` | topic 提交重放到新 base |
| `rebase --continue` | `cli.merge-rebase-cherry-revert-smoke` | 无会话时明确失败；冲突续跑场景另行补充 |
| `cherry-pick <commit>` | `cli.merge-rebase-cherry-revert-smoke` | 指定提交修改被重放到当前分支 |
| `revert <commit>` | `cli.merge-rebase-cherry-revert-smoke` | 创建反向提交并撤销目标修改 |
| `grep` / `grep -F/-i/-n/-c/-l/-e/-f/--tree` | `cli.grep-blame-describe-shortlog` | 工作区、pathspec、pattern file 和历史 tree 搜索可观察 |
| `blame` / `blame -L` | `cli.grep-blame-describe-shortlog` | 行级作者、提交和范围限制可观察 |
| `describe --tags/--always/--abbrev` | `cli.grep-blame-describe-shortlog` | tag 描述和 hash fallback 可观察 |
| `shortlog` / `shortlog -s` / `shortlog -n` | `cli.grep-blame-describe-shortlog` | 作者汇总和排序可观察 |
| `clean -n/-f/-fd/-fX` | `cli.clean-rm-mv-lfs-basic` | dry-run、文件删除、目录删除、ignored-only 删除覆盖 |
| `rm <path>` | `cli.clean-rm-mv-lfs-basic` | tracked 文件从工作区和 index 移除 |
| `mv <src> <dst>` | `cli.clean-rm-mv-lfs-basic` | tracked 文件移动并更新 index |
| `lfs track/untrack/ls-files` | `cli.clean-rm-mv-lfs-basic` | `.libra_attributes` pattern 和 LFS tracked 文件列表可观察 |
| `reflog show` / `reflog show --stat` / `reflog exists` / `reflog expire --dry-run` | `cli.reflog-symbolic-ref` | HEAD/ref 更新记录可读，exists 可脚本探测，expire 可预览清理并对无 ref 入参返回稳定错误 |
| `symbolic-ref` / `symbolic-ref --short` / `symbolic-ref HEAD <target>` | `cli.reflog-symbolic-ref` | HEAD 符号引用读写可观察 |
| `--json open` | `cli.open-smoke` | 只输出 URL 和 `launched=false`，不启动外部程序 |
