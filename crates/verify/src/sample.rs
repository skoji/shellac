//! Per-sample orchestration: scenario sequencing (add / modify-comment /
//! add-multiline / remove / loop x10), the C8 endurance aggregate, and the
//! refused-mode branch for encrypted fixtures the engine must not touch.

use std::path::Path;

use crate::anchor::{TextAnchor, find_text_anchor, find_text_anchor_poppler};
use crate::checks::bbox::{PosCheck, PosDerive};
use crate::checks::enc_qpdf::qpdf_check_ok;
use crate::checks::pdfkit::pdfkit_check;
use crate::checks::producer::producer_info;
use crate::checks::qpdf::{QpdfDoc, QuadCheck};
use crate::consts::{C8_MAX_DELTA, SKIP_REFUSED};
use crate::encrypted::{EncMode, classify_encrypted, extract_status};
use crate::engine::SaveEngine;
use crate::geom::{
    Rect, derive_highlight_rect, derive_loop_rect, derive_multiline_quads, derive_underline_rect,
    legacy_highlight_rect, legacy_loop_rect, legacy_underline_rect,
};
use crate::ids::NmIds;
use crate::scenario::{Bins, Scenario, run_scenario};
use crate::state::load_state;
use crate::util::{count_eof, dedup};

#[derive(Default)]
pub struct SampleResult {
    pub name: String,
    pub fatal_err: String,
    pub base_size: usize,
    pub base_eof: usize,
    pub pages: i64,
    pub base_producer: String,
    pub base_exif_producer: String,
    pub base_annot_types: Vec<String>,
    pub base_thumb_ok: bool,
    /// True when the run had no PDFKit helpers, so the baseline facts only
    /// PDFKit can report are absent rather than negative.
    pub pdfkit_disabled: bool,
    pub add: Scenario,
    pub modified: Scenario,
    pub modified_skipped: bool,
    pub add_multiline: Scenario,
    pub add_multiline_skipped: bool,
    pub removed: Scenario,
    pub loop_scenarios: Vec<Scenario>,
    pub loop_sizes: Vec<usize>,
    pub loop_deltas: Vec<i64>,
    pub c8_pass: bool,
    pub c8_detail: String,
    pub anchor: TextAnchor,
    pub anchor_err: String,
    pub enc_mode: EncMode,
    pub enc_want_status: String,
    pub enc_got_status: String,
    pub enc_bytes_equal: bool,
    pub enc_refused_note: String,
    pub enc_add_fatal_msg: String,
}

impl SampleResult {
    fn new(name: &str) -> Self {
        let (enc_mode, want) = classify_encrypted(name);
        SampleResult {
            name: name.to_string(),
            enc_mode,
            enc_want_status: want.to_string(),
            ..Default::default()
        }
    }
}

fn copy_file(src: &Path, dst: &Path) -> Result<(), String> {
    let b = std::fs::read(src).map_err(|e| e.to_string())?;
    std::fs::write(dst, b).map_err(|e| e.to_string())
}

/// Appends a C-enc-qpdf check to `sc` for encrypted roundtrip fixtures.
/// No-op for every other sample.
pub fn attach_enc_qpdf(sc: &mut Scenario, path: &str, mode: EncMode) {
    if mode != EncMode::RoundtripPass {
        return;
    }
    let (pass, detail) = qpdf_check_ok(path);
    sc.add("C-enc-qpdf", pass, detail.clone());
    if !pass {
        sc.fail_logs.push(format!("C-enc-qpdf: {detail}"));
    }
}

