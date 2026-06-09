# `libra clean`

从工作树移除未跟踪文件（以及可选的目录）。

## 概要

```
libra clean -n [-d] [-x | -X] [-e <pattern>]... [--json] [--quiet]
libra clean -f[f] [-d] [-x | -X] [-e <pattern>]... [--json] [--quiet]
libra clean -i [-d] [-x | -X] [-e <pattern>]... [--quiet]
```

## 说明

`libra clean` 从工作树移除未跟踪文件。与 Git 一样，Libra 要求显式模式标志：`-n` 用于 dry-run 预览，`-f` 用于实际删除，或 `-i` 用于交互式选择。不带上述任一标志运行 `libra clean` 是错误，**除非** `clean.requireForce` 被设为 `false`。这通过强制用户明确意图来防止意外数据丢失。

默认情况下，只移除文件，并遵守 `.libraignore` 规则（忽略文件会被跳过）。`-d` 标志选择同时移除未跟踪目录；`-x` 选择移除原本会受 ignore 规则保护的文件；`-X` 会反转规则，使得*只有*被忽略文件会被移除。每个候选路径都会被规范化并验证位于工作树根目录内，然后才删除，从而防止 symlink-escape 攻击。

## 选项

| 标志 | 短选项 | 长选项 | 说明 |
|------|-------|------|-------------|
| Dry run | `-n` | `--dry-run` | 显示会被移除的内容，但不删除任何东西。 |
| Force | `-f` | `--force` | 实际移除未跟踪文件。重复（`-ff`）以同时移除嵌套子仓（见下文）。 |
| Interactive | `-i` | `--interactive` | 通过菜单选择要移除的未跟踪项。与 `--json` 和 `-n` 互斥。 |
| Directories | `-d` | `--dir` | 同时移除未跟踪目录（否则只移除文件）。 |
| Include ignored | `-x` | | 移除未跟踪文件，**包括**被 `.libraignore` 匹配的文件。 |
| Only ignored | `-X` | | **仅**移除被 `.libraignore` 匹配的未跟踪文件。 |
| Exclude | `-e` | `--exclude <pattern>` | 添加额外排除模式；可重复。 |
| JSON | | `--json` | 输出结构化 JSON（见下方）。 |
| Quiet | `-q` | `--quiet` | 抑制人类可读 stdout **以及** `Skipping repository` / `warning: failed to remove` 的 stderr 警告。这是全局 `-q`/`--quiet`（`OutputConfig.quiet`），而非 clean 专属字段。 |

`-x` 和 `-X` 互斥；`-x` 会在普通未跟踪文件之外*包含*被忽略文件，`-X` 则将操作限制为仅被忽略文件。

### 选项细节

**`-n` / `--dry-run`**

预览模式。列出每个*会*被删除的未跟踪路径，但不触碰文件系统：

```bash
$ libra clean -n
Would remove build/output.log
Would remove notes.txt
```

**`-f` / `--force`**

删除模式。移除每个未跟踪路径并报告每次移除：

```bash
$ libra clean -f
Removing build/output.log
Removing notes.txt
```

**`-d` / `--dir`**

显式选择未跟踪目录。没有 `-d` 时，未跟踪目录会保留原位（如果目录本身被跟踪，其内容仍会被考虑）。使用 `-d` 时，会遍历目录树，并在文件移除后移除空目录。

**`-x`**

覆盖 `.libraignore`。没有此标志时，被忽略文件（构建产物、缓存等）会被跳过。使用 `-x` 后，它们会像任何其他未跟踪文件一样被移除。

**`-X`**

`-x` 的反向。只移除 `.libraignore` 通常会保护的文件。适合“清理构建产物但保留手工编辑文件”的场景。

**`--exclude <pattern>`**

为本次调用添加额外排除模式（使用 `.libraignore` 语法）。可多次传递以叠加模式：

```bash
libra clean -f --exclude '*.log' --exclude 'tmp/**'
```

**`-i` / `--interactive`**

显示未跟踪候选项菜单，让你在删除任何东西之前细化选择。每个候选项初始都被选中；菜单提供六个子命令（对标 `git clean -i`）：

```text
Would remove the following items:
  *   1: build/output.log
  *   2: notes.txt
*** Commands ***
    1: clean                2: filter by pattern    3: select by numbers
    4: ask each             5: quit                 6: help
What now>
```

