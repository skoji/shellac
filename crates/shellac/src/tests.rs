//! Unit tests for the ops engine and the annotation helpers.
//!
//! These build synthetic 1-page PDFs in memory (via lopdf), serialize them
//! to bytes, and exercise the incremental-save path end-to-end via
//! [`apply_ops_to_bytes`] — a test-only variant of the FFI entry point that
//! works on byte buffers instead of file paths.
//!
//! The C1/C2/C3/C6 checks below mirror the corpus regression harness's
//! structural checks of the same names (previous bytes preserved, one new
//! `%%EOF` per increment, the increment carries the annotation's keys, and
//! add-then-remove resolves), so a failure here and a failure there mean
//! the same thing. The rest are semantics tests for the op engine: skip
//! cases, the three legal `/Annots` shapes, filtered/non-filtered
//! equivalence, encryption round-trips and the Highlight appearance stream.
//!
//! The harness's C11a check (`/Rotate 0` is identity) is covered by
//! `transform::tests::rotate_0_is_identity_at_origin` instead: it exercises
//! the `rect_page_space_to_user` helper, which `apply_ops` does not call.

use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;

use lopdf::encryption::crypt_filters::{Aes128CryptFilter, Aes256CryptFilter, CryptFilter};
use lopdf::{
    Dictionary, Document, EncryptionState, EncryptionVersion, IncrementalDocument, Object,
    ObjectId, Permissions, StringFormat,
};

use crate::annots::{open_incremental_from_bytes, save_incremental_to_vec};
use crate::ops::{apply_ops, ApplyResult, OpsBatch, SkipReason, Skipped, Status};
use crate::transform::{Rect, UserSpaceRect};

// ---------------------------------------------------------------------------
// Fixture builder
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnnotsShape {
    Absent,
    DirectArray,
    IndirectRef,
}

/// Build a minimal 1-page PDF with the requested /Annots shape and a
/// specified rotation. `existing_annot` optionally seeds the page with one
/// text-markup annotation the tests can look up by /NM. MediaBox defaults
/// to `[0, 0, 612, 792]`; call [`make_fixture_pdf_with_mediabox`] to use a
/// non-origin `/MediaBox`.
fn make_fixture_pdf(
    shape: AnnotsShape,
    rotate: i32,
    existing_annot: Option<(&str, &str, Rect)>,
) -> Vec<u8> {
    make_fixture_pdf_with_mediabox(
        shape,
        rotate,
        Rect {
            llx: 0.0,
            lly: 0.0,
            urx: 612.0,
            ury: 792.0,
        },
        existing_annot,
    )
}

fn make_fixture_pdf_with_mediabox(
    shape: AnnotsShape,
    rotate: i32,
    mediabox: Rect,
    existing_annot: Option<(&str, &str, Rect)>,
) -> Vec<u8> {
    let mut doc = Document::with_version("1.4");

    let mut page_dict = Dictionary::new();
    page_dict.set("Type", Object::Name(b"Page".to_vec()));
    page_dict.set(
        "MediaBox",
        Object::Array(vec![
            Object::Real(mediabox.llx as f32),
            Object::Real(mediabox.lly as f32),
            Object::Real(mediabox.urx as f32),
            Object::Real(mediabox.ury as f32),
        ]),
    );
    if rotate != 0 {
        page_dict.set("Rotate", Object::Integer(rotate as i64));
    }

    // Reserve a page id so annotations can reference it if we need to.
    let page_id = doc.new_object_id();

    let mut annot_ids: Vec<ObjectId> = Vec::new();
    if let Some((subtype, nm, rect)) = existing_annot {
        let mut d = Dictionary::new();
        d.set("Type", Object::Name(b"Annot".to_vec()));
        d.set("Subtype", Object::Name(subtype.as_bytes().to_vec()));
        d.set(
            "Rect",
            Object::Array(vec![
                Object::Real(rect.llx as f32),
                Object::Real(rect.lly as f32),
                Object::Real(rect.urx as f32),
                Object::Real(rect.ury as f32),
            ]),
        );
        d.set(
            "NM",
            Object::String(nm.as_bytes().to_vec(), StringFormat::Literal),
        );
        d.set(
            "Contents",
            Object::String(b"seed".to_vec(), StringFormat::Literal),
        );
        annot_ids.push(doc.add_object(Object::Dictionary(d)));
    }

    match shape {
        AnnotsShape::Absent => { /* no /Annots key at all */ }
        AnnotsShape::DirectArray => {
            let arr: Vec<Object> = annot_ids.iter().map(|id| Object::Reference(*id)).collect();
            page_dict.set("Annots", Object::Array(arr));
        }
        AnnotsShape::IndirectRef => {
            let arr: Vec<Object> = annot_ids.iter().map(|id| Object::Reference(*id)).collect();
            let arr_id = doc.add_object(Object::Array(arr));
            page_dict.set("Annots", Object::Reference(arr_id));
        }
    }

    let pages_id = doc.new_object_id();
    page_dict.set("Parent", Object::Reference(pages_id));
    doc.set_object(page_id, Object::Dictionary(page_dict));

    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Count", Object::Integer(1));
    pages.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    doc.set_object(pages_id, Object::Dictionary(pages));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));

    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut buf: Vec<u8> = Vec::new();
    doc.save_to(&mut buf).expect("save fixture PDF");
    buf
}

// ---------------------------------------------------------------------------
// Helpers to run ops end-to-end on bytes
// ---------------------------------------------------------------------------

/// Write `bytes` to a fresh temp file under the target directory, run
/// [`apply_ops`], then read the result back. Returns
/// (result, post_bytes).
fn apply_ops_on_bytes(bytes: &[u8], batch_json: &str) -> (ApplyResult, Vec<u8>) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    // Combine PID + a monotonic counter so parallel `cargo test` threads
    // never collide on the same directory name.
    let dir = std::env::temp_dir().join(format!(
        "shellac-test-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fixture.pdf");
    fs::write(&path, bytes).unwrap();

    let batch: OpsBatch = serde_json::from_str(batch_json).expect("valid batch json");
    let res = apply_ops(&path, batch);
    let post = fs::read(&path).unwrap_or_default();
    let _ = fs::remove_dir_all(&dir);
    (res, post)
}

fn find_nm(doc: &Document, nm: &str) -> Option<ObjectId> {
    for (_pn, page_id) in doc.get_pages() {
        if let Some(id) = crate::annots::find_by_nm(doc, page_id, nm) {
            return Some(id);
        }
    }
    None
}

/// Read the `/Rect` from an annotation dict as `[f64; 4]`.
fn read_rect(doc: &Document, annot_id: ObjectId) -> [f64; 4] {
    let rect_arr = doc
        .get_object(annot_id)
        .unwrap()
        .as_dict()
        .unwrap()
        .get(b"Rect")
        .unwrap()
        .as_array()
        .unwrap();
    let mut vals = [0.0_f64; 4];
    for i in 0..4 {
        vals[i] = match &rect_arr[i] {
            Object::Real(r) => *r as f64,
            Object::Integer(n) => *n as f64,
            _ => panic!("unexpected Rect element"),
        };
    }
    vals
}

fn assert_rect_approx(got: [f64; 4], want: [f64; 4]) {
    for i in 0..4 {
        assert!(
            (got[i] - want[i]).abs() < 0.01,
            "/Rect[{}] = {} want {} (verbatim expected)",
            i,
            got[i],
            want[i]
        );
    }
}

// ---------------------------------------------------------------------------
// C1: prev bytes prefix is preserved
// ---------------------------------------------------------------------------

#[test]
fn c1_prev_prefix_preserved_after_add() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"hl-1",
         "subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":300.0,"ury":120.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"hi"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 1);
    assert!(res.skipped.is_empty());
    assert!(post.len() > bytes.len(), "post must be strictly larger");
    assert_eq!(
        &post[..bytes.len()],
        &bytes[..],
        "prev prefix must be verbatim"
    );
}

// ---------------------------------------------------------------------------
// C2: %%EOF count grows by +1 per increment
// ---------------------------------------------------------------------------

fn count_eof(bytes: &[u8]) -> usize {
    let needle = b"%%EOF";
    let mut count = 0;
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            count += 1;
            i += needle.len();
        } else {
            i += 1;
        }
    }
    count
}

#[test]
fn c2_eof_marker_grows_by_one() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let pre_eofs = count_eof(&bytes);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"c2","subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":300.0,"ury":120.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"c2"}
    ]}"#;
    let (_res, post) = apply_ops_on_bytes(&bytes, batch);
    let post_eofs = count_eof(&post);
    assert_eq!(post_eofs, pre_eofs + 1);
}