/// Handles refused-mode encrypted fixtures: try one add, require it to
/// fail with the expected status, and require the file to stay
/// byte-for-byte identical. All scenario slots are filled with skip
/// markers.
fn run_refused_sample(
    mut res: SampleResult,
    base: &Path,
    dir: &Path,
    eng: &dyn SaveEngine,
) -> SampleResult {
    let base_bytes = match std::fs::read(base) {
        Ok(b) => b,
        Err(e) => {
            res.fatal_err = format!("baseline read: {e}");
            return res;
        }
    };
    res.base_size = base_bytes.len();
    res.base_eof = count_eof(&base_bytes);
    res.pages = 0;

    let add_path = dir.join("add.pdf");
    if let Err(e) = copy_file(base, &add_path) {
        res.fatal_err = format!("copy base -> add: {e}");
        return res;
    }

    // Fixed legacy rects — never actually written, but the engine CLI
    // requires non-zero rectangles for its flags.
    let add_err = eng.add(
        &add_path.to_string_lossy(),
        legacy_highlight_rect(),
        legacy_underline_rect(),
    );
    match add_err {
        Ok(()) => {
            res.enc_refused_note =
                "unexpected success: engine accepted add on a refused-mode fixture".to_string();
            res.enc_add_fatal_msg = res.enc_refused_note.clone();
        }
        Err(e) => {
            res.enc_add_fatal_msg = e;
            res.enc_got_status = extract_status(&res.enc_add_fatal_msg);
        }
    }

    // The bytes-equal check runs regardless of add's outcome.
    match std::fs::read(&add_path) {
        Err(e) => {
            res.enc_bytes_equal = false;
            res.enc_refused_note = format!("post-add read: {e}");
        }
        Ok(post) => {
            res.enc_bytes_equal = base_bytes == post;
        }
    }

    if res.enc_refused_note.is_empty() {
        let got = if res.enc_got_status.is_empty() {
            "<none>"
        } else {
            &res.enc_got_status
        };
        let bytes = if res.enc_bytes_equal {
            "unchanged"
        } else {
            "CHANGED"
        };
        res.enc_refused_note = format!(
            "status={} (want {}), bytes={}",
            got, res.enc_want_status, bytes
        );
    }

    res.add = Scenario::fatal("add", SKIP_REFUSED);
    res.modified = Scenario::fatal("modify-comment", SKIP_REFUSED);
    res.modified_skipped = true;
    res.add_multiline = Scenario::fatal("add-multiline", SKIP_REFUSED);
    res.add_multiline_skipped = true;
    res.removed = Scenario::fatal("remove", SKIP_REFUSED);
    res.c8_pass = true;
    res.c8_detail = SKIP_REFUSED.to_string();
    res
}

