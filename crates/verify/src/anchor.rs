//! Text-anchor resolution via the pdfkit_textbbox helper. The anchor is a
//! short text run (the "needle") chosen from a page's extracted text plus
//! its bounds in raw PDF user space; annotation placements derive from it.

use serde::Deserialize;

use crate::checks::bbox::{BboxWord, pdftotext_bbox_words};
use crate::checks::qpdf::{PageGeometry, QpdfDoc};
use crate::geom::Rect;
use crate::proc::run;
use crate::util::first_line;

#[derive(Clone, Copy, Debug, Default, Deserialize)]
struct RectJson {
    llx: f64,
    lly: f64,
    urx: f64,
    ury: f64,
}

impl RectJson {
    fn rect(&self) -> Rect {
        Rect::new(self.llx, self.lly, self.urx, self.ury)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct AnchorJson {
    found: bool,
    needle: String,
    bounds: RectJson,
    #[serde(rename = "pageRotation")]
    page_rotation: i64,
    #[serde(rename = "mediaBox")]
    media_box: RectJson,
    #[serde(rename = "cropBox")]
    crop_box: RectJson,
}

/// The resolved text anchor for one sample's page 1. `found == false` means
/// the page has no anchor text; placements fall back to fixed coordinates
/// and position checks are skipped.
#[derive(Clone, Debug, Default)]
pub struct TextAnchor {
    pub found: bool,
    pub needle: String,
    /// PDFKit-reported needle bounds (user space).
    pub user_bounds: Rect,
    pub page_rotation: i64,
    pub media_box: Rect,
    /// Recorded for completeness; no current check consumes it.
    #[allow(dead_code)]
    pub crop_box: Rect,
}

/// Runs the pdfkit_textbbox helper against `path`'s page (1-based).
pub fn find_text_anchor(bin: &str, path: &str, page: i64) -> Result<TextAnchor, String> {
    let r = run(bin, &[path, &format!("{page}")]);
    if let Some(e) = &r.err {
        return Err(format!(
            "pdfkit_textbbox: {} ({})",
            e,
            first_line(&r.stderr_str())
        ));
    }
    let a: AnchorJson =
        serde_json::from_slice(&r.stdout).map_err(|e| format!("pdfkit_textbbox json: {e}"))?;
    let mut ta = TextAnchor {
        found: a.found,
        needle: a.needle,
        page_rotation: a.page_rotation,
        media_box: a.media_box.rect(),
        crop_box: a.crop_box.rect(),
        ..Default::default()
    };
    if a.found {
        // PDFKit reports selection bounds in unrotated user space, so no
        // rotation transform is applied here.
        ta.user_bounds = a.bounds.rect();
    }
    Ok(ta)
}

/// Chooses the anchor needle from one page's `pdftotext -bbox-layout`
/// words, mirroring the rule pdfkit_textbbox applies to PDFKit's own
/// extraction: the first run of at least three non-whitespace characters,
/// or the longest run when none reaches three. Both sides keep the earliest
/// candidate on a tie.
pub fn choose_needle_word(_words: &[BboxWord]) -> Option<&BboxWord> {
    todo!()
}

/// Builds a text anchor from one page's poppler words and the page geometry
/// read from qpdf — the PDFKit-free path. `found == false` when the page has
/// no word to anchor on, which is the same outcome pdfkit_textbbox reports
/// for a page with no extractable text.
pub fn anchor_from_words(_words: &[BboxWord], _geom: &PageGeometry) -> TextAnchor {
    todo!()
}

/// Resolves a sample's anchor without PDFKit: page geometry from qpdf, the
/// needle and its bounds from `pdftotext -bbox-layout`.
pub fn find_text_anchor_poppler(_path: &str, _page: i64) -> Result<TextAnchor, String> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, x_min: f64, y_min: f64, x_max: f64, y_max: f64) -> BboxWord {
        BboxWord {
            text: text.to_string(),
            x_min,
            y_min,
            x_max,
            y_max,
        }
    }

