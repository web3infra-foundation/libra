//! Static command-safety checks for shell-like AI tool invocations.
//!
//! Boundary: checks identify obviously dangerous command forms before runtime launch,
//! but they are not a replacement for filesystem sandbox enforcement. Hardening tests
//! cover separators, redirects, destructive commands, and allowlisted cases.

use std::path::Path;

use shlex::split as shlex_split;
use tree_sitter::{Node, Parser, Tree};
use tree_sitter_bash::LANGUAGE as BASH;

pub fn is_known_safe_shell_command(command: &str) -> bool {
    if let Some(commands) = parse_word_only_commands_sequence(command) {
        return !commands.is_empty() && commands.iter().all(|cmd| is_safe_to_call_with_exec(cmd));
    }

    if contains_shell_metacharacters(command) {
        return false;
    }

    shlex_split(command)
        .map(|parts| !parts.is_empty() && is_safe_to_call_with_exec(&parts))
        .unwrap_or(false)
}

pub fn shell_command_might_be_dangerous(command: &str) -> bool {
    if let Some(commands) = parse_word_only_commands_sequence(command) {
        return commands
            .iter()
            .any(|cmd| is_dangerous_to_call_with_exec(cmd));
    }

    if contains_shell_metacharacters(command) {
        return true;
    }

    shlex_split(command)
        .map(|parts| !parts.is_empty() && is_dangerous_to_call_with_exec(&parts))
        .unwrap_or(true)
}

fn contains_shell_metacharacters(command: &str) -> bool {
    command.contains(['&', '|', ';', '$', '>', '<', '`', '(', ')', '{', '}'])
}

fn parse_word_only_commands_sequence(src: &str) -> Option<Vec<Vec<String>>> {
    let tree = try_parse_shell(src)?;
    try_parse_word_only_commands_sequence_tree(&tree, src)
}

fn try_parse_shell(src: &str) -> Option<Tree> {
    let lang = BASH.into();
    let mut parser = Parser::new();
    parser.set_language(&lang).ok()?;
    parser.parse(src, None)
}

fn try_parse_word_only_commands_sequence_tree(tree: &Tree, src: &str) -> Option<Vec<Vec<String>>> {
    if tree.root_node().has_error() {
        return None;
    }

    const ALLOWED_KINDS: &[&str] = &[
        "program",
        "list",
        "pipeline",
        "command",
        "command_name",
        "word",
        "string",
        "string_content",
        "raw_string",
        "number",
        "concatenation",
    ];
    const ALLOWED_PUNCT_TOKENS: &[&str] = &["&&", "||", ";", "|", "\"", "'"];

    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut stack = vec![root];
    let mut command_nodes = Vec::new();

    while let Some(node) = stack.pop() {
        let kind = node.kind();
        if node.is_named() {
            if !ALLOWED_KINDS.contains(&kind) {
                return None;
            }
            if kind == "command" {
                command_nodes.push(node);
            }
        } else {
            if kind.chars().any(|c| "&;|".contains(c)) && !ALLOWED_PUNCT_TOKENS.contains(&kind) {
                return None;
            }
            if !(ALLOWED_PUNCT_TOKENS.contains(&kind) || kind.trim().is_empty()) {
                return None;
            }
        }

        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    command_nodes.sort_by_key(Node::start_byte);

    let mut commands = Vec::new();
    for node in command_nodes {
        let words = parse_plain_command_from_node(node, src)?;
        commands.push(words);
    }

    Some(commands)
}

fn parse_plain_command_from_node(cmd: Node, src: &str) -> Option<Vec<String>> {
    if cmd.kind() != "command" {
        return None;
    }

    let mut words = Vec::new();
    let mut cursor = cmd.walk();
    for child in cmd.named_children(&mut cursor) {
        match child.kind() {
            "command_name" => {
                let word_node = child.named_child(0)?;
                if word_node.kind() != "word" {
                    return None;
                }
                words.push(word_node.utf8_text(src.as_bytes()).ok()?.to_owned());
            }
            "word" | "number" => {
                words.push(child.utf8_text(src.as_bytes()).ok()?.to_owned());
            }
            "string" => {
                words.push(parse_double_quoted_string(child, src)?);
            }
            "raw_string" => {
                words.push(parse_raw_string(child, src)?);
            }
            "concatenation" => {
                let mut concatenated = String::new();
                let mut concat_cursor = child.walk();
                for part in child.named_children(&mut concat_cursor) {
                    match part.kind() {
                        "word" | "number" => {
                            concatenated.push_str(part.utf8_text(src.as_bytes()).ok()?);
                        }
                        "string" => {
                            concatenated.push_str(&parse_double_quoted_string(part, src)?);
                        }
                        "raw_string" => {
                            concatenated.push_str(&parse_raw_string(part, src)?);
                        }
                        _ => return None,
                    }
                }
                if concatenated.is_empty() {
                    return None;
                }
                words.push(concatenated);
            }
            _ => return None,
        }
    }

    Some(words)
}

fn parse_double_quoted_string(node: Node, src: &str) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }

    let mut cursor = node.walk();
    for part in node.named_children(&mut cursor) {
        if part.kind() != "string_content" {
            return None;
        }
    }
    let raw = node.utf8_text(src.as_bytes()).ok()?;
    let stripped = raw
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))?;
    Some(stripped.to_string())
}

