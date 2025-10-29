use anyhow::{Context, Result, format_err};
use std::process;

pub fn check_spawn(cmd: &mut process::Command, cmd_msg: &str) -> Result<process::Child> {
    cmd.spawn()
        .with_context(|| format!("failed to start {cmd_msg}"))
}

pub fn run_command(mut cmd: process::Command, cmd_msg: &str, stdin: Option<&[u8]>) -> Result<()> {
    if stdin.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }
    let mut child = check_spawn(&mut cmd, cmd_msg)?;
    if let Some(stdin) = stdin {
        use std::io::Write;
        child.stdin.as_mut().unwrap().write_all(stdin)?;
    }
    let status = child.wait()?;
    if !status.success() {
        Err(format_err!("command '{}' failed: {}", cmd_msg, status))
    } else {
        Ok(())
    }
}
