//! Annotation construction and incremental-update plumbing.
//!
//! Design notes:
//! - lopdf has no official API to edit a page's /Annots array
//!   (https://github.com/J-F-Liu/lopdf/issues/332); we manipulate the
//!   `Dictionary` / `Object` tree directly.
//! - Incremental saving uses `lopdf::IncrementalDocument`
//!   (`Document::new_from_prev`). `save_modern` is never used: it rewrites
//!   the whole document, which is the very thing this engine exists to
//!   avoid (see lopdf#479).
//! - Content-stripping filtered loading is always applied: `/Type /ObjStm`
//!   and `/Type /XRef` streams are preserved verbatim; every other stream's
//!   byte payload is dropped so heavy image/font/content-stream data never
//!   enters memory. The original file bytes are still moved verbatim into
//!   `IncrementalDocument`, so the saved prefix is byte-identical.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use lopdf::{
    Dictionary, Document, IncrementalDocument, LoadOptions, Object, ObjectId, Result, Stream,
    StringFormat,
};

use crate::transform::{Rect, UserSpacePoint, UserSpaceRect};

// ---------------------------------------------------------------------------
// Filtered load
// ---------------------------------------------------------------------------

/// lopdf `FilterFunc` used at load time. Empties the byte payload of every
/// content-bearing stream while leaving `/Type /ObjStm` and `/Type /XRef`
/// streams intact (the parser relies on those structurally).
///
/// Nothing here ever re-serializes a content stream — the previous file's
/// bytes are copied through verbatim — so keeping page content, images and
/// embedded fonts in memory would buy nothing and cost a lot on a large
/// scanned document. The stream dictionaries (including their now-stale
/// `/Length`) are left untouched so the object graph still resolves.
pub fn content_strip_filter(id: (u32, u16), obj: &mut Object) -> Option<((u32, u16), Object)> {
    if let Object::Stream(stream) = obj {
        let structural = stream
            .dict
            .get(b"Type")
            .ok()
            .and_then(|t| t.as_name().ok())
            .map(|name| name == b"ObjStm" || name == b"XRef")
            .unwrap_or(false);
        if !structural {
            // Keep the dictionary (and its now-stale /Length) as-is; only the
            // byte payload is discarded. Direct field assignment rather than
            // `set_content`, which would rewrite /Length to 0.
            stream.content = Vec::new();
        }
    }
    Some((id, obj.clone()))
}

/// `LoadOptions` used for every load in this crate (always filtered).
pub fn load_options() -> LoadOptions {
    LoadOptions::with_filter(content_strip_filter)
}

// ---------------------------------------------------------------------------
// Text string encoding
// ---------------------------------------------------------------------------

/// Encode `s` as a PDF text string: UTF-16BE with a leading byte-order mark,
/// the encoding PDF 1.7 §7.9.2.2 defines for text strings outside PDFDoc
/// encoding. Used for /Contents, which must survive arbitrary Unicode.
fn utf16be_bom(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + s.len() * 2);
    out.push(0xFE);
    out.push(0xFF);
    for unit in s.encode_utf16() {
        out.extend_from_slice(&unit.to_be_bytes());
    }
    out
}

fn rect_array(r: UserSpaceRect) -> Object {
    let r = r.0;
    Object::Array(vec![
        Object::Real(r.llx as f32),
        Object::Real(r.lly as f32),
        Object::Real(r.urx as f32),
        Object::Real(r.ury as f32),
    ])
}

/// QuadPoints for a single-quad rectangle, ordered upper-left, upper-right,
/// lower-left, lower-right.
///
/// That is a "Z" order, and it is deliberately **not** what the text-markup
/// annotation clause of ISO 32000-1 says: the prose there describes the four
/// vertices as being given in counterclockwise order, which would be
/// UL, LL, LR, UR. Acrobat has always read and written the Z order instead,
/// every other viewer followed Acrobat rather than the prose, and the
/// discrepancy is long-standing and widely known. Writing what the sentence
/// says would misdraw the markup in every viewer that matters, so this
/// follows the practice.
fn quad_points_array(r: UserSpaceRect) -> Object {
    let r = r.0;
    Object::Array(vec![
        Object::Real(r.llx as f32),
        Object::Real(r.ury as f32),
        Object::Real(r.urx as f32),
        Object::Real(r.ury as f32),
        Object::Real(r.llx as f32),
        Object::Real(r.lly as f32),
        Object::Real(r.urx as f32),
        Object::Real(r.lly as f32),
    ])
}

