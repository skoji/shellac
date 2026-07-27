//! C-enc-qpdf: structural integrity of an encrypted file after a save,
//! via `qpdf --check --password=`.

use crate::proc::run;
use crate::util::trunc;

/// Runs `qpdf --check --password= <path>`.
/// - exit 0: clean (pass)
/// - exit 3: warnings only, file still valid (pass)
/// - anything else: hard failure (fail)
///
/// The empty `--password=` is required for empty-user-password fixtures;
/// without it qpdf would try to prompt interactively.
pub fn qpdf_check_ok(path: &str) -> (bool, String) {
    let r = run("qpdf", &["--check", "--password=", path]);
    let combined = format!("{}{}", r.stdout_str(), r.stderr_str());
    let combined = combined.trim();
    match &r.err {
        None => (true, "qpdf --check: clean".to_string()),
        Some(_) if r.code == Some(3) => (
            true,
            "qpdf --check: warnings only (exit 3, file still valid)".to_string(),
        ),
        Some(e) => (
            false,
            format!("qpdf --check: {} — {}", e, trunc(combined, 500)),
        ),
    }
}
