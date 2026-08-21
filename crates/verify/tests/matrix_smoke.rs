//! End-to-end smoke test: runs the matrix binary against the committed S1
//! fixture with a real save-engine CLI. Ignored by default because it
//! needs qpdf/poppler plus an engine binary (and, for the default mode,
//! macOS with Xcode); opt in with:
//!
//! ```sh
//! SHELLAC_TEST_ENGINE_CMD=/path/to/engine SHELLAC_TEST_NM_PREFIX=prefix \
//!   cargo test --test matrix_smoke -- --ignored
//! ```

use std::path::PathBuf;
use std::process::Command;

struct Run {
    tmp: PathBuf,
    out: PathBuf,
    prefix: String,
}

/// Runs the matrix over S1 alone. `extra` carries mode flags.
fn run_matrix_on_s1(tag: &str, extra: &[&str]) -> Run {
    let engine = std::env::var("SHELLAC_TEST_ENGINE_CMD")
        .expect("SHELLAC_TEST_ENGINE_CMD must point at the save-engine CLI");
    let prefix = std::env::var("SHELLAC_TEST_NM_PREFIX")
        .expect("SHELLAC_TEST_NM_PREFIX must hold the engine's /NM prefix");

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest.parent().unwrap().parent().unwrap();
    let tmp = std::env::temp_dir().join(format!("verify-smoke-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let samples = tmp.join("samples");
    std::fs::create_dir_all(&samples).unwrap();
    std::fs::copy(repo.join("corpus/fixtures/S1.pdf"), samples.join("S1.pdf")).unwrap();
    let out = tmp.join("matrix.md");

    let mut args: Vec<String> = [
        "matrix",
        "--samples",
        samples.to_str().unwrap(),
        "--work",
        tmp.join("work").to_str().unwrap(),
        "--scripts",
        repo.join("scripts").to_str().unwrap(),
        "--bin",
        tmp.join("bin").to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--engine-cmd",
        &engine,
        "--nm-prefix",
        &prefix,
        "--redact-nm-prefix",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(extra.iter().map(|s| s.to_string()));

    let status = Command::new(env!("CARGO_BIN_EXE_verify"))
        .args(&args)
        .status()
        .expect("running verify matrix");
    assert!(status.success(), "verify matrix exited with {status}");
    Run { tmp, out, prefix }
}

fn assert_common_report_shape(run: &Run, md: &str) {
    assert!(md.contains("# Incremental-save verification matrix"));
    assert!(md.contains("| S1 |"), "summary row for S1 missing:\n{md}");
    assert!(md.contains("#### Scenario: add"));
    assert!(md.contains("### loop (10 incremental saves)"));
    // Sanitization: no run-local absolute paths, and the nm prefix is
    // redacted.
    assert!(
        !md.contains(run.tmp.to_str().unwrap()),
        "work path leaked into the report"
    );
    assert!(
        !md.contains(&run.prefix),
        "nm prefix leaked despite --redact-nm-prefix"
    );
    assert!(md.contains("<nm>-hl-1"), "redacted ids missing:\n{md}");
}

#[test]
#[ignore = "requires external tools and an engine CLI; see module docs"]
fn matrix_runs_on_s1() {
    let run = run_matrix_on_s1("full", &[]);
    let md = std::fs::read_to_string(&run.out).unwrap();
    assert_common_report_shape(&run, &md);
    assert!(md.contains("- PDFKit helpers: enabled"));
    let _ = std::fs::remove_dir_all(&run.tmp);
}

#[test]
#[ignore = "requires external tools and an engine CLI; see module docs"]
fn matrix_runs_on_s1_without_the_pdfkit_helpers() {
    let run = run_matrix_on_s1("no-pdfkit", &["--no-pdfkit"]);
    let md = std::fs::read_to_string(&run.out).unwrap();
    assert_common_report_shape(&run, &md);
    assert!(md.contains("- PDFKit helpers: disabled"));
    // The checks the helpers provide are absent, not failing. The legend
    // lists every check whatever the run evaluated, so absence is asserted
    // over the verdict a scenario table would carry.
    for id in ["C5", "C7", "C11b"] {
        for verdict in ["pass", "**FAIL**"] {
            let row = format!("| {id} | {verdict} |");
            assert!(
                !md.contains(&row),
                "{row:?} must not appear in a scenario table:\n{md}"
            );
        }
    }
    // The checks that need no PDFKit still ran.
    assert!(md.contains("| C1 | pass |"));
    assert!(md.contains("| C11a | pass |"));
    let _ = std::fs::remove_dir_all(&run.tmp);
}
