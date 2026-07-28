//! C6 / C6-quads / C10 support: qpdf `--json=2` parsing and judgment.
//!
//! C6 asserts two facts per expected-present id, both derived from one qpdf
//! JSON pass: the annotation object exists somewhere in the file (whole-tree
//! walk for a matching `/NM`), and it is reachable from a page's `/Annots`
//! array (page attachment). Expected-absent ids fail if either fact holds.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::proc::run;
use crate::util::{bracket_list, first_line};

#[derive(Clone, Debug, Default)]
pub struct NmInfo {
    pub found: bool,
    pub has_ap: bool,
    /// `/Rect` as [llx, lly, urx, ury] when present and well-formed
    /// (length 4, all numbers); `None` otherwise.
    pub rect: Option<[f64; 4]>,
    /// `/QuadPoints` flattened; `None` when absent or malformed.
    pub quad_points: Option<Vec<f64>>,
    /// Whether the id was reachable from some page's `/Annots`.
    pub page_attached: bool,
}

impl NmInfo {
    pub fn rect_geom(&self) -> Option<crate::geom::Rect> {
        self.rect
            .map(|r| crate::geom::Rect::new(r[0], r[1], r[2], r[3]))
    }
}

/// Decodes qpdf JSON v2 string encoding: `u:<utf8>` for unicode strings,
/// `b:<hex>` for binary strings.
pub fn decode_string(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("u:") {
        return rest.to_string();
    }
    if let Some(hex) = s.strip_prefix("b:")
        && let Some(bytes) = decode_hex(hex)
    {
        return String::from_utf8_lossy(&bytes).into_owned();
    }
    s.to_string()
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16)?;
        let lo = (bytes[i + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    Some(out)
}

/// A parsed qpdf `--json=2` document.
pub struct QpdfDoc {
    root: Value,
}

impl QpdfDoc {
    /// Runs `qpdf --json=2 <path> -`. Exit 3 (warnings, processing
    /// completed) keeps the JSON and surfaces the stderr text as a warning;
    /// any other failure is an error.
    pub fn load(path: &str) -> Result<(QpdfDoc, String), String> {
        let r = run("qpdf", &["--json=2", path, "-"]);
        let mut warning = String::new();
        if let Some(e) = &r.err {
            if r.code != Some(3) {
                return Err(format!(
                    "qpdf --json: {} ({})",
                    e,
                    first_line(&r.stderr_str())
                ));
            }
            warning = r.stderr_str().trim().to_string();
        }
        let doc = Self::parse(&r.stdout)?;
        Ok((doc, warning))
    }

    pub fn parse(bytes: &[u8]) -> Result<QpdfDoc, String> {
        let root: Value =
            serde_json::from_slice(bytes).map_err(|e| format!("qpdf json decode: {e}"))?;
        Ok(QpdfDoc { root })
    }

    /// The object table (`"obj:N G R"` -> entry) of a v2 JSON document.
    fn object_table(&self) -> Option<&serde_json::Map<String, Value>> {
        let arr = self.root.get("qpdf")?.as_array()?;
        arr.iter()
            .filter_map(|v| v.as_object())
            .find(|m| m.keys().any(|k| k.starts_with("obj:")))
    }

    /// Resolves an object reference string like `"9 0 R"` to the referenced
    /// object's dictionary value (unwrapping `value` / `stream.dict`).
    fn resolve_ref(&self, refstr: &str) -> Option<&Value> {
        let table = self.object_table()?;
        let entry = table.get(&format!("obj:{refstr}"))?;
        if let Some(v) = entry.get("value") {
            return Some(v);
        }
        entry.get("stream")?.get("dict")
    }

    /// Returns `v` itself when it is already a container, or the referenced
    /// object when `v` is a reference string.
    fn deref<'a>(&'a self, v: &'a Value) -> Option<&'a Value> {
        match v {
            Value::String(s) if looks_like_ref(s) => self.resolve_ref(s),
            _ => Some(v),
        }
    }

    /// Collects the decoded `/NM` values of every annotation reachable from
    /// some page's `/Annots` array.
    pub fn attached_nm_set(&self) -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        let Some(pages) = self.root.get("pages").and_then(|v| v.as_array()) else {
            return set;
        };
        for page in pages {
            let Some(pref) = page.get("object").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(pval) = self.resolve_ref(pref) else {
                continue;
            };
            let Some(annots_raw) = pval.get("/Annots") else {
                continue;
            };
            let Some(annots) = self.deref(annots_raw).and_then(|v| v.as_array()) else {
                continue;
            };
            for a in annots {
                let Some(dict) = self.deref(a).and_then(|v| v.as_object()) else {
                    continue;
                };
                if let Some(nm) = dict.get("/NM").and_then(|v| v.as_str()) {
                    set.insert(decode_string(nm));
                }
            }
        }
        set
    }

    /// Walks the whole JSON tree looking for dicts whose decoded `/NM`
    /// equals one of `ids`, then annotates each hit with page attachment.
    /// The first hit (in deterministic traversal order) wins per id.
    pub fn find_nm(&self, ids: &[String]) -> BTreeMap<String, NmInfo> {
        let mut res: BTreeMap<String, NmInfo> = ids
            .iter()
            .map(|id| (id.clone(), NmInfo::default()))
            .collect();
        walk(&self.root, &mut res);
        let attached = self.attached_nm_set();
        for (id, info) in res.iter_mut() {
            info.page_attached = attached.contains(id);
        }
        res
    }
}

