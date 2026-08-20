//! Fixture-based tests over real external-tool output: qpdf --json=2
//! documents, pdfkit helper JSON, and pdftotext -bbox-layout XHTML,
//! captured from actual corpus runs (annotation ids normalized to the
//! test-verify prefix).

use std::collections::BTreeMap;

use verify::anchor::{TextAnchor, anchor_from_words, choose_needle_word};
use verify::checks::bbox::{PosCheck, PosDerive, evaluate_c11a, parse_page_dims, parse_words};
use verify::checks::pdfkit::{PdfkitCheck, evaluate_c7, evaluate_c11b};
use verify::checks::qpdf::{
    NmInfo, PageGeometry, QpdfDoc, QuadCheck, evaluate_c6, evaluate_c6_quads,
};
use verify::geom::{Rect, derive_highlight_rect};

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

const HL: &str = "test-verify-hl-1";
const UL: &str = "test-verify-ul-1";
const ML: &str = "test-verify-hl-multiline-1";

#[test]
fn qpdf_add_document_reports_attached_annotations() {
    let doc = QpdfDoc::parse(fixture("qpdf-s1-add.json").as_bytes()).unwrap();
    let nm = doc.find_nm(&ids(&[HL, UL]));
    let hl = &nm[HL];
    assert!(hl.found && hl.page_attached && hl.has_ap);
    let r = hl.rect.unwrap();
    assert!((r[0] - 498.8049).abs() < 1e-6, "hl llx: {}", r[0]);
    assert!((r[3] - 743.45).abs() < 1e-6, "hl ury: {}", r[3]);
    let ul = &nm[UL];
    assert!(ul.found && ul.page_attached);
    assert!(!ul.has_ap, "underline must not carry /AP");

    let present = ids(&[HL, UL]);
    let out = evaluate_c6(&present, &[], Ok(&nm), "");
    assert!(out.pass);
    assert_eq!(
        out.detail,
        format!("present=[{HL} {UL}] absent=[] confirmed via qpdf json (object + page attachment)")
    );
    assert_eq!(
        out.ap_by_id,
        vec![(HL.to_string(), true), (UL.to_string(), false)]
    );
}

#[test]
fn qpdf_removed_document_passes_absent_check() {
    let doc = QpdfDoc::parse(fixture("qpdf-s1-removed.json").as_bytes()).unwrap();
    let absent = ids(&[HL, UL]);
    let nm = doc.find_nm(&absent);
    assert!(!nm[HL].found && !nm[HL].page_attached);
    let out = evaluate_c6(&[], &absent, Ok(&nm), "");
    assert!(out.pass, "{}", out.detail);
}

#[test]
fn qpdf_multiline_document_passes_quad_check() {
    let doc = QpdfDoc::parse(fixture("qpdf-s1-multiline.json").as_bytes()).unwrap();
    let nm = doc.find_nm(&ids(&[ML]));
    assert!(nm[ML].found && nm[ML].page_attached);
    assert_eq!(nm[ML].quad_points.as_ref().map(|q| q.len()), Some(16));
    let (pass, detail, logs) = evaluate_c6_quads(
        &QuadCheck {
            id: ML.to_string(),
            min_points: 8,
        },
        &nm,
    );
    assert!(pass, "{detail}");
    assert_eq!(
        detail,
        format!("{ML}: /QuadPoints has 16 floats = 2 rects (want ≥ 2 rects)")
    );
    assert!(logs.is_empty());
}

#[test]
fn qpdf_classic_xref_document_parses_like_stream_xref() {
    // The S3-derived fixture uses a different xref arrangement than S1;
    // both must resolve page attachment identically.
    let doc = QpdfDoc::parse(fixture("qpdf-s3-add.json").as_bytes()).unwrap();
    let nm = doc.find_nm(&ids(&[HL, UL]));
    assert!(nm[HL].found && nm[HL].page_attached);
    assert!(nm[UL].found && nm[UL].page_attached);
    assert_eq!(nm[HL].rect, Some([100.0, 100.0, 300.0, 120.0]));
}

#[test]
fn qpdf_encrypted_document_parses_and_attaches() {
    let doc = QpdfDoc::parse(fixture("qpdf-s9-add.json").as_bytes()).unwrap();
    let nm = doc.find_nm(&ids(&[HL, UL]));
    assert!(
        nm[HL].found && nm[HL].page_attached,
        "encrypted add must be visible"
    );
    assert!(nm[UL].found && nm[UL].page_attached);
}

