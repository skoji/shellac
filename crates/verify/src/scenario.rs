//! One scenario = one incremental-save transition (prev state -> saved
//! file) evaluated against the applicable checks.

use std::collections::BTreeMap;
use std::path::Path;

use crate::anchor::TextAnchor;
use crate::checks::audit::{IncAudit, audit_increment};
use crate::checks::bbox::{NeedleSelection, PosCheck, check_c11a};
use crate::checks::pdfkit::{evaluate_c7, evaluate_c11b, pdfkit_check};
use crate::checks::prefix::check_prefix;
use crate::checks::producer::producer_info;
use crate::checks::qpdf::{QpdfDoc, QuadCheck, evaluate_c6, evaluate_c6_quads};
use crate::checks::text::{check_c4, check_c5};
use crate::state::{FileState, load_state};
use crate::util::trunc;

#[derive(Clone, Debug)]
pub struct Check {
    pub id: String,
    pub pass: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Default)]
pub struct Scenario {
    pub name: String,
    pub checks: Vec<Check>,
    pub audit: Option<IncAudit>,
    pub c4_exact: bool,
    pub producer: String,
    pub exif_prod: String,
    pub ap_by_id: BTreeMap<String, bool>,
    pub fail_logs: Vec<String>,
    /// Non-empty when the save operation itself (or state loading) failed;
    /// checks are then absent.
    pub fatal: String,
    pub c11a_selection: Option<NeedleSelection>,
}

impl Scenario {
    pub fn fatal(name: &str, fatal: impl Into<String>) -> Self {
        Scenario {
            name: name.to_string(),
            fatal: fatal.into(),
            ..Default::default()
        }
    }

    pub fn add(&mut self, id: &str, pass: bool, detail: impl Into<String>) {
        self.checks.push(Check {
            id: id.to_string(),
            pass,
            detail: detail.into(),
        });
    }

    pub fn all_pass(&self) -> bool {
        if !self.fatal.is_empty() {
            return false;
        }
        self.checks.iter().all(|c| c.pass)
    }
}

/// Paths of the compiled PDFKit helper binaries.
pub struct Bins {
    pub pdfkit_text: String,
    pub pdfkit_check: String,
    pub pdfkit_textbbox: String,
}

