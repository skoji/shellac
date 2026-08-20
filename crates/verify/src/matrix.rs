//! The `matrix` subcommand: environment setup, corpus iteration, report
//! generation, and sanitization.

use std::path::Path;

use crate::cli::MatrixOpts;
use crate::engine::{CliEngine, SaveEngine};
use crate::exceptions::KnownExceptions;
use crate::gate::{
    FailCell, FailCells, apply_exceptions, collect_fail_cells, render_gate_section, sanitize_cells,
};
use crate::ids::NmIds;
use crate::proc::run;
use crate::report::build_report;
use crate::sample::run_sample;
use crate::sanitize::Sanitizer;
use crate::scenario::Bins;
use crate::swift::ensure_swift_bin;
use crate::util::first_line;

pub fn run_matrix(opts: &MatrixOpts) -> Result<(), String> {
    if std::fs::metadata(&opts.engine_cmd).is_err() {
        return Err(format!("--engine-cmd not found at {}", opts.engine_cmd));
    }
    std::fs::create_dir_all(&opts.bin).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&opts.work).map_err(|e| e.to_string())?;

    let bins = build_bins(opts)?;

    let mut pdfs: Vec<std::path::PathBuf> = std::fs::read_dir(&opts.samples)
        .map_err(|e| format!("reading samples dir {}: {}", opts.samples, e))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().map(|x| x == "pdf").unwrap_or(false)
                && p.file_name()
                    .map(|n| n.to_string_lossy().ends_with(".pdf"))
                    .unwrap_or(false)
        })
        .collect();
    pdfs.sort();
    if pdfs.is_empty() {
        return Err(format!("no PDFs found in {}", opts.samples));
    }

    let eng = CliEngine {
        cmd: opts.engine_cmd.clone(),
    };
    let ids = NmIds::new(&opts.nm_prefix);
    let results: Vec<_> = pdfs
        .iter()
        .map(|p| run_sample(p, &opts.work, bins.as_ref(), &eng, &ids))
        .collect();

    let env_lines = env_versions(&eng, opts.no_pdfkit);
    let sanitizer = build_sanitizer(opts);
    let cells = sanitize_cells(&collect_fail_cells(&results), &sanitizer);
    let mut md = build_report(&results, &env_lines, &rfc3339_utc_now(), &sanitizer);
    md.push_str(&gate_annotation(opts, &cells)?);
    // Final whole-document pass as a safety net for anything assembled
    // outside the per-field sanitization.
    std::fs::write(&opts.out, sanitizer.apply(&md)).map_err(|e| e.to_string())?;
    eprintln!("report written to {}", opts.out);

    if !opts.fails_out.is_empty() {
        FailCells::new(cells).write(&opts.fails_out)?;
        eprintln!("failing cells written to {}", opts.fails_out);
    }
    Ok(())
}

/// The report's gate section, or nothing when no list was supplied. The
/// verdict is recorded here but never acted on: `matrix` reports, and
/// `verify gate` is what turns a verdict into an exit status.
fn gate_annotation(opts: &MatrixOpts, cells: &[FailCell]) -> Result<String, String> {
    if opts.exceptions.is_empty() {
        return Ok(String::new());
    }
    let list = KnownExceptions::load(&opts.exceptions)?;
    Ok(format!(
        "\n{}",
        render_gate_section(&apply_exceptions(cells, &list))
    ))
}

/// Compiles the PDFKit helpers, or reports that this run has none. With
/// `--no-pdfkit` no compilation is attempted at all, which is what lets the
/// mode run where Xcode does not exist.
fn build_bins(opts: &MatrixOpts) -> Result<Option<Bins>, String> {
    if opts.no_pdfkit {
        todo!()
    }
    Ok(Some(Bins {
        pdfkit_text: ensure_swift_bin(&opts.bin, &opts.scripts, "pdfkit_text")?,
        pdfkit_check: ensure_swift_bin(&opts.bin, &opts.scripts, "pdfkit_check")?,
        pdfkit_textbbox: ensure_swift_bin(&opts.bin, &opts.scripts, "pdfkit_textbbox")?,
    }))
}

/// How the report's environment section describes the helper state. The
/// report is the audit trail, so which checks a run could not evaluate has
/// to be visible in it.
fn pdfkit_env_note(no_pdfkit: bool) -> &'static str {
    todo!()
}

fn env_versions(eng: &dyn SaveEngine, no_pdfkit: bool) -> String {
    let q = run("qpdf", &["--version"]);
    let pt = run("pdftotext", &["-v"]);
    format!(
        "- {}\n- {}\n- save engine: {}\n- PDFKit helpers: {}\n",
        first_line(q.stdout_str().trim()),
        // pdftotext prints its version banner on stderr.
        first_line(pt.stderr_str().trim()),
        eng.name(),
        pdfkit_env_note(no_pdfkit)
    )
}

/// Builds the report sanitizer: run-local paths become role names, the
/// engine path becomes its basename, the home directory becomes `~`, and
/// (optionally) the nm prefix is redacted.
fn build_sanitizer(opts: &MatrixOpts) -> Sanitizer {
    let mut s = Sanitizer::new();
    let engine_base = Path::new(&opts.engine_cmd)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "engine".to_string());
    s.add_path_with_canonical(&opts.engine_cmd, &engine_base);
    s.add_path_with_canonical(&opts.work, "work");
    s.add_path_with_canonical(&opts.samples, "samples");
    s.add_path_with_canonical(&opts.scripts, "scripts");
    s.add_path_with_canonical(&opts.bin, "bin");
    if let Ok(home) = std::env::var("HOME") {
        s.add_path(&home, "~");
    }
    if opts.redact_nm_prefix {
        s.add_literal(&opts.nm_prefix, "<nm>");
    }
    s.finalize();
    s
}

/// RFC3339 UTC timestamp without external dependencies.
pub fn rfc3339_utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Converts days since 1970-01-01 to a (year, month, day) civil date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts_with(no_pdfkit: bool) -> MatrixOpts {
        MatrixOpts {
            // Deliberately unusable paths: with --no-pdfkit nothing may be
            // compiled, so nothing may be read either.
            bin: "/nonexistent/bin".to_string(),
            scripts: "/nonexistent/scripts".to_string(),
            no_pdfkit,
            ..Default::default()
        }
    }

    #[test]
    fn no_pdfkit_skips_helper_compilation_entirely() {
        assert!(build_bins(&opts_with(true)).unwrap().is_none());
        assert!(
            build_bins(&opts_with(false)).is_err(),
            "without the flag a missing helper source is still an error"
        );
    }

    #[test]
    fn the_environment_section_records_which_checks_were_unavailable() {
        assert_eq!(
            pdfkit_env_note(true),
            "disabled (C5/C7/C11b not evaluated on this run)"
        );
        assert_eq!(pdfkit_env_note(false), "enabled");
    }

    #[test]
    fn civil_from_days_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1)); // 2024-01-01
        assert_eq!(civil_from_days(20_661), (2026, 7, 27)); // 2026-07-27
    }

    #[test]
    fn rfc3339_shape() {
        let s = rfc3339_utc_now();
        assert_eq!(s.len(), 20);
        assert!(s.ends_with('Z'));
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[10..11], "T");
    }
}
