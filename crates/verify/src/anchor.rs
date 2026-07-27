//! Text-anchor resolution via the pdfkit_textbbox helper. The anchor is a
//! short text run (the "needle") chosen from a page's extracted text plus
//! its bounds in raw PDF user space; annotation placements derive from it.

use serde::Deserialize;

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
    let a: AnchorJson = serde_json::from_slice(&r.stdout)
        .map_err(|e| format!("pdfkit_textbbox json: {e}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

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
