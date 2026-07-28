//! PDF user-space rectangles, rotation-aware coordinate transforms, and the
//! anchor-relative placement derivations shared by placement and position
//! checks (C11a).

use std::fmt;

/// Position-match tolerance in points for C11a/C11b.
pub const POS_TOLERANCE: f64 = 3.0;

/// Vertical gap (pt) between successive loop-scenario highlights, on top of
/// the anchor's own height.
pub const LOOP_GAP: f64 = 4.0;

/// A PDF user-space rectangle (llx, lly, urx, ury), y-up, bottom-left origin.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub llx: f64,
    pub lly: f64,
    pub urx: f64,
    pub ury: f64,
}

impl Rect {
    pub fn new(llx: f64, lly: f64, urx: f64, ury: f64) -> Self {
        Rect { llx, lly, urx, ury }
    }

    pub fn height(&self) -> f64 {
        self.ury - self.lly
    }

    /// Returns this rect translated by (dx, dy).
    pub fn shifted(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.llx + dx, self.lly + dy, self.urx + dx, self.ury + dy)
    }

    /// Nudges the rect (preserving width/height) so it stays within `mb`.
    /// The right/top edge is checked before the left/bottom edge, so a rect
    /// larger than `mb` ends up aligned to mb's lower-left origin.
    pub fn clamp_to_media_box(&self, mb: Rect) -> Rect {
        let w = self.urx - self.llx;
        let h = self.ury - self.lly;
        let (mut llx, mut urx) = (self.llx, self.urx);
        if urx > mb.urx {
            llx = mb.urx - w;
            urx = mb.urx;
        }
        if llx < mb.llx {
            llx = mb.llx;
            urx = mb.llx + w;
        }
        let (mut lly, mut ury) = (self.lly, self.ury);
        if ury > mb.ury {
            lly = mb.ury - h;
            ury = mb.ury;
        }
        if lly < mb.lly {
            lly = mb.lly;
            ury = mb.lly + h;
        }
        Rect::new(llx, lly, urx, ury)
    }

    /// Reports whether every coordinate is within `tol` (inclusive) of
    /// `want`'s corresponding coordinate.
    pub fn close_to(&self, want: Rect, tol: f64) -> bool {
        (self.llx - want.llx).abs() <= tol
            && (self.lly - want.lly).abs() <= tol
            && (self.urx - want.urx).abs() <= tol
            && (self.ury - want.ury).abs() <= tol
    }

    /// Center point of the rect.
    pub fn center(&self) -> (f64, f64) {
        ((self.llx + self.urx) / 2.0, (self.lly + self.ury) / 2.0)
    }

    /// The wire format used by the save engine's rect flags.
    pub fn csv(&self) -> String {
        format!(
            "{:.4},{:.4},{:.4},{:.4}",
            self.llx, self.lly, self.urx, self.ury
        )
    }
}

impl fmt::Display for Rect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "({:.2},{:.2})-({:.2},{:.2})",
            self.llx, self.lly, self.urx, self.ury
        )
    }
}

/// Euclidean distance between the centers of two rects.
pub fn center_distance(a: Rect, b: Rect) -> f64 {
    let (ax, ay) = a.center();
    let (bx, by) = b.center();
    ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt()
}

fn norm_rotate(rotate: i64) -> i64 {
    rotate.rem_euclid(360)
}

/// Returns the on-screen (rotation-applied) width/height for a page whose
/// unrotated MediaBox is uw x uh.
pub fn rotated_dims(rotate: i64, uw: f64, uh: f64) -> (f64, f64) {
    match norm_rotate(rotate) {
        90 | 270 => (uh, uw),
        _ => (uw, uh),
    }
}

/// Maps one point from page space (y-up, bottom-left origin, rotated
/// extents) back to raw PDF user space (y-up, bottom-left origin, unrotated
/// extents uw x uh).
pub fn page_space_to_user(rotate: i64, uw: f64, uh: f64, x: f64, y: f64) -> (f64, f64) {
    match norm_rotate(rotate) {
        90 => (uw - y, x),
        180 => (uw - x, uh - y),
        270 => (y, uh - x),
        _ => (x, y),
    }
}