pub fn run_sample(
    sample_path: &Path,
    work_root: &str,
    bins: Option<&Bins>,
    eng: &dyn SaveEngine,
    ids: &NmIds,
) -> SampleResult {
    let name = sample_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut res = SampleResult::new(&name);
    eprintln!("=== sample {name} ===");

    let dir = Path::new(work_root).join(&name);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        res.fatal_err = e.to_string();
        return res;
    }
    let base = dir.join("base.pdf");
    if let Err(e) = copy_file(sample_path, &base) {
        res.fatal_err = e.to_string();
        return res;
    }

    if res.enc_mode == EncMode::RefusedUnchanged {
        return run_refused_sample(res, &base, &dir, eng);
    }

    let base_str = base.to_string_lossy().into_owned();
    let base_state = match load_state(&base, bins.map(|b| b.pdfkit_text.as_str())) {
        Ok(s) => s,
        Err(e) => {
            res.fatal_err = format!("baseline load: {e}");
            return res;
        }
    };
    res.base_size = base_state.data.len();
    res.base_eof = base_state.eof;
    // The baseline page count, plus the annotation types and thumbnail bit
    // only PDFKit reports. Without the helpers the page count comes from
    // qpdf and the other two are unavailable rather than empty.
    match bins {
        Some(b) => {
            let (ck, _raw) = pdfkit_check(&b.pdfkit_check, &base_str, 0, "", "");
            let ck = match ck {
                Ok(c) => c,
                Err(e) => {
                    res.fatal_err = format!("baseline pdfkit_check: {e}");
                    return res;
                }
            };
            res.pages = ck.page_count;
            res.base_thumb_ok = ck.thumbnail_ok;
            res.base_annot_types = dedup(&ck.annot_types);
        }
        None => {
            res.pdfkit_disabled = true;
            match QpdfDoc::load(&base_str) {
                Ok((doc, _warning)) => res.pages = doc.page_count() as i64,
                Err(e) => {
                    res.fatal_err = format!("baseline qpdf: {e}");
                    return res;
                }
            }
        }
    }
    let (producer, exif) = producer_info(&base_str);
    res.base_producer = producer;
    res.base_exif_producer = exif;

    // Text anchor: resolved once per sample from the untouched baseline.
    let anchor_result = match bins {
        Some(b) => find_text_anchor(&b.pdfkit_textbbox, &base_str, 1),
        None => find_text_anchor_poppler(&base_str, 1),
    };
    let mut anchor = match anchor_result {
        Ok(a) => a,
        Err(e) => {
            res.anchor_err = e;
            TextAnchor::default()
        }
    };
    res.anchor = anchor.clone();
    if !res.anchor_err.is_empty() {
        anchor = TextAnchor::default();
    }

    let (hl_rect, ul_rect) = if anchor.found {
        (
            derive_highlight_rect(anchor.user_bounds, anchor.media_box),
            derive_underline_rect(anchor.user_bounds, anchor.media_box),
        )
    } else {
        (legacy_highlight_rect(), legacy_underline_rect())
    };
    let loop_rect = |i: i64| -> Rect {
        if anchor.found {
            derive_loop_rect(anchor.user_bounds, anchor.media_box, i)
        } else {
            legacy_loop_rect(i)
        }
    };
    let add_pos_checks: Vec<PosCheck> = if anchor.found {
        vec![
            PosCheck {
                id: ids.highlight(),
                derive: PosDerive::Highlight,
            },
            PosCheck {
                id: ids.underline(),
                derive: PosDerive::Underline,
            },
        ]
    } else {
        Vec::new()
    };

    // Scenario: add
    let add_path = dir.join("add.pdf");
    let add_path_str = add_path.to_string_lossy().into_owned();
    let mut add_state = None;
    if let Err(e) = copy_file(&base, &add_path) {
        res.add = Scenario::fatal("add", e);
    } else if let Err(e) = eng.add(&add_path_str, hl_rect, ul_rect) {
        res.add = Scenario::fatal("add", format!("add: {e}"));
    } else {
        let (sc, cur) = run_scenario(
            "add",
            &base_state,
            &add_path_str,
            res.pages,
            &[ids.highlight(), ids.underline()],
            &[],
            bins,
            &anchor,
            &add_pos_checks,
            Some(&ids.highlight()),
            None,
        );
        res.add = sc;
        add_state = cur;
        attach_enc_qpdf(&mut res.add, &add_path_str, res.enc_mode);
    }

    // Scenario: modify-comment (branches off add.pdf; add.pdf untouched).
    let mod_path = dir.join("modified.pdf");
    if !res.add.fatal.is_empty() {
        res.modified = Scenario::fatal("modify-comment", "skipped: add failed");
    } else if let Err(e) = copy_file(&add_path, &mod_path) {
        res.modified = Scenario::fatal("modify-comment", e);
    } else {
        let mod_path_str = mod_path.to_string_lossy().into_owned();
        if let Err(e) = eng.modify_comment(&mod_path_str, &ids.modified_contents()) {
            res.modified = Scenario::fatal("modify-comment", format!("modify-comment: {e}"));
        } else {
            let (sc, _) = run_scenario(
                "modify-comment",
                add_state.as_ref().unwrap_or(&base_state),
                &mod_path_str,
                res.pages,
                &[ids.highlight(), ids.underline()],
                &[],
                bins,
                &anchor,
                &[],
                None,
                None,
            );
            res.modified = sc;
            attach_enc_qpdf(&mut res.modified, &mod_path_str, res.enc_mode);
        }
    }

    // Scenario: add-multiline (independent of add.pdf). Skipped for
    // samples without a text anchor — nothing to split into stripes.
    let multi_path = dir.join("add-multiline.pdf");
    if !anchor.found {
        res.add_multiline =
            Scenario::fatal("add-multiline", "skipped: no text anchor for this sample");
        res.add_multiline_skipped = true;
    } else if let Err(e) = copy_file(&base, &multi_path) {
        res.add_multiline = Scenario::fatal("add-multiline", e);
    } else {
        let multi_path_str = multi_path.to_string_lossy().into_owned();
        let quads = derive_multiline_quads(hl_rect);
        if let Err(e) = eng.add_multiline(&multi_path_str, &quads) {
            res.add_multiline = Scenario::fatal("add-multiline", format!("add-multiline: {e}"));
        } else {
            let (sc, _) = run_scenario(
                "add-multiline",
                &base_state,
                &multi_path_str,
                res.pages,
                &[ids.multiline_highlight()],
                &[],
                bins,
                &anchor,
                &[],
                None,
                Some(&QuadCheck {
                    id: ids.multiline_highlight(),
                    min_points: 8,
                }),
            );
            res.add_multiline = sc;
            attach_enc_qpdf(&mut res.add_multiline, &multi_path_str, res.enc_mode);
        }
    }

    // Scenario: remove (based on add.pdf)
    let rm_path = dir.join("removed.pdf");
    if !res.add.fatal.is_empty() {
        res.removed = Scenario::fatal("remove", "skipped: add failed");
    } else if let Err(e) = copy_file(&add_path, &rm_path) {
        res.removed = Scenario::fatal("remove", e);
    } else {
        let rm_path_str = rm_path.to_string_lossy().into_owned();
        if let Err(e) = eng.remove(&rm_path_str) {
            res.removed = Scenario::fatal("remove", format!("remove: {e}"));
        } else {
            let (sc, _) = run_scenario(
                "remove",
                add_state.as_ref().unwrap_or(&base_state),
                &rm_path_str,
                res.pages,
                &[],
                &[ids.highlight(), ids.underline()],
                bins,
                &anchor,
                &[],
                None,
                None,
            );
            res.removed = sc;
            attach_enc_qpdf(&mut res.removed, &rm_path_str, res.enc_mode);
        }
    }

    // Scenario: loop x10 (C8 aggregate)
    let loop_path = dir.join("loop.pdf");
    let loop_path_str = loop_path.to_string_lossy().into_owned();
    let mut c8 = true;
    let mut c8_notes: Vec<String> = Vec::new();
    if let Err(e) = copy_file(&base, &loop_path) {
        c8 = false;
        c8_notes.push(e);
    } else {
        let mut prev = base_state;
        let mut loop_ids: Vec<String> = Vec::new();
        for i in 1..=10i64 {
            if let Err(e) = eng.loop_one(&loop_path_str, i, loop_rect(i)) {
                c8 = false;
                c8_notes.push(format!("iter {i} loopOne: {e}"));
                break;
            }
            loop_ids.push(ids.loop_id(i));
            let loop_pos_checks: Vec<PosCheck> = if anchor.found {
                vec![PosCheck {
                    id: ids.loop_id(i),
                    derive: PosDerive::Loop(i),
                }]
            } else {
                Vec::new()
            };
            let (mut sc, cur) = run_scenario(
                &format!("loop-{i:02}"),
                &prev,
                &loop_path_str,
                res.pages,
                &loop_ids,
                &[],
                bins,
                &anchor,
                &loop_pos_checks,
                None,
                None,
            );
            // Only the final iteration gets the extra qpdf --check; a
            // corrupt re-encryption in iter i would fail the same way in
            // iter 10.
            if i == 10 {
                attach_enc_qpdf(&mut sc, &loop_path_str, res.enc_mode);
            }
            // A scenario-level fatal (state load failure) keeps prev as
            // the reference state: delta 0, iteration recorded as failing.
            let (cur_len, delta) = match &cur {
                Some(c) => (c.data.len(), c.data.len() as i64 - prev.data.len() as i64),
                None => (prev.data.len(), 0),
            };
            res.loop_sizes.push(cur_len);
            res.loop_deltas.push(delta);
            if !sc.all_pass() {
                c8 = false;
                c8_notes.push(format!("iter {i} has failing checks"));
            }
            if delta >= C8_MAX_DELTA {
                c8 = false;
                c8_notes.push(format!("iter {i} increment {delta} bytes >= 50KB"));
            }
            res.loop_scenarios.push(sc);
            if let Some(c) = cur {
                prev = c;
            }
        }
    }
    if res.loop_scenarios.len() < 10 {
        c8 = false;
        c8_notes.push(format!(
            "only {}/10 iterations completed",
            res.loop_scenarios.len()
        ));
    }
    res.c8_pass = c8;
    if c8_notes.is_empty() {
        let max_d = res.loop_deltas.iter().copied().max().unwrap_or(0);
        c8_notes.push(format!(
            "10/10 iterations pass, max increment {max_d} bytes"
        ));
    }
    res.c8_detail = c8_notes.join("; ");

    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct MockEngine {
        add_result: Result<(), String>,
        mutate_on_add: bool,
        calls: RefCell<Vec<String>>,
    }

    impl MockEngine {
        fn refusing(msg: &str) -> Self {
            MockEngine {
                add_result: Err(msg.to_string()),
                mutate_on_add: false,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl SaveEngine for MockEngine {
        fn name(&self) -> String {
            "mock".to_string()
        }
        fn add(&self, path: &str, _hl: Rect, _ul: Rect) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("add {path}"));
            if self.mutate_on_add {
                let mut b = std::fs::read(path).unwrap();
                b.push(b'!');
                std::fs::write(path, b).unwrap();
            }
            self.add_result.clone()
        }
        fn remove(&self, _path: &str) -> Result<(), String> {
            unreachable!("remove must not run in refused mode")
        }
        fn loop_one(&self, _path: &str, _i: i64, _r: Rect) -> Result<(), String> {
            unreachable!("loop must not run in refused mode")
        }
        fn modify_comment(&self, _path: &str, _c: &str) -> Result<(), String> {
            unreachable!("modify must not run in refused mode")
        }
        fn add_multiline(&self, _path: &str, _q: &str) -> Result<(), String> {
            unreachable!("multiline must not run in refused mode")
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("verify-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn refused_fixture(dir: &Path) -> std::path::PathBuf {
        let base = dir.join("base.pdf");
        std::fs::write(&base, b"%PDF-1.6 fake\n%%EOF\n").unwrap();
        base
    }

    #[test]
    fn refused_sample_expected_status_and_unchanged_bytes() {
        let dir = temp_dir("refused-ok");
        let base = refused_fixture(&dir);
        let eng = MockEngine::refusing(
            "engine add x: exit status 1\nengine add: {\"status\":\"encrypted_refused\",\"applied\":0,\"skipped\":[]}",
        );
        let res = run_refused_sample(
            SampleResult::new("S11-password-required"),
            &base,
            &dir,
            &eng,
        );
        assert_eq!(res.enc_got_status, "encrypted_refused");
        assert!(res.enc_bytes_equal);
        assert_eq!(
            res.enc_refused_note,
            "status=encrypted_refused (want encrypted_refused), bytes=unchanged"
        );
        assert!(res.modified_skipped && res.add_multiline_skipped);
        assert!(res.c8_pass);
        assert_eq!(res.c8_detail, SKIP_REFUSED);
        assert_eq!(res.add.fatal, SKIP_REFUSED);
    }

    #[test]
    fn refused_sample_unexpected_success_is_noted() {
        let dir = temp_dir("refused-success");
        let base = refused_fixture(&dir);
        let eng = MockEngine {
            add_result: Ok(()),
            mutate_on_add: false,
            calls: RefCell::new(Vec::new()),
        };
        let res = run_refused_sample(
            SampleResult::new("S12-annotations-restricted"),
            &base,
            &dir,
            &eng,
        );
        assert_eq!(
            res.enc_refused_note,
            "unexpected success: engine accepted add on a refused-mode fixture"
        );
        assert!(res.enc_got_status.is_empty());
        assert!(res.enc_bytes_equal);
    }

    #[test]
    fn refused_sample_detects_mutation_despite_reported_failure() {
        let dir = temp_dir("refused-mutated");
        let base = refused_fixture(&dir);
        let eng = MockEngine {
            add_result: Err("boom \"status\":\"encrypted_refused\" end".to_string()),
            mutate_on_add: true,
            calls: RefCell::new(Vec::new()),
        };
        let res = run_refused_sample(
            SampleResult::new("S11-password-required"),
            &base,
            &dir,
            &eng,
        );
        assert!(!res.enc_bytes_equal);
        assert_eq!(
            res.enc_refused_note,
            "status=encrypted_refused (want encrypted_refused), bytes=CHANGED"
        );
    }

    #[test]
    fn refused_note_reports_missing_status() {
        let dir = temp_dir("refused-nostatus");
        let base = refused_fixture(&dir);
        let eng = MockEngine::refusing("engine crashed without json");
        let res = run_refused_sample(
            SampleResult::new("S11-password-required"),
            &base,
            &dir,
            &eng,
        );
        assert_eq!(
            res.enc_refused_note,
            "status=<none> (want encrypted_refused), bytes=unchanged"
        );
    }
}