#[allow(clippy::too_many_arguments)]
pub fn run_scenario(
    name: &str,
    prev: &FileState,
    cur_path: &str,
    pages: i64,
    expect_present: &[String],
    expect_absent: &[String],
    bins: Option<&Bins>,
    anchor: &TextAnchor,
    pos_checks: &[PosCheck],
    c11b_id: Option<&str>,
    quad_check: Option<&QuadCheck>,
) -> (Scenario, Option<FileState>) {
    let mut sc = Scenario {
        name: name.to_string(),
        ..Default::default()
    };
    let base_name = Path::new(cur_path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    eprintln!("    scenario {name}: {base_name}");

    let cur = match load_state(Path::new(cur_path), bins.map(|b| b.pdfkit_text.as_str())) {
        Ok(c) => c,
        Err(e) => {
            sc.fatal = format!("load state: {e}");
            return (sc, None);
        }
    };

    // C1: forward byte immutability
    let (p, d) = check_prefix(&prev.data, &cur.data);
    sc.add("C1", p, d.clone());
    if !p {
        sc.fail_logs.push(format!("C1: {d}"));
    }

    // C2: %%EOF count +1
    sc.add(
        "C2",
        cur.eof == prev.eof + 1,
        format!("%%EOF count {} -> {}", prev.eof, cur.eof),
    );

    // C3: increment audit
    if cur.data.len() > prev.data.len() {
        let audit = audit_increment(&cur.data[prev.data.len()..]);
        sc.add("C3", audit.pass(), audit.summary());
        if !audit.pass() {
            sc.fail_logs.push(format!("C3 audit: {}", audit.summary()));
        }
        sc.audit = Some(audit);
    } else {
        sc.add("C3", false, "no appended bytes to audit");
    }

    // C4: pdftotext comparison
    let c4 = check_c4(&prev.ptext, &cur.ptext);
    sc.c4_exact = c4.exact;
    sc.add("C4", c4.pass, c4.detail);
    if !c4.pass {
        sc.fail_logs.push(format!(
            "C4 pdftotext mismatch: prev(len={}) head={:?} cur(len={}) head={:?}",
            prev.ptext.len(),
            trunc(&prev.ptext, 300),
            cur.ptext.len(),
            trunc(&cur.ptext, 300)
        ));
    }

    // C5: PDFKit text comparison (exact). Without the helpers there is no
    // PDFKit text on either side, so the check is absent rather than a
    // comparison of two empty strings that would trivially pass.
    if bins.is_some() {
        let (c5_pass, c5_detail) = check_c5(&prev.ktext, &cur.ktext);
        sc.add("C5", c5_pass, c5_detail);
        if !c5_pass {
            sc.fail_logs.push(format!(
                "C5 PDFKit text mismatch: prev(len={}) head={:?} cur(len={}) head={:?}",
                prev.ktext.len(),
                trunc(&prev.ktext, 300),
                cur.ktext.len(),
                trunc(&cur.ktext, 300)
            ));
        }
    }

    // One qpdf --json pass shared by C6 / C6-quads / C10 / C11a.
    let mut all_ids: Vec<String> = Vec::new();
    for id in expect_present.iter().chain(expect_absent.iter()) {
        if !all_ids.contains(id) {
            all_ids.push(id.clone());
        }
    }
    for pc in pos_checks {
        if !all_ids.contains(&pc.id) {
            all_ids.push(pc.id.clone());
        }
    }
    let qres = QpdfDoc::load(cur_path);
    let (nm, qwarn) = match &qres {
        Ok((doc, warning)) => (Ok(doc.find_nm(&all_ids)), warning.clone()),
        Err(e) => (Err(e.as_str()), String::new()),
    };

    // C6: annotation presence/absence via qpdf json
    let c6 = evaluate_c6(
        expect_present,
        expect_absent,
        nm.as_ref().map_err(|e| *e),
        &qwarn,
    );
    sc.add("C6", c6.pass, c6.detail);
    sc.fail_logs.extend(c6.fail_logs);
    for (id, has_ap) in c6.ap_by_id {
        sc.ap_by_id.insert(id, has_ap);
    }

    // C6-quads: only when requested and the qpdf pass succeeded.
    if let (Some(qc), Ok(nm_map)) = (quad_check, nm.as_ref()) {
        let (pass, detail, logs) = evaluate_c6_quads(qc, nm_map);
        sc.add("C6-quads", pass, detail);
        sc.fail_logs.extend(logs);
    }

    // C7: PDFKit reload (carries C11b's needle/anchorNM when applicable).
    // The one pdfkit_check run feeds both C7 and C11b, so C11b's verdict is
    // held here and appended after C11a to keep the table's column order.
    let mut c11b_outcome = None;
    if let Some(bins) = bins {
        let (needle_arg, anchor_nm_arg) = match c11b_id {
            Some(id) if anchor.found => (anchor.needle.clone(), id.to_string()),
            _ => (String::new(), String::new()),
        };
        let (ck, raw) = pdfkit_check(
            &bins.pdfkit_check,
            cur_path,
            pages,
            &needle_arg,
            &anchor_nm_arg,
        );
        match &ck {
            Err(cerr) => {
                sc.add("C7", false, format!("pdfkit_check failed: {cerr}"));
                sc.fail_logs.push(format!(
                    "C7 pdfkit_check error: {}\nraw:\n{}",
                    cerr,
                    trunc(&raw, 1000)
                ));
            }
            Ok(c) => {
                let out = evaluate_c7(c, &raw, pages, expect_present, expect_absent);
                sc.add("C7", out.pass, out.detail);
                if let Some(log) = out.fail_log {
                    sc.fail_logs.push(log);
                }
            }
        }
        c11b_outcome = Some(evaluate_c11b(
            !needle_arg.is_empty(),
            ck.as_ref().map_err(|e| e.as_str()),
        ));
    }

    // C11a: independent (pdftotext-derived) position check.
    let c11a = check_c11a(cur_path, anchor, pos_checks, nm.as_ref().map_err(|e| *e));
    sc.add("C11a", c11a.pass, c11a.detail.clone());
    if !c11a.pass {
        sc.fail_logs.push(format!("C11a: {}", c11a.detail));
    }
    sc.c11a_selection = c11a.selection;

    // C11b: PDFKit-side re-verification.
    if let Some(c11b) = c11b_outcome {
        sc.add("C11b", c11b.pass, c11b.detail);
        if let Some(log) = c11b.fail_log {
            sc.fail_logs.push(log);
        }
    }

    // C9 record
    let (producer, exif) = producer_info(cur_path);
    sc.producer = producer;
    sc.exif_prod = exif;

    (sc, Some(cur))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_pass_requires_no_fatal_and_all_checks_passing() {
        let mut sc = Scenario::default();
        assert!(sc.all_pass());
        sc.add("C1", true, "ok");
        assert!(sc.all_pass());
        sc.add("C2", false, "bad");
        assert!(!sc.all_pass());

        let f = Scenario::fatal("add", "add: boom");
        assert!(!f.all_pass());
    }
}