// ---------------------------------------------------------------------------
// C3: increment contains /NM, /Contents (UTF-16BE BOM), /Rect
// ---------------------------------------------------------------------------

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn c3_increment_carries_nm_contents_rect() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"c3","subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":300.0,"ury":120.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"hi"}
    ]}"#;
    let (_res, post) = apply_ops_on_bytes(&bytes, batch);
    // Only inspect the increment (bytes appended after the prev prefix).
    let inc = &post[bytes.len()..];
    assert!(contains_bytes(inc, b"/NM"), "increment must contain /NM");
    assert!(
        contains_bytes(inc, b"/Contents"),
        "increment must contain /Contents"
    );
    assert!(
        contains_bytes(inc, b"/Rect"),
        "increment must contain /Rect"
    );
    // UTF-16BE BOM (0xFE 0xFF) present in the increment as part of the
    // /Contents literal. lopdf emits hex strings as e.g. <FEFF00680069>
    // (uppercase). Match either lowercase-nibble or uppercase.
    assert!(
        contains_bytes(inc, b"FEFF") || contains_bytes(inc, b"feff"),
        "increment must contain UTF-16BE BOM hex prefix"
    );
}

// ---------------------------------------------------------------------------
// C6: add then remove leaves /NM absent from the resolvable graph
// ---------------------------------------------------------------------------

#[test]
fn c6_add_then_remove_by_nm() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let add = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"c6","subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":300.0,"ury":120.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"c6"}
    ]}"#;
    let (r1, bytes_a) = apply_ops_on_bytes(&bytes, add);
    assert_eq!(r1.status, Status::Ok);

    // Confirm /NM is now resolvable.
    let doc_a = Document::load_mem_with_options(&bytes_a, crate::annots::load_options()).unwrap();
    assert!(
        find_nm(&doc_a, "c6").is_some(),
        "/NM should be present after add"
    );

    let rm = r#"{"ops":[
        {"type":"remove","index":1,"page_index":0,"annot_id":"c6",
         "subtype":"Highlight","user_point":{"x":150.0,"y":110.0}}
    ]}"#;
    let (r2, bytes_b) = apply_ops_on_bytes(&bytes_a, rm);
    assert_eq!(r2.status, Status::Ok);
    assert_eq!(r2.applied, 1);

    // Confirm /NM is gone (annotation object is now null in the merged xref).
    let doc_b = Document::load_mem_with_options(&bytes_b, crate::annots::load_options()).unwrap();
    assert!(
        find_nm(&doc_b, "c6").is_none(),
        "/NM should be absent after remove"
    );
}

// ---------------------------------------------------------------------------
// Wire-format compatibility: the pre-rename `shelff_id` key
// ---------------------------------------------------------------------------

/// The identity field was named `shelff_id` before this engine was extracted
/// into its own crate, and `#[serde(alias = "shelff_id")]` keeps that spelling
/// accepted so an existing caller can migrate its writer independently.
///
/// Each op type is checked twice over. The written files must be
/// **byte-identical** between the two spellings — nothing this crate emits is
/// time-dependent (no `/M`, no `/ID` rewrite; see
/// `filtered_and_non_filtered_produce_identical_output`, which relies on the
/// same property), so equality is exact rather than approximate.
///
/// Byte equality alone would not prove the alias does anything, though. On
/// `remove` and `modify_comment` the field is an `Option` with
/// `#[serde(default)]`: were the alias missing, an unrecognised key would
/// deserialize to `None` and the op would silently fall back to resolving by
/// `/Rect` containment instead of failing. So those two ops are given a
/// `user_point` outside every annotation on the page, which disarms the
/// fallback: `applied == 1` is then only reachable through an `/NM` lookup,
/// which is only reachable if the alias was honored.
#[test]
fn legacy_shelff_id_key_is_equivalent_to_annot_id() {
    // --- add ---------------------------------------------------------------
    const ADD_NEW: &str = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"alias-1","subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":300.0,"ury":120.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"alias"}
    ]}"#;
    const ADD_OLD: &str = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"shelff_id":"alias-1","subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":300.0,"ury":120.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"alias"}
    ]}"#;

    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let (res_add_new, post_add_new) = apply_ops_on_bytes(&bytes, ADD_NEW);
    let (res_add_old, post_add_old) = apply_ops_on_bytes(&bytes, ADD_OLD);
    assert_eq!(res_add_new.status, Status::Ok);
    assert_eq!(res_add_new.applied, 1);
    assert_eq!(res_add_old.status, Status::Ok);
    assert_eq!(res_add_old.applied, 1);
    assert_ne!(post_add_new, bytes, "add must have written an increment");
    assert_eq!(
        post_add_old, post_add_new,
        "`shelff_id` must add exactly as `annot_id`"
    );

    // --- remove ------------------------------------------------------------
    // `user_point` (-1, -1) is outside the annotation's /Rect, so the
    // subtype + containment fallback cannot resolve the target.
    const REMOVE_NEW: &str = r#"{"ops":[
        {"type":"remove","index":0,"page_index":0,"annot_id":"alias-1",
         "subtype":"Highlight","user_point":{"x":-1.0,"y":-1.0}}
    ]}"#;
    const REMOVE_OLD: &str = r#"{"ops":[
        {"type":"remove","index":0,"page_index":0,"shelff_id":"alias-1",
         "subtype":"Highlight","user_point":{"x":-1.0,"y":-1.0}}
    ]}"#;
    let (res_rm_new, post_rm_new) = apply_ops_on_bytes(&post_add_new, REMOVE_NEW);
    let (res_rm_old, post_rm_old) = apply_ops_on_bytes(&post_add_new, REMOVE_OLD);
    assert_eq!(res_rm_new.status, Status::Ok);
    assert_eq!(res_rm_new.applied, 1);
    assert_eq!(res_rm_old.status, Status::Ok);
    assert_eq!(
        res_rm_old.applied, 1,
        "`shelff_id` must resolve the remove target by /NM"
    );
    assert_eq!(
        post_rm_old, post_rm_new,
        "`shelff_id` must remove exactly as `annot_id`"
    );
    let doc_rm = Document::load_mem_with_options(&post_rm_old, crate::annots::load_options())
        .expect("post-remove doc must parse");
    assert!(find_nm(&doc_rm, "alias-1").is_none());

    // --- modify_comment ----------------------------------------------------
    const MODIFY_NEW: &str = r#"{"ops":[
        {"type":"modify_comment","index":0,"page_index":0,"annot_id":"alias-1",
         "subtype":"Highlight","user_point":{"x":-1.0,"y":-1.0},
         "new_comment":"via alias"}
    ]}"#;
    const MODIFY_OLD: &str = r#"{"ops":[
        {"type":"modify_comment","index":0,"page_index":0,"shelff_id":"alias-1",
         "subtype":"Highlight","user_point":{"x":-1.0,"y":-1.0},
         "new_comment":"via alias"}
    ]}"#;
    let (res_mod_new, post_mod_new) = apply_ops_on_bytes(&post_add_new, MODIFY_NEW);
    let (res_mod_old, post_mod_old) = apply_ops_on_bytes(&post_add_new, MODIFY_OLD);
    assert_eq!(res_mod_new.status, Status::Ok);
    assert_eq!(res_mod_new.applied, 1);
    assert_eq!(res_mod_old.status, Status::Ok);
    assert_eq!(
        res_mod_old.applied, 1,
        "`shelff_id` must resolve the modify_comment target by /NM"
    );
    assert_eq!(
        post_mod_old, post_mod_new,
        "`shelff_id` must modify exactly as `annot_id`"
    );
}

/// Guards the byte-equality assertions above: if a future change (or a lopdf
/// upgrade) started stamping a timestamp into the increment, those assertions
/// would begin failing intermittently and look like flakes rather than the
/// regression they are. Applying the same batch twice must produce the same
/// bytes, and the increment must carry no PDF date string.
#[test]
fn increments_are_byte_reproducible_and_carry_no_timestamp() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"repro","subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":300.0,"ury":120.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"repro"}
    ]}"#;
    let (_r1, first) = apply_ops_on_bytes(&bytes, batch);
    let (_r2, second) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(first, second, "the same batch must produce the same bytes");

    let increment = &first[bytes.len()..];
    // `D:` prefixes every PDF date string (PDF 1.7 §7.9.4), and `/M` is the
    // annotation modification date the engine deliberately does not write.
    assert!(
        !contains_bytes(increment, b"D:"),
        "increment must contain no PDF date string"
    );
    assert!(
        !contains_bytes(increment, b"/M "),
        "increment must contain no /M entry"
    );
}

#[test]
fn add_writes_rect_verbatim_under_rotate_90() {
    // Input `rect` is raw user space; `/Rect` on disk equals
    // the input verbatim regardless of /Rotate.
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 90, None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"r90",
         "subtype":"Highlight",
         "rect":{"llx":100.0,"lly":200.0,"urx":300.0,"ury":240.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"r"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);

    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    let annot_id = find_nm(&doc, "r90").expect("annotation must exist");
    assert_rect_approx(read_rect(&doc, annot_id), [100.0, 200.0, 300.0, 240.0]);
}