- **clean** — 删除当前选中项并退出。
- **filter by pattern** — 输入 `.libraignore` 风格的 glob 以取消选择匹配项；匹配到目录的模式会同时取消选择其下的所有项（祖先继承）。空行返回。
- **select by numbers** — 按编号替换选择：单个数字、逗号/空格列表、闭区间（`2-5`）、开区间（`7-`）、`*` 表示全部，以及 `-` 前缀表示取消选择（`-3`）。越界标记被忽略。
- **ask each** — 用 `Remove <path>? [y/N]` 逐项确认。
- **quit** — 不删除任何东西直接退出。
- **help** — 打印每个子命令的帮助屏。

命令接受前导数字、完整单词或不区分大小写的首字母。交互循环本身从不触碰文件系统：它把选中的路径返回给与 `-f` 相同的容错删除路径。EOF（已关闭/管道 stdin 无内容可读）被视为 `quit`，因此非交互式调用永不挂起。`-i` 不能与 `--json`（机器输出）或 `-n`（dry-run）组合——两者都在预检阶段以 `LBR-CLI-002` 报错。

**嵌套子仓（`-ff`）**

直接子项包含 `.git` 或 `.libra` 的目录是一个独立子仓。单个 `-f` 会**跳过**这类目录（及其下所有文件）并向 stderr 打印 `Skipping repository <path>`，使误运行的 `clean` 永不会抹除无关的检出。传入第二个 `-f`（`-ff`，基于计数——`-f -f` 也可）以选择删除嵌套子仓，对标 `git clean -ffd`。请先用 `libra clean -n -ffd` 预览。

**容错删除**

删除不再在首次失败时中止。若某个路径无法被移除（例如只读文件），Libra 会向 stderr 打印 `warning: failed to remove <path>: <detail>`，把该路径记入 JSON `failed` 数组，并继续处理其余候选项。在输出（部分）成功清单后，命令以 `128`（`LBR-IO-002`）退出，使调用方仍能观察到失败。

**组合 `-n` 和 `-f`**：两个标志都传递时，dry-run 优先，不会删除文件。

## 常用命令

```bash
# 预览会移除什么
libra clean -n

# 移除所有未跟踪文件（仅文件）
libra clean -f

# 也移除未跟踪目录
libra clean -fd

# 移除未跟踪文件，包括被忽略文件（构建产物、缓存）
libra clean -fx

# 只移除被忽略文件（保留手工编辑文件）
libra clean -fX

# 交互式选择要移除哪些未跟踪项
libra clean -i

# 在决定使用 -ff 之前，先预览是否会移除嵌套子仓
libra clean -n -ffd

# 强制移除未跟踪文件以及任意嵌套的 .git/.libra 子仓
libra clean -ffd

# 在 .libraignore 之上叠加一个额外排除模式
libra clean -f --exclude '*.log'

# 以 JSON 格式预览（适合脚本）
libra clean -n --json
```

## 人类可读输出

Dry-run：

```text
Would remove build/output.log
Would remove notes.txt
```

强制移除：

```text
Removing build/output.log
Removing notes.txt
```

`--quiet` 会抑制 stdout。

## 结构化输出（JSON）

```json
{
  "ok": true,
  "command": "clean",
  "data": {
    "dry_run": true,
    "removed": ["build/output.log", "notes.txt"]
  }
}
```

没有可清理内容时，`removed` 为空。当容错删除遇到无法删除的路径时，信封还会携带一个 `failed` 数组（残留的路径），并以 `128` 退出：

```json
{
  "ok": true,
  "command": "clean",
  "data": {
    "dry_run": false,
    "removed": ["notes.txt"],
    "failed": ["locked/output.bin"]
  }
}
```

当每次删除都成功时，`failed` 会被整体省略（它是 `#[serde(default, skip_serializing_if = "Vec::is_empty")]` 字段，因此旧解析器保持向前兼容）。交互模式（`-i`）从不输出 JSON。

## 设计理由

### 为什么要求显式模式标志？

