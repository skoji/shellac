//! The `verify gate` exit-code contract. CI distinguishes "the matrix found
//! something new" from "the run could not be set up", so the two must not
//! share a status:
//!
//!   0  every failing cell is covered by the registry (or there were none)
//!   1  a setup or IO error
//!   2  a usage error
//!   3  at least one unknown failure

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn committed_exceptions() -> String {
    repo()
        .join("corpus/known-exceptions.json")
        .to_string_lossy()
        .into_owned()
}

fn tmp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("verify-gate-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_fails(dir: &Path, cells_json: &str) -> String {
    let p = dir.join("fails.json");
    std::fs::write(&p, format!(r#"{{"version":1,"cells":{cells_json}}}"#)).unwrap();
    p.to_string_lossy().into_owned()
}

fn exit_code(args: &[&str]) -> i32 {
    Command::new(env!("CARGO_BIN_EXE_verify"))
        .args(args)
        .output()
        .expect("running verify")
        .status
        .code()
        .expect("verify exited without a status code")
}

const KNOWN_CELL: &str = r#"[{"sample":"S1","check":"C11a","scenario":"add","detail":"mismatch"}]"#;
const UNKNOWN_CELL: &str =
    r#"[{"sample":"S2","check":"C1","scenario":"add","detail":"prefix diverges"}]"#;

#[test]
fn no_failing_cells_at_all_exits_zero() {
    let dir = tmp_dir("empty");
    let fails = write_fails(&dir, "[]");
    assert_eq!(
        exit_code(&[
            "gate",
            "--fails",
            &fails,
            "--exceptions",
            &committed_exceptions()
        ]),
        0
    );
}

#[test]
fn failures_the_registry_covers_exit_zero() {
    let dir = tmp_dir("known");
    let fails = write_fails(&dir, KNOWN_CELL);
    assert_eq!(
        exit_code(&[
            "gate",
            "--fails",
            &fails,
            "--exceptions",
            &committed_exceptions()
        ]),
        0
    );
}

#[test]
fn an_unknown_failure_exits_three() {
    let dir = tmp_dir("unknown");
    let fails = write_fails(&dir, UNKNOWN_CELL);
    assert_eq!(
        exit_code(&[
            "gate",
            "--fails",
            &fails,
            "--exceptions",
            &committed_exceptions()
        ]),
        3
    );
}

#[test]
fn a_fatal_cell_exits_three_even_though_the_registry_has_wildcards() {
    let dir = tmp_dir("fatal");
    let fails = write_fails(
        &dir,
        r#"[{"sample":"S1","check":"fatal","scenario":"loop","detail":"engine exited 1"}]"#,
    );
    assert_eq!(
        exit_code(&[
            "gate",
            "--fails",
            &fails,
            "--exceptions",
            &committed_exceptions()
        ]),
        3
    );
}

#[test]
fn an_unreadable_input_exits_one() {
    let dir = tmp_dir("missing");
    let absent = dir.join("nope.json").to_string_lossy().into_owned();
    assert_eq!(
        exit_code(&[
            "gate",
            "--fails",
            &absent,
            "--exceptions",
            &committed_exceptions()
        ]),
        1,
        "a missing fails file is a setup error, not a verification failure"
    );

    let fails = write_fails(&dir, "[]");
    let absent_list = dir.join("nolist.json").to_string_lossy().into_owned();
    assert_eq!(
        exit_code(&["gate", "--fails", &fails, "--exceptions", &absent_list]),
        1
    );
}

#[test]
fn a_malformed_input_exits_one() {
    let dir = tmp_dir("malformed");
    let p = dir.join("fails.json");
    std::fs::write(&p, "{not json").unwrap();
    assert_eq!(
        exit_code(&[
            "gate",
            "--fails",
            &p.to_string_lossy(),
            "--exceptions",
            &committed_exceptions()
        ]),
        1
    );
}

#[test]
fn a_usage_error_exits_two() {
    assert_eq!(exit_code(&["gate"]), 2);
    assert_eq!(exit_code(&["gate", "--fails", "x.json"]), 2);
    assert_eq!(
        exit_code(&[
            "gate",
            "--fails",
            "x.json",
            "--exceptions",
            "y.json",
            "--bogus"
        ]),
        2
    );
}