#[test]
fn add_writes_rect_verbatim_under_rotate_180() {
    // /Rotate 180 fixture; input rect equals `/Rect` verbatim.
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 180, None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"r180",
         "subtype":"Highlight",
         "rect":{"llx":100.0,"lly":200.0,"urx":300.0,"ury":240.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"r"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);

    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    let annot_id = find_nm(&doc, "r180").expect("annotation must exist");
    assert_rect_approx(read_rect(&doc, annot_id), [100.0, 200.0, 300.0, 240.0]);
}

#[test]
fn add_writes_rect_verbatim_under_rotate_270() {
    // /Rotate 270 fixture; input rect equals `/Rect` verbatim.
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 270, None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"r270",
         "subtype":"Highlight",
         "rect":{"llx":100.0,"lly":200.0,"urx":300.0,"ury":240.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"r"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);

    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    let annot_id = find_nm(&doc, "r270").expect("annotation must exist");
    assert_rect_approx(read_rect(&doc, annot_id), [100.0, 200.0, 300.0, 240.0]);
}

// ---------------------------------------------------------------------------
// Skip semantics
// ---------------------------------------------------------------------------

#[test]
fn skip_add_duplicate_nm_vs_prev() {
    let seed_rect = Rect {
        llx: 100.0,
        lly: 100.0,
        urx: 300.0,
        ury: 120.0,
    };
    let bytes = make_fixture_pdf(
        AnnotsShape::DirectArray,
        0,
        Some(("Highlight", "dup", seed_rect)),
    );
    let batch = r#"{"ops":[
        {"type":"add","index":7,"page_index":0,"annot_id":"dup","subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":300.0,"ury":120.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"dup"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 0);
    assert_eq!(
        res.skipped,
        vec![Skipped {
            index: 7,
            reason: SkipReason::AddDuplicateNm
        }]
    );
    // File must be untouched (nothing to save when applied == 0).
    assert_eq!(post, bytes);
}

#[test]
fn skip_add_duplicate_nm_within_batch() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"same","subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":300.0,"ury":120.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"a"},
        {"type":"add","index":1,"page_index":0,"annot_id":"same","subtype":"Highlight",
         "rect":{"llx":100.0,"lly":140.0,"urx":300.0,"ury":160.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"b"}
    ]}"#;
    let (res, _post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 1);
    assert_eq!(
        res.skipped,
        vec![Skipped {
            index: 1,
            reason: SkipReason::AddDuplicateNm
        }]
    );
}

#[test]
fn skip_remove_target_not_found_leaves_file_untouched() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"remove","index":3,"page_index":0,"annot_id":"nope",
         "subtype":"Highlight","user_point":{"x":150.0,"y":110.0}}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 0);
    assert_eq!(
        res.skipped,
        vec![Skipped {
            index: 3,
            reason: SkipReason::RemoveTargetNotFound
        }]
    );
    assert_eq!(post, bytes, "no changes → file must be untouched");
}

#[test]
fn skip_modify_target_not_found() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"modify_comment","index":4,"page_index":0,"annot_id":"nope",
         "subtype":"Highlight","user_point":{"x":150.0,"y":110.0},
         "new_comment":"x"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 0);
    assert_eq!(
        res.skipped,
        vec![Skipped {
            index: 4,
            reason: SkipReason::ModifyTargetNotFound
        }]
    );
    assert_eq!(post, bytes);
}

#[test]
fn remove_falls_back_to_subtype_rect_when_nm_missing() {
    let seed_rect = Rect {
        llx: 100.0,
        lly: 100.0,
        urx: 300.0,
        ury: 120.0,
    };
    let bytes = make_fixture_pdf(
        AnnotsShape::DirectArray,
        0,
        Some(("Highlight", "seed-nm", seed_rect)),
    );
    // Send a remove op *without* /annot_id, matching only by subtype +
    // rect containment. Point (150,110) lies inside seed_rect.
    let batch = r#"{"ops":[
        {"type":"remove","index":9,"page_index":0,"annot_id":null,
         "subtype":"Highlight","user_point":{"x":150.0,"y":110.0}}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 1);
    assert!(res.skipped.is_empty());

    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    assert!(find_nm(&doc, "seed-nm").is_none());
}

#[test]
fn modify_comment_replaces_contents() {
    let seed_rect = Rect {
        llx: 100.0,
        lly: 100.0,
        urx: 300.0,
        ury: 120.0,
    };
    let bytes = make_fixture_pdf(
        AnnotsShape::DirectArray,
        0,
        Some(("Highlight", "mod-nm", seed_rect)),
    );
    let batch = r#"{"ops":[
        {"type":"modify_comment","index":0,"page_index":0,"annot_id":"mod-nm",
         "subtype":"Highlight","user_point":{"x":150.0,"y":110.0},
         "new_comment":"replaced"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 1);

    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    let annot_id = find_nm(&doc, "mod-nm").expect("annotation must survive");
    let dict = doc.get_object(annot_id).unwrap().as_dict().unwrap();
    let contents = dict.get(b"Contents").unwrap();
    // We stored as UTF-16BE + BOM; the string bytes here are the raw
    // encoded form.
    match contents {
        Object::String(bytes, _) => {
            // First two bytes: BOM.
            assert_eq!(bytes[0], 0xFE);
            assert_eq!(bytes[1], 0xFF);
            // Decode UTF-16BE and compare.
            let units: Vec<u16> = bytes[2..]
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            let s = String::from_utf16(&units).unwrap();
            assert_eq!(s, "replaced");
        }
        other => panic!("/Contents unexpected: {:?}", other),
    }
}

#[test]
fn modify_comment_null_removes_contents() {
    let seed_rect = Rect {
        llx: 100.0,
        lly: 100.0,
        urx: 300.0,
        ury: 120.0,
    };
    let bytes = make_fixture_pdf(
        AnnotsShape::DirectArray,
        0,
        Some(("Highlight", "mod-nm", seed_rect)),
    );
    let batch = r#"{"ops":[
        {"type":"modify_comment","index":0,"page_index":0,"annot_id":"mod-nm",
         "subtype":"Highlight","user_point":{"x":150.0,"y":110.0},
         "new_comment":null}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 1);

    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    let annot_id = find_nm(&doc, "mod-nm").expect("annotation must survive");
    let dict = doc.get_object(annot_id).unwrap().as_dict().unwrap();
    assert!(dict.get(b"Contents").is_err(), "/Contents must be removed");
}

// ---------------------------------------------------------------------------
// /Annots 3 shapes
// ---------------------------------------------------------------------------

fn add_and_remove_roundtrip(shape: AnnotsShape) {
    let bytes = make_fixture_pdf(shape, 0, None);
    let add = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"shape","subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":300.0,"ury":120.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"s"}
    ]}"#;
    let (r1, bytes_a) = apply_ops_on_bytes(&bytes, add);
    assert_eq!(r1.status, Status::Ok);
    assert_eq!(r1.applied, 1);
    let doc = Document::load_mem_with_options(&bytes_a, crate::annots::load_options()).unwrap();
    assert!(find_nm(&doc, "shape").is_some());

    let rm = r#"{"ops":[
        {"type":"remove","index":1,"page_index":0,"annot_id":"shape",
         "subtype":"Highlight","user_point":{"x":150.0,"y":110.0}}
    ]}"#;
    let (r2, bytes_b) = apply_ops_on_bytes(&bytes_a, rm);
    assert_eq!(r2.status, Status::Ok);
    assert_eq!(r2.applied, 1);
    let doc = Document::load_mem_with_options(&bytes_b, crate::annots::load_options()).unwrap();
    assert!(find_nm(&doc, "shape").is_none());
}

#[test]
fn annots_shape_absent_supports_add_remove() {
    add_and_remove_roundtrip(AnnotsShape::Absent);
}

#[test]
fn annots_shape_direct_array_supports_add_remove() {
    add_and_remove_roundtrip(AnnotsShape::DirectArray);
}

#[test]
fn annots_shape_indirect_ref_supports_add_remove() {
    add_and_remove_roundtrip(AnnotsShape::IndirectRef);
}

// ---------------------------------------------------------------------------
// Filtered vs non-filtered equivalence
// ---------------------------------------------------------------------------
//
// The public `open_incremental_from_bytes` uses filtered load. To assert
// the two paths produce byte-identical increments, we build the same
// fixture (which has no content-bearing streams for the filter to touch
// anyway), open one via filtered and one via lopdf's default load, apply
// the exact same add op, and compare the serialized outputs.