/// Maps an axis-aligned page-space rect to the axis-aligned user-space rect
/// it corresponds to, by transforming all four corners and re-deriving the
/// min/max bounds.
pub fn rect_page_space_to_user(rotate: i64, uw: f64, uh: f64, r: Rect) -> Rect {
    let pts = [
        page_space_to_user(rotate, uw, uh, r.llx, r.lly),
        page_space_to_user(rotate, uw, uh, r.urx, r.ury),
        page_space_to_user(rotate, uw, uh, r.llx, r.ury),
        page_space_to_user(rotate, uw, uh, r.urx, r.lly),
    ];
    let min_x = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let max_x = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let min_y = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let max_y = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    Rect::new(min_x, min_y, max_x, max_y)
}

/// Maps an axis-aligned rect given in device space (top-left origin, y-down,
/// rotated extents, e.g. `pdftotext -bbox-layout` output) to raw PDF user
/// space.
pub fn rect_device_top_left_to_user(
    rotate: i64,
    uw: f64,
    uh: f64,
    x_min: f64,
    y_min: f64,
    x_max: f64,
    y_max: f64,
) -> Rect {
    let (_dw, dh) = rotated_dims(rotate, uw, uh);
    let page_rect = Rect::new(x_min, dh - y_max, x_max, dh - y_min);
    rect_page_space_to_user(rotate, uw, uh, page_rect)
}

/// Places the Highlight exactly at the anchor bounds.
pub fn derive_highlight_rect(bounds: Rect, _mb: Rect) -> Rect {
    bounds
}

/// Places the Underline directly below the anchor bounds (offset by the
/// anchor's own height), clamped to stay inside `mb`.
pub fn derive_underline_rect(bounds: Rect, mb: Rect) -> Rect {
    bounds.shifted(0.0, -bounds.height()).clamp_to_media_box(mb)
}

/// Places loop iteration i's highlight above the anchor bounds by
/// i * (height + LOOP_GAP), clamped to stay inside `mb`.
pub fn derive_loop_rect(bounds: Rect, mb: Rect, i: i64) -> Rect {
    bounds
        .shifted(0.0, (i as f64) * (bounds.height() + LOOP_GAP))
        .clamp_to_media_box(mb)
}

/// Splits the anchor bounds into two vertically stacked stripes and returns
/// the two-quad /QuadPoints wire encoding (`x0,y0,...;x0,y0,...`, TL, TR,
/// BL, BR order per quad).
pub fn derive_multiline_quads(bounds: Rect) -> String {
    let mid = (bounds.lly + bounds.ury) / 2.0;
    let upper = format!(
        "{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}",
        bounds.llx, bounds.ury, bounds.urx, bounds.ury, bounds.llx, mid, bounds.urx, mid
    );
    let lower = format!(
        "{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}",
        bounds.llx, mid, bounds.urx, mid, bounds.llx, bounds.lly, bounds.urx, bounds.lly
    );
    format!("{upper};{lower}")
}

/// Fixed-coordinate fallback placements for samples without a text anchor.
pub fn legacy_highlight_rect() -> Rect {
    Rect::new(100.0, 100.0, 300.0, 120.0)
}

pub fn legacy_underline_rect() -> Rect {
    Rect::new(100.0, 140.0, 300.0, 160.0)
}

