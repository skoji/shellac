//! The save engine under test. All save operations shell out to an external
//! CLI implementing the add / remove / loop-one / modify-comment /
//! add-multiline contract; the harness never mutates PDFs in-process.

use crate::geom::Rect;
use crate::proc::run;

pub trait SaveEngine {
    fn name(&self) -> String;
    fn add(&self, path: &str, hl: Rect, ul: Rect) -> Result<(), String>;
    fn remove(&self, path: &str) -> Result<(), String>;
    fn loop_one(&self, path: &str, i: i64, r: Rect) -> Result<(), String>;
    fn modify_comment(&self, path: &str, new_contents: &str) -> Result<(), String>;
    fn add_multiline(&self, path: &str, quads: &str) -> Result<(), String>;
}

pub struct CliEngine {
    pub cmd: String,
}

impl CliEngine {
    fn run_args(&self, args: &[&str]) -> Result<(), String> {
        let r = run(&self.cmd, args);
        match &r.err {
            Some(e) => Err(format!(
                "{} {}: {}\n{}{}",
                self.cmd,
                args.join(" "),
                e,
                r.stdout_str(),
                r.stderr_str()
            )),
            None => Ok(()),
        }
    }
}

impl SaveEngine for CliEngine {
    fn name(&self) -> String {
        format!("cli: {}", self.cmd)
    }

    fn add(&self, path: &str, hl: Rect, ul: Rect) -> Result<(), String> {
        self.run_args(&["add", path, "--hl-rect", &hl.csv(), "--ul-rect", &ul.csv()])
    }

    fn remove(&self, path: &str) -> Result<(), String> {
        self.run_args(&["remove", path])
    }

    fn loop_one(&self, path: &str, i: i64, r: Rect) -> Result<(), String> {
        self.run_args(&["loop-one", &format!("{i}"), path, "--rect", &r.csv()])
    }

    fn modify_comment(&self, path: &str, new_contents: &str) -> Result<(), String> {
        self.run_args(&["modify-comment", new_contents, path])
    }

    fn add_multiline(&self, path: &str, quads: &str) -> Result<(), String> {
        self.run_args(&["add-multiline", path, "--hl-quads", quads])
    }
}