#[test]
fn filtered_and_non_filtered_produce_identical_output() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);

    // Path A: filtered (via the crate's own open_incremental_from_bytes).
    let mut idoc_a = open_incremental_from_bytes(bytes.clone()).unwrap();
    // Path B: non-filtered (lopdf default LoadOptions).
    let doc_b = Document::load_mem(&bytes).unwrap();
    let mut idoc_b = IncrementalDocument::create_from(bytes.clone(), doc_b);

    // Apply the same single add to both idocs. Determinism note: lopdf's
    // ObjectId allocator uses a shared counter per Document, so the newly
    // added annotation gets the same id in both paths as long as we build
    // them in the same order.
    let rect = UserSpaceRect(Rect {
        llx: 100.0,
        lly: 100.0,
        urx: 300.0,
        ury: 120.0,
    });
    let dict = crate::annots::text_markup_dict(
        "Highlight",
        "eq-check",
        "eq",
        rect,
        (1.0, 0.8, 0.0),
        0.5,
        None,
    );
    let id_a = idoc_a
        .new_document
        .add_object(Object::Dictionary(dict.clone()));
    let id_b = idoc_b.new_document.add_object(Object::Dictionary(dict));
    assert_eq!(id_a, id_b);

    let page_id_a = idoc_a
        .get_prev_documents()
        .get_pages()
        .get(&1)
        .copied()
        .unwrap();
    let page_id_b = idoc_b
        .get_prev_documents()
        .get_pages()
        .get(&1)
        .copied()
        .unwrap();
    assert_eq!(page_id_a, page_id_b);

    crate::annots::append_annot_refs(&mut idoc_a, page_id_a, &[id_a]).unwrap();
    crate::annots::append_annot_refs(&mut idoc_b, page_id_b, &[id_b]).unwrap();

    let bytes_a = save_incremental_to_vec(&mut idoc_a).unwrap();
    let bytes_b = save_incremental_to_vec(&mut idoc_b).unwrap();
    assert_eq!(bytes_a, bytes_b, "filtered/non-filtered outputs must match");
}

// ---------------------------------------------------------------------------
// page-not-found
// ---------------------------------------------------------------------------

#[test]
fn skip_add_page_index_out_of_range() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"add","index":11,"page_index":99,"annot_id":"oor","subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":300.0,"ury":120.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"oor"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 0);
    assert_eq!(
        res.skipped,
        vec![Skipped {
            index: 11,
            reason: SkipReason::AddPageNotFound
        }]
    );
    assert_eq!(post, bytes);
}

#[test]
fn skip_remove_page_index_out_of_range() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"remove","index":12,"page_index":99,"annot_id":"x",
         "subtype":"Highlight","user_point":{"x":100.0,"y":100.0}}
    ]}"#;
    let (res, _post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 0);
    assert_eq!(
        res.skipped,
        vec![Skipped {
            index: 12,
            reason: SkipReason::RemovePageNotFound
        }]
    );
}

#[test]
fn skip_modify_page_index_out_of_range() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"modify_comment","index":13,"page_index":99,"annot_id":"x",
         "subtype":"Highlight","user_point":{"x":100.0,"y":100.0},
         "new_comment":"y"}
    ]}"#;
    let (res, _post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 0);
    assert_eq!(
        res.skipped,
        vec![Skipped {
            index: 13,
            reason: SkipReason::ModifyPageNotFound
        }]
    );
}

// ---------------------------------------------------------------------------
// MediaBox origin != (0, 0) — /Rect is still written verbatim
// ---------------------------------------------------------------------------

fn offset_mb() -> Rect {
    Rect {
        llx: 9.0,
        lly: 9.0,
        urx: 621.0,
        ury: 801.0,
    }
}

#[test]
fn must3_add_writes_rect_in_mediabox_absolute_coords_rotate_0() {
    // MediaBox [9 9 621 801], rotate=0: input rect must be written verbatim
    // (identity) — no origin drift.
    let bytes = make_fixture_pdf_with_mediabox(AnnotsShape::DirectArray, 0, offset_mb(), None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"origin-ok",
         "subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":200.0,"ury":120.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"o"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    let annot_id = find_nm(&doc, "origin-ok").unwrap();
    assert_rect_approx(read_rect(&doc, annot_id), [100.0, 100.0, 200.0, 120.0]);
}

#[test]
fn must3_add_writes_rect_verbatim_with_rotate_90_and_offset_mediabox() {
    // /Rotate 90 + offset MediaBox [9 9 621 801]. Input rect is
    // raw user space and must be written into `/Rect` verbatim.
    let bytes = make_fixture_pdf_with_mediabox(AnnotsShape::DirectArray, 90, offset_mb(), None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"r90-off",
         "subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":200.0,"ury":120.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"r"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);

    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    let annot_id = find_nm(&doc, "r90-off").unwrap();
    assert_rect_approx(read_rect(&doc, annot_id), [100.0, 100.0, 200.0, 120.0]);
}

#[test]
fn must3_remove_fallback_matches_absolute_rect_under_rotate_90() {
    // Seed the fixture with an annotation whose /Rect is in absolute user
    // space (MediaBox [9 9 621 801], /Rotate=90) at (510, 100)-(530, 200).
    let seed_rect = Rect {
        llx: 510.0,
        lly: 100.0,
        urx: 530.0,
        ury: 200.0,
    };
    let bytes = make_fixture_pdf_with_mediabox(
        AnnotsShape::DirectArray,
        90,
        offset_mb(),
        Some(("Highlight", "seed", seed_rect)),
    );
    // Remove without /NM — force the subtype + /Rect fallback. Under the
    // `user_point` is raw user space, so we send a point
    // directly inside the seed /Rect.
    let batch = r#"{"ops":[
        {"type":"remove","index":0,"page_index":0,"annot_id":null,
         "subtype":"Highlight","user_point":{"x":520.0,"y":150.0}}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 1);
    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    assert!(find_nm(&doc, "seed").is_none());
}

// ---------------------------------------------------------------------------
// Extra coverage
// ---------------------------------------------------------------------------

#[test]
fn add_with_quad_points_writes_them_verbatim_under_rotate_90() {
    // `quad_points` are raw user space; each point is written
    // into `/QuadPoints` verbatim regardless of /Rotate.
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 90, None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"quad",
         "subtype":"Highlight",
         "rect":{"llx":100.0,"lly":200.0,"urx":300.0,"ury":240.0},
         "quad_points":[[100.0,200.0],[300.0,200.0],[100.0,240.0],[300.0,240.0]],
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"q"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 1);
    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    let annot_id = find_nm(&doc, "quad").unwrap();
    let quads = doc
        .get_object(annot_id)
        .unwrap()
        .as_dict()
        .unwrap()
        .get(b"QuadPoints")
        .unwrap()
        .as_array()
        .unwrap();
    let want = [
        (100.0, 200.0),
        (300.0, 200.0),
        (100.0, 240.0),
        (300.0, 240.0),
    ];
    assert_eq!(quads.len(), 8, "QuadPoints must have 8 numbers (4 points)");
    for i in 0..4 {
        let x = match &quads[i * 2] {
            Object::Real(r) => *r as f64,
            Object::Integer(n) => *n as f64,
            _ => panic!(),
        };
        let y = match &quads[i * 2 + 1] {
            Object::Real(r) => *r as f64,
            Object::Integer(n) => *n as f64,
            _ => panic!(),
        };
        assert!(
            (x - want[i].0).abs() < 0.01,
            "quad[{}].x = {} want {}",
            i,
            x,
            want[i].0
        );
        assert!(
            (y - want[i].1).abs() < 0.01,
            "quad[{}].y = {} want {}",
            i,
            y,
            want[i].1
        );
    }
}

// ---------------------------------------------------------------------------
// Round-2 MUST: /Parent cycle must not hang
// ---------------------------------------------------------------------------