fn quad_points_from_points(pts: &[UserSpacePoint]) -> Object {
    let mut arr = Vec::with_capacity(pts.len() * 2);
    for p in pts {
        arr.push(Object::Real(p.x as f32));
        arr.push(Object::Real(p.y as f32));
    }
    Object::Array(arr)
}

/// For `Highlight` quads that are **taller than wide** (i.e. vertical text
/// layout), the PDFKit Z-order `[TL, TR, BL, BR]` callers send disagrees with
/// Acrobat's implementation-quirk order for vertical quads. Acrobat reads
/// `/QuadPoints` for vertical quads as `[BL, TL, BR, TR]`; feeding it the
/// Z-order variant makes it (and the shared Adobe imaging pipeline used
/// by Preview.app's internal thumbnails) fill a bow-tie polygon instead of
/// the intended rectangle. Confirmed experimentally across PDFKit,
/// Preview (thumbnail and body), Acrobat, and poppler.
///
/// This helper walks the input point list in 4-tuples, reconstructs each
/// quad's bounding box `(llx, lly, urx, ury)` from `min`/`max` (so the
/// caller's input point ordering does not matter — Z-order, CCW, CW all
/// resolve to the same bbox), and — **only when `H > W` and only for
/// Highlight** — emits the 4 corners in `[BL, TL, BR, TR]` order:
///
/// ```text
///     BL = (llx, lly)
///     TL = (llx, ury)
///     BR = (urx, lly)
///     TR = (urx, ury)
/// ```
///
/// Horizontal quads (`H ≤ W`) pass through with the caller's input order
/// preserved: PDFKit's Z-order `[TL, TR, BL, BR]` remains the correct read
/// order for wide/normal quads on every viewer tested. Non-Highlight
/// subtypes (`Underline` / `StrikeOut` / `Squiggly`) also pass through
/// verbatim regardless of aspect ratio — their imaging is line-based, not
/// polygon-fill, so the Acrobat bow-tie behaviour does not apply, and
/// changing the point order for them would mis-align the underline against
/// PDFKit's Z-order interpretation on non-Acrobat viewers.
///
/// Retained invariants:
/// - If `pts.len() % 4 != 0` (malformed input — should not happen because
///   callers feed full 4-tuples), the input is returned verbatim
///   rather than panicking, matching the existing helper's style.
/// - Degenerate quads (four coincident points, or `H == W`) are treated
///   as horizontal and pass through unchanged. Border-case: exactly-square
///   quads stay as-is; the Acrobat behavior only misfires on `H > W`.
fn quad_points_for_subtype(pts: &[UserSpacePoint], subtype: &str) -> Object {
    if subtype != "Highlight" || !pts.len().is_multiple_of(4) || pts.is_empty() {
        return quad_points_from_points(pts);
    }
    let mut arr: Vec<Object> = Vec::with_capacity(pts.len() * 2);
    for chunk in pts.chunks_exact(4) {
        let xs = [chunk[0].x, chunk[1].x, chunk[2].x, chunk[3].x];
        let ys = [chunk[0].y, chunk[1].y, chunk[2].y, chunk[3].y];
        let llx = xs.iter().cloned().fold(f64::INFINITY, f64::min);
        let urx = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let lly = ys.iter().cloned().fold(f64::INFINITY, f64::min);
        let ury = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let w = urx - llx;
        let h = ury - lly;
        if h > w {
            // Vertical quad — emit the bbox corners in Acrobat's [BL, TL, BR, TR].
            arr.push(Object::Real(llx as f32));
            arr.push(Object::Real(lly as f32));
            arr.push(Object::Real(llx as f32));
            arr.push(Object::Real(ury as f32));
            arr.push(Object::Real(urx as f32));
            arr.push(Object::Real(lly as f32));
            arr.push(Object::Real(urx as f32));
            arr.push(Object::Real(ury as f32));
        } else {
            // Horizontal quad — preserve the caller's input (Z-)order.
            for p in chunk {
                arr.push(Object::Real(p.x as f32));
                arr.push(Object::Real(p.y as f32));
            }
        }
    }
    Object::Array(arr)
}