#[test]
fn pdfkit_check_add_output_passes_c7_and_c11b() {
    let raw = fixture("pdfkit-check-s1-add.json");
    let ck: PdfkitCheck = serde_json::from_str(&raw).unwrap();
    assert!(ck.loaded && ck.thumbnail_ok);
    assert_eq!(ck.page_count, 1);
    assert_eq!(ck.found_nm, vec![HL, UL]);

    let out = evaluate_c7(&ck, &raw, 1, &ids(&[HL, UL]), &[]);
    assert!(out.pass, "{}", out.detail);
    assert_eq!(
        out.detail,
        "loaded=true pages=1/1 missingNM=[] unexpectedNM=[] thumbnails=true annotTypes=[Highlight Underline]"
    );

    let c11b = evaluate_c11b(true, Ok(&ck));
    assert!(c11b.pass);
    assert!(c11b.detail.contains("maxDelta="));
}

#[test]
fn pdfkit_check_baseline_output_reports_unevaluated_c11b() {
    let raw = fixture("pdfkit-check-s1-base.json");
    let ck: PdfkitCheck = serde_json::from_str(&raw).unwrap();
    assert!(!ck.c11b_checked);
    assert!(ck.found_nm.is_empty());
}

fn anchor_from_fixture(name: &str) -> TextAnchor {
    let v: serde_json::Value = serde_json::from_str(&fixture(name)).unwrap();
    let rect = |k: &str| {
        let r = &v[k];
        Rect::new(
            r["llx"].as_f64().unwrap(),
            r["lly"].as_f64().unwrap(),
            r["urx"].as_f64().unwrap(),
            r["ury"].as_f64().unwrap(),
        )
    };
    TextAnchor {
        found: v["found"].as_bool().unwrap(),
        needle: v["needle"].as_str().unwrap().to_string(),
        user_bounds: if v["found"].as_bool().unwrap() {
            rect("bounds")
        } else {
            Rect::default()
        },
        page_rotation: v["pageRotation"].as_i64().unwrap(),
        media_box: rect("mediaBox"),
        crop_box: rect("cropBox"),
    }
}

#[test]
fn textbbox_fixtures_expose_anchor_and_rotation() {
    let s1 = anchor_from_fixture("textbbox-s1.json");
    assert!(s1.found);
    assert_eq!(s1.needle, "銀河鉄道の夜");
    assert_eq!(s1.page_rotation, 0);
    let s3 = anchor_from_fixture("textbbox-s3.json");
    assert!(!s3.found);
    let s8 = anchor_from_fixture("textbbox-s8.json");
    assert!(s8.found);
    assert_eq!(s8.page_rotation, 90);
    // PDFKit reports unrotated user-space bounds: identical to S1's.
    assert_eq!(s8.user_bounds, s1.user_bounds);
}

#[test]
fn bbox_fixture_dims_and_candidate_counts() {
    for (file, needle, expected) in [
        ("bbox-s1.xml", "銀河鉄道の夜", 2usize),
        ("bbox-s7.xml", "PREFACE", 1),
        ("bbox-s8.xml", "銀河鉄道の夜", 2),
    ] {
        let s = fixture(file);
        let (w, h) = parse_page_dims(&s).unwrap();
        assert!(w > 500.0 && h > 500.0, "{file}: dims {w}x{h}");
        let words = parse_words(&s);
        assert!(words.len() > 10, "{file}: only {} words", words.len());
        let hits = words.iter().filter(|x| x.text.trim() == needle).count();
        assert_eq!(hits, expected, "{file}: needle candidates");
    }
}

#[test]
fn bbox_s8_reports_unrotated_page_dims() {
    // poppler reports S8's page box in the same (unrotated) dims as S1.
    let (w1, h1) = parse_page_dims(&fixture("bbox-s1.xml")).unwrap();
    let (w8, h8) = parse_page_dims(&fixture("bbox-s8.xml")).unwrap();
    assert_eq!((w1, h1), (w8, h8));
}