/// Build a 1-page PDF whose page dict's /Parent is a two-node cycle
/// (A → B → A). lopdf's parser accepts this — the cycle only manifests
/// when we walk the /Parent chain for inheritable attributes.
fn make_fixture_pdf_with_parent_cycle() -> Vec<u8> {
    let mut doc = Document::with_version("1.4");

    let page_id = doc.new_object_id();
    let parent_id = doc.new_object_id();

    // Page has no /MediaBox — it would be inherited from /Parent under
    // normal inheritance rules; here the cycle means the inheritance
    // walk must terminate anyway.
    let mut page_dict = Dictionary::new();
    page_dict.set("Type", Object::Name(b"Page".to_vec()));
    page_dict.set("Parent", Object::Reference(parent_id));
    page_dict.set("Annots", Object::Array(vec![]));
    doc.set_object(page_id, Object::Dictionary(page_dict));

    // Parent's /Parent points back to page_id → A→B→A cycle. `pages_id`
    // in the catalog also points at parent_id so `get_pages()` still finds
    // the page as page 1.
    let mut parent_dict = Dictionary::new();
    parent_dict.set("Type", Object::Name(b"Pages".to_vec()));
    parent_dict.set("Count", Object::Integer(1));
    parent_dict.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    parent_dict.set("Parent", Object::Reference(page_id));
    doc.set_object(parent_id, Object::Dictionary(parent_dict));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(parent_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut buf: Vec<u8> = Vec::new();
    doc.save_to(&mut buf).unwrap();
    buf
}

#[test]
fn parent_chain_cycle_does_not_hang_direct() {
    // This test passing = the /Parent walk terminated. If the cycle
    // guard regressed to self-reference-only, this test would loop
    // forever (Rust #[test] has no timeout, but cargo test would just
    // hang until CI kills it — the qualitative signal is still clear).
    //
    // `apply_ops` does not read
    // `/MediaBox`, so this test drives `document_page_rotate_and_mediabox`
    // directly to cover the cycle guard.
    let bytes = make_fixture_pdf_with_parent_cycle();
    let doc = Document::load_mem_with_options(&bytes, crate::annots::load_options()).unwrap();
    let page_id = doc.get_pages().get(&1).copied().unwrap();

    // No /MediaBox anywhere in the cycle → should return an Err in
    // finite time.
    let result = crate::annots::document_page_rotate_and_mediabox(&doc, page_id);
    assert!(
        result.is_err(),
        "should not find /MediaBox on a cyclic parent chain"
    );
}

#[test]
fn modify_comment_over_indirect_annots_array() {
    let seed_rect = Rect {
        llx: 100.0,
        lly: 100.0,
        urx: 300.0,
        ury: 120.0,
    };
    let bytes = make_fixture_pdf(
        AnnotsShape::IndirectRef,
        0,
        Some(("Highlight", "mod-ind", seed_rect)),
    );
    let batch = r#"{"ops":[
        {"type":"modify_comment","index":0,"page_index":0,"annot_id":"mod-ind",
         "subtype":"Highlight","user_point":{"x":150.0,"y":110.0},
         "new_comment":"updated"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 1);
    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    let annot_id = find_nm(&doc, "mod-ind").unwrap();
    let dict = doc.get_object(annot_id).unwrap().as_dict().unwrap();
    if let Object::String(bytes, _) = dict.get(b"Contents").unwrap() {
        assert_eq!(bytes[0], 0xFE);
        assert_eq!(bytes[1], 0xFF);
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(String::from_utf16(&units).unwrap(), "updated");
    } else {
        panic!("/Contents unexpected");
    }
}

// ---------------------------------------------------------------------------
// Encrypted-PDF safeguard
// ---------------------------------------------------------------------------
//
// The incremental writer must never touch an encrypted PDF whose key is not
// available: writing an increment against such a trailer drops the
// `/Encrypt` entry and leaves every page unreadable (blank / gibberish) on
// the next open. The safety net originally covered both `can_open` codes 5
// (password_required) and 6 (encrypted). lopdf#522 — which preserves
// `/Encrypt` across incremental writes and re-encrypts new objects under
// the retained key — narrowed it to code 5 only: code 6 files
// (`was_encrypted() && ANNOTABLE`) now round-trip safely, which the tests
// below assert rather than assume.
//
// Fixture strategy: we build minimal 1-page PDFs in-memory the same way
// the plain fixtures above do, add a stable `/ID` to the trailer (all
// standard-security handlers except V5 hash /ID into the file encryption
// key), and then call `Document::encrypt` with a chosen
// `EncryptionVersion`. Four variants are covered:
//
// - RC4 R3 with an empty user password (V2, key_length 128)
// - AESV2 (V4, R4) with an empty user password
// - AESV3 (V5, R6) with an empty user password
// - AESV2 (V4, R4) with a non-empty user password (password_required)

/// Build a 1-page Document with an /ID trailer entry so it can be encrypted
/// by any of the standard-security handlers. Returns the Document *before*
/// serialization so callers can encrypt in place and then save.
fn build_document_with_id() -> Document {
    let mut doc = Document::with_version("1.4");

    let mut page_dict = Dictionary::new();
    page_dict.set("Type", Object::Name(b"Page".to_vec()));
    page_dict.set(
        "MediaBox",
        Object::Array(vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(612.0),
            Object::Real(792.0),
        ]),
    );

    let page_id = doc.new_object_id();
    let pages_id = doc.new_object_id();
    page_dict.set("Parent", Object::Reference(pages_id));
    doc.set_object(page_id, Object::Dictionary(page_dict));

    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Count", Object::Integer(1));
    pages.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    doc.set_object(pages_id, Object::Dictionary(pages));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));

    doc.trailer.set("Root", Object::Reference(catalog_id));
    // Stable /ID: R2/R3/R4 hash the first element into the file-encryption
    // key, so it must exist and be a hex string. Fixed value → deterministic
    // ciphertext across test runs.
    let id_bytes = b"shellac-test-fixture-id-01234567".to_vec();
    let id_element = Object::String(id_bytes.clone(), StringFormat::Hexadecimal);
    doc.trailer
        .set("ID", Object::Array(vec![id_element.clone(), id_element]));

    doc
}

/// A permissive `/P` bitfield — every user-facing capability set,
/// including `ANNOTABLE` so `shellac_can_open` returns `6`
/// (encrypted) rather than `7` (annotations_restricted) for the plain
/// encrypted fixtures.
///
/// Only the flags this crate treats as observable are set here.
/// `MODIFIABLE`, `FILLABLE`, and `ASSEMBLABLE` are left off because they
/// are not required for the encryption fixtures to exercise the code
/// paths we care about (parse succeeds, `was_encrypted() == true`,
/// `ANNOTABLE` present).
fn permissions_all() -> Permissions {
    Permissions::PRINTABLE
        | Permissions::COPYABLE
        | Permissions::ANNOTABLE
        | Permissions::COPYABLE_FOR_ACCESSIBILITY
        | Permissions::PRINTABLE_IN_HIGH_QUALITY
}

/// Permissions with the `ANNOTABLE` bit intentionally cleared. Used to
/// reproduce the field-reported bug where an encrypted PDF
/// with `/P` disabling annotations was silently rejected by PDFKit.
fn permissions_no_annot() -> Permissions {
    Permissions::PRINTABLE
        | Permissions::COPYABLE
        | Permissions::COPYABLE_FOR_ACCESSIBILITY
        | Permissions::PRINTABLE_IN_HIGH_QUALITY
    // Notice: no `Permissions::ANNOTABLE`.
}

/// Encrypt with RC4 R3 (V2, 128-bit) and the given passwords.
fn encrypt_rc4_r3(doc: &mut Document, owner_pw: &str, user_pw: &str) {
    let version = EncryptionVersion::V2 {
        document: doc,
        owner_password: owner_pw,
        user_password: user_pw,
        key_length: 128,
        permissions: permissions_all(),
    };
    let state = EncryptionState::try_from(version).unwrap();
    doc.encrypt(&state).unwrap();
}

/// Encrypt with AESV2 (V4, R4) and the given passwords.
fn encrypt_aes_v4(doc: &mut Document, owner_pw: &str, user_pw: &str) {
    encrypt_aes_v4_with_perms(doc, owner_pw, user_pw, permissions_all())
}

/// Encrypt with AESV2 (V4, R4) and a caller-supplied `/P` bitfield —
/// used by the `annotations_restricted` fixture.
fn encrypt_aes_v4_with_perms(
    doc: &mut Document,
    owner_pw: &str,
    user_pw: &str,
    permissions: Permissions,
) {
    let crypt_filter: Arc<dyn CryptFilter> = Arc::new(Aes128CryptFilter);
    let version = EncryptionVersion::V4 {
        document: doc,
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), crypt_filter)]),
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: owner_pw,
        user_password: user_pw,
        permissions,
    };
    let state = EncryptionState::try_from(version).unwrap();
    doc.encrypt(&state).unwrap();
}

/// Encrypt with AESV3 (V5, R6) and the given passwords. V5 does not hash
/// `/ID` into the key, but the trailer /ID is still legal so we keep it.
fn encrypt_aes_v5(doc: &mut Document, owner_pw: &str, user_pw: &str) {
    let crypt_filter: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
    // Deterministic key so ciphertext is byte-stable across runs (helps
    // when the safety-net assertion checks "post == prev" — that assertion
    // does not actually need determinism, but avoiding rand as a dev-dep
    // keeps the crate lean).
    let file_encryption_key: [u8; 32] = [
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF,
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
        0x0F, 0x10,
    ];
    let version = EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), crypt_filter)]),
        file_encryption_key: &file_encryption_key,
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: owner_pw,
        user_password: user_pw,
        permissions: permissions_all(),
    };
    let state = EncryptionState::try_from(version).unwrap();
    doc.encrypt(&state).unwrap();
}

/// Serialize a Document (encrypted or otherwise) to bytes.
fn save_doc(doc: &mut Document) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    doc.save_to(&mut buf).expect("save encrypted fixture");
    buf
}

fn fixture_rc4_r3_empty_pw() -> Vec<u8> {
    let mut doc = build_document_with_id();
    encrypt_rc4_r3(&mut doc, "", "");
    save_doc(&mut doc)
}

fn fixture_aes_v4_empty_pw() -> Vec<u8> {
    let mut doc = build_document_with_id();
    encrypt_aes_v4(&mut doc, "", "");
    save_doc(&mut doc)
}

fn fixture_aes_v5_empty_pw() -> Vec<u8> {
    let mut doc = build_document_with_id();
    encrypt_aes_v5(&mut doc, "", "");
    save_doc(&mut doc)
}

