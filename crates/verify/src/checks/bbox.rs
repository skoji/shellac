//! C11a: independent position verification. Re-derives the anchor text's
//! bounding box from `pdftotext -bbox-layout` (a different extraction path
//! than the one used for placement) and compares the actually-written /Rect
//! (read back via qpdf) against the placement derived from it.
//!
//! Needle selection: candidates are `<word>` elements whose trimmed text
//! equals the needle exactly, in document order. With multiple candidates
//! the one whose user-space center is nearest to the PDFKit-reported anchor
//! bounds is chosen and the ambiguity is noted in the detail (the note does
//! not affect the verdict). No fuzzy/substring fallback: an unmatched
//! needle is a skip, not a fail.

use std::collections::BTreeMap;

use crate::anchor::TextAnchor;
use crate::geom::{
    center_distance, derive_highlight_rect, derive_loop_rect, derive_underline_rect,
    rect_device_top_left_to_user, Rect, POS_TOLERANCE,
};
use crate::proc::run;
use crate::util::trunc;

use super::qpdf::NmInfo;

#[derive(Clone, Debug, PartialEq)]
pub struct BboxWord {
    pub text: String,
    /// Device space: top-left origin, y-down, post-rotation.
    pub x_min: f64,
    pub y_min: f64,
    pub x_max: f64,
    pub y_max: f64,
}

fn parse_float_or_zero(s: &str) -> f64 {
    s.parse::<f64>().unwrap_or(0.0)
}

/// Reads a `[0-9.]+` run starting at `pos`; returns (matched text, end).
fn read_number_run(s: &str, pos: usize) -> Option<(&str, usize)> {
    let bytes = s.as_bytes();
    let mut i = pos;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        i += 1;
    }
    if i == pos {
        return None;
    }
    Some((&s[pos..i], i))
}

fn expect_literal(s: &str, pos: usize, lit: &str) -> Option<usize> {
    if s[pos..].starts_with(lit) {
        Some(pos + lit.len())
    } else {
        None
    }
}

/// Extracts the first `<page width="W" height="H">` element's dimensions.
pub fn parse_page_dims(s: &str) -> Option<(f64, f64)> {
    const OPEN: &str = "<page width=\"";
    let mut from = 0;
    while let Some(rel) = s[from..].find(OPEN) {
        let start = from + rel;
        let attempt = (|| {
            let mut i = start + OPEN.len();
            let (w, ni) = read_number_run(s, i)?;
            i = expect_literal(s, ni, "\" height=\"")?;
            let (h, ni2) = read_number_run(s, i)?;
            let _ = expect_literal(s, ni2, "\">")?;
            Some((parse_float_or_zero(w), parse_float_or_zero(h)))
        })();
        if let Some(dims) = attempt {
            return Some(dims);
        }
        from = start + 1;
    }
    None
}

/// Extracts every
/// `<word xMin="..." yMin="..." xMax="..." yMax="...">text</word>`
/// element in document order. Word text has its entity escapes expanded.
pub fn parse_words(s: &str) -> Vec<BboxWord> {
    const OPEN: &str = "<word xMin=\"";
    const CLOSE: &str = "</word>";
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = s[from..].find(OPEN) {
        let start = from + rel;
        let attempt = (|| {
            let mut i = start + OPEN.len();
            let (x_min, ni) = read_number_run(s, i)?;
            i = expect_literal(s, ni, "\" yMin=\"")?;
            let (y_min, ni2) = read_number_run(s, i)?;
            i = expect_literal(s, ni2, "\" xMax=\"")?;
            let (x_max, ni3) = read_number_run(s, i)?;
            i = expect_literal(s, ni3, "\" yMax=\"")?;
            let (y_max, ni4) = read_number_run(s, i)?;
            i = expect_literal(s, ni4, "\">")?;
            let text_end = i + s[i..].find(CLOSE)?;
            Some((
                BboxWord {
                    text: html_unescape(&s[i..text_end]),
                    x_min: parse_float_or_zero(x_min),
                    y_min: parse_float_or_zero(y_min),
                    x_max: parse_float_or_zero(x_max),
                    y_max: parse_float_or_zero(y_max),
                },
                text_end + CLOSE.len(),
            ))
        })();
        match attempt {
            Some((word, end)) => {
                out.push(word);
                from = end;
            }
            None => from = start + 1,
        }
    }
    out
}