/// Build a self-generated Appearance Stream (`/AP /N`) Form XObject for a
/// Highlight annotation. This works around a
/// long-standing Acrobat imaging quirk in the internal thumbnail
/// renderer: when a Highlight has no explicit /AP, Acrobat (and, on macOS,
/// Preview.app's internal thumbnail cache) synthesizes its own AP by
/// filling `/QuadPoints` as a single polygon — which becomes a bow-tie
/// for vertical quads even with the `[BL, TL, BR, TR]` ordering fix. A
/// self-generated AP that fills each quad as an independent rectangle
/// bypasses that pipeline entirely and renders identically in PDFKit,
/// Preview (body + thumbnail), Acrobat, and poppler.
///
/// The AP is a Form XObject whose content stream fills `n_quads`
/// rectangles under `/BM Multiply` (so the highlight colour multiplies
/// against the page like a real marker):
///
/// ```text
///     /GS0 gs
///     r g b rg
///     x0 y0 w0 h0 re f
///     x1 y1 w1 h1 re f
///     ...
/// ```
///
/// The Form XObject uses Acrobat's canonical `BBox + Matrix` construction:
/// `BBox = [0 0 W H]` (Rect-local, origin at the Rect's lower-left) and
/// `Matrix = [1 0 0 1 llx lly]` translates the form into user space. Each
/// `re f` command therefore takes Rect-local coordinates
/// `(qx - llx, qy - lly, qw, qh)`. Reconstructing quad bboxes from
/// point order (`min`/`max`) means the caller may pass either the original
/// input quads or the reordered `[BL, TL, BR, TR]` output — both resolve
/// to the same rectangle.
///
/// The Stream is returned as a fully-formed `Object::Stream`; the caller
/// registers it with `IncrementalDocument::new_document.add_object` and
/// wires the returned `ObjectId` into the annotation dict as
/// `/AP << /N <ref> >>`. lopdf 0.44's incremental writer preserves the
/// parent document's `/Encrypt` and re-encrypts every newly-added object
/// under the retained key, so no explicit `encrypt_object` call is needed
/// here — that would double-encrypt the payload (confirmed by reading
/// lopdf 0.44's `writer.rs` incremental-save path).
pub fn build_highlight_ap_stream(
    quads: &[UserSpacePoint],
    rect: UserSpaceRect,
    color: (f32, f32, f32),
) -> Object {
    let r = rect.0;
    let width = (r.urx - r.llx).max(0.0);
    let height = (r.ury - r.lly).max(0.0);

    // Content stream: /GS0 gs → colour → one `re f` per quad.
    let mut content = String::new();
    content.push_str("/GS0 gs\n");
    // Colour components are `f32`; format them without trailing zeros
    // where possible but keep enough precision for viewers.
    content.push_str(&format!("{} {} {} rg\n", color.0, color.1, color.2));
    if quads.len().is_multiple_of(4) && !quads.is_empty() {
        for chunk in quads.chunks_exact(4) {
            let xs = [chunk[0].x, chunk[1].x, chunk[2].x, chunk[3].x];
            let ys = [chunk[0].y, chunk[1].y, chunk[2].y, chunk[3].y];
            let qllx = xs.iter().cloned().fold(f64::INFINITY, f64::min);
            let qurx = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let qlly = ys.iter().cloned().fold(f64::INFINITY, f64::min);
            let qury = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let x = qllx - r.llx;
            let y = qlly - r.lly;
            let w = qurx - qllx;
            let h = qury - qlly;
            content.push_str(&format!("{} {} {} {} re f\n", x, y, w, h));
        }
    } else {
        // No explicit quads — degrade to a single Rect-covering fill so
        // the AP is never structurally empty. Malformed inputs still
        // produce a valid form.
        content.push_str(&format!("0 0 {} {} re f\n", width, height));
    }

    // Resources: /ExtGState /GS0 with BM Multiply, CA/ca = 1 inside the AP.
    // Rationale: ISO 32000-1 §12.5.6.2 (Table 170, markup annotations)
    // defines the annotation-dict /CA entry and states that when the
    // annotation has an appearance stream, the dict-level /CA "shall not
    // be used" and the AP should carry the transparency itself. In
    // practice, however, Acrobat and PDFKit still apply the dict /CA as a
    // constant alpha against the AP output. Writing another CA/ca inside
    // the AP would then double-attenuate against the dict-level /CA. We
    // set the AP-internal CA/ca = 1 and rely on the annotation-dict /CA
    // (set from the `opacity` parameter in `text_markup_dict`) as the
    // single source of truth for composited opacity, matching the real-
    // viewer behaviour. BM Multiply keeps the underlying text legible
    // through the highlight fill.
    //
    // Note: in ISO 32000-2 (PDF 2.0) the CA/ca entries were promoted to
    // the common annotation-dictionary entries in §12.5.2 (Table 166);
    // the semantics are equivalent, only the location in the standard
    // moved.
    let mut gs0 = Dictionary::new();
    gs0.set("Type", Object::Name(b"ExtGState".to_vec()));
    gs0.set("BM", Object::Name(b"Multiply".to_vec()));
    gs0.set("CA", Object::Real(1.0));
    gs0.set("ca", Object::Real(1.0));

    let mut extgstate = Dictionary::new();
    extgstate.set("GS0", Object::Dictionary(gs0));

    let mut resources = Dictionary::new();
    resources.set("ExtGState", Object::Dictionary(extgstate));

    // Form XObject dictionary.
    let mut form_dict = Dictionary::new();
    form_dict.set("Type", Object::Name(b"XObject".to_vec()));
    form_dict.set("Subtype", Object::Name(b"Form".to_vec()));
    form_dict.set("FormType", Object::Integer(1));
    form_dict.set(
        "BBox",
        Object::Array(vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(width as f32),
            Object::Real(height as f32),
        ]),
    );
    form_dict.set(
        "Matrix",
        Object::Array(vec![
            Object::Real(1.0),
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(1.0),
            Object::Real(r.llx as f32),
            Object::Real(r.lly as f32),
        ]),
    );
    form_dict.set("Resources", Object::Dictionary(resources));

    // Stream::new sets /Length; disable compression so `re f` commands
    // stay inspectable in `shellac-cli` / qpdf diagnostics.
    let stream = Stream::new(form_dict, content.into_bytes()).with_compression(false);
    Object::Stream(stream)
}