/// AESV2 encrypted with both owner AND user passwords set to "secret" —
/// the empty user password can NOT authenticate this, so lopdf leaves
/// /Encrypt in the trailer and `is_encrypted()` returns true after load.
fn fixture_password_required() -> Vec<u8> {
    let mut doc = build_document_with_id();
    encrypt_aes_v4(&mut doc, "secret-owner", "secret-user");
    save_doc(&mut doc)
}

/// AESV2 encrypted with empty user password AND `/P` with the ANNOTABLE
/// bit cleared — the field-reported bug case. lopdf opens
/// the document (empty user pw authenticates), so `was_encrypted()` is
/// true, but `document.encryption_state.permissions()` reports
/// `ANNOTABLE` clear and `shellac_can_open` returns `7`.
fn fixture_aes_v4_annotations_restricted() -> Vec<u8> {
    let mut doc = build_document_with_id();
    encrypt_aes_v4_with_perms(&mut doc, "", "", permissions_no_annot());
    save_doc(&mut doc)
}

/// Run `apply_ops` against `bytes` and return `(status, post_bytes)`.
/// Uses the same temp-file convention as `apply_ops_on_bytes`.
fn apply_ops_on_encrypted(bytes: &[u8], batch_json: &str) -> (ApplyResult, Vec<u8>) {
    apply_ops_on_bytes(bytes, batch_json)
}

/// A tiny add batch to feed apply_ops with. Used across the
/// "encrypted round-trip" tests below — the payload itself does not need
/// to exercise any interesting op semantics, only prove that an
/// increment landed for encryption fixtures that previously refused.
const SAMPLE_ADD_BATCH: &str = r#"{"ops":[
    {"type":"add","index":0,"page_index":0,"annot_id":"enc-roundtrip",
     "subtype":"Highlight",
     "rect":{"llx":100.0,"lly":100.0,"urx":200.0,"ury":120.0},
     "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"x"}
]}"#;

/// Removes the highlight added by `SAMPLE_ADD_BATCH` (by /NM) on the
/// same page — companion to the 2-stage round-trip test below.
const SAMPLE_REMOVE_BATCH: &str = r#"{"ops":[
    {"type":"remove","index":0,"page_index":0,"annot_id":"enc-roundtrip",
     "subtype":"Highlight",
     "user_point":{"x":0.0,"y":0.0}}
]}"#;

/// With the lopdf#522 fix in place, `was_encrypted()` PDFs (empty-password
/// auto-decrypt succeeded — lopdf still holds the encryption_state)
/// round-trip through incremental save with /Encrypt preserved. Before that
/// fix these same fixtures were refused and left alone.
///
/// Each variant checks:
///   1. status is `Ok` (no `EncryptedRefused` for `was_encrypted`).
///   2. `applied == 1` (the highlight from `SAMPLE_ADD_BATCH` landed).
///   3. `post != bytes` (an increment was actually written).
///   4. The post-increment document reloads with `was_encrypted() == true`,
///      proving lopdf preserved encryption_state through the round-trip.

#[test]
fn apply_ops_round_trips_rc4_r3_and_preserves_encryption() {
    let bytes = fixture_rc4_r3_empty_pw();
    let (res, post) = apply_ops_on_encrypted(&bytes, SAMPLE_ADD_BATCH);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 1);
    assert!(res.skipped.is_empty());
    assert_ne!(
        post, bytes,
        "incremental increment must actually append bytes"
    );
    let post_doc = Document::load_mem_with_options(&post, crate::annots::load_options())
        .expect("post-increment doc must remain parseable");
    assert!(
        post_doc.was_encrypted(),
        "post-increment doc must retain encryption (lopdf#522 contract)"
    );
}

#[test]
fn apply_ops_round_trips_aes_v4_and_preserves_encryption() {
    let bytes = fixture_aes_v4_empty_pw();
    let (res, post) = apply_ops_on_encrypted(&bytes, SAMPLE_ADD_BATCH);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 1);
    assert!(res.skipped.is_empty());
    assert_ne!(post, bytes);
    let post_doc = Document::load_mem_with_options(&post, crate::annots::load_options())
        .expect("post-increment doc must remain parseable");
    assert!(post_doc.was_encrypted());
}

#[test]
fn apply_ops_round_trips_aes_v5_and_preserves_encryption() {
    let bytes = fixture_aes_v5_empty_pw();
    let (res, post) = apply_ops_on_encrypted(&bytes, SAMPLE_ADD_BATCH);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 1);
    assert!(res.skipped.is_empty());
    assert_ne!(post, bytes);
    let post_doc = Document::load_mem_with_options(&post, crate::annots::load_options())
        .expect("post-increment doc must remain parseable");
    assert!(post_doc.was_encrypted());
}

#[test]
fn apply_ops_refuses_password_required_and_leaves_bytes_untouched() {
    // The `is_encrypted()` path (auto-decrypt failed — no
    // encryption_state retained) still refuses. Without a key lopdf has
    // nothing to encrypt new objects under, so an incremental round-trip
    // would leave the trailer /Encrypt pointing at data written in the
    // clear.
    let bytes = fixture_password_required();
    let (res, post) = apply_ops_on_encrypted(&bytes, SAMPLE_ADD_BATCH);
    assert_eq!(res.status, Status::EncryptedRefused);
    assert_eq!(res.applied, 0);
    assert_eq!(post, bytes);
}

#[test]
fn apply_ops_round_trips_aes_v4_add_then_remove_preserves_encryption() {
    // Rust-level companion to the shell-level verify-encrypted-roundtrip.sh
    // suite: one variant exercised in Rust so CI catches a regression even
    // if the shell script is skipped locally. Add → remove keeps the
    // Rust engine on the encrypted incremental path across two saves and
    // proves that /NM lookup still resolves after the first increment.
    let bytes = fixture_aes_v4_empty_pw();

    let (r_add, bytes_a) = apply_ops_on_encrypted(&bytes, SAMPLE_ADD_BATCH);
    assert_eq!(r_add.status, Status::Ok);
    assert_eq!(r_add.applied, 1);
    assert!(r_add.skipped.is_empty());
    assert_ne!(bytes_a, bytes, "add increment must append bytes");
    let doc_a = Document::load_mem_with_options(&bytes_a, crate::annots::load_options())
        .expect("post-add doc must remain parseable");
    assert!(doc_a.was_encrypted());

    let (r_rm, bytes_b) = apply_ops_on_encrypted(&bytes_a, SAMPLE_REMOVE_BATCH);
    assert_eq!(r_rm.status, Status::Ok);
    assert_eq!(r_rm.applied, 1);
    assert!(r_rm.skipped.is_empty());
    assert_ne!(bytes_b, bytes_a, "remove increment must append bytes");
    let doc_b = Document::load_mem_with_options(&bytes_b, crate::annots::load_options())
        .expect("post-remove doc must remain parseable");
    assert!(
        doc_b.was_encrypted(),
        "encryption must survive both increments in a row"
    );
}

// can_open: exercise the newly-added return codes 5/6 via the FFI entry
// point. We use `shellac_can_open` directly (with a real temp path)
// rather than a helper on the internal logic — this catches regressions
// in the FFI shim as well.

use std::ffi::CString;

fn run_can_open(bytes: &[u8]) -> i32 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "shellac-can-open-test-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fixture.pdf");
    fs::write(&path, bytes).unwrap();
    let c_path = CString::new(path.to_string_lossy().as_bytes()).unwrap();
    let code = unsafe { crate::shellac_can_open(c_path.as_ptr()) };
    let _ = fs::remove_dir_all(&dir);
    code
}

#[test]
fn can_open_returns_ok_for_plain_pdf() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    // 0 = OK
    assert_eq!(run_can_open(&bytes), 0);
}

#[test]
fn can_open_returns_encrypted_for_empty_pw_encrypted_pdf() {
    let bytes = fixture_aes_v4_empty_pw();
    // 6 = CAN_OPEN_ENCRYPTED (empty user password auto-decrypted)
    assert_eq!(run_can_open(&bytes), 6);
}

#[test]
fn can_open_returns_password_required_for_password_protected_pdf() {
    let bytes = fixture_password_required();
    // 5 = CAN_OPEN_PASSWORD_REQUIRED
    assert_eq!(run_can_open(&bytes), 5);
}

#[test]
fn can_open_encrypted_variants_all_report_encrypted_or_password_required() {
    // Sanity: RC4 R3 empty pw → 6, V5 empty pw → 6 (already covered above
    // for V4; this test locks in the other two variants).
    assert_eq!(run_can_open(&fixture_rc4_r3_empty_pw()), 6);
    assert_eq!(run_can_open(&fixture_aes_v5_empty_pw()), 6);
}

