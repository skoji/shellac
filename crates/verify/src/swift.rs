//! On-demand compilation of the PDFKit helper binaries from their Swift
//! sources (`xcrun swiftc -O`). Rebuilds when the binary is missing or the
//! source is newer than the binary.

use std::path::Path;

use crate::proc::run;

pub fn ensure_swift_bin(bin_dir: &str, scripts_dir: &str, name: &str) -> Result<String, String> {
    let bin = Path::new(bin_dir).join(name);
    let src = Path::new(scripts_dir).join(format!("{name}.swift"));
    let rebuild = match std::fs::metadata(&bin) {
        Err(_) => true,
        Ok(bin_meta) => match (std::fs::metadata(&src), bin_meta.modified()) {
            (Ok(src_meta), Ok(bin_time)) => src_meta
                .modified()
                .map(|src_time| src_time > bin_time)
                .unwrap_or(false),
            _ => false,
        },
    };
    let bin_str = bin.to_string_lossy().into_owned();
    if rebuild {
        eprintln!("compiling {name}...");
        let src_str = src.to_string_lossy().into_owned();
        let r = run("xcrun", &["swiftc", "-O", "-o", &bin_str, &src_str]);
        if let Some(e) = &r.err {
            return Err(format!(
                "swiftc {}: {}\n{}{}",
                name,
                e,
                r.stdout_str(),
                r.stderr_str()
            ));
        }
    }
    Ok(bin_str)
}
