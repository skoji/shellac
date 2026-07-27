//! C9: Producer metadata (record only, never pass/fail). Missing tools or
//! command failures simply yield empty strings.

use crate::proc::run;

/// Returns (pdfinfo Producer, exiftool Producer) for `path`. pdfinfo's
/// value is the last `Producer:` line; exiftool's is its trimmed stdout.
pub fn producer_info(path: &str) -> (String, String) {
    let r = run("pdfinfo", &[path]);
    let mut pdfinfo_producer = String::new();
    for line in r.stdout_str().split('\n') {
        if let Some(rest) = line.strip_prefix("Producer:") {
            pdfinfo_producer = rest.trim().to_string();
        }
    }
    let r2 = run("exiftool", &["-s3", "-Producer", path]);
    let exif_producer = r2.stdout_str().trim().to_string();
    (pdfinfo_producer, exif_producer)
}