#[test]
fn can_open_returns_annotations_restricted_when_p_disables_annotable() {
    // Field-reported bug: PDF is encrypted, empty user password
    // authenticates, but `/P` has the ANNOTABLE bit cleared. Such a file
    // used to be reported as plain `encrypted` (6), which told a caller
    // nothing about the permission — and PDFKit's own `write(to:)` fails
    // silently on the permissions check, so the user saw a save that
    // appeared to work and did not. Reporting 7 lets a caller refuse the
    // annotate operation up front and say why.
    let bytes = fixture_aes_v4_annotations_restricted();
    assert_eq!(run_can_open(&bytes), 7);
}

#[test]
fn apply_ops_refuses_annotations_restricted_and_leaves_bytes_untouched() {
    // Defense in depth. The fixture uses an empty user password
    // (auto-decrypt succeeds — `was_encrypted() == true`,
    // `is_encrypted() == false`) with `/P` clearing the ANNOTABLE bit. A
    // caller that probes the file first (`can_open` == 7) would normally
    // never enqueue the batch, but probing is asynchronous: a save queued
    // while the document is opening can race past the caller's own guard.
    // In that window the engine must refuse on its own and leave the file
    // byte-for-byte untouched. The status is distinct from
    // `EncryptedRefused` so the caller can upgrade whatever it cached
    // instead of mislabeling the file as password-protected.
    let bytes = fixture_aes_v4_annotations_restricted();
    let (res, post) = apply_ops_on_encrypted(&bytes, SAMPLE_ADD_BATCH);
    assert_eq!(res.status, Status::AnnotationsRestricted);
    assert_eq!(res.applied, 0);
    assert!(res.skipped.is_empty());
    assert_eq!(
        post, bytes,
        "annotations_restricted must not touch the file"
    );
}

// ---------------------------------------------------------------------------
// Vertical Highlight quad reordering (Acrobat [BL, TL, BR, TR])
// ---------------------------------------------------------------------------
//
// Callers send `/QuadPoints` in PDFKit Z-order `[TL, TR, BL, BR]` per
// quad. For **vertical** (`H > W`) Highlight quads that ordering is
// re-interpreted by Acrobat / Preview's shared imaging pipeline as a
// bow-tie polygon instead of the intended rectangle — established by
// rendering the same file across every viewer, not by reading a spec.
// `apply_ops` reorders those quads to `[BL, TL, BR, TR]` so every viewer
// sees a rectangle. Horizontal quads and non-Highlight subtypes are
// untouched.

fn read_quads(doc: &Document, annot_id: ObjectId) -> Vec<f64> {
    let quads = doc
        .get_object(annot_id)
        .unwrap()
        .as_dict()
        .unwrap()
        .get(b"QuadPoints")
        .unwrap()
        .as_array()
        .unwrap();
    quads
        .iter()
        .map(|o| match o {
            Object::Real(r) => *r as f64,
            Object::Integer(n) => *n as f64,
            _ => panic!("unexpected QuadPoint element"),
        })
        .collect()
}

fn assert_quads_approx(got: &[f64], want: &[f64]) {
    assert_eq!(got.len(), want.len(), "QuadPoints length differs");
    for i in 0..got.len() {
        assert!(
            (got[i] - want[i]).abs() < 0.01,
            "QuadPoints[{}] = {} want {}",
            i,
            got[i],
            want[i]
        );
    }
}

#[test]
fn s502_vertical_highlight_quad_is_reordered_to_bl_tl_br_tr() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    // Vertical quad: `H > W`. Caller-side Z-order: TL, TR, BL, BR.
    //   TL = (100, 200), TR = (110, 200), BL = (100, 100), BR = (110, 100)
    // Expected on-disk order: BL, TL, BR, TR.
    //   BL = (100, 100), TL = (100, 200), BR = (110, 100), TR = (110, 200)
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"v-hl",
         "subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":110.0,"ury":200.0},
         "quad_points":[[100.0,200.0],[110.0,200.0],[100.0,100.0],[110.0,100.0]],
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"v"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    let annot_id = find_nm(&doc, "v-hl").unwrap();
    assert_quads_approx(
        &read_quads(&doc, annot_id),
        &[
            100.0, 100.0, // BL
            100.0, 200.0, // TL
            110.0, 100.0, // BR
            110.0, 200.0, // TR
        ],
    );
}

#[test]
fn s502_horizontal_highlight_quad_preserves_input_order() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    // Horizontal quad: `W > H`. Caller-side Z-order preserved verbatim.
    //   TL = (100, 220), TR = (300, 220), BL = (100, 200), BR = (300, 200)
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"h-hl",
         "subtype":"Highlight",
         "rect":{"llx":100.0,"lly":200.0,"urx":300.0,"ury":220.0},
         "quad_points":[[100.0,220.0],[300.0,220.0],[100.0,200.0],[300.0,200.0]],
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"h"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    let annot_id = find_nm(&doc, "h-hl").unwrap();
    assert_quads_approx(
        &read_quads(&doc, annot_id),
        &[
            100.0, 220.0, // TL
            300.0, 220.0, // TR
            100.0, 200.0, // BL
            300.0, 200.0, // BR
        ],
    );
}

#[test]
fn s502_multi_vertical_highlight_quads_are_reordered_independently() {
    // Two vertical columns in one Highlight: each quad reorders to
    // [BL, TL, BR, TR] independently.
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"v-hl-2col",
         "subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":130.0,"ury":200.0},
         "quad_points":[
             [100.0,200.0],[110.0,200.0],[100.0,100.0],[110.0,100.0],
             [120.0,200.0],[130.0,200.0],[120.0,100.0],[130.0,100.0]
         ],
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"vv"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    let annot_id = find_nm(&doc, "v-hl-2col").unwrap();
    assert_quads_approx(
        &read_quads(&doc, annot_id),
        &[
            // Column 1: BL, TL, BR, TR
            100.0, 100.0, 100.0, 200.0, 110.0, 100.0, 110.0, 200.0,
            // Column 2: BL, TL, BR, TR
            120.0, 100.0, 120.0, 200.0, 130.0, 100.0, 130.0, 200.0,
        ],
    );
}

#[test]
fn s502_vertical_underline_quad_preserves_input_order() {
    // Non-Highlight subtypes never reorder — even for vertical quads.
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"v-ul",
         "subtype":"Underline",
         "rect":{"llx":100.0,"lly":100.0,"urx":110.0,"ury":200.0},
         "quad_points":[[100.0,200.0],[110.0,200.0],[100.0,100.0],[110.0,100.0]],
         "color":[0.9,0.2,0.3],"opacity":0.5,"contents":"vu"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    let annot_id = find_nm(&doc, "v-ul").unwrap();
    assert_quads_approx(
        &read_quads(&doc, annot_id),
        &[
            100.0, 200.0, // TL
            110.0, 200.0, // TR
            100.0, 100.0, // BL
            110.0, 100.0, // BR
        ],
    );
}

#[test]
fn s502_vertical_strikeout_quad_preserves_input_order() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"v-so",
         "subtype":"StrikeOut",
         "rect":{"llx":100.0,"lly":100.0,"urx":110.0,"ury":200.0},
         "quad_points":[[100.0,200.0],[110.0,200.0],[100.0,100.0],[110.0,100.0]],
         "color":[0.9,0.2,0.3],"opacity":0.5,"contents":"vs"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    let annot_id = find_nm(&doc, "v-so").unwrap();
    assert_quads_approx(
        &read_quads(&doc, annot_id),
        &[100.0, 200.0, 110.0, 200.0, 100.0, 100.0, 110.0, 100.0],
    );
}

// ---------------------------------------------------------------------------
// Self-generated Highlight AP (/AP /N Form XObject)
// ---------------------------------------------------------------------------

/// Follow `/AP /N` to the Form XObject stream for an annotation, returning
/// the Stream's dictionary and content bytes.
fn read_highlight_ap(doc: &Document, annot_id: ObjectId) -> (&Dictionary, Vec<u8>) {
    let ap = doc
        .get_object(annot_id)
        .unwrap()
        .as_dict()
        .unwrap()
        .get(b"AP")
        .expect("Highlight must carry /AP")
        .as_dict()
        .unwrap();
    let n_ref = ap
        .get(b"N")
        .expect("/AP must have /N")
        .as_reference()
        .expect("/AP /N must be an indirect reference");
    let stream = doc
        .get_object(n_ref)
        .expect("/AP /N target must exist")
        .as_stream()
        .expect("/AP /N must point at a stream");
    // Streams may be flate-compressed on disk; decode for inspection.
    let content = stream
        .get_plain_content()
        .unwrap_or_else(|_| stream.content.clone());
    (&stream.dict, content)
}

fn count_substring(hay: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if &hay[i..i + needle.len()] == needle {
            count += 1;
            i += needle.len();
        } else {
            i += 1;
        }
    }
    count
}

