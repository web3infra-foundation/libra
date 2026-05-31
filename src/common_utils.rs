//! Common helpers for formatting commit messages, parsing embedded GPG signatures, and
//! validating Conventional Commit styles.
//!
//! 用于格式化提交消息、解析嵌入式 GPG 签名和验证 Conventional Commit 样式的通用助手。
//!
//! This module is intentionally dependency-light so that it can be shared by both the CLI
//! command layer and lower-level repository code without introducing dependency cycles.
//! All functions here are pure (no I/O, no global state) and operate on string slices.
//!
//! 用于格式化提交消息、解析嵌入的 GPG 签名和验证约定式提交样式的通用帮助程序。
//!
//! 此模块有意保持依赖轻量，以便可以由 CLI 命令层和较低级别的存储库代码共享，而不会引入依赖
//! 循环。此处的所有函数都是纯函数（无 I/O、无全局状态）并在字符串切片上运行。

use std::sync::LazyLock;

use regex::Regex;

/// Build the canonical commit body that will be hashed into a commit object.
///
/// Functional scope:
/// - When `gpg_sig` is `None`, prepends a single blank line before `msg`. The leading
///   newline is required: remote `git unpack` fails when the blank-line separator is
///   missing.
/// - When `gpg_sig` is `Some(sig)`, places the signature first, then a single blank
///   line, then the user-provided message. The blank line separates signature trailers
///   from the message body and is mandated by the Git object format.
///
/// Boundary conditions:
/// - `msg` is not trimmed; trailing whitespace inside the message is preserved as-is.
/// - `gpg_sig` is treated as opaque text — no parsing is performed.
///
/// 构建将被哈希到提交对象中的规范提交正文。
///
/// 功能范围：
/// - 当 `gpg_sig` 为 `None` 时，在 `msg` 前附加单个空白行。前导换行符是必需的：当缺少空白行
///   分隔符时，远程 `git unpack` 会失败。
/// - 当 `gpg_sig` 为 `Some(sig)` 时，首先放置签名，然后是单个空白行，然后是用户提供的消息。
///   空白行将签名预告片与消息正文分开，由 Git 对象格式要求。
///
/// 边界条件：
/// - `msg` 未被修剪；消息内的尾部空格按原样保留。
/// - `gpg_sig` 被视为不透明文本 — 不执行解析。
pub fn format_commit_msg(msg: &str, gpg_sig: Option<&str>) -> String {
    match gpg_sig {
        None => {
            format!("\n{msg}")
        }
        Some(gpg) => {
            format!("{gpg}\n\n{msg}")
        }
    }
}

/// Split a stored commit body into `(message, optional_signature)`.
///
/// Functional scope:
/// - Detects an embedded `gpgsig` header at the start of the input (PGP or SSH).
/// - When a signature is present, returns the trimmed message body and a borrowed
///   slice covering only the signature block (without the leading `gpgsig ` prefix).
/// - When no signature header is found, returns the trimmed input as the message and
///   `None` for the signature.
///
/// Boundary conditions:
/// - The returned `&str` slices borrow from the original `msg_gpg` buffer; callers
///   must keep that buffer alive.
/// - Both PGP and SSH signature blocks are recognised; any other prefix (or a missing
///   prefix) is treated as a plain message.
/// - Leading whitespace on the message body is trimmed, but inner whitespace is kept
///   verbatim so that commit content survives a round-trip through this function.
///
/// 将存储的提交正文分解为 `(message, optional_signature)`。
///
/// 功能范围：
/// - 检测输入开头嵌入的 `gpgsig` 标头（PGP 或 SSH）。
/// - 当存在签名时，返回修剪的消息正文和仅覆盖签名块的借用切片（不带前导 `gpgsig ` 前缀）。
/// - 当未找到签名标头时，返回修剪的输入作为消息，签名的 `None`。
///
/// 边界条件：
/// - 返回的 `&str` 切片从原始 `msg_gpg` 缓冲区借用；调用者必须保持该缓冲区存活。
/// - PGP 和 SSH 签名块都被识别；任何其他前缀（或缺少前缀）都被视为纯消息。
/// - 消息正文上的前导空格被修剪，但内部空格逐字保留，以便提交内容在通过此函数的往返中存活。
pub fn parse_commit_msg(msg_gpg: &str) -> (&str, Option<&str>) {
    const SIG_PATTERN: &str = r"^gpgsig (-----BEGIN (?:PGP|SSH) SIGNATURE-----[\s\S]*?-----END (?:PGP|SSH) SIGNATURE-----)";
    const GPGSIG_PREFIX_LEN: usize = 7; // length of "gpgsig "
    static SIG_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        // INVARIANT: SIG_PATTERN is a validated regex literal checked in tests.
        Regex::new(SIG_PATTERN).expect("SIG_PATTERN must compile")
    });

    if let Some(caps) = SIG_REGEX.captures(msg_gpg) {
        // INVARIANT: SIG_PATTERN defines capture group 1 for the full signature body.
        let signature = caps
            .get(1)
            .expect("SIG_PATTERN must capture the signature body")
            .as_str();

        let msg = &msg_gpg[signature.len() + GPGSIG_PREFIX_LEN..].trim_start();
        (msg, Some(signature))
    } else {
        (msg_gpg.trim_start(), None)
    }
}