fn parse_raw_string(node: Node, src: &str) -> Option<String> {
    if node.kind() != "raw_string" {
        return None;
    }

    let raw_string = node.utf8_text(src.as_bytes()).ok()?;
    let stripped = raw_string
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''));
    stripped.map(str::to_owned)
}

fn is_safe_to_call_with_exec(command: &[String]) -> bool {
    let Some(cmd0) = command.first().map(String::as_str) else {
        return false;
    };

    match Path::new(cmd0).file_name().and_then(|osstr| osstr.to_str()) {
        Some(cmd) if cfg!(target_os = "linux") && matches!(cmd, "numfmt" | "tac") => true,
        Some(
            "cat" | "cd" | "cut" | "echo" | "expr" | "false" | "grep" | "head" | "id" | "ls" | "nl"
            | "paste" | "pwd" | "rev" | "seq" | "stat" | "tail" | "tr" | "true" | "uname" | "uniq"
            | "wc" | "which" | "whoami",
        ) => true,
        Some("base64") => {
            const UNSAFE_BASE64_OPTIONS: &[&str] = &["-o", "--output"];
            !command.iter().skip(1).any(|arg| {
                UNSAFE_BASE64_OPTIONS.contains(&arg.as_str())
                    || arg.starts_with("--output=")
                    || (arg.starts_with("-o") && arg != "-o")
            })
        }
        Some("find") => {
            const UNSAFE_FIND_OPTIONS: &[&str] = &[
                "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fls", "-fprint", "-fprint0",
                "-fprintf",
            ];

            !command
                .iter()
                .any(|arg| UNSAFE_FIND_OPTIONS.contains(&arg.as_str()))
        }
        Some("rg") => {
            const UNSAFE_RIPGREP_OPTIONS_WITH_ARGS: &[&str] = &["--pre", "--hostname-bin"];
            const UNSAFE_RIPGREP_OPTIONS_WITHOUT_ARGS: &[&str] = &["--search-zip", "-z"];

            !command.iter().any(|arg| {
                UNSAFE_RIPGREP_OPTIONS_WITHOUT_ARGS.contains(&arg.as_str())
                    || UNSAFE_RIPGREP_OPTIONS_WITH_ARGS
                        .iter()
                        .any(|opt| arg == opt || arg.starts_with(&format!("{opt}=")))
            })
        }
        Some("git") => {
            if git_has_config_override_global_option(command) {
                return false;
            }

            let Some((subcommand_idx, subcommand)) =
                find_git_subcommand(command, &["status", "log", "diff", "show", "branch"])
            else {
                return false;
            };

            let subcommand_args = &command[subcommand_idx + 1..];
            match subcommand {
                "status" | "log" | "diff" | "show" => {
                    git_subcommand_args_are_read_only(subcommand_args)
                }
                "branch" => {
                    git_subcommand_args_are_read_only(subcommand_args)
                        && git_branch_is_read_only(subcommand_args)
                }
                _ => false,
            }
        }
        Some("sed")
            if command.len() <= 4
                && command.get(1).map(String::as_str) == Some("-n")
                && is_valid_sed_n_arg(command.get(2).map(String::as_str)) =>
        {
            true
        }
        _ => false,
    }
}

fn is_git_global_option_with_value(arg: &str) -> bool {
    matches!(
        arg,
        "-C" | "-c"
            | "--config-env"
            | "--exec-path"
            | "--git-dir"
            | "--namespace"
            | "--super-prefix"
            | "--work-tree"
    )
}

fn is_git_global_option_with_inline_value(arg: &str) -> bool {
    matches!(
        arg,
        s if s.starts_with("--config-env=")
            || s.starts_with("--exec-path=")
            || s.starts_with("--git-dir=")
            || s.starts_with("--namespace=")
            || s.starts_with("--super-prefix=")
            || s.starts_with("--work-tree=")
    ) || ((arg.starts_with("-C") || arg.starts_with("-c")) && arg.len() > 2)
}

fn find_git_subcommand<'a>(
    command: &'a [String],
    subcommands: &[&str],
) -> Option<(usize, &'a str)> {
    let cmd0 = command.first().map(String::as_str)?;
    if !cmd0.ends_with("git") {
        return None;
    }

    let mut skip_next = false;
    for (idx, arg) in command.iter().enumerate().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }

        let arg = arg.as_str();
        if is_git_global_option_with_inline_value(arg) {
            continue;
        }
        if is_git_global_option_with_value(arg) {
            skip_next = true;
            continue;
        }
        if arg == "--" || arg.starts_with('-') {
            continue;
        }

        if subcommands.contains(&arg) {
            return Some((idx, arg));
        }
        return None;
    }

    None
}

