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

/// A page's raw geometry as recorded in the file: the unrotated MediaBox in
/// user space and the /Rotate entry that applies to it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PageGeometry {
    pub media_box: crate::geom::Rect,
    pub rotate: i64,
}

/// Cap on how far `/Parent` is followed. The page tree is a tree in a
/// well-formed file; the cap is what keeps a malformed one from looping.
const MAX_PAGE_TREE_DEPTH: usize = 64;

impl QpdfDoc {
    /// Number of pages in the document.
    pub fn page_count(&self) -> usize {
        self.root
            .get("pages")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0)
    }

    /// Geometry of one page (0-based), resolving `/MediaBox` and `/Rotate`
    /// through the page tree when the page itself does not carry them.
    /// A missing `/Rotate` means 0; a `/MediaBox` that cannot be resolved is
    /// an error, since every placement derives from it.
    pub fn page_geometry(&self, index: usize) -> Result<PageGeometry, String> {
        let pages = self
            .root
            .get("pages")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "qpdf json has no pages array".to_string())?;
        let page = pages.get(index).ok_or_else(|| {
            format!(
                "no page {} in qpdf json (document has {})",
                index + 1,
                pages.len()
            )
        })?;
        let dict = page
            .get("object")
            .and_then(|v| v.as_str())
            .and_then(|r| self.resolve_ref(r))
            .ok_or_else(|| format!("page {} object is not in the object table", index + 1))?;
        let media_box = self
            .inherited(dict, "/MediaBox")
            .and_then(|v| self.rect_value(v))
            .ok_or_else(|| format!("page {} has no usable /MediaBox", index + 1))?;
        let rotate = self
            .inherited(dict, "/Rotate")
            .and_then(|v| self.deref(v))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        Ok(PageGeometry { media_box, rotate })
    }

    /// The value of an inheritable page attribute, taken from the page or
    /// the nearest ancestor that carries it.
    fn inherited<'a>(&'a self, start: &'a Value, key: &str) -> Option<&'a Value> {
        let mut node = start;
        for _ in 0..MAX_PAGE_TREE_DEPTH {
            if let Some(v) = node.get(key) {
                return Some(v);
            }
            node = self.resolve_ref(node.get("/Parent")?.as_str()?)?;
        }
        None
    }

    /// A four-number rectangle, with its corners normalized: a `/MediaBox`
    /// may legally be written with either corner first.
    fn rect_value(&self, v: &Value) -> Option<crate::geom::Rect> {
        let arr = self.deref(v)?.as_array()?;
        if arr.len() != 4 {
            return None;
        }
        let mut n = [0f64; 4];
        for (slot, x) in n.iter_mut().zip(arr) {
            *slot = self.deref(x)?.as_f64()?;
        }
        Some(crate::geom::Rect::new(
            n[0].min(n[2]),
            n[1].min(n[3]),
            n[0].max(n[2]),
            n[1].max(n[3]),
        ))
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

    // ---- page geometry ----

    fn geometry_doc() -> QpdfDoc {
        // Page 1 carries its own geometry; page 2 inherits both entries
        // from the /Pages node; page 3 inherits /MediaBox but overrides
        // /Rotate; page 4 has no /MediaBox anywhere.
        let json = r#"{
          "pages": [
            { "object": "10 0 R" }, { "object": "11 0 R" },
            { "object": "12 0 R" }, { "object": "13 0 R" }
          ],
          "qpdf": [
            { "jsonversion": 2 },
            {
              "obj:5 0 R": { "value": { "/Type": "/Pages", "/MediaBox": [0, 0, 595.486, 842.202], "/Rotate": 180 } },
              "obj:6 0 R": { "value": { "/Type": "/Pages" } },
              "obj:10 0 R": { "value": { "/Type": "/Page", "/Parent": "5 0 R", "/MediaBox": [0, 0, 612, 792], "/Rotate": 90 } },
              "obj:11 0 R": { "value": { "/Type": "/Page", "/Parent": "5 0 R" } },
              "obj:12 0 R": { "value": { "/Type": "/Page", "/Parent": "5 0 R", "/Rotate": 270 } },
              "obj:13 0 R": { "value": { "/Type": "/Page", "/Parent": "6 0 R" } }
            }
          ]
        }"#;
        QpdfDoc::parse(json.as_bytes()).unwrap()
    }

    #[test]
    fn page_count_is_the_length_of_the_pages_array() {
        assert_eq!(geometry_doc().page_count(), 4);
        assert_eq!(QpdfDoc::parse(b"{}").unwrap().page_count(), 0);
    }

    #[test]
    fn page_geometry_prefers_the_page_over_the_page_tree() {
        let g = geometry_doc().page_geometry(0).unwrap();
        assert_eq!(g.media_box, crate::geom::Rect::new(0.0, 0.0, 612.0, 792.0));
        assert_eq!(g.rotate, 90);
    }

    #[test]
    fn page_geometry_inherits_through_parent() {
        let doc = geometry_doc();
        let g = doc.page_geometry(1).unwrap();
        assert_eq!(
            g.media_box,
            crate::geom::Rect::new(0.0, 0.0, 595.486, 842.202)
        );
        assert_eq!(g.rotate, 180);

        // Inheritance is per entry: /MediaBox comes from the parent while
        // /Rotate is overridden on the page.
        let g3 = doc.page_geometry(2).unwrap();
        assert_eq!(
            g3.media_box,
            crate::geom::Rect::new(0.0, 0.0, 595.486, 842.202)
        );
        assert_eq!(g3.rotate, 270);
    }

    #[test]
    fn a_page_without_a_media_box_anywhere_is_an_error() {
        let err = geometry_doc().page_geometry(3).unwrap_err();
        assert!(err.contains("/MediaBox"), "unexpected error: {err}");
    }

    #[test]
    fn a_page_index_past_the_end_is_an_error() {
        let err = geometry_doc().page_geometry(4).unwrap_err();
        assert!(err.contains("page 5"), "unexpected error: {err}");
    }

    #[test]
    fn page_geometry_resolves_indirect_values_and_normalizes_corner_order() {
        let json = r#"{
          "pages": [ { "object": "10 0 R" } ],
          "qpdf": [
            { "jsonversion": 2 },
            {
              "obj:10 0 R": { "value": { "/MediaBox": "20 0 R", "/Rotate": "21 0 R" } },
              "obj:20 0 R": { "value": [595.486, 842.202, 0, 0] },
              "obj:21 0 R": { "value": -90 }
            }
          ]
        }"#;
        let g = QpdfDoc::parse(json.as_bytes())
            .unwrap()
            .page_geometry(0)
            .unwrap();
        assert_eq!(
            g.media_box,
            crate::geom::Rect::new(0.0, 0.0, 595.486, 842.202)
        );
        assert_eq!(g.rotate, -90);
    }

    #[test]
    fn a_parent_cycle_does_not_hang() {
        let json = r#"{
          "pages": [ { "object": "10 0 R" } ],
          "qpdf": [
            { "jsonversion": 2 },
            {
              "obj:10 0 R": { "value": { "/Parent": "11 0 R" } },
              "obj:11 0 R": { "value": { "/Parent": "10 0 R" } }
            }
          ]
        }"#;
        assert!(
            QpdfDoc::parse(json.as_bytes())
                .unwrap()
                .page_geometry(0)
                .is_err()
        );
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