    #[test]
    fn needle_is_the_first_run_of_at_least_three_characters() {
        let words = vec![
            word("ー", 0.0, 0.0, 1.0, 1.0),
            word("ab", 0.0, 0.0, 1.0, 1.0),
            word("abc", 2.0, 0.0, 3.0, 1.0),
            word("abcdefg", 4.0, 0.0, 5.0, 1.0),
        ];
        assert_eq!(choose_needle_word(&words).unwrap().text, "abc");
    }

    #[test]
    fn needle_falls_back_to_the_longest_run_and_keeps_the_earliest_tie() {
        let short = vec![
            word("a", 0.0, 0.0, 1.0, 1.0),
            word("xy", 1.0, 0.0, 2.0, 1.0),
            word("zw", 2.0, 0.0, 3.0, 1.0),
        ];
        assert_eq!(choose_needle_word(&short).unwrap().text, "xy");
    }

    #[test]
    fn needle_ignores_words_that_are_only_whitespace() {
        let words = vec![
            word("   ", 0.0, 0.0, 1.0, 1.0),
            word(" ab ", 1.0, 0.0, 2.0, 1.0),
        ];
        assert_eq!(choose_needle_word(&words).unwrap().text, " ab ");
        assert!(choose_needle_word(&[word("  ", 0.0, 0.0, 1.0, 1.0)]).is_none());
        assert!(choose_needle_word(&[]).is_none());
    }

    fn geometry(rotate: i64) -> PageGeometry {
        PageGeometry {
            media_box: Rect::new(0.0, 0.0, 600.0, 800.0),
            rotate,
        }
    }

    #[test]
    fn anchor_reports_needle_bounds_in_unrotated_user_space() {
        // Device (10, 88)-(60, 100) on an 800-high page -> user y 700..712.
        let words = vec![word("needle", 10.0, 88.0, 60.0, 100.0)];
        let a = anchor_from_words(&words, &geometry(0));
        assert!(a.found);
        assert_eq!(a.needle, "needle");
        assert_eq!(a.user_bounds, Rect::new(10.0, 700.0, 60.0, 712.0));
        assert_eq!(a.page_rotation, 0);
        assert_eq!(a.media_box, Rect::new(0.0, 0.0, 600.0, 800.0));
    }

    #[test]
    fn anchor_needle_is_trimmed_but_bounds_come_from_the_word() {
        let words = vec![word(" needle\n", 10.0, 88.0, 60.0, 100.0)];
        let a = anchor_from_words(&words, &geometry(0));
        assert_eq!(a.needle, "needle");
        assert_eq!(a.user_bounds, Rect::new(10.0, 700.0, 60.0, 712.0));
    }

    #[test]
    fn a_page_with_no_words_yields_an_anchorless_result_with_geometry() {
        let a = anchor_from_words(&[], &geometry(90));
        assert!(!a.found);
        assert_eq!(a.needle, "");
        assert_eq!(a.user_bounds, Rect::default());
        assert_eq!(a.page_rotation, 90);
        assert_eq!(a.media_box, Rect::new(0.0, 0.0, 600.0, 800.0));
    }

    #[test]
    fn parse_anchor_json() {
        let json = r#"{"found":true,"needle":"PREFACE","bounds":{"llx":76.9,"lly":764.5,"urx":146.3,"ury":776.4},
                       "pageRotation":90,"mediaBox":{"llx":0,"lly":0,"urx":595.3,"ury":842.1},
                       "cropBox":{"llx":0,"lly":0,"urx":595.3,"ury":842.1}}"#;
        let a: AnchorJson = serde_json::from_str(json).unwrap();
        assert!(a.found);
        assert_eq!(a.needle, "PREFACE");
        assert_eq!(a.page_rotation, 90);
        assert_eq!(a.bounds.rect(), Rect::new(76.9, 764.5, 146.3, 776.4));
    }

    #[test]
    fn parse_anchor_json_not_found_defaults() {
        let json = r#"{"found":false,"needle":"","bounds":{"llx":0,"lly":0,"urx":0,"ury":0},
                       "pageRotation":0,"mediaBox":{"llx":0,"lly":0,"urx":595.2,"ury":841.9},
                       "cropBox":{"llx":0,"lly":0,"urx":595.2,"ury":841.9}}"#;
        let a: AnchorJson = serde_json::from_str(json).unwrap();
        assert!(!a.found);
    }
}
