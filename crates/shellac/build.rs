use std::env;
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
    println!("cargo:rerun-if-env-changed=DOCS_RS");

    // Declared unconditionally: tests/header_fresh.rs gates on this cfg, and
    // clippy's `unexpected_cfgs` lint (denied via `-D warnings` in CI) needs
    // it registered even on branches that never emit it.
    println!("cargo:rustc-check-cfg=cfg(shellac_header_generated)");

    // Escape hatch: CI or downstream builds that only need to compile the
    // staticlib and already have a checked-in header can skip cbindgen.
    if env::var_os("SHELLAC_SKIP_CBINDGEN").is_some() {
        return;
    }

    // docs.rs builds in a sandbox whose source directory is read-only, and a
    // build script that writes into it fails the build. The header is not
    // part of the documentation anyway.
    if env::var_os("DOCS_RS").is_some() {
        return;
    }

    let crate_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set");
    let out_header: PathBuf = [&crate_dir, "include", "shellac.h"].iter().collect();

    let config = cbindgen::Config::from_file(PathBuf::from(&crate_dir).join("cbindgen.toml"))
        .expect("failed to read cbindgen.toml");

    match cbindgen::Builder::new()
        .with_config(config)
        .with_crate(&crate_dir)
        .generate()
    {
        Ok(bindings) => {
            bindings.write_to_file(&out_header);
            println!("cargo:rustc-cfg=shellac_header_generated");
        }
        Err(err) => {
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
