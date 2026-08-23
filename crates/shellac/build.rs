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
    // rustc's `unexpected_cfgs` lint (surfaced as an error only because CI
    // runs clippy with `-D warnings`) needs it registered even on the
    // branch that never emits it.
    println!("cargo:rustc-check-cfg=cfg(shellac_cbindgen_ran)");

    let skip_cbindgen = env::var_os("SHELLAC_SKIP_CBINDGEN").is_some();
    let update_header = env::var_os("SHELLAC_UPDATE_HEADER").is_some();

    // Combining them would silently do nothing: skip means generate() never
    // runs, so there is nothing fresh to copy into include/, and the
    // maintainer opt-in below would leave that header stale without saying
    // so.
    if skip_cbindgen && update_header {
        panic!("SHELLAC_SKIP_CBINDGEN and SHELLAC_UPDATE_HEADER cannot both be set");
    }

    // Escape hatch: CI or downstream builds that only need to compile the
    // staticlib and already have a checked-in header can skip cbindgen.
    if skip_cbindgen {
        return;
    }

    // See tests/header_fresh.rs for why this is emitted for "ran", not "succeeded".
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

    // Cargo does not clear OUT_DIR between build script runs, and a failed
    // generate() below never overwrites out_header -- so without this, a
    // stale header left by an earlier successful build (e.g. an incremental
    // build, or CI's cached `target`) would make the freshness test pass
    // even though this run's cbindgen failed.
    match fs::remove_file(&out_header) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => panic!("failed to remove stale {}: {}", out_header.display(), err),
    }

    // with_src (parses each listed file directly) rather than with_crate
    // (which shells out to `cargo metadata` against this crate's own
    // manifest): that subprocess can touch Cargo.lock or the registry
    // cache, which is more than the "writes only under OUT_DIR" guarantee
    // above is meant to allow.
    let src_dir = PathBuf::from(&crate_dir).join("src");
    match cbindgen::Builder::new()
        .with_config(config)
        .with_src(src_dir.join("lib.rs"))
        .with_src(src_dir.join("ops.rs"))
        .with_src(src_dir.join("annots.rs"))
        .with_src(src_dir.join("transform.rs"))
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