/// Build a text-markup annotation dictionary (Highlight, Underline, Text, ...).
/// Includes /QuadPoints for the text-markup subtypes that take them;
/// pass `explicit_quads = Some(...)` to override the default
/// rect-derived quad ordering. Explicit quads route through
/// `quad_points_for_subtype`, so a vertical Highlight quad is emitted as
/// its bounding box rather than as the points handed in.
pub fn text_markup_dict(
    subtype: &str,
    id: &str,
    contents: &str,
    rect: UserSpaceRect,
    color: (f32, f32, f32),
    opacity: f32,
    explicit_quads: Option<&[UserSpacePoint]>,
) -> Dictionary {
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"Annot".to_vec()));
    d.set("Subtype", Object::Name(subtype.as_bytes().to_vec()));
    d.set("Rect", rect_array(rect));
    d.set(
        "C",
        Object::Array(vec![
            Object::Real(color.0),
            Object::Real(color.1),
            Object::Real(color.2),
        ]),
    );
    d.set("CA", Object::Real(opacity));
    d.set(
        "NM",
        Object::String(id.as_bytes().to_vec(), StringFormat::Literal),
    );
    d.set(
        "Contents",
        Object::String(utf16be_bom(contents), StringFormat::Hexadecimal),
    );
    match subtype {
        "Highlight" | "Underline" | "StrikeOut" | "Squiggly" => {
            // For explicit quads, route through the subtype-aware helper
            // which reorders vertical Highlight quads to Acrobat's
            // `[BL, TL, BR, TR]` order. Non-Highlight subtypes,
            // horizontal quads, and the rect-derived fallback all pass
            // through unchanged.
            let quads = explicit_quads
                .map(|pts| quad_points_for_subtype(pts, subtype))
                .unwrap_or_else(|| quad_points_array(rect));
            d.set("QuadPoints", quads);
        }
        _ => {}
    }
    d
}