Git 的 `clean` 在没有 `-f` 时（且没有 `clean.requireForce = false`）会打印要求 `-f` 的错误。Libra 与之对齐：默认（`clean.requireForce = true`）你必须传递 `-n`、`-f` 或 `-i` 之一，这消除了一整类“我不小心运行了 clean”的事故。与早期 Libra 不同，护栏现在是可配置的——把 `clean.requireForce` 设为 `false`（本地配置优先于全局）允许裸 `libra clean` 直接删除，对标 Git 的配置契约，便于明确选择放宽的脚本环境。

### 为什么提供交互模式（`-i`）？

早期 Libra 以“AI 代理和脚本工作流无法驱动交互式提示”为由拒绝了 `git clean -i`。该理由对*自动化*路径成立，却给人工整理凌乱工作树留下了真实的对等缺口。`-i` 现已实现为基于泛型 `BufRead`/`Write` 的纯粹、完全单元可测的状态机，因此无需 TTY 即可在测试中被驱动，且永不阻塞代理：该循环仅在显式传入 `-i` 时可达，与 `--json`（机器可读路径）和 `-n`（预览）互斥，并把 EOF 视为 `quit`。代理继续使用 `-n --json` → `-f` 两步法；人类则获得菜单。

### 为什么在最初拒绝后又提供 `-d` / `-x` / `-X`？

最初的 `clean` 设计出于安全考虑有意拒绝目录和 ignore 覆盖标志（`docs/improvement/clean.md` 将它们列为非目标）。后续用户反馈显示，代理驱动环境中的构建工作流经常需要清理被忽略产物，缺失这些标志迫使用户退回到原始 `rm -rf`，而这严格来说不如 `clean` 安全（没有 symlink-escape 验证，没有 dry-run 预览）。这些标志加入时使用了与基础模式相同的工作树限制和 symlink 检查，在恢复与 `git clean` 对等能力的同时保留安全保证。

## 参数对比：Libra vs Git vs jj

| 参数 | Libra | Git | jj |
|-----------|-------|-----|----|
| Dry run | `-n` / `--dry-run` | `-n` / `--dry-run` | N/A（无 clean 命令） |
| 强制删除 | `-f` / `--force` | `-f` / `--force` | N/A |
| 移除目录 | `-d` / `--dir` | `-d` | N/A |
| Ignore 覆盖（全部） | `-x` | `-x` | N/A |
| Ignore 覆盖（仅被忽略） | `-X` | `-X` | N/A |
| 排除模式 | `-e` / `--exclude <pattern>`（可重复） | `-e <pattern>`（可重复） | N/A |
| 交互模式 | `-i` / `--interactive` | `-i` | N/A |
| Quiet 模式 | 全局 `-q` / `--quiet`（不暴露 clean 专属 `--quiet`；使用 `OutputConfig.quiet`） | `-q` / `--quiet` | N/A |
| 嵌套子仓双重 force | `-ff`（基于计数） | `-ff` | N/A |
| JSON 输出 | `--json` | 不支持 | N/A |
| Pathspec 过滤 | 不支持 | `<pathspec>...` | N/A |
| Require force 配置 | `clean.requireForce`（默认 true） | `clean.requireForce`（默认 true） | N/A |

注意：jj 没有 `clean` 命令，因为其工作副本模型会自动跟踪所有文件，未跟踪文件不是 jj 数据模型中的概念。

## 错误处理

| 场景 | StableErrorCode | 退出码 |
|----------|-----------------|------|
| 缺少 `-f` / `-n` / `-i`（且 `clean.requireForce` 非 `false`） | `LBR-CLI-002` | 129 |
| `-i` 与 `--json` 组合 | `LBR-CLI-002` | 129 |
| `-i` 与 `-n`（dry-run）组合 | `LBR-CLI-002` | 129 |
| `-x` 与 `-X` 组合 | `LBR-CLI-002` | 129 |
| 索引损坏或未跟踪扫描失败 | `LBR-IO-001` | 128 |
| 路径解析到工作树外部 | `LBR-CONFLICT-002` | 128 |
| 文件删除失败（容错——仍输出部分清单） | `LBR-IO-002` | 128 |

退出码为默认的*粗粒度*映射；设置 `LIBRA_FINE_EXIT_CODES=1` 可使用细粒度码（`LBR-CLI-002` → 2，等等）。`clap` 解析错误（未知标志）会直接以 `2` 退出，不经过 `CleanError`。