/// Real-data C11a evaluation over the S1 fixtures: the needle appears
/// twice in pdftotext output, and the selection must prefer the candidate
/// nearest to the PDFKit anchor bounds (the vertical title column near
/// x=500), not the first candidate in document order (near x=77).
#[test]
fn c11a_real_s1_data_selects_nearest_candidate() {
    let anchor = anchor_from_fixture("textbbox-s1.json");
    let words = parse_words(&fixture("bbox-s1.xml"));
    let doc = QpdfDoc::parse(fixture("qpdf-s1-add.json").as_bytes()).unwrap();
    let nm = doc.find_nm(&ids(&[HL, UL]));
    let nm: BTreeMap<String, NmInfo> = nm.into_iter().collect();
    let out = evaluate_c11a(
        &anchor,
        &[
            PosCheck {
                id: HL.to_string(),
                derive: PosDerive::Highlight,
            },
            PosCheck {
                id: UL.to_string(),
                derive: PosDerive::Underline,
            },
        ],
        &words,
        &nm,
    );
    let sel = out.selection.expect("selection must be recorded");
    assert_eq!(sel.candidates, 2);
    assert!(
        sel.chosen.llx > 400.0,
        "must select the vertical-column candidate, got {}",
        sel.chosen
    );
    assert!(
        out.detail
            .contains("needle matched 2 times on page 1, selected nearest to PDFKit anchor bounds")
    );
}

// ---- the PDFKit-free anchor path ----

/// S1's page geometry as qpdf reports it: an unrotated A4 MediaBox.
fn s1_geometry(rotate: i64) -> PageGeometry {
    PageGeometry {
        media_box: Rect::new(0.0, 0.0, 595.486, 842.202),
        rotate,
    }
}

#[test]
fn the_poppler_anchor_picks_the_same_needle_text_as_pdfkit_on_s1() {
    let words = parse_words(&fixture("bbox-s1.xml"));
    let chosen = choose_needle_word(&words).expect("S1 page 1 has words");
    assert_eq!(
        chosen.text, "銀河鉄道の夜",
        "the first poppler word of at least three characters"
    );
    assert_eq!(
        chosen.text,
        anchor_from_fixture("textbbox-s1.json").needle,
        "the two extractors agree on the needle text for this page"
    );
}

#[test]
fn the_poppler_anchor_maps_a_rotated_page_to_the_same_user_bounds() {
    // S8 is S1 with /Rotate 90: poppler reports the words already rotated,
    // so the transform must land on S1's unrotated user-space bounds.
    let s1 = anchor_from_words(&parse_words(&fixture("bbox-s1.xml")), &s1_geometry(0));
    let s8 = anchor_from_words(&parse_words(&fixture("bbox-s8.xml")), &s1_geometry(90));
    assert!(s1.found && s8.found);
    assert_eq!(s1.needle, s8.needle);
    let (a, b) = (s1.user_bounds, s8.user_bounds);
    assert!(
        a.close_to(b, 1e-6),
        "rotated bounds {b} should equal unrotated {a}"
    );
}

#[test]
fn the_poppler_anchor_selects_the_first_page_occurrence_not_pdfkits() {
    // The needle occurs twice on S1's page 1. PDFKit's helper reports the
    // vertical title column (x > 400); the poppler rule takes the first
    // word in document order instead. Both are valid anchors -- what
    // matters is that placement and C11a then use the same one.
    let anchor = anchor_from_words(&parse_words(&fixture("bbox-s1.xml")), &s1_geometry(0));
    assert!(anchor.user_bounds.llx < 200.0, "{}", anchor.user_bounds);
    assert!(anchor_from_fixture("textbbox-s1.json").user_bounds.llx > 400.0);
}

#[test]
fn c11a_agrees_with_a_placement_derived_from_the_poppler_anchor() {
    // The round trip the Linux mode depends on: place from the poppler
    // anchor, then let C11a re-derive the needle from the same extractor.
    let words = parse_words(&fixture("bbox-s1.xml"));
    let anchor = anchor_from_words(&words, &s1_geometry(0));
    let placed = derive_highlight_rect(anchor.user_bounds, anchor.media_box);

    let mut nm: BTreeMap<String, NmInfo> = BTreeMap::new();
    nm.insert(
        HL.to_string(),
        NmInfo {
            found: true,
            rect: Some([placed.llx, placed.lly, placed.urx, placed.ury]),
            ..Default::default()
        },
    );
    let out = evaluate_c11a(
        &anchor,
        &[PosCheck {
            id: HL.to_string(),
            derive: PosDerive::Highlight,
        }],
        &words,
        &nm,
    );
    assert!(out.pass, "{}", out.detail);
    assert!(out.detail.contains("match=true"));
}

#[test]
fn a_page_without_words_produces_the_same_skip_as_pdfkit_reports_for_s3() {
    let anchor = anchor_from_words(&[], &s1_geometry(0));
    assert!(!anchor.found);
    assert_eq!(anchor.found, anchor_from_fixture("textbbox-s3.json").found);
}