// ---------------------------------------------------------------------------
// Incremental open/save
// ---------------------------------------------------------------------------

/// Open `path` as the basis for an incremental update. Always uses the
/// content-stripping filter for the in-memory `prev` object graph; the
/// original bytes are still moved verbatim into `IncrementalDocument`, so
/// the saved output's previous-bytes prefix is byte-identical.
pub fn open_incremental(path: &Path) -> Result<IncrementalDocument> {
    let bytes = fs::read(path)?;
    let prev = Document::load_mem_with_options(&bytes, load_options())?;
    Ok(IncrementalDocument::create_from(bytes, prev))
}

/// Same as [`open_incremental`] but from a pre-read byte slice — used by
/// unit tests that build fixtures in memory.
pub fn open_incremental_from_bytes(bytes: Vec<u8>) -> Result<IncrementalDocument> {
    let prev = Document::load_mem_with_options(&bytes, load_options())?;
    Ok(IncrementalDocument::create_from(bytes, prev))
}

/// Write `idoc` (previous bytes + one new increment) back to `path`. Writes
/// to a sibling temp file first and renames it into place, so a partial
/// write never corrupts the original file. If serialization or the rename
/// fails, the temp file is removed rather than left behind: the PDF often
/// lives in a directory that is synced or presented to the user, where a
/// stray `*.tmp-shellac-incr` sibling is not merely untidy but visible.
pub fn save_incremental(path: &Path, idoc: &mut IncrementalDocument) -> Result<()> {
    let tmp_path = path.with_extension("tmp-shellac-incr");
    let write_result = (|| -> Result<()> {
        let mut f = fs::File::create(&tmp_path)?;
        idoc.save_to(&mut f)?;
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e.into());
    }
    Ok(())
}

/// Serialize `idoc` to a byte vector (used by unit tests).
pub fn save_incremental_to_vec(idoc: &mut IncrementalDocument) -> Result<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::new();
    idoc.save_to(&mut buf)?;
    Ok(buf)
}

// ---------------------------------------------------------------------------
// Page lookup / inheritable attributes
// ---------------------------------------------------------------------------

pub fn get_page_1based(doc: &Document, page_number: u32) -> Result<ObjectId> {
    doc.get_pages()
        .get(&page_number)
        .copied()
        .ok_or(lopdf::Error::PageNumberNotFound(page_number))
}

/// Walk the /Parent chain from `page_id` collecting `page_id` then each
/// ancestor. Used to look up inheritable page attributes (/Rotate,
/// /MediaBox) per PDF 1.7 §7.7.3.4.
///
/// Cycle guard: a `HashSet<ObjectId>` tracks every node already visited on
/// this walk, so we break out on any cycle — not just self-references but
/// arbitrarily-long loops (A → B → A, A → B → C → A, ...). Malformed PDFs
/// whose page tree circles back through /Parent are rare but real, and
/// `catch_unwind` at the FFI boundary cannot rescue an infinite loop —
/// a hang is not a panic.
fn page_and_ancestors(doc: &Document, page_id: ObjectId) -> Vec<ObjectId> {
    let mut out = vec![page_id];
    let mut visited: HashSet<ObjectId> = HashSet::new();
    visited.insert(page_id);
    let mut cur = page_id;
    while let Ok(dict) = doc.get_dictionary(cur) {
        match dict.get(b"Parent") {
            Ok(Object::Reference(pid)) => {
                if !visited.insert(*pid) {
                    // Already visited — /Parent chain loops back on itself.
                    break;
                }
                out.push(*pid);
                cur = *pid;
            }
            _ => break,
        }
    }
    out
}