fn looks_like_ref(s: &str) -> bool {
    let mut parts = s.split(' ');
    let (Some(a), Some(b), Some(c), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    c == "R"
        && !a.is_empty()
        && a.bytes().all(|x| x.is_ascii_digit())
        && !b.is_empty()
        && b.bytes().all(|x| x.is_ascii_digit())
}

fn walk(v: &Value, res: &mut BTreeMap<String, NmInfo>) {
    match v {
        Value::Object(map) => {
            if let Some(raw) = map.get("/NM").and_then(|x| x.as_str()) {
                let nm = decode_string(raw);
                if let Some(slot) = res.get_mut(&nm)
                    && !slot.found
                {
                    let mut info = NmInfo {
                        found: true,
                        has_ap: map.contains_key("/AP"),
                        ..Default::default()
                    };
                    if let Some(rect_any) = map.get("/Rect").and_then(|x| x.as_array())
                        && rect_any.len() == 4
                    {
                        let nums: Vec<f64> = rect_any.iter().filter_map(|x| x.as_f64()).collect();
                        if nums.len() == 4 {
                            info.rect = Some([nums[0], nums[1], nums[2], nums[3]]);
                        }
                    }
                    if let Some(quads_any) = map.get("/QuadPoints").and_then(|x| x.as_array())
                        && !quads_any.is_empty()
                    {
                        let nums: Vec<f64> = quads_any.iter().filter_map(|x| x.as_f64()).collect();
                        if nums.len() == quads_any.len() {
                            info.quad_points = Some(nums);
                        }
                    }
                    *slot = info;
                }
            }
            for vv in map.values() {
                walk(vv, res);
            }
        }
        Value::Array(arr) => {
            for vv in arr {
                walk(vv, res);
            }
        }
        _ => {}
    }
}

// ---- C6 judgment ----

pub struct C6Outcome {
    pub pass: bool,
    pub detail: String,
    pub fail_logs: Vec<String>,
    /// C10 record: /AP presence per expected-present id found via qpdf.
    pub ap_by_id: Vec<(String, bool)>,
}

/// Evaluates C6 for one scenario. `nm` is `Err(qpdf error)` when the qpdf
/// pass itself failed; `warning` carries qpdf's exit-3 stderr text (noted
/// in the detail without affecting the verdict).
pub fn evaluate_c6(
    expect_present: &[String],
    expect_absent: &[String],
    nm: Result<&BTreeMap<String, NmInfo>, &str>,
    warning: &str,
) -> C6Outcome {
    let mut pass = true;
    let mut notes: Vec<String> = Vec::new();
    let mut fail_logs: Vec<String> = Vec::new();
    let mut ap_by_id: Vec<(String, bool)> = Vec::new();

    if let Err(qerr) = nm {
        pass = false;
        notes.push(format!("qpdf error: {qerr}"));
        fail_logs.push(format!("C6 qpdf error: {qerr}"));
    } else if !warning.is_empty() {
        notes.push(format!(
            "qpdf warning (exit 3, processing completed): {warning}"
        ));
    }

    let lookup = |id: &String| -> (bool, bool) {
        match nm {
            Ok(map) => map
                .get(id)
                .map(|i| (i.found, i.page_attached))
                .unwrap_or((false, false)),
            Err(_) => (false, false),
        }
    };

    for id in expect_present {
        let (object, attached) = lookup(id);
        if !object || !attached {
            pass = false;
            notes.push(format!(
                "{id}: object={object} page_attached={attached} (want present)"
            ));
            fail_logs.push(format!(
                "C6 {id} expected present: object={object} page_attached={attached}"
            ));
        }
        if let Ok(map) = nm
            && let Some(info) = map.get(id)
            && info.found
        {
            ap_by_id.push((id.clone(), info.has_ap));
        }
    }
    for id in expect_absent {
        let (object, attached) = lookup(id);
        if object || attached {
            pass = false;
            notes.push(format!(
                "{id}: object={object} page_attached={attached} (want absent)"
            ));
            fail_logs.push(format!(
                "C6 {id} expected absent: object={object} page_attached={attached}"
            ));
        }
    }
    if notes.is_empty() {
        notes.push(format!(
            "present={} absent={} confirmed via qpdf json (object + page attachment)",
            bracket_list(expect_present),
            bracket_list(expect_absent)
        ));
    }
    C6Outcome {
        pass,
        detail: notes.join("; "),
        fail_logs,
        ap_by_id,
    }
}

// ---- C6-quads judgment ----

pub struct QuadCheck {
    pub id: String,
    /// Minimum number of (x, y) points; the float count must be at least
    /// twice this and a multiple of 8.
    pub min_points: usize,
}

/// Evaluates C6-quads against the same qpdf pass used for C6. Only called
/// when the qpdf pass succeeded.
pub fn evaluate_c6_quads(
    qc: &QuadCheck,
    nm: &BTreeMap<String, NmInfo>,
) -> (bool, String, Vec<String>) {
    let info = nm.get(&qc.id);
    let found = info.map(|i| i.found).unwrap_or(false);
    let got = info
        .and_then(|i| i.quad_points.as_ref().map(|q| q.len()))
        .unwrap_or(0);
    if !found {
        return (
            false,
            format!(
                "{}: annotation not present per qpdf (want /QuadPoints ≥ {} floats)",
                qc.id,
                qc.min_points * 2
            ),
            vec![format!("C6-quads {}: annotation missing", qc.id)],
        );
    }
    if got < qc.min_points * 2 {
        return (
            false,
            format!(
                "{}: /QuadPoints has {} floats, want ≥ {} (min {} points)",
                qc.id,
                got,
                qc.min_points * 2,
                qc.min_points
            ),
            vec![format!(
                "C6-quads {}: got {} floats, want ≥ {}",
                qc.id,
                got,
                qc.min_points * 2
            )],
        );
    }
    if !got.is_multiple_of(8) {
        return (
            false,
            format!(
                "{}: /QuadPoints has {} floats, not a multiple of 8",
                qc.id, got
            ),
            vec![format!(
                "C6-quads {}: {} floats, not multiple of 8",
                qc.id, got
            )],
        );
    }
    (
        true,
        format!(
            "{}: /QuadPoints has {} floats = {} rects (want ≥ {} rects)",
            qc.id,
            got,
            got / 8,
            qc.min_points / 4
        ),
        Vec::new(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn decode_string_variants() {
        assert_eq!(decode_string("u:test-verify-hl-1"), "test-verify-hl-1");
        assert_eq!(decode_string("b:68656c6c6f"), "hello");
        // Invalid hex falls back to the raw string.
        assert_eq!(decode_string("b:zz"), "b:zz");
        assert_eq!(decode_string("b:123"), "b:123");
        assert_eq!(decode_string("plain"), "plain");
    }

    fn doc_with_two_annots() -> QpdfDoc {
        let json = r#"{
          "version": 2,
          "pages": [ { "object": "9 0 R" } ],
          "qpdf": [
            { "jsonversion": 2 },
            {
              "obj:9 0 R": { "value": { "/Type": "/Page", "/Annots": ["28 0 R", "29 0 R"] } },
              "obj:28 0 R": { "value": { "/NM": "u:id-hl", "/AP": "30 0 R", "/Rect": [100, 100.5, 300, 120], "/QuadPoints": [1,2,3,4,5,6,7,8] } },
              "obj:29 0 R": { "value": { "/NM": "u:id-ul", "/Rect": [100, 140, "5 0 R", 160] } },
              "obj:31 0 R": { "value": { "/NM": "u:id-orphan", "/Rect": [0, 0, 1, 1] } }
            }
          ]
        }"#;
        QpdfDoc::parse(json.as_bytes()).unwrap()
    }

    #[test]
    fn find_nm_reports_object_ap_rect_quads_and_attachment() {
        let doc = doc_with_two_annots();
        let nm = doc.find_nm(&ids(&["id-hl", "id-ul", "id-orphan", "id-missing"]));
        let hl = &nm["id-hl"];
        assert!(hl.found && hl.has_ap && hl.page_attached);
        assert_eq!(hl.rect, Some([100.0, 100.5, 300.0, 120.0]));
        assert_eq!(hl.quad_points.as_ref().map(|q| q.len()), Some(8));
        let ul = &nm["id-ul"];
        assert!(ul.found && !ul.has_ap && ul.page_attached);
        // Malformed /Rect (non-numeric entry) is not captured.
        assert_eq!(ul.rect, None);
        let orphan = &nm["id-orphan"];
        assert!(orphan.found && !orphan.page_attached);
        let missing = &nm["id-missing"];
        assert!(!missing.found && !missing.page_attached);
    }

    #[test]
    fn find_nm_integer_rect_values_are_accepted() {
        let doc = doc_with_two_annots();
        let nm = doc.find_nm(&ids(&["id-hl"]));
        assert_eq!(nm["id-hl"].rect, Some([100.0, 100.5, 300.0, 120.0]));
    }

    #[test]
    fn find_nm_first_hit_wins_deterministically() {
        let json = r#"{
          "qpdf": [
            { "jsonversion": 2 },
            {
              "obj:1 0 R": { "value": { "/NM": "u:dup", "/Rect": [1, 1, 2, 2] } },
              "obj:2 0 R": { "value": { "/NM": "u:dup", "/Rect": [9, 9, 10, 10] } }
            }
          ]
        }"#;
        let doc = QpdfDoc::parse(json.as_bytes()).unwrap();
        let nm = doc.find_nm(&ids(&["dup"]));
        // Deterministic traversal: obj:1 0 R sorts before obj:2 0 R.
        assert_eq!(nm["dup"].rect, Some([1.0, 1.0, 2.0, 2.0]));
    }

    #[test]
    fn annots_via_indirect_array_reference() {
        let json = r#"{
          "pages": [ { "object": "3 0 R" } ],
          "qpdf": [
            { "jsonversion": 2 },
            {
              "obj:3 0 R": { "value": { "/Annots": "4 0 R" } },
              "obj:4 0 R": { "value": ["5 0 R"] },
              "obj:5 0 R": { "value": { "/NM": "u:via-indirect" } }
            }
          ]
        }"#;
        let doc = QpdfDoc::parse(json.as_bytes()).unwrap();
        assert!(doc.attached_nm_set().contains("via-indirect"));
    }

    #[test]
    fn c6_present_requires_object_and_attachment() {
        let doc = doc_with_two_annots();
        let present = ids(&["id-hl", "id-ul"]);
        let nm = doc.find_nm(&present);
        let out = evaluate_c6(&present, &[], Ok(&nm), "");
        assert!(out.pass);
        assert_eq!(
            out.detail,
            "present=[id-hl id-ul] absent=[] confirmed via qpdf json (object + page attachment)"
        );
        assert_eq!(
            out.ap_by_id,
            vec![("id-hl".to_string(), true), ("id-ul".to_string(), false)]
        );
    }

    #[test]
    fn c6_present_fails_for_orphan_object() {
        let doc = doc_with_two_annots();
        let present = ids(&["id-orphan"]);
        let nm = doc.find_nm(&present);
        let out = evaluate_c6(&present, &[], Ok(&nm), "");
        assert!(!out.pass);
        assert_eq!(
            out.detail,
            "id-orphan: object=true page_attached=false (want present)"
        );
        // C10 still records the found object's /AP bit.
        assert_eq!(out.ap_by_id, vec![("id-orphan".to_string(), false)]);
    }

    #[test]
    fn c6_absent_fails_when_object_lingers() {
        let doc = doc_with_two_annots();
        let absent = ids(&["id-orphan"]);
        let nm = doc.find_nm(&absent);
        let out = evaluate_c6(&[], &absent, Ok(&nm), "");
        assert!(!out.pass);
        assert_eq!(
            out.detail,
            "id-orphan: object=true page_attached=false (want absent)"
        );
    }

    #[test]
    fn c6_absent_passes_when_gone() {
        let doc = doc_with_two_annots();
        let absent = ids(&["id-gone"]);
        let nm = doc.find_nm(&absent);
        let out = evaluate_c6(&[], &absent, Ok(&nm), "");
        assert!(out.pass);
        assert_eq!(
            out.detail,
            "present=[] absent=[id-gone] confirmed via qpdf json (object + page attachment)"
        );
    }

    #[test]
    fn c6_warning_is_noted_without_failing() {
        let doc = doc_with_two_annots();
        let present = ids(&["id-hl"]);
        let nm = doc.find_nm(&present);
        let out = evaluate_c6(&present, &[], Ok(&nm), "some warning");
        assert!(out.pass);
        assert_eq!(
            out.detail,
            "qpdf warning (exit 3, processing completed): some warning"
        );
    }

    #[test]
    fn c6_qpdf_error_fails_with_per_id_notes() {
        let present = ids(&["id-hl"]);
        let out = evaluate_c6(&present, &[], Err("exit status 2 (boom)"), "");
        assert!(!out.pass);
        assert_eq!(
            out.detail,
            "qpdf error: exit status 2 (boom); id-hl: object=false page_attached=false (want present)"
        );
        assert!(out.ap_by_id.is_empty());
    }

    #[test]
    fn c6_quads_judgments() {
        let doc = doc_with_two_annots();
        let nm = doc.find_nm(&ids(&["id-hl", "id-missing"]));
        // 8 floats < 16 wanted.
        let qc = QuadCheck {
            id: "id-hl".to_string(),
            min_points: 8,
        };
        let (pass, detail, _) = evaluate_c6_quads(&qc, &nm);
        assert!(!pass);
        assert_eq!(
            detail,
            "id-hl: /QuadPoints has 8 floats, want ≥ 16 (min 8 points)"
        );
        // Missing annotation.
        let qcm = QuadCheck {
            id: "id-missing".to_string(),
            min_points: 8,
        };
        let (pass2, detail2, logs2) = evaluate_c6_quads(&qcm, &nm);
        assert!(!pass2);
        assert_eq!(
            detail2,
            "id-missing: annotation not present per qpdf (want /QuadPoints ≥ 16 floats)"
        );
        assert_eq!(logs2, vec!["C6-quads id-missing: annotation missing"]);
    }

    #[test]
    fn c6_quads_pass_and_multiple_of_8() {
        let json = r#"{
          "qpdf": [
            { "jsonversion": 2 },
            {
              "obj:1 0 R": { "value": { "/NM": "u:ok16", "/QuadPoints": [1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16] } },
              "obj:2 0 R": { "value": { "/NM": "u:bad18", "/QuadPoints": [1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18] } }
            }
          ]
        }"#;
        let doc = QpdfDoc::parse(json.as_bytes()).unwrap();
        let nm = doc.find_nm(&ids(&["ok16", "bad18"]));
        let (pass, detail, _) = evaluate_c6_quads(
            &QuadCheck {
                id: "ok16".to_string(),
                min_points: 8,
            },
            &nm,
        );
        assert!(pass);
        assert_eq!(
            detail,
            "ok16: /QuadPoints has 16 floats = 2 rects (want ≥ 2 rects)"
        );
        let (pass2, detail2, _) = evaluate_c6_quads(
            &QuadCheck {
                id: "bad18".to_string(),
                min_points: 8,
            },
            &nm,
        );
        assert!(!pass2);
        assert_eq!(
            detail2,
            "bad18: /QuadPoints has 18 floats, not a multiple of 8"
        );
    }
}