#[test]
fn s502_highlight_add_produces_self_generated_ap() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"ap-hl",
         "subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":300.0,"ury":120.0},
         "quad_points":[[100.0,120.0],[300.0,120.0],[100.0,100.0],[300.0,100.0]],
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"hl"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 1);

    // Use plain (non-filtered) load so the AP stream content survives —
    // `load_options()` intentionally strips non-structural stream bytes.
    let doc = Document::load_mem(&post).unwrap();
    let annot_id = find_nm(&doc, "ap-hl").unwrap();
    let (dict, content) = read_highlight_ap(&doc, annot_id);
    let content = content.as_slice();

    // Type / Subtype / FormType.
    assert_eq!(dict.get(b"Type").unwrap().as_name().unwrap(), b"XObject");
    assert_eq!(dict.get(b"Subtype").unwrap().as_name().unwrap(), b"Form");
    assert_eq!(dict.get(b"FormType").unwrap().as_i64().unwrap(), 1);

    // BBox = [0 0 W H].
    let bbox = dict.get(b"BBox").unwrap().as_array().unwrap();
    let bbox_vals: Vec<f64> = bbox
        .iter()
        .map(|o| match o {
            Object::Real(r) => *r as f64,
            Object::Integer(n) => *n as f64,
            _ => panic!(),
        })
        .collect();
    assert!((bbox_vals[0] - 0.0).abs() < 0.01);
    assert!((bbox_vals[1] - 0.0).abs() < 0.01);
    assert!((bbox_vals[2] - 200.0).abs() < 0.01);
    assert!((bbox_vals[3] - 20.0).abs() < 0.01);

    // Matrix = [1 0 0 1 llx lly].
    let matrix = dict.get(b"Matrix").unwrap().as_array().unwrap();
    let m_vals: Vec<f64> = matrix
        .iter()
        .map(|o| match o {
            Object::Real(r) => *r as f64,
            Object::Integer(n) => *n as f64,
            _ => panic!(),
        })
        .collect();
    assert!((m_vals[0] - 1.0).abs() < 0.01);
    assert!((m_vals[1] - 0.0).abs() < 0.01);
    assert!((m_vals[2] - 0.0).abs() < 0.01);
    assert!((m_vals[3] - 1.0).abs() < 0.01);
    assert!((m_vals[4] - 100.0).abs() < 0.01);
    assert!((m_vals[5] - 100.0).abs() < 0.01);

    // Resources.ExtGState.GS0 with BM Multiply, CA=1, ca=1.
    let resources = dict.get(b"Resources").unwrap().as_dict().unwrap();
    let ext = resources.get(b"ExtGState").unwrap().as_dict().unwrap();
    let gs0 = ext.get(b"GS0").unwrap().as_dict().unwrap();
    assert_eq!(gs0.get(b"BM").unwrap().as_name().unwrap(), b"Multiply");
    let ca_val = match gs0.get(b"CA").unwrap() {
        Object::Real(r) => *r as f64,
        Object::Integer(n) => *n as f64,
        _ => panic!(),
    };
    let small_ca_val = match gs0.get(b"ca").unwrap() {
        Object::Real(r) => *r as f64,
        Object::Integer(n) => *n as f64,
        _ => panic!(),
    };
    assert!((ca_val - 1.0).abs() < 0.01);
    assert!((small_ca_val - 1.0).abs() < 0.01);

    // Content: /GS0 gs, one `re f` command per quad (1 quad here), and
    // an `rg` command with the highlight colour.
    assert_eq!(count_substring(content, b"/GS0 gs"), 1);
    assert_eq!(count_substring(content, b" re "), 1, "one re per quad");
    assert_eq!(count_substring(content, b" rg"), 1, "one colour set");
    let content_str = std::str::from_utf8(content).unwrap();
    // The exact `rg` command should carry our r/g/b values (0.8 as 0.8 or
    // similar float rendering — accept both `0.8` and `0.800...`).
    assert!(
        content_str.contains("1 0.8 0 rg")
            || content_str.contains("1.0 0.8 0.0 rg")
            || content_str.contains("1 0.8 0.0 rg")
            || content_str.contains("1.0 0.8 0 rg"),
        "content must contain the highlight colour rg: {}",
        content_str
    );
}

#[test]
fn s502_highlight_ap_has_one_re_per_quad() {
    // 3 quads → 3 `re f` commands.
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"ap-3quads",
         "subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":140.0,"ury":200.0},
         "quad_points":[
             [100.0,200.0],[110.0,200.0],[100.0,100.0],[110.0,100.0],
             [120.0,200.0],[130.0,200.0],[120.0,100.0],[130.0,100.0],
             [130.0,200.0],[140.0,200.0],[130.0,100.0],[140.0,100.0]
         ],
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"3q"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    let doc = Document::load_mem(&post).unwrap();
    let annot_id = find_nm(&doc, "ap-3quads").unwrap();
    let (_dict, content) = read_highlight_ap(&doc, annot_id);
    assert_eq!(count_substring(&content, b" re "), 3, "one re per quad");
}

#[test]
fn s502_underline_add_has_no_ap() {
    // Regression: Underline / StrikeOut must NOT carry `/AP`. Their PDFKit
    // display path is line-based, and Preview / Acrobat draw them fine
    // from `/QuadPoints` alone; a self-generated AP could actually make
    // them disappear (a Form XObject with no line commands would produce
    // an empty appearance).
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"no-ap-ul",
         "subtype":"Underline",
         "rect":{"llx":100.0,"lly":100.0,"urx":300.0,"ury":120.0},
         "quad_points":[[100.0,120.0],[300.0,120.0],[100.0,100.0],[300.0,100.0]],
         "color":[0.9,0.2,0.3],"opacity":0.5,"contents":"ul"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    let annot_id = find_nm(&doc, "no-ap-ul").unwrap();
    let dict = doc.get_object(annot_id).unwrap().as_dict().unwrap();
    assert!(dict.get(b"AP").is_err(), "Underline must not carry /AP");
}

#[test]
fn s502_strikeout_add_has_no_ap() {
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"no-ap-so",
         "subtype":"StrikeOut",
         "rect":{"llx":100.0,"lly":100.0,"urx":300.0,"ury":120.0},
         "quad_points":[[100.0,120.0],[300.0,120.0],[100.0,100.0],[300.0,100.0]],
         "color":[0.9,0.2,0.3],"opacity":0.5,"contents":"so"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    let doc = Document::load_mem_with_options(&post, crate::annots::load_options()).unwrap();
    let annot_id = find_nm(&doc, "no-ap-so").unwrap();
    let dict = doc.get_object(annot_id).unwrap().as_dict().unwrap();
    assert!(dict.get(b"AP").is_err(), "StrikeOut must not carry /AP");
}

#[test]
fn s502_highlight_add_without_quad_points_still_gets_ap() {
    // Fallback: when `quad_points` is omitted, the AP still covers the
    // four corners of `/Rect`.
    let bytes = make_fixture_pdf(AnnotsShape::DirectArray, 0, None);
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"ap-rect",
         "subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":300.0,"ury":120.0},
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"r"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    let doc = Document::load_mem(&post).unwrap();
    let annot_id = find_nm(&doc, "ap-rect").unwrap();
    let (_dict, content) = read_highlight_ap(&doc, annot_id);
    assert_eq!(count_substring(&content, b"/GS0 gs"), 1);
    // Rect fallback → exactly one `re f`.
    assert_eq!(count_substring(&content, b" re "), 1);
}

#[test]
fn s502_highlight_ap_survives_encrypted_incremental_roundtrip() {
    // Encrypted round-trip: the incremental writer must re-encrypt the
    // AP stream under the retained key. On reload, the AP stream must
    // decrypt to the expected content (`/GS0 gs` + one `re f`).
    let bytes = fixture_aes_v4_empty_pw();
    let batch = r#"{"ops":[
        {"type":"add","index":0,"page_index":0,"annot_id":"enc-ap-hl",
         "subtype":"Highlight",
         "rect":{"llx":100.0,"lly":100.0,"urx":200.0,"ury":120.0},
         "quad_points":[[100.0,120.0],[200.0,120.0],[100.0,100.0],[200.0,100.0]],
         "color":[1.0,0.8,0.0],"opacity":0.5,"contents":"e"}
    ]}"#;
    let (res, post) = apply_ops_on_bytes(&bytes, batch);
    assert_eq!(res.status, Status::Ok);
    assert_eq!(res.applied, 1);
    assert_ne!(post, bytes);

    // Plain load (no strip filter, no explicit password — empty pw auto-
    // decrypts) so we can inspect the AP stream's decrypted content.
    let doc = Document::load_mem(&post).expect("encrypted post-doc must reload");
    assert!(doc.was_encrypted(), "encryption must survive round-trip");
    let annot_id = find_nm(&doc, "enc-ap-hl").expect("annotation must round-trip");
    let (dict, content) = read_highlight_ap(&doc, annot_id);
    assert_eq!(
        dict.get(b"Subtype").unwrap().as_name().unwrap(),
        b"Form",
        "AP dictionary must decrypt to a Form XObject"
    );
    assert_eq!(
        count_substring(&content, b"/GS0 gs"),
        1,
        "AP content stream must decrypt to the expected commands"
    );
    assert_eq!(count_substring(&content, b" re "), 1);
}
