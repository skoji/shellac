//! Guards against a checked-in `include/shellac.h` drifting from what
//! `build.rs` currently generates via cbindgen.
//!
//! Gated on `shellac_cbindgen_ran`, which build.rs emits whenever it did
//! not skip cbindgen (`SHELLAC_SKIP_CBINDGEN` unset) -- regardless of
//! whether generation succeeded. So this test does not exist when
//! generation was skipped, but if cbindgen ran and failed, `OUT_DIR/shellac.h`
//! is missing and this file fails to *compile*, which still fails `cargo
//! test`. That's deliberate: build.rs only warns on a cbindgen failure (so
//! a downstream build isn't broken by it), and `tests/` isn't part of the
//! published package, so this compile failure is a CI-only tripwire for a
//! stale or broken header, not something a downstream consumer can hit.
#[cfg(shellac_cbindgen_ran)]
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
