pub const ALLOWED_COMMANDS: &[&str] = &[
    "status", "diff", "branch", "log", "show", "show-ref", "add", "commit", "switch",
];

pub const ALLOWED_COMMANDS_DISPLAY: &str =
    "status, diff, branch, log, show, show-ref, add, commit, switch";

pub fn run_libra_vcs_tool_guidance() -> String {
    format!(
        "Allowed run_libra_vcs commands: {ALLOWED_COMMANDS_DISPLAY}. Pass flags and paths in \
         args. For working tree state prefer `status --json` or `status --porcelain v2 \
         --untracked-files=all`. For raw file discovery use workspace file tools instead of \
         `ls-files`. Do not use Git-only forms like `ls-files`, `status -uall`, or `status -a`."
    )
}

pub fn unsupported_command_message(prefix: &str, command: &str) -> String {
    format!(
        "unsupported {prefix} command '{command}'; allowed commands: {ALLOWED_COMMANDS_DISPLAY}. \
         For working tree state use `status --json` or `status --porcelain v2 \
         --untracked-files=all`. For raw file discovery use workspace file tools instead of \
         `ls-files`."
    )
}

pub fn normalize_tool_args(command: &str, args: &[String]) -> Result<Vec<String>, String> {
    if command != "status" {
        return Ok(args.to_vec());
    }

    let mut normalized = Vec::with_capacity(args.len());
    for arg in args {
        match arg.as_str() {
            "-uall" => normalized.push("--untracked-files=all".to_string()),
            "-unormal" => normalized.push("--untracked-files=normal".to_string()),
            "-uno" => normalized.push("--untracked-files=no".to_string()),
            "-a" => {
                return Err(
                    "run_libra_vcs status does not support '-a'; use '--untracked-files=all' \
                     when you need every untracked file listed"
                        .to_string(),
                );
            }
            _ => normalized.push(arg.clone()),
        }
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_guidance_mentions_allowed_commands_and_git_only_forms() {
        let guidance = run_libra_vcs_tool_guidance();

        assert!(guidance.contains(ALLOWED_COMMANDS_DISPLAY));
        assert!(guidance.contains("status --json"));
        assert!(guidance.contains("ls-files"));
        assert!(guidance.contains("status -uall"));
    }

    #[test]
    fn unsupported_command_message_is_actionable() {
        let message = unsupported_command_message("run_libra_vcs", "ls-files");

        assert!(message.contains("allowed commands"));
        assert!(message.contains("status --json"));
        assert!(message.contains("workspace file tools"));
    }

    #[test]
    fn normalize_status_args_rewrites_git_untracked_shorthand() {
        let args = normalize_tool_args("status", &["-uall".to_string()]).unwrap();

        assert_eq!(args, vec!["--untracked-files=all"]);
    }

    #[test]
    fn normalize_status_args_rejects_invalid_status_a_with_hint() {
        let error = normalize_tool_args("status", &["-a".to_string()]).unwrap_err();

        assert!(error.contains("--untracked-files=all"));
    }
}