/// Expands the entity escapes that appear in poppler's XHTML output:
/// the five predefined XML entities plus decimal/hex numeric references.
/// Unrecognized sequences are left untouched.
pub fn html_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        let semi = match tail.find(';') {
            Some(i) => i,
            None => {
                out.push_str(tail);
                return out;
            }
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => {
                if let Some(num) = entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X"))
                {
                    u32::from_str_radix(num, 16).ok().and_then(char::from_u32)
                } else if let Some(num) = entity.strip_prefix('#') {
                    num.parse::<u32>().ok().and_then(char::from_u32)
                } else {
                    None
                }
            }
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Runs `pdftotext -bbox-layout` restricted to one page and parses the
/// page dims and words.
pub fn pdftotext_bbox_words(
    path: &str,
    page: i64,
) -> Result<(f64, f64, Vec<BboxWord>), String> {
    let pg = format!("{page}");
    let r = run(
        "pdftotext",
        &["-bbox-layout", "-f", &pg, "-l", &pg, path, "-"],
    );
    if let Some(e) = &r.err {
        return Err(format!(
            "pdftotext -bbox-layout: {} ({})",
            e,
            r.stderr_str().trim()
        ));
    }
    let s = r.stdout_str();
    let Some((w, h)) = parse_page_dims(&s) else {
        return Err("pdftotext -bbox-layout: no <page> element found in output".to_string());
    };
    Ok((w, h, parse_words(&s)))
}

// ---- C11a judgment ----

/// How one positioned annotation's expected rect derives from the needle
/// bounds.
#[derive(Clone, Debug)]
pub enum PosDerive {
    Highlight,
    Underline,
    Loop(i64),
}

impl PosDerive {
    pub fn apply(&self, bounds: Rect, mb: Rect) -> Rect {
        match self {
            PosDerive::Highlight => derive_highlight_rect(bounds, mb),
            PosDerive::Underline => derive_underline_rect(bounds, mb),
            PosDerive::Loop(i) => derive_loop_rect(bounds, mb, *i),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PosCheck {
    pub id: String,
    pub derive: PosDerive,
}

/// Record of the needle-candidate selection made for one C11a evaluation.
#[derive(Clone, Debug)]
pub struct NeedleSelection {
    pub candidates: usize,
    /// The chosen candidate's user-space rect.
    pub chosen: Rect,
    /// User-space center distance from the PDFKit-reported anchor bounds.
    pub center_distance: f64,
}

pub struct C11aOutcome {
    pub pass: bool,
    pub detail: String,
    pub selection: Option<NeedleSelection>,
}

/// Pure C11a judgment over already-parsed inputs. `nm` carries the /Rect
/// values read back via qpdf for the pos-check ids.
pub fn evaluate_c11a(
    anchor: &TextAnchor,
    pos_checks: &[PosCheck],
    words: &[BboxWord],
    nm: &BTreeMap<String, NmInfo>,
) -> C11aOutcome {
    let uw = anchor.media_box.urx - anchor.media_box.llx;
    let uh = anchor.media_box.ury - anchor.media_box.lly;

    let candidates: Vec<&BboxWord> = words
        .iter()
        .filter(|w| w.text.trim() == anchor.needle)
        .collect();
    if candidates.is_empty() {
        return C11aOutcome {
            pass: true,
            detail: format!(
                "skip: needle {:?} not found verbatim in pdftotext -bbox-layout output for this page",
                anchor.needle
            ),
            selection: None,
        };
    }

    let user_rects: Vec<Rect> = candidates
        .iter()
        .map(|w| {
            rect_device_top_left_to_user(
                anchor.page_rotation,
                uw,
                uh,
                w.x_min,
                w.y_min,
                w.x_max,
                w.y_max,
            )
        })
        .collect();
    let mut best = 0usize;
    let mut best_dist = center_distance(user_rects[0], anchor.user_bounds);
    for (i, r) in user_rects.iter().enumerate().skip(1) {
        let d = center_distance(*r, anchor.user_bounds);
        if d < best_dist {
            best = i;
            best_dist = d;
        }
    }
    let needle_user = user_rects[best];
    let selection = NeedleSelection {
        candidates: candidates.len(),
        chosen: needle_user,
        center_distance: best_dist,
    };

    let mut pass = true;
    let mut notes: Vec<String> = Vec::new();
    for c in pos_checks {
        let want = c.derive.apply(needle_user, anchor.media_box);
        let got = nm.get(&c.id).and_then(|i| i.rect_geom());
        let Some(got) = got else {
            pass = false;
            notes.push(format!("{}: no /Rect found via qpdf", c.id));
            continue;
        };
        let matched = got.close_to(want, POS_TOLERANCE);
        if !matched {
            pass = false;
        }
        notes.push(format!(
            "{}: got {} want(pdftotext-derived) {} match={}",
            c.id, got, want, matched
        ));
    }
    let mut detail = notes.join("; ");
    if selection.candidates >= 2 {
        detail.push_str(&format!(
            "; needle matched {} times on page 1, selected nearest to PDFKit anchor bounds (center distance {:.2}pt)",
            selection.candidates, selection.center_distance
        ));
    }
    C11aOutcome {
        pass,
        detail,
        selection: Some(selection),
    }
}

/// IO wrapper for C11a: skips when there is nothing to check, collects the
/// page's words, and delegates to `evaluate_c11a`. `nm` is the shared qpdf
/// result for this scenario (`Err` when the qpdf pass failed).
pub fn check_c11a(
    cur_path: &str,
    anchor: &TextAnchor,
    pos_checks: &[PosCheck],
    nm: Result<&BTreeMap<String, NmInfo>, &str>,
) -> C11aOutcome {
    if !anchor.found || pos_checks.is_empty() {
        return C11aOutcome {
            pass: true,
            detail: crate::consts::SKIP_NO_ANCHOR.to_string(),
            selection: None,
        };
    }
    let words = match pdftotext_bbox_words(cur_path, 1) {
        Ok((_, _, words)) => words,
        Err(e) => {
            return C11aOutcome {
                pass: false,
                detail: format!("pdftotext -bbox-layout error: {}", trunc(&e, 400)),
                selection: None,
            };
        }
    };
    let nm = match nm {
        Ok(map) => map,
        Err(qerr) => {
            return C11aOutcome {
                pass: false,
                detail: format!("qpdf error: {qerr}"),
                selection: None,
            };
        }
    };
    evaluate_c11a(anchor, pos_checks, &words, nm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchor::TextAnchor;

    #[test]
    fn parse_page_dims_basic() {
        let s = r#"<?xml version="1.0"?><doc><page width="595.49" height="842.20">"#;
        assert_eq!(parse_page_dims(s), Some((595.49, 842.20)));
    }

    #[test]
    fn parse_page_dims_requires_exact_shape() {
        assert_eq!(parse_page_dims(r#"<page height="1" width="2">"#), None);
        assert_eq!(parse_page_dims(r#"<page width="1" height="2" >"#), None);
    }

    #[test]
    fn parse_words_multiline_text_and_entities() {
        let s = "<word xMin=\"1.5\" yMin=\"2\" xMax=\"3\" yMax=\"4\">A&amp;B\nC</word>\
                 <word xMin=\"5\" yMin=\"6\" xMax=\"7\" yMax=\"8\">&#x3042;</word>";
        let words = parse_words(s);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].text, "A&B\nC");
        assert_eq!(words[0].x_min, 1.5);
        assert_eq!(words[1].text, "あ");
        assert_eq!(words[1].y_max, 8.0);
    }

    #[test]
    fn parse_words_bad_float_becomes_zero() {
        // "1.2.3" matches the number-run class but fails to parse -> 0.0.
        let s = "<word xMin=\"1.2.3\" yMin=\"2\" xMax=\"3\" yMax=\"4\">t</word>";
        let words = parse_words(s);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].x_min, 0.0);
    }

    #[test]
    fn html_unescape_variants() {
        assert_eq!(html_unescape("a&lt;b&gt;c&quot;d&apos;e&amp;f"), "a<b>c\"d'e&f");
        assert_eq!(html_unescape("&#65;&#x42;"), "AB");
        assert_eq!(html_unescape("&unknown;&"), "&unknown;&");
    }

    fn anchor_at(bounds: Rect) -> TextAnchor {
        TextAnchor {
            found: true,
            needle: "needle".to_string(),
            user_bounds: bounds,
            page_rotation: 0,
            media_box: Rect::new(0.0, 0.0, 600.0, 800.0),
            crop_box: Rect::default(),
        }
    }

    fn word(text: &str, x_min: f64, y_min: f64, x_max: f64, y_max: f64) -> BboxWord {
        BboxWord {
            text: text.to_string(),
            x_min,
            y_min,
            x_max,
            y_max,
        }
    }

    fn nm_with_rect(id: &str, r: Rect) -> BTreeMap<String, NmInfo> {
        let mut m = BTreeMap::new();
        m.insert(
            id.to_string(),
            NmInfo {
                found: true,
                rect: Some([r.llx, r.lly, r.urx, r.ury]),
                ..Default::default()
            },
        );
        m
    }

    #[test]
    fn c11a_skip_when_needle_not_found() {
        let anchor = anchor_at(Rect::new(10.0, 700.0, 60.0, 712.0));
        let words = vec![word("other", 0.0, 0.0, 1.0, 1.0)];
        let out = evaluate_c11a(
            &anchor,
            &[PosCheck {
                id: "x".into(),
                derive: PosDerive::Highlight,
            }],
            &words,
            &BTreeMap::new(),
        );
        assert!(out.pass);
        assert_eq!(
            out.detail,
            "skip: needle \"needle\" not found verbatim in pdftotext -bbox-layout output for this page"
        );
        assert!(out.selection.is_none());
    }

    #[test]
    fn c11a_single_match_verdict_and_no_ambiguity_note() {
        // Device word (10, 88)-(60, 100) on an 800-high page -> user
        // (10, 700)-(60, 712).
        let anchor = anchor_at(Rect::new(10.0, 700.0, 60.0, 712.0));
        let words = vec![word("needle", 10.0, 88.0, 60.0, 100.0)];
        let nm = nm_with_rect("hl", Rect::new(11.0, 701.0, 61.0, 713.0));
        let out = evaluate_c11a(
            &anchor,
            &[PosCheck {
                id: "hl".into(),
                derive: PosDerive::Highlight,
            }],
            &words,
            &nm,
        );
        assert!(out.pass);
        assert_eq!(
            out.detail,
            "hl: got (11.00,701.00)-(61.00,713.00) want(pdftotext-derived) (10.00,700.00)-(60.00,712.00) match=true"
        );
        let sel = out.selection.unwrap();
        assert_eq!(sel.candidates, 1);
    }

    #[test]
    fn c11a_multi_match_selects_nearest_and_notes_ambiguity() {
        let anchor = anchor_at(Rect::new(500.0, 640.0, 520.0, 740.0));
        // Candidate A: near top-left in user space (far from anchor).
        // Candidate B: near the anchor bounds.
        let words = vec![
            word("needle", 70.0, 60.0, 150.0, 75.0),
            word("needle", 500.0, 60.0, 520.0, 160.0),
        ];
        // Written rect equals candidate B's user rect: (500,640)-(520,740).
        let nm = nm_with_rect("hl", Rect::new(500.0, 640.0, 520.0, 740.0));
        let out = evaluate_c11a(
            &anchor,
            &[PosCheck {
                id: "hl".into(),
                derive: PosDerive::Highlight,
            }],
            &words,
            &nm,
        );
        assert!(out.pass, "nearest candidate should match: {}", out.detail);
        assert!(out.detail.contains(
            "; needle matched 2 times on page 1, selected nearest to PDFKit anchor bounds (center distance 0.00pt)"
        ));
        let sel = out.selection.unwrap();
        assert_eq!(sel.candidates, 2);
        assert_eq!(sel.chosen, Rect::new(500.0, 640.0, 520.0, 740.0));
    }

    #[test]
    fn c11a_tie_prefers_document_order() {
        let anchor = anchor_at(Rect::new(300.0, 390.0, 310.0, 400.0));
        // Two candidates equidistant from the anchor center (305, 395):
        // centers at (295,395) and (315,395), both 10pt away.
        let words = vec![
            word("needle", 290.0, 400.0, 300.0, 410.0),
            word("needle", 310.0, 400.0, 320.0, 410.0),
        ];
        let nm = nm_with_rect("hl", Rect::new(290.0, 390.0, 300.0, 400.0));
        let out = evaluate_c11a(
            &anchor,
            &[PosCheck {
                id: "hl".into(),
                derive: PosDerive::Highlight,
            }],
            &words,
            &nm,
        );
        let sel = out.selection.unwrap();
        assert_eq!(sel.chosen, Rect::new(290.0, 390.0, 300.0, 400.0));
        assert!(out.pass);
    }

    #[test]
    fn c11a_missing_rect_fails() {
        let anchor = anchor_at(Rect::new(10.0, 700.0, 60.0, 712.0));
        let words = vec![word("needle", 10.0, 88.0, 60.0, 100.0)];
        let mut nm = BTreeMap::new();
        nm.insert("hl".to_string(), NmInfo::default());
        let out = evaluate_c11a(
            &anchor,
            &[PosCheck {
                id: "hl".into(),
                derive: PosDerive::Highlight,
            }],
            &words,
            &nm,
        );
        assert!(!out.pass);
        assert_eq!(out.detail, "hl: no /Rect found via qpdf");
    }

    #[test]
    fn c11a_position_mismatch_fails_with_go_style_detail() {
        let anchor = anchor_at(Rect::new(10.0, 700.0, 60.0, 712.0));
        let words = vec![word("needle", 10.0, 88.0, 60.0, 100.0)];
        let nm = nm_with_rect("hl", Rect::new(100.0, 100.0, 300.0, 120.0));
        let out = evaluate_c11a(
            &anchor,
            &[PosCheck {
                id: "hl".into(),
                derive: PosDerive::Highlight,
            }],
            &words,
            &nm,
        );
        assert!(!out.pass);
        assert_eq!(
            out.detail,
            "hl: got (100.00,100.00)-(300.00,120.00) want(pdftotext-derived) (10.00,700.00)-(60.00,712.00) match=false"
        );
    }

    #[test]
    fn c11a_underline_and_loop_derivations_are_applied() {
        let anchor = anchor_at(Rect::new(10.0, 700.0, 60.0, 712.0));
        let words = vec![word("needle", 10.0, 88.0, 60.0, 100.0)];
        // Underline: shifted down by height (12) -> (10,688)-(60,700).
        let nm = nm_with_rect("ul", Rect::new(10.0, 688.0, 60.0, 700.0));
        let out = evaluate_c11a(
            &anchor,
            &[PosCheck {
                id: "ul".into(),
                derive: PosDerive::Underline,
            }],
            &words,
            &nm,
        );
        assert!(out.pass, "{}", out.detail);
        // Loop 1: shifted up by (12 + 4) -> (10,716)-(60,728).
        let nm2 = nm_with_rect("loop-1", Rect::new(10.0, 716.0, 60.0, 728.0));
        let out2 = evaluate_c11a(
            &anchor,
            &[PosCheck {
                id: "loop-1".into(),
                derive: PosDerive::Loop(1),
            }],
            &words,
            &nm2,
        );
        assert!(out2.pass, "{}", out2.detail);
    }
}