/// Check whether the first line of `msg` matches the Conventional Commits 1.0 grammar.
///
/// Functional scope:
/// - Only the *first* line (the subject) is validated. Body and footer text are
///   ignored, mirroring the Conventional Commits specification which only places
///   constraints on the subject line.
/// - Accepts the form `type(scope)?!?: description`, where `type` is restricted to
///   letters/digits/`_`/`-` and `scope`/`description` accept any visible Unicode.
///
/// Boundary conditions:
/// - Returns `false` for empty input, since `lines().next()` yields an empty subject
///   that cannot match the regex.
/// - The eight conventional types (`build`, `chore`, `ci`, `docs`, `feat`, `fix`,
///   `perf`, `refactor`) are recognised but **not** required — any non-empty type
///   token is accepted, matching the spec which only treats those names as
///   recommendations.
/// - The breaking-change marker `!` after `type` (or `(scope)`) is allowed but not
///   required.
///
/// Reference: <https://www.conventionalcommits.org/en/v1.0.0/>
///
/// 检查 `msg` 的第一行是否与约定式提交 1.0 语法匹配。
///
/// 功能范围：
/// - 仅验证第一行（主题）。正文和页脚文本被忽略，镜像约定式提交规范，该规范仅对主题行
///   施加约束。
/// - 接受格式 `type(scope)?!?: description`，其中 `type` 限于字母/数字/`_`/`-`，
///   `scope`/`description` 接受任何可见的 Unicode。
///
/// 边界条件：
/// - 对于空输入返回 `false`，因为 `lines().next()` 产生无法匹配正则表达式的空主题。
/// - 八个约定类型（`build`、`chore`、`ci`、`docs`、`feat`、`fix`、`perf`、`refactor`）被
///   识别但**不是**必需的 — 接受任何非空类型令牌，与仅将这些名称视为建议的规范匹配。
/// - 在 `type`（或 `(scope)`）之后的破坏性变更标记 `!` 被允许但不是必需的。
///
/// 参考：<https://www.conventionalcommits.org/en/v1.0.0/>
pub fn check_conventional_commits_message(msg: &str) -> bool {
    let first_line = msg.lines().next().unwrap_or_default();
    #[allow(unused_variables)]
    let body_footer = msg.lines().skip(1).collect::<Vec<_>>().join("\n");

    let unicode_pattern = r"\p{L}\p{N}\p{P}\p{S}\p{Z}";
    // type only support characters&numbers, others fields support all unicode characters
    let regex_str = format!(
        r"^(?P<type>[\p{{L}}\p{{N}}_-]+)(?:\((?P<scope>[{unicode_pattern}]+)\))?!?: (?P<description>[{unicode_pattern}]+)$",
    );

    // INVARIANT: regex_str is assembled from static, validated fragments.
    let re = Regex::new(&regex_str).expect("conventional commit regex must compile");
    const RECOMMENDED_TYPES: [&str; 8] = [
        "build", "chore", "ci", "docs", "feat", "fix", "perf", "refactor",
    ];

    if let Some(captures) = re.captures(first_line) {
        let commit_type = captures.name("type").map(|m| m.as_str().to_string());
        #[allow(unused_variables)]
        let scope = captures.name("scope").map(|m| m.as_str().to_string());
        let description = captures.name("description").map(|m| m.as_str().to_string());
        if commit_type.is_none() || description.is_none() {
            return false;
        }

        let Some(commit_type) = commit_type else {
            return false;
        };
        let _is_recommended = RECOMMENDED_TYPES.contains(&commit_type.to_lowercase().as_str());

        // println!("{}({}): {}\n{}", commit_type, scope.unwrap_or("None".to_string()), description.unwrap(), body_footer);

        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    //! `format_commit_msg` is used as a fixture builder in several
    //! command integration tests, but its exact output contract, the
    //! `parse_commit_msg` round-trip, and `check_conventional_commits_message`
    //! (which has no test references at all) were never directly
    //! asserted. These pins guard the commit-object byte format — the
    //! leading-newline / blank-line separators are load-bearing
    //! (a missing separator makes remote `git unpack` reject the
    //! object).
    //!
    //! `format_commit_msg` 在多个命令集成测试中用作夹具生成器，但其确切的输出合约、
    //! `parse_commit_msg` 往返和 `check_conventional_commits_message`（根本没有测试引用）
    //! 从未直接被声称。这些管脚保护提交对象字节格式 — 前导换行符/空白行分隔符是负载承载的
    //! （缺少分隔符会导致远程 `git unpack` 拒绝该对象）。

    use super::*;

    /// Without a signature the body is exactly `"\n{msg}"` — the
    /// leading blank line is required by the Git object format; remote
    /// `git unpack` fails without it.
    ///
    /// 测试场景：没有签名时，正文正好是 `"\n{msg}"` — Git 对象格式要求前导空白行；没有它远程
    /// `git unpack` 会失败。
    #[test]
    fn format_commit_msg_unsigned_prepends_blank_line() {
        assert_eq!(format_commit_msg("hello", None), "\nhello");
        // Inner / trailing whitespace in the message is preserved.
        assert_eq!(format_commit_msg("a\nb ", None), "\na\nb ");
    }

    /// With a signature the body is exactly `"{gpg}\n\n{msg}"` — the
    /// blank line separates the signature trailer from the message.
    ///
    /// 测试场景：有签名时，正文正好是 `"{gpg}\n\n{msg}"` — 空白行将签名预告片与消息分开。
    #[test]
    fn format_commit_msg_signed_places_sig_then_blank_line() {
        assert_eq!(
            format_commit_msg("subject", Some("gpgsig BLOCK")),
            "gpgsig BLOCK\n\nsubject",
        );
    }

    /// A plain (unsigned) body parses back to the trimmed message and
    /// `None`; leading whitespace is stripped but inner content is kept.
    ///
    /// 测试场景：纯（无签名）正文解析回修剪的消息和 `None`；前导空格被删除但内部内容被保留。
    #[test]
    fn parse_commit_msg_plain_message_has_no_signature() {
        assert_eq!(parse_commit_msg("  hello\nworld"), ("hello\nworld", None));
        assert_eq!(parse_commit_msg("subject"), ("subject", None));
    }

    /// A `gpgsig`-prefixed PGP signature block round-trips: the parsed
    /// signature is the BEGIN..END block (without the `gpgsig ` prefix)
    /// and the message is the trimmed remainder. Built via
    /// `format_commit_msg` so the format⇄parse pair is exercised
    /// together.
    ///
    /// 测试场景：`gpgsig` 前缀的 PGP 签名块往返：解析的签名是 BEGIN..END 块（不带 `gpgsig ` 前缀），
    /// 消息是修剪的余数。通过 `format_commit_msg` 构建，以便一起运行格式⇄解析对。
    #[test]
    fn parse_commit_msg_round_trips_pgp_signature() {
        let sig = "-----BEGIN PGP SIGNATURE-----\nabcDEF123\n-----END PGP SIGNATURE-----";
        let body = format_commit_msg("the subject", Some(&format!("gpgsig {sig}")));
        let (msg, parsed_sig) = parse_commit_msg(&body);
        assert_eq!(msg, "the subject");
        assert_eq!(parsed_sig, Some(sig));
    }

    /// SSH signature blocks are recognised the same way as PGP.
    ///
    /// 测试场景：SSH 签名块以与 PGP 相同的方式被识别。
    #[test]
    fn parse_commit_msg_recognises_ssh_signature() {
        let sig = "-----BEGIN SSH SIGNATURE-----\nU1NIU0lH\n-----END SSH SIGNATURE-----";
        let body = format!("gpgsig {sig}\n\nssh-signed subject");
        let (msg, parsed_sig) = parse_commit_msg(&body);
        assert_eq!(msg, "ssh-signed subject");
        assert_eq!(parsed_sig, Some(sig));
    }

    /// Conventional-commit subjects: accept `type: desc`, optional
    /// `(scope)` and breaking `!`.
    ///
    /// 测试场景：约定式提交主题：接受 `type: desc`、可选的 `(scope)` 和破坏性 `!`。
    #[test]
    fn conventional_commits_accepts_valid_subjects() {
        for ok in [
            "feat: add a thing",
            "fix(parser): handle empty input",
            "feat!: breaking change",
            "chore(api)!: drop field",
            "refactor: tidy module",
            // Only the first line matters; body is ignored.
            "docs: update readme\n\nlong body here",
            // Non-recommended type tokens are still accepted (spec only
            // *recommends* the eight canonical types).
            "wibble: custom type allowed",
        ] {
            assert!(
                check_conventional_commits_message(ok),
                "expected `{ok}` to be a valid conventional-commit subject",
            );
        }
    }

    /// Rejects subjects that break the grammar: empty, no `: `
    /// separator, empty description, or a leading space before the
    /// type.
    ///
    /// 测试场景：拒绝违反语法的主题：空、无 `: ` 分隔符、空描述或类型前的前导空格。
    #[test]
    fn conventional_commits_rejects_invalid_subjects() {
        for bad in [
            "",
            "just a plain message",
            "feat:no space after colon",
            "feat: ",            // empty description
            "feat",              // no colon at all
            " feat: leading sp", // leading space before type
            ": no type",         // empty type
        ] {
            assert!(
                !check_conventional_commits_message(bad),
                "expected `{bad}` to be rejected as a conventional-commit subject",
            );
        }
    }
}
