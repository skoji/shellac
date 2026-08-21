use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // Re-run whenever any source file that affects the generated header changes.
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/ops.rs");
    println!("cargo:rerun-if-changed=src/annots.rs");
    println!("cargo:rerun-if-changed=src/transform.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-env-changed=SHELLAC_SKIP_CBINDGEN");
    println!("cargo:rerun-if-env-changed=SHELLAC_UPDATE_HEADER");

    // Declared unconditionally: tests/header_fresh.rs gates on this cfg, and
    // clippy's `unexpected_cfgs` lint (denied via `-D warnings` in CI) needs
    // it registered even on the branch that never emits it.
    println!("cargo:rustc-check-cfg=cfg(shellac_cbindgen_ran)");

    // Escape hatch: CI or downstream builds that only need to compile the
    // staticlib and already have a checked-in header can skip cbindgen.
    if env::var_os("SHELLAC_SKIP_CBINDGEN").is_some() {
        return;
    }

    // Emitted before cbindgen runs, not after it succeeds: a cbindgen
    // failure below only warns (so a downstream build isn't broken by it),
    // and gating the freshness test on "succeeded" would let that failure
    // silently disable the one check that catches a stale committed header.
    // Gating on "ran" instead means a real failure surfaces as a compile
    // error in tests/header_fresh.rs, which fails `cargo test` in this repo
    // (tests/ isn't packaged, so this cfg's meaning is invisible downstream).
    println!("cargo:rustc-cfg=shellac_cbindgen_ran");

    let crate_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set");
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR must be set");
    // Cargo's convention is that a build script writes only under OUT_DIR --
    // CARGO_MANIFEST_DIR is the extracted registry source when this crate is
    // built as a crates.io dependency, and writing there is unreproducible
    // and can upset source checksum verification. The committed
    // include/shellac.h is a shipped artifact, refreshed explicitly (see
    // SHELLAC_UPDATE_HEADER below) rather than on every build.
    let out_header: PathBuf = [&out_dir, "shellac.h"].iter().collect();

    let config = cbindgen::Config::from_file(PathBuf::from(&crate_dir).join("cbindgen.toml"))
        .expect("failed to read cbindgen.toml");

    let update_header = env::var_os("SHELLAC_UPDATE_HEADER").is_some();

    match cbindgen::Builder::new()
        .with_config(config)
        .with_crate(&crate_dir)
        .generate()
    {
        Ok(bindings) => {
            bindings.write_to_file(&out_header);

            if update_header {
                let committed_header: PathBuf =
                    [&crate_dir, "include", "shellac.h"].iter().collect();
                fs::copy(&out_header, &committed_header).unwrap_or_else(|err| {
                    panic!(
                        "SHELLAC_UPDATE_HEADER: failed to copy {} to {}: {}",
                        out_header.display(),
                        committed_header.display(),
                        err
                    )
                });
            }
        }
        Err(err) => {
            if update_header {
                // Under explicit maintainer opt-in, a failed regeneration
                // must not silently leave the committed header stale.
                panic!(
                    "SHELLAC_UPDATE_HEADER: cbindgen failed to generate {}: {}",
                    out_header.display(),
                    err
                );
            }
            // Do NOT fail the build if cbindgen cannot run (e.g. missing tool
            // in an unusual environment). The checked-in header remains valid.
            println!(
                "cargo:warning=cbindgen failed to generate {}: {}",
                out_header.display(),
                err
            );
        }
    }
}
