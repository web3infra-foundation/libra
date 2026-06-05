use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

use anyhow::{Context, Result, bail};

use super::types::{CommandRecord, ScenarioCtx};
use crate::support::{ensure_no_secret_leak, redact, tail};

impl<'a> ScenarioCtx<'a> {
    pub(crate) fn command(
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

        let mut cmd = Command::new(&self.run.binary);
        cmd.args(args)
            .current_dir(&cwd)
            .env_clear()
            .env("PATH", &self.run.safe_path)
            .env("HOME", self.run.run_root.join("home"))
            .env("USERPROFILE", self.run.run_root.join("home"))
            .env("XDG_CONFIG_HOME", self.run.run_root.join("xdg-config"))
            .env("XDG_CACHE_HOME", self.run.run_root.join("xdg-cache"))
            .env("TMPDIR", self.run.run_root.join("tmp"))
            .env(
                "LIBRA_CONFIG_GLOBAL_DB",
                self.run.run_root.join("home/.libra/config.db"),
            )
            .env("LIBRA_TEST", "1")
            .env("LANG", "C")
            .env("LC_ALL", "C");
        if let Ok(sock) = std::env::var("SSH_AUTH_SOCK") {
            cmd.env("SSH_AUTH_SOCK", sock);
        }
        let output = cmd
            .output()
            .with_context(|| format!("spawn libra {}", args.join(" ")))?;

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
                "scenario={} cwd={} binary={} args={:?} env=isolated\n",
                self.id,
                cwd.display(),
                self.run.binary.display(),
                args
            ),
        )?;

        let record = CommandRecord {
            seq,
            command: args.iter().map(|arg| (*arg).to_string()).collect(),
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
                "libra {} failed with {:?}: {}",
                args.join(" "),
                output.status.code(),
                tail(&raw_stderr, 1200)
            );
        }
        if !expect_success && output.status.success() {
            bail!("libra {} unexpectedly succeeded", args.join(" "));
        }
        Ok(output)
    }
    pub(crate) fn command_with_stdin(
        &mut self,
        args: &[&str],
        cwd: PathBuf,
        stdin_body: &str,
        expect_success: bool,
    ) -> Result<Output> {
        fs::create_dir_all(&cwd).with_context(|| format!("create cwd {}", cwd.display()))?;
        self.seq += 1;
        let seq = self.seq;
        let scenario_log_dir = self.run.run_root.join("logs").join(&self.id);
        fs::create_dir_all(&scenario_log_dir)
            .with_context(|| format!("create log dir {}", scenario_log_dir.display()))?;

        let mut cmd = Command::new(&self.run.binary);
        cmd.args(args)
            .current_dir(&cwd)
            .env_clear()
            .env("PATH", &self.run.safe_path)
            .env("HOME", self.run.run_root.join("home"))
            .env("USERPROFILE", self.run.run_root.join("home"))
            .env("XDG_CONFIG_HOME", self.run.run_root.join("xdg-config"))
            .env("XDG_CACHE_HOME", self.run.run_root.join("xdg-cache"))
            .env("TMPDIR", self.run.run_root.join("tmp"))
            .env(
                "LIBRA_CONFIG_GLOBAL_DB",
                self.run.run_root.join("home/.libra/config.db"),
            )
            .env("LIBRA_TEST", "1")
            .env("LANG", "C")
            .env("LC_ALL", "C");
        if let Ok(sock) = std::env::var("SSH_AUTH_SOCK") {
            cmd.env("SSH_AUTH_SOCK", sock);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn libra {}", args.join(" ")))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(stdin_body.as_bytes())
                .context("write libra stdin")?;
        }
        let output = child
            .wait_with_output()
            .with_context(|| format!("wait for libra {}", args.join(" ")))?;

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
                "scenario={} cwd={} binary={} args={:?} stdin=<{} bytes> env=isolated\n",
                self.id,
                cwd.display(),
                self.run.binary.display(),
                args,
                stdin_body.len()
            ),
        )?;

        self.commands.push(CommandRecord {
            seq,
            command: args.iter().map(|arg| (*arg).to_string()).collect(),
            cwd: cwd.display().to_string(),
            exit_code: output.status.code(),
            success: output.status.success(),
            stdout_log: stdout_log.display().to_string(),
            stderr_log: stderr_log.display().to_string(),
            stderr_tail: tail(&redact(&raw_stderr), 1200),
        });

        if expect_success && !output.status.success() {
            bail!(
                "libra {} failed with {:?}: {}",
                args.join(" "),
                output.status.code(),
                tail(&raw_stderr, 1200)
            );
        }
        if !expect_success && output.status.success() {
            bail!("libra {} unexpectedly succeeded", args.join(" "));
        }
        Ok(output)
    }
}
