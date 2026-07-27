//! Thin external-process wrapper. No timeouts; stdout/stderr fully
//! buffered; stdin connected to the null device.

use std::ffi::OsStr;
use std::process::{Command, Stdio};

pub struct RunResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Exit code when the process ran and exited normally.
    pub code: Option<i32>,
    /// `Some(message)` when the process could not be spawned or exited
    /// non-zero. The message mirrors classic process-error phrasing
    /// ("exit status N") because some judgment strings embed it.
    pub err: Option<String>,
}

impl RunResult {
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

pub fn run<S: AsRef<OsStr>>(name: impl AsRef<OsStr>, args: &[S]) -> RunResult {
    match Command::new(name.as_ref())
        .args(args)
        .stdin(Stdio::null())
        .output()
    {
        Ok(out) => {
            let code = out.status.code();
            let err = if out.status.success() {
                None
            } else {
                Some(match code {
                    Some(c) => format!("exit status {c}"),
                    None => format!("terminated abnormally: {}", out.status),
                })
            };
            RunResult {
                stdout: out.stdout,
                stderr: out.stderr,
                code,
                err,
            }
        }
        Err(e) => RunResult {
            stdout: Vec::new(),
            stderr: Vec::new(),
            code: None,
            err: Some(e.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_command_has_no_err() {
        let r = run("true", &[] as &[&str]);
        assert!(r.err.is_none());
        assert_eq!(r.code, Some(0));
    }

    #[test]
    fn nonzero_exit_is_reported_as_exit_status() {
        let r = run("false", &[] as &[&str]);
        assert_eq!(r.err.as_deref(), Some("exit status 1"));
        assert_eq!(r.code, Some(1));
    }

    #[test]
    fn missing_binary_reports_spawn_error() {
        let r = run("/nonexistent/definitely-not-a-binary", &[] as &[&str]);
        assert!(r.err.is_some());
        assert_eq!(r.code, None);
    }

    #[test]
    fn captures_stdout() {
        let r = run("echo", &["hello"]);
        assert_eq!(r.stdout_str(), "hello\n");
    }
}
