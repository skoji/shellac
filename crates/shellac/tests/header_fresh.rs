//! Guards against a checked-in `include/shellac.h` drifting from what
//! `build.rs` currently generates via cbindgen.
//!
//! Gated on `shellac_header_generated`, which build.rs emits only when it
//! wrote a header to `OUT_DIR`, so this test does not exist (and cannot
//! fail to compile) when generation was skipped (`SHELLAC_SKIP_CBINDGEN`)
//! or cbindgen itself failed.

#[cfg(shellac_header_generated)]
#[test]
fn generated_header_matches_committed() {
    let generated = include_str!(concat!(env!("OUT_DIR"), "/shellac.h"));
    let committed = include_str!("../include/shellac.h");
    assert_eq!(
        generated, committed,
        "crates/shellac/include/shellac.h is stale; regenerate it with \
         `SHELLAC_UPDATE_HEADER=1 cargo build -p shellac` and commit the result"
    );
}