fn inheritable(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<Object> {
    for id in page_and_ancestors(doc, page_id) {
        let Ok(dict) = doc.get_dictionary(id) else {
            continue;
        };
        if let Ok(obj) = dict.get(key) {
            let resolved = deref(doc, obj.clone());
            return Some(resolved);
        }
    }
    None
}

fn deref(doc: &Document, obj: Object) -> Object {
    if let Object::Reference(r) = obj {
        if let Ok(target) = doc.get_object(r) {
            return target.clone();
        }
    }
    obj
}

fn as_number(obj: &Object) -> Option<f64> {
    match obj {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

/// Look up a page's effective /Rotate (with inheritance; defaults to 0) and
/// /MediaBox as `(rotate, UserSpaceRect)`. The rect is returned in raw PDF
/// user space (as stored in /MediaBox — unrotated). Errors if the
/// /MediaBox is missing or malformed.
pub fn document_page_rotate_and_mediabox(
    doc: &Document,
    page_id: ObjectId,
) -> Result<(i32, UserSpaceRect)> {
    let rotate = inheritable(doc, page_id, b"Rotate")
        .as_ref()
        .and_then(as_number)
        .map(|n| n as i32)
        .unwrap_or(0);
    let mb = inheritable(doc, page_id, b"MediaBox").ok_or_else(|| {
        lopdf::Error::Syntax("page has no /MediaBox (even after inheritance)".to_string())
    })?;
    let arr = match &mb {
        Object::Array(a) => a,
        _ => {
            return Err(lopdf::Error::Syntax(
                "/MediaBox is not an array".to_string(),
            ))
        }
    };
    if arr.len() < 4 {
        return Err(lopdf::Error::Syntax(
            "/MediaBox has fewer than 4 elements".to_string(),
        ));
    }
    let mut vals = [0.0_f64; 4];
    for i in 0..4 {
        vals[i] = as_number(&arr[i])
            .ok_or_else(|| lopdf::Error::Syntax(format!("/MediaBox[{}] is not a number", i)))?;
    }
    let (llx, lly, urx, ury) = (vals[0], vals[1], vals[2], vals[3]);
    // Normalize so llx<=urx, lly<=ury (spec allows either corner ordering).
    let rect = Rect {
        llx: llx.min(urx),
        lly: lly.min(ury),
        urx: llx.max(urx),
        ury: lly.max(ury),
    };
    Ok((rotate, UserSpaceRect(rect)))
}

// ---------------------------------------------------------------------------
// /Annots reads (3-shape aware)
// ---------------------------------------------------------------------------

/// Reads the /Annots entries on `page_id` in `doc`, handling all three PDF
/// shapes (absent, direct array, indirect reference). Returns the list of
/// annotation ObjectIds (references only — inline dicts are skipped).
fn read_annot_refs(doc: &Document, page_id: ObjectId) -> Vec<ObjectId> {
    let mut out = Vec::new();
    let Ok(page_dict) = doc.get_dictionary(page_id) else {
        return out;
    };
    let annots_obj = match page_dict.get(b"Annots") {
        Ok(o) => o,
        Err(_) => return out,
    };
    let arr = match annots_obj {
        Object::Array(a) => a.clone(),
        Object::Reference(r) => match doc.get_object(*r).and_then(Object::as_array) {
            Ok(a) => a.clone(),
            Err(_) => return out,
        },
        _ => return out,
    };
    for entry in arr {
        if let Ok(rid) = entry.as_reference() {
            out.push(rid);
        }
    }
    out
}

/// Find annotation ObjectIds on `page_id` in `prev` whose /NM matches
/// `target_id`. Handles all three /Annots shapes.
pub fn find_by_nm(prev: &Document, page_id: ObjectId, target_id: &str) -> Option<ObjectId> {
    for rid in read_annot_refs(prev, page_id) {
        let Ok(dict) = prev.get_object(rid).and_then(Object::as_dict) else {
            continue;
        };
        let Ok(nm) = dict.get(b"NM").and_then(Object::as_str) else {
            continue;
        };
        if nm == target_id.as_bytes() {
            return Some(rid);
        }
    }
    None
}

/// Return true iff any annotation on `page_id` in `prev` has /NM equal to
/// `target_id`.
pub fn has_nm(prev: &Document, page_id: ObjectId, target_id: &str) -> bool {
    find_by_nm(prev, page_id, target_id).is_some()
}

/// Fallback resolver: find an annotation on `page_id` whose /Subtype matches
/// `subtype` and whose /Rect contains `point` (in raw PDF user space). Used
/// when the caller has no `/NM` to match on — for example an annotation
/// authored by another application.
///
/// Ties are broken by picking the smallest-area /Rect containing the point:
/// with several overlapping annotations of the same subtype, the tightest
/// one is the one the user was pointing at.
pub fn find_by_rect_point(
    prev: &Document,
    page_id: ObjectId,
    subtype: &str,
    point: UserSpacePoint,
) -> Option<ObjectId> {
    let (px, py) = (point.x, point.y);
    let mut best: Option<(ObjectId, f64)> = None;
    for rid in read_annot_refs(prev, page_id) {
        let Ok(dict) = prev.get_object(rid).and_then(Object::as_dict) else {
            continue;
        };
        let Ok(st) = dict.get(b"Subtype").and_then(Object::as_name) else {
            continue;
        };
        if st != subtype.as_bytes() {
            continue;
        }
        let Ok(rect_obj) = dict.get(b"Rect") else {
            continue;
        };
        let Ok(arr) = rect_obj.as_array() else {
            continue;
        };
        if arr.len() < 4 {
            continue;
        }
        let mut vals = [0.0_f64; 4];
        let mut ok = true;
        for i in 0..4 {
            match as_number(&arr[i]) {
                Some(v) => vals[i] = v,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        let (llx, lly, urx, ury) = (vals[0], vals[1], vals[2], vals[3]);
        let r = Rect {
            llx: llx.min(urx),
            lly: lly.min(ury),
            urx: llx.max(urx),
            ury: lly.max(ury),
        };
        if r.contains(px, py) {
            let area = (r.urx - r.llx) * (r.ury - r.lly);
            let take = match best {
                None => true,
                Some((_, ba)) => area < ba,
            };
            if take {
                best = Some((rid, area));
            }
        }
    }
    best.map(|(id, _)| id)
}

// ---------------------------------------------------------------------------
// /Annots writes (3-shape aware)
// ---------------------------------------------------------------------------

/// Append `new_refs` to page `page_id`'s /Annots, handling all three PDF
/// shapes (absent, direct array, indirect reference).
pub fn append_annot_refs(
    idoc: &mut IncrementalDocument,
    page_id: ObjectId,
    new_refs: &[ObjectId],
) -> Result<()> {
    if new_refs.is_empty() {
        return Ok(());
    }
    idoc.opt_clone_object_to_new_document(page_id)?;

    let indirect_annots: Option<ObjectId> = {
        let page_dict = idoc.new_document.get_dictionary(page_id)?;
        match page_dict.get(b"Annots") {
            Ok(Object::Reference(r)) => Some(*r),
            _ => None,
        }
    };
    let has_direct_array = {
        let page_dict = idoc.new_document.get_dictionary(page_id)?;
        matches!(page_dict.get(b"Annots"), Ok(Object::Array(_)))
    };

    let refs = new_refs.iter().map(|id| Object::Reference(*id));

    if let Some(annots_id) = indirect_annots {
        // Case (c): /Annots is an indirect reference to an array elsewhere.
        idoc.opt_clone_object_to_new_document(annots_id)?;
        let arr = idoc
            .new_document
            .get_object_mut(annots_id)?
            .as_array_mut()?;
        arr.extend(refs);
    } else if has_direct_array {
        // Case (b): /Annots is already a direct array on the page dict.
        let page_dict = idoc.new_document.get_dictionary_mut(page_id)?;
        let arr = page_dict.get_mut(b"Annots")?.as_array_mut()?;
        arr.extend(refs);
    } else {
        // Case (a): no /Annots yet.
        let page_dict = idoc.new_document.get_dictionary_mut(page_id)?;
        page_dict.set("Annots", Object::Array(refs.collect()));
    }
    Ok(())
}

/// Remove `remove_ids` from page `page_id`'s /Annots (all three shapes) and
/// null out the annotation objects themselves in the new increment.
///
/// Why `Object::Null` rather than freeing the object: an incremental update
/// can only append. The previous revision's bytes — including the annotation
/// dictionary — stay on disk untouched by definition, so "deleting" means
/// writing a newer revision of the same object number that supersedes it.
/// A null object is the smallest such revision.
///
/// Marking the cross-reference entry free in the appended section would be
/// the other option, and a reader that resolves a now-dangling reference is
/// required to treat it as null rather than to fail, so the two are
/// equivalent as far as the object graph is concerned. The explicit null is
/// preferred for two practical reasons: it is a visible object in the
/// appended bytes, so what an increment removed can be read straight out of
/// a diff or a `qpdf --check`, and it does not depend on a reader threading
/// the free-entry chain correctly across several appended cross-reference
/// sections — an area where real files and real readers disagree more often
/// than the object rules do.
pub fn remove_annot_refs(
    idoc: &mut IncrementalDocument,
    page_id: ObjectId,
    remove_ids: &[ObjectId],
) -> Result<()> {
    if remove_ids.is_empty() {
        return Ok(());
    }
    idoc.opt_clone_object_to_new_document(page_id)?;

    let indirect_annots: Option<ObjectId> = {
        let page_dict = idoc.new_document.get_dictionary(page_id)?;
        match page_dict.get(b"Annots") {
            Ok(Object::Reference(r)) => Some(*r),
            _ => None,
        }
    };

    if let Some(annots_id) = indirect_annots {
        idoc.opt_clone_object_to_new_document(annots_id)?;
        let arr = idoc
            .new_document
            .get_object_mut(annots_id)?
            .as_array_mut()?;
        arr.retain(|o| !matches!(o.as_reference(), Ok(id) if remove_ids.contains(&id)));
    } else {
        let page_dict = idoc.new_document.get_dictionary_mut(page_id)?;
        if let Ok(annots) = page_dict.get_mut(b"Annots") {
            if let Ok(arr) = annots.as_array_mut() {
                arr.retain(|o| !matches!(o.as_reference(), Ok(id) if remove_ids.contains(&id)));
            }
        }
    }

    for id in remove_ids {
        idoc.new_document.set_object(*id, Object::Null);
    }
    Ok(())
}

/// Modify an existing annotation's /Contents (or delete /Contents if
/// `new_contents` is `None`). The full annotation dictionary is cloned from
/// the previous document, `/Contents` is replaced/removed, and the resulting
/// dictionary is written back into `idoc.new_document` at the same
/// [`ObjectId`] using `set_object`. This overwrites the annotation object in
/// the new increment while preserving all other fields (subtype, rect,
/// color, ...).
pub fn update_contents(
    idoc: &mut IncrementalDocument,
    annot_id: ObjectId,
    new_contents: Option<&str>,
) -> Result<()> {
    let dict = idoc
        .get_prev_documents()
        .get_object(annot_id)?
        .as_dict()?
        .clone();
    let mut updated = dict;
    match new_contents {
        Some(text) => {
            updated.set(
                "Contents",
                Object::String(utf16be_bom(text), StringFormat::Hexadecimal),
            );
        }
        None => {
            updated.remove(b"Contents");
        }
    }
    idoc.new_document
        .set_object(annot_id, Object::Dictionary(updated));
    Ok(())
}