fn git_branch_is_read_only(branch_args: &[String]) -> bool {
    if branch_args.is_empty() {
        return true;
    }

    let mut saw_read_only_flag = false;
    for arg in branch_args.iter().map(String::as_str) {
        match arg {
            "--list" | "-l" | "--show-current" | "-a" | "--all" | "-r" | "--remotes" | "-v"
            | "-vv" | "--verbose" => {
                saw_read_only_flag = true;
            }
            _ if arg.starts_with("--format=") => {
                saw_read_only_flag = true;
            }
            _ => {
                return false;
            }
        }
    }

    saw_read_only_flag
}

fn git_has_config_override_global_option(command: &[String]) -> bool {
    command.iter().map(String::as_str).any(|arg| {
        matches!(arg, "-c" | "--config-env")
            || (arg.starts_with("-c") && arg.len() > 2)
            || arg.starts_with("--config-env=")
    })
}

fn git_subcommand_args_are_read_only(args: &[String]) -> bool {
    const UNSAFE_GIT_FLAGS: &[&str] = &[
        "--output",
        "--ext-diff",
        "--textconv",
        "--exec",
        "--paginate",
    ];

    !args.iter().map(String::as_str).any(|arg| {
        UNSAFE_GIT_FLAGS.contains(&arg)
            || arg.starts_with("--output=")
            || arg.starts_with("--exec=")
    })
}

fn is_dangerous_to_call_with_exec(command: &[String]) -> bool {
    let cmd0 = command.first().map(String::as_str);

    match cmd0 {
        Some(cmd) if cmd.ends_with("git") => {
            let Some((subcommand_idx, subcommand)) =
                find_git_subcommand(command, &["reset", "rm", "branch", "push", "clean"])
            else {
                return false;
            };

            match subcommand {
                "reset" | "rm" => true,
                "branch" => git_branch_is_delete(&command[subcommand_idx + 1..]),
                "push" => git_push_is_dangerous(&command[subcommand_idx + 1..]),
                "clean" => git_clean_is_force(&command[subcommand_idx + 1..]),
                _ => false,
            }
        }
        Some("rm") => matches!(command.get(1).map(String::as_str), Some("-f" | "-rf")),
        Some("sudo") => is_dangerous_to_call_with_exec(&command[1..]),
        _ => false,
    }
}

fn git_branch_is_delete(branch_args: &[String]) -> bool {
    branch_args.iter().map(String::as_str).any(|arg| {
        matches!(arg, "-d" | "-D" | "--delete")
            || arg.starts_with("--delete=")
            || short_flag_group_contains(arg, 'd')
            || short_flag_group_contains(arg, 'D')
    })
}

fn short_flag_group_contains(arg: &str, target: char) -> bool {
    arg.starts_with('-') && !arg.starts_with("--") && arg.chars().skip(1).any(|c| c == target)
}

fn git_push_is_dangerous(push_args: &[String]) -> bool {
    push_args.iter().map(String::as_str).any(|arg| {
        matches!(
            arg,
            "--force" | "--force-with-lease" | "--force-if-includes" | "--delete" | "-f" | "-d"
        ) || arg.starts_with("--force-with-lease=")
            || arg.starts_with("--force-if-includes=")
            || arg.starts_with("--delete=")
            || short_flag_group_contains(arg, 'f')
            || short_flag_group_contains(arg, 'd')
            || git_push_refspec_is_dangerous(arg)
    })
}

fn git_push_refspec_is_dangerous(arg: &str) -> bool {
    (arg.starts_with('+') || arg.starts_with(':')) && arg.len() > 1
}

fn git_clean_is_force(clean_args: &[String]) -> bool {
    clean_args.iter().map(String::as_str).any(|arg| {
        matches!(arg, "--force" | "-f")
            || arg.starts_with("--force=")
            || short_flag_group_contains(arg, 'f')
    })
}

fn is_valid_sed_n_arg(arg: Option<&str>) -> bool {
    let s = match arg {
        Some(s) => s,
        None => return false,
    };

    let core = match s.strip_suffix('p') {
        Some(rest) => rest,
        None => return false,
    };

    let parts: Vec<&str> = core.split(',').collect();
    match parts.as_slice() {
        [num] => !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()),
        [a, b] => {
            !a.is_empty()
                && !b.is_empty()
                && a.chars().all(|c| c.is_ascii_digit())
                && b.chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_known_safe_shell_command, shell_command_might_be_dangerous};

    #[test]
    fn safe_commands_are_detected() {
        assert!(is_known_safe_shell_command("ls -la"));
        assert!(is_known_safe_shell_command("git status"));
        assert!(is_known_safe_shell_command("ls && pwd"));
    }

    #[test]
    fn dangerous_commands_are_detected() {
        assert!(shell_command_might_be_dangerous("git reset --hard"));
        assert!(shell_command_might_be_dangerous("rm -rf target"));
        assert!(shell_command_might_be_dangerous(
            "echo hi && git push --force"
        ));
        assert!(shell_command_might_be_dangerous("echo $(cat /etc/passwd)"));
    }

    #[test]
    fn regular_commands_are_not_auto_safe() {
        assert!(!is_known_safe_shell_command("python script.py"));
        assert!(!shell_command_might_be_dangerous("python script.py"));
    }
}