pub fn legacy_loop_rect(i: i64) -> Rect {
    Rect::new(100.0, (180 + 20 * i) as f64, 300.0, (196 + 20 * i) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_to_boundary_is_inclusive() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let exactly = Rect::new(3.0, 0.0, 10.0, 10.0);
        let over = Rect::new(3.0001, 0.0, 10.0, 10.0);
        assert!(a.close_to(exactly, POS_TOLERANCE));
        assert!(!a.close_to(over, POS_TOLERANCE));
    }

    #[test]
    fn clamp_checks_upper_edge_before_lower_edge() {
        let mb = Rect::new(0.0, 0.0, 100.0, 100.0);
        // Sticks out to the right: pulled back to the right edge.
        let r = Rect::new(90.0, 10.0, 110.0, 20.0).clamp_to_media_box(mb);
        assert_eq!(r, Rect::new(80.0, 10.0, 100.0, 20.0));
        // Wider than the page: right-align first, then left-align wins.
        let wide = Rect::new(-10.0, 10.0, 120.0, 20.0).clamp_to_media_box(mb);
        assert_eq!(wide, Rect::new(0.0, 10.0, 130.0, 20.0));
        // Below the page: pushed up to the bottom edge.
        let low = Rect::new(10.0, -5.0, 20.0, 5.0).clamp_to_media_box(mb);
        assert_eq!(low, Rect::new(10.0, 0.0, 20.0, 10.0));
    }

    #[test]
    fn display_and_csv_formats() {
        let r = Rect::new(76.96, 764.512, 146.3, 776.0);
        assert_eq!(r.to_string(), "(76.96,764.51)-(146.30,776.00)");
        assert_eq!(r.csv(), "76.9600,764.5120,146.3000,776.0000");
    }

    #[test]
    fn derive_highlight_is_identity() {
        let b = Rect::new(10.0, 20.0, 30.0, 40.0);
        let mb = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(derive_highlight_rect(b, mb), b);
    }

    #[test]
    fn derive_underline_shifts_down_by_height() {
        let b = Rect::new(10.0, 20.0, 30.0, 40.0);
        let mb = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(
            derive_underline_rect(b, mb),
            Rect::new(10.0, 0.0, 30.0, 20.0)
        );
    }

    #[test]
    fn derive_loop_shifts_up_with_gap() {
        let b = Rect::new(10.0, 20.0, 30.0, 40.0);
        let mb = Rect::new(0.0, 0.0, 100.0, 200.0);
        // i=2: dy = 2 * (20 + 4) = 48
        assert_eq!(
            derive_loop_rect(b, mb, 2),
            Rect::new(10.0, 68.0, 30.0, 88.0)
        );
    }

    #[test]
    fn derive_multiline_quads_format() {
        let b = Rect::new(0.0, 0.0, 10.0, 4.0);
        assert_eq!(
            derive_multiline_quads(b),
            "0.0000,4.0000,10.0000,4.0000,0.0000,2.0000,10.0000,2.0000;\
             0.0000,2.0000,10.0000,2.0000,0.0000,0.0000,10.0000,0.0000"
        );
    }

    #[test]
    fn legacy_rects() {
        assert_eq!(
            legacy_highlight_rect(),
            Rect::new(100.0, 100.0, 300.0, 120.0)
        );
        assert_eq!(
            legacy_underline_rect(),
            Rect::new(100.0, 140.0, 300.0, 160.0)
        );
        assert_eq!(legacy_loop_rect(3), Rect::new(100.0, 240.0, 300.0, 256.0));
    }

    #[test]
    fn rotation_identity() {
        let r = rect_device_top_left_to_user(0, 595.0, 842.0, 10.0, 20.0, 110.0, 40.0);
        assert_eq!(r, Rect::new(10.0, 802.0, 110.0, 822.0));
    }

    #[test]
    fn rotation_90() {
        // Page 100x200 unrotated; displayed 200x100. Device rect
        // (10,20)-(30,40) -> page space y-flip with dh=100:
        // (10, 60)-(30, 80) -> rotate-90 user mapping (uw-y, x).
        let r = rect_device_top_left_to_user(90, 100.0, 200.0, 10.0, 20.0, 30.0, 40.0);
        assert_eq!(r, Rect::new(20.0, 10.0, 40.0, 30.0));
    }

    #[test]
    fn rotation_180() {
        let r = rect_device_top_left_to_user(180, 100.0, 200.0, 10.0, 20.0, 30.0, 40.0);
        // dh=200, page rect (10,160)-(30,180); 180: (uw-x, uh-y)
        assert_eq!(r, Rect::new(70.0, 20.0, 90.0, 40.0));
    }

    #[test]
    fn rotation_270() {
        let r = rect_device_top_left_to_user(270, 100.0, 200.0, 10.0, 20.0, 30.0, 40.0);
        // dh=100 (rotated dims of 100x200 -> 200x100), page rect (10,60)-(30,80)
        // 270: (y, uh-x)
        assert_eq!(r, Rect::new(60.0, 170.0, 80.0, 190.0));
    }

    #[test]
    fn negative_rotation_normalizes() {
        assert_eq!(rotated_dims(-90, 10.0, 20.0), (20.0, 10.0));
        assert_eq!(
            page_space_to_user(-270, 100.0, 200.0, 5.0, 6.0),
            page_space_to_user(90, 100.0, 200.0, 5.0, 6.0)
        );
    }

    #[test]
    fn center_distance_euclidean() {
        let a = Rect::new(0.0, 0.0, 2.0, 2.0); // center (1,1)
        let b = Rect::new(3.0, 4.0, 5.0, 6.0); // center (4,5)
        assert!((center_distance(a, b) - 5.0).abs() < 1e-12);
    }
}
