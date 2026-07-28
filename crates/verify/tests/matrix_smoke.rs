//! End-to-end smoke test: runs the matrix binary against the committed S1
//! fixture with a real save-engine CLI. Ignored by default because it
//! needs macOS with qpdf/poppler/Xcode plus an engine binary; opt in with:
//!
//! ```sh
//! SHELLAC_TEST_ENGINE_CMD=/path/to/engine SHELLAC_TEST_NM_PREFIX=prefix \
//!   cargo test --test matrix_smoke -- --ignored
//! ```

use std::path::PathBuf;
use std::process::Command;

#[test]
#[ignore = "requires external tools and an engine CLI; see module docs"]
fn matrix_runs_on_s1() {
    let engine = std::env::var("SHELLAC_TEST_ENGINE_CMD")
        .expect("SHELLAC_TEST_ENGINE_CMD must point at the save-engine CLI");
    let prefix = std::env::var("SHELLAC_TEST_NM_PREFIX")
        .expect("SHELLAC_TEST_NM_PREFIX must hold the engine's /NM prefix");

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest.parent().unwrap().parent().unwrap();
    let tmp = std::env::temp_dir().join(format!("verify-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let samples = tmp.join("samples");
    std::fs::create_dir_all(&samples).unwrap();
    std::fs::copy(repo.join("corpus/fixtures/S1.pdf"), samples.join("S1.pdf")).unwrap();
    let out = tmp.join("matrix.md");

    let status = Command::new(env!("CARGO_BIN_EXE_verify"))
        .args([
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
        ])
        .status()
        .expect("running verify matrix");
    assert!(status.success(), "verify matrix exited with {status}");

    let md = std::fs::read_to_string(&out).unwrap();
    assert!(md.contains("# Incremental-save verification matrix"));
    assert!(md.contains("| S1 |"), "summary row for S1 missing:\n{md}");
    assert!(md.contains("#### Scenario: add"));
    assert!(md.contains("### loop (10 incremental saves)"));
    // Sanitization: no run-local absolute paths, and the nm prefix is
    // redacted.
    assert!(
        !md.contains(tmp.to_str().unwrap()),
        "work path leaked into the report"
    );
    assert!(
        !md.contains(&prefix),
        "nm prefix leaked despite --redact-nm-prefix"
    );
    assert!(md.contains("<nm>-hl-1"), "redacted ids missing:\n{md}");

    let _ = std::fs::remove_dir_all(&tmp);
}
