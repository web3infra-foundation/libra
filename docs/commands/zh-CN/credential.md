# `libra credential`

Vault 支撑的 Git 凭证助手。在 stdin/stdout 上讲 Git 凭证 key/value 协议，凭证经仓库 vault 用 AES-256-GCM 加密存储 —— 凭证绝不以明文落盘。

## 用法

```
libra credential fill
libra credential store
libra credential erase
```

每个子命令从 stdin 读取 Git 凭证属性（`key=value` 行，空行终止）。

## 说明

- **`fill`** —— 打印所请求 `protocol`/`host`/`path` 的 `username`/`password`，或不打印。未命中（无条目、已过期、用户名不符、无 vault）与命中都退出 0、除输出外不可区分，故退出码绝不暴露凭证是否存在。
- **`store`** —— 加密并持久化 stdin 中的 `username`/`password`。尊重可选的 `password_expiry_utc`；不给则 30 天后过期。已过期的 `password_expiry_utc` 会被拒绝。
- **`erase`** —— 删除所请求上下文的凭证。

条目以 `protocol/host/path` 的 SHA-256 摘要为键，故 config 中绝不含明文 host 或用户名。每个 `protocol/host/path` 仅一条凭证；`fill` 带与存储不符的 `username=` 视为未命中。

**安全**：密码与 token 绝不被记录、trace（即便 `RUST_LOG=debug`）或回显在错误消息中 —— 错误仅提及非敏感路由上下文（`protocol://host`）。

## 配置为 Git 助手

```
[credential]
    helper = "!libra credential"
```

## 退出码

| 退出码 | 含义 |
|--------|------|
| `0` | 成功 —— `fill` 打印了匹配或为空；`store`/`erase` 完成。 |
| `128` | `store` 缺少 username/password、时间戳已过期，或 vault 未初始化；或请求不可读。 |

## 示例

```bash
# 存储凭证
printf 'protocol=https\nhost=example.com\nusername=alice\npassword=TOKEN\n' \
  | libra credential store

# 取回
printf 'protocol=https\nhost=example.com\n' | libra credential fill

# 删除
printf 'protocol=https\nhost=example.com\n' | libra credential erase
```

## 与 Git 对比

| 任务 | Libra | Git |
|------|-------|-----|
| Fill | `libra credential fill` | `git credential fill` |
| Store | `libra credential store` | `git credential-store store` |
| Erase | `libra credential erase` | `git credential-store erase` |

差异：存储为 vault 加密（非明文 `~/.git-credentials`）且**仓库范围**（vault unseal key 按仓库），条目带过期（默认 30 天），每个 `protocol/host/path` 仅一条凭证。未公开：`credential-cache`、每 host 多用户名、消费侧 `credential.helper` 链（Libra **是**助手，不调用外部助手）。
