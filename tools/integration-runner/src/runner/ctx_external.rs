use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

use anyhow::{Context, Result, bail};

use super::types::{CommandRecord, ScenarioCtx};
use crate::support::{ensure_no_secret_leak, redact, tail};

impl<'a> ScenarioCtx<'a> {
    pub(crate) fn gitfix(
        &mut self,
        args: &[&str],
        cwd: PathBuf,
        expect_success: bool,
    ) -> Result<Output> {
        fs::create_dir_all(&cwd).with_context(|| format!("create cwd {}", cwd.display()))?;
        self.seq += 1;
        let seq = self.seq;
        let scenario_log_dir = self.run.run_root.join("logs").join(&self.id);
        fs::create_dir_all(&scenario_log_dir)
            .with_context(|| format!("create log dir {}", scenario_log_dir.display()))?;

        let mut cmd = Command::new("git");
        cmd.args(args)
            .current_dir(&cwd)
            .env_clear()
            .env("PATH", &self.run.safe_path)
            .env("HOME", self.run.run_root.join("home"))
            .env("USERPROFILE", self.run.run_root.join("home"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("TMPDIR", self.run.run_root.join("tmp"))
            .env("GIT_AUTHOR_NAME", "Libra Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "Libra Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .env("LANG", "C")
            .env("LC_ALL", "C");
        if let Ok(sock) = std::env::var("SSH_AUTH_SOCK") {
            cmd.env("SSH_AUTH_SOCK", sock);
        }
        let output = cmd
            .output()
            .with_context(|| format!("spawn git {}", args.join(" ")))?;

        let raw_stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let raw_stderr = String::from_utf8_lossy(&output.stderr).to_string();
        ensure_no_secret_leak(seq, &self.id, &raw_stdout, &raw_stderr)?;

        let stdout_log = scenario_log_dir.join(format!("{seq:03}.stdout"));
        let stderr_log = scenario_log_dir.join(format!("{seq:03}.stderr"));
        let exit_log = scenario_log_dir.join(format!("{seq:03}.exit"));
        let cmd_log = scenario_log_dir.join(format!("{seq:03}.cmd"));
        fs::write(&stdout_log, redact(&raw_stdout))?;
        fs::write(&stderr_log, redact(&raw_stderr))?;
        fs::write(&exit_log, format!("{:?}\n", output.status.code()))?;
        fs::write(
            &cmd_log,
            format!(
                "scenario={} cwd={} binary=git args={:?} env=gitfix\n",
                self.id,
                cwd.display(),
                args
            ),
        )?;

        self.commands.push(CommandRecord {
            seq,
            command: std::iter::once("git".to_string())
                .chain(args.iter().map(|arg| (*arg).to_string()))
                .collect(),
            cwd: cwd.display().to_string(),
            exit_code: output.status.code(),
            success: output.status.success(),
            stdout_log: stdout_log.display().to_string(),
            stderr_log: stderr_log.display().to_string(),
            stderr_tail: tail(&redact(&raw_stderr), 1200),
        });

        if expect_success && !output.status.success() {
            bail!(
                "git {} failed with {:?}: {}",
                args.join(" "),
                output.status.code(),
                tail(&raw_stderr, 1200)
            );
        }
        if !expect_success && output.status.success() {
            bail!("git {} unexpectedly succeeded", args.join(" "));
        }
        Ok(output)
    }
    pub(crate) fn gh(
        &mut self,
        args: &[&str],
        cwd: PathBuf,
        expect_success: bool,
    ) -> Result<Output> {
        fs::create_dir_all(&cwd).with_context(|| format!("create cwd {}", cwd.display()))?;
        self.seq += 1;
        let seq = self.seq;
        let scenario_log_dir = self.run.run_root.join("logs").join(&self.id);
        fs::create_dir_all(&scenario_log_dir)
            .with_context(|| format!("create log dir {}", scenario_log_dir.display()))?;

        // Do NOT env_clear: gh CLI auth lives in the caller's real HOME (~/.config/gh or
        // OS keychain). We inherit the runner's env (user's) so `gh auth status` / create work.
        // cwd is still forced for reproducibility. Outputs are logged + redacted.
        let output = Command::new("gh")
            .args(args)
            .current_dir(&cwd)
            .output()
            .with_context(|| format!("spawn gh {}", args.join(" ")))?;

        let raw_stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let raw_stderr = String::from_utf8_lossy(&output.stderr).to_string();
        ensure_no_secret_leak(seq, &self.id, &raw_stdout, &raw_stderr)?;

        let stdout_log = scenario_log_dir.join(format!("{seq:03}.stdout"));
        let stderr_log = scenario_log_dir.join(format!("{seq:03}.stderr"));
        let exit_log = scenario_log_dir.join(format!("{seq:03}.exit"));
        let cmd_log = scenario_log_dir.join(format!("{seq:03}.cmd"));
        fs::write(&stdout_log, redact(&raw_stdout))?;
        fs::write(&stderr_log, redact(&raw_stderr))?;
        fs::write(&exit_log, format!("{:?}\n", output.status.code()))?;
        fs::write(
            &cmd_log,
            format!(
                "scenario={} cwd={} binary=gh args={:?} env=host-gh (live only)\n",
                self.id,
                cwd.display(),
                args
            ),
        )?;

        let record = CommandRecord {
            seq,
            command: std::iter::once("gh".to_string())
                .chain(args.iter().map(|arg| (*arg).to_string()))
                .collect(),
            cwd: cwd.display().to_string(),
            exit_code: output.status.code(),
            success: output.status.success(),
            stdout_log: stdout_log.display().to_string(),
            stderr_log: stderr_log.display().to_string(),
            stderr_tail: tail(&redact(&raw_stderr), 1200),
        };
        self.commands.push(record);

        if expect_success && !output.status.success() {
            bail!(
                "gh {} failed with {:?}: {}",
                args.join(" "),
                output.status.code(),
                tail(&raw_stderr, 1200)
            );
        }
        if !expect_success && output.status.success() {
            bail!("gh {} unexpectedly succeeded", args.join(" "));
        }
        Ok(output)
    }
}
