//! Per-file measured state: raw bytes, %%EOF count, pdftotext output, and
//! PDFKit text output. Extraction failures are substituted into the text
//! fields (rather than failing the load) so the C4/C5 comparisons can still
//! run: two revisions failing identically compare equal.

use std::path::Path;

use crate::proc::run;
use crate::util::count_eof;

pub struct FileState {
    pub data: Vec<u8>,
    pub eof: usize,
    pub ptext: String,
    pub ktext: String,
}

/// Runs `pdftotext -enc UTF-8 <path> -`.
pub fn pdftotext(path: &str) -> Result<String, String> {
    let r = run("pdftotext", &["-enc", "UTF-8", path, "-"]);
    match &r.err {
        Some(e) => Err(format!("pdftotext: {} ({})", e, r.stderr_str().trim())),
        None => Ok(r.stdout_str()),
    }
}

/// Runs the pdfkit_text helper.
pub fn pdfkit_text(bin: &str, path: &str) -> Result<String, String> {
    let r = run(bin, &[path]);
    match &r.err {
        Some(e) => Err(format!("pdfkit_text: {} ({})", e, r.stderr_str().trim())),
        None => Ok(r.stdout_str()),
    }
}

pub fn load_state(path: &Path, pdfkit_text_bin: &str) -> Result<FileState, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let path_str = path.to_string_lossy();
    let ptext = match pdftotext(&path_str) {
        Ok(t) => t,
        Err(e) => format!("PDFTOTEXT-ERROR: {e}"),
    };
    let ktext = match pdfkit_text(pdfkit_text_bin, &path_str) {
        Ok(t) => t,
        Err(e) => format!("PDFKIT-ERROR: {e}"),
    };
    let eof = count_eof(&data);
    Ok(FileState {
        data,
        eof,
        ptext,
        ktext,
    })
}
