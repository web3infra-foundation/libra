# Show-Ref 命令改进详细计划

## 所属批次

第七批：轻量命令与底层契约收口（P2）

## 已完成前置条件与当前代码状态

> **实施状态：✅ 已落地（2026-06-08 Git-parity 收口）** — `show-ref` 现已支持 JSON / machine 输出、Git 风格 path-segment suffix pattern、`--exists <ref>` 原始 reference 行存在性探测、`--verify` 精确 full-refname 模式、`-d` / `--dereference` annotated tag peeled `^{}` 行、remote-tracking refs、稳定错误码和命令文档。整体兼容 tier 仍是 `partial`，因为 `--exclude-existing` 与 `--abbrev` / `--hash=<n>` 位宽控制仍未实现。

### 已确认落地的基线
- `show-ref` 已具备 JSON / machine 输出
- 主要 refs 读取错误已绑定稳定错误码
- `docs/commands/show-ref.md` 已补命令契约
- `tests/command/show_ref_test.rs` 已补 JSON 输出回归
- remote-tracking refs 已输出为 `refs/remotes/<remote>/<branch>`，human 与 JSON 回归均已覆盖
- `--exists <ref>` 按原始 reference 行核对，不解析目标对象；命中 exit 0 且无输出，缺失 exit 2
- `--verify` 精确匹配完整 refname；缺失 ref 默认 exit 128，`--quiet` exit 1 且静默
- `-d` / `--dereference` 对 annotated tags 输出 peeled `^{}` 行，lightweight tags 保持单行
- pattern 过滤已按 Git 风格改为完整 refname path-segment suffix 匹配

### 基于当前代码的 Review 结论
- 实现层已覆盖本计划核心兼容项；剩余缺口是 Git 的 stdin `--exclude-existing` 过滤模式和 hash abbreviation width 控制。

## 目标与非目标

**已完成目标：**
- 命令文档同步
- JSON 回归测试
- README 计划状态同步
- `--exists <ref>` 按原始 reference 行核对，不解析目标对象
- `--verify` 精确 full-refname 验证，缺失 ref 默认退出 128，`--quiet` 退出 1
- `-d` / `--dereference` 为 annotated tags 输出 peeled `^{}` 行，lightweight tags 保持单行
- pattern 过滤已按 Git 风格改为 refname path-segment suffix 匹配，避免 `main` 命中 `main-2`

**后续维护目标：**
- 新增 ref 类型时继续以向后兼容方式扩展 `entries`
- 若后续实现 `--exclude-existing` 或 `--hash=<n>`，需要同步 `COMPATIBILITY.md`、`docs/commands/show-ref.md`、`SHOW_REF_EXAMPLES` 和专项 tests

**本批非目标：**
- 不引入 Git `show-ref --exclude-existing`
- 暂不实现 `--abbrev` / `--hash=<n>` 宽度控制

## 验证方式

1. `cargo +nightly fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test show_ref_test`
4. `docs/commands/show-ref.md` 与命令输出保持一致
