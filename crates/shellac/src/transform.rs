//! Coordinate-space types for this crate plus a pair of utility transforms
//! between an abstract "rotated page space" and raw PDF user space.
//!
//! # `apply_ops` inputs are unrotated PDF user space
//!
//! This was established empirically rather than assumed. On device
//! (iPadOS 26.5, iPad Pro 13-inch simulator) and on macOS (a corpus
//! regression comparing an unrotated sample against its `/Rotate`-bearing
//! sibling), the rectangles PDFKit exposes for content that lives on a page
//! are already **unrotated PDF user space** — they equal the PDF file's raw
//! `/Rect` on save:
//!
//! * `PDFSelection.bounds(for:)` returned the same numbers on the four
//!   `/Rotate 0/90/180/270` fixtures for the exact same glyph sequence.
//! * `PDFDocument.write(to:)` after adding a `PDFAnnotation` with a given
//!   `bounds: CGRect` produced a raw `/Rect [minX minY maxX maxY]` equal
//!   to that `CGRect` verbatim, again on all four rotations.
//!
//! [`crate::ops::apply_ops`] therefore treats its input `rect`,
//! `quad_points`, and `user_point` fields as **default user space**
//! directly — the same coordinate space the PDF file itself stores in
//! `/Rect` — and writes them verbatim into `/Rect` / `/QuadPoints` (and
//! feeds them straight to the `/Rect contains(point)` fallback). A caller
//! plumbing PDFKit-observed rectangles into `apply_ops` passes them through
//! as-is; no page-space→user-space transform is needed on either side.
//!
//! # When [`rect_page_space_to_user`] / [`page_space_to_user`] are still useful
//!
//! The two rotated-page-space → user-space utilities below remain the
//! correct interpretation for rectangles authored in a `/Rotate`-applied
//! display frame — the coordinate space someone would compute from
//! view-space touches on a rotated page. They are **not** used by
//! `apply_ops`; they are here for callers whose inputs originate outside
//! PDFKit and whose coordinate space has not been verified against user
//! space:
//!
//! * `PDFView.convert(_:to:)` and other view-coordinate → page-coordinate
//!   PDFView APIs — the space they return has **not** been verified for
//!   /Rotate ≠ 0. A feature accepting manual coordinate input from
//!   touch/gesture pipelines on rotated pages should add a rotation
//!   fixture test of its own before feeding rectangles into `apply_ops`.
//! * Any harness or debug tool that hand-constructs rectangles in a
//!   rotated display frame.
//!
//! # Coordinate contract (rotated page space → user space)
//!
//! Both spaces are y-up with the bottom-left corner as origin — the PDF
//! convention.
//!
//! * **PDF user space** (output; `apply_ops` input): the page's raw stored
//!   coordinate system. The `/MediaBox` rectangle `[mx0, my0, mx1, my1]` is
//!   expressed directly in this space. `/MediaBox` may start at an
//!   arbitrary origin — `[9, 9, 621, 801]` is common for Acrobat-OCR'd
//!   scans, and real-world OCR PDFs with a non-zero MediaBox origin are
//!   exactly the documents this engine exists to keep intact, so the
//!   offset terms below are not hypothetical.
//!
//! * **Rotated page space** (input to the transforms below only): a
//!   `/Rotate`-applied display frame with:
//!     - Lower-left corner: `(mx0, my0)` (i.e. **MediaBox-absolute**, NOT
//!       origin-relative — coordinates already include the MediaBox offset).
//!     - Extents: `(dw, dh) = rotated_dims(rotate, uw, uh)` — width and
//!       height swap for /Rotate ∈ {90, 270}.
//!
//!   This used to be labeled "page space (PDFKit-shaped)". That label was
//!   misleading: PDFKit's bounds getters do not return this space (see the
//!   note above). The formulas themselves are correct for
//!   rotated-display-frame inputs, so callers with such inputs still route
//!   through [`rect_page_space_to_user`].
//!
//! # Transformations
//!
//! For `/Rotate` == 0 the mapping is the identity; e.g. a
//! `[100, 100, 200, 120]` rect on a MediaBox `[9, 9, 621, 801]` page stays
//! `[100, 100, 200, 120]`. For 90/180/270 the mapping is derived from a
//! clockwise rotation of the MediaBox rectangle that keeps the display
//! frame's bottom-left corner at `(mx0, my0)`:
//!
//! | /Rotate | user_x                     | user_y                     |
//! |--------:|----------------------------|----------------------------|
//! |     0   | dx                         | dy                         |
//! |    90   | mx1 + my0 − dy             | my0 + dx − mx0             |
//! |   180   | mx1 + mx0 − dx             | my1 + my0 − dy             |
//! |   270   | mx0 + dy − my0             | my1 + mx0 − dx             |
//!
//! All four formulas are affine (rotation + translation) so
//! `rect_page_space_to_user` transforms the four corners and takes their
//! axis-aligned bounding box.
//!
//! `/Rotate` is a multiple of 90 but may be negative or exceed a full turn,
//! so it is reduced into `[0, 360)` first — see [`norm_rotate`].
//!
//! The formulas generalize an earlier verification harness that only ever
//! exercised MediaBox origin `(0, 0)` and therefore did not need the offset
//! terms above; the offsets were added in review precisely because
//! origin-anchored MediaBoxes are the exception, not the rule.

/// An axis-aligned rectangle in some coordinate space: `(llx, lly, urx, ury)`,
/// y-up, bottom-left origin. Concrete space is imposed by the wrapping
/// newtype ([`UserSpaceRect`] or [`PageSpaceRect`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub llx: f64,
    pub lly: f64,
    pub urx: f64,
    pub ury: f64,
}

impl Rect {
    /// Returns true iff (x, y) is inside (or exactly on the boundary of) this
    /// rectangle.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.llx && x <= self.urx && y >= self.lly && y <= self.ury
    }
}

/// An axis-aligned rectangle in raw PDF **user space** (MediaBox-absolute,
/// y-up, bottom-left origin, unrotated extents). This is the space PDF
/// files themselves store in `/Rect` and the space PDFKit's
/// `PDFSelection.bounds(for:)` / `PDFAnnotation.bounds` return.
/// `apply_ops` treats its JSON input as this space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UserSpaceRect(pub Rect);

/// A point in raw PDF **user space** (see [`UserSpaceRect`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UserSpacePoint {
    pub x: f64,
    pub y: f64,
}

/// An axis-aligned rectangle in **rotated page space** — a `/Rotate`-applied
/// display frame with lower-left corner at `(mx0, my0)` and extents
/// `rotated_dims(rotate, uw, uh)`. Not used by `apply_ops`;
/// kept as an input to [`rect_page_space_to_user`] for callers
/// whose inputs originate outside PDFKit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSpaceRect(pub Rect);

/// A point in **rotated page space** (see [`PageSpaceRect`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSpacePoint {
    pub x: f64,
    pub y: f64,
}

/// Reduce a `/Rotate` value into the half-open range `[0, 360)`.
///
/// PDF 1.7 §7.7.3.3 requires `/Rotate` to be a multiple of 90 but allows it
/// to be negative or beyond a full turn, so `-90`, `270` and `630` all have
/// to mean the same rotation. For any conforming value the reduced result is
/// one of 0/90/180/270, which is what the callers' `match` arms enumerate.
///
/// A non-conforming value is reduced but **not** snapped: `45` reduces to
/// `45` and falls through to the identity arm. That is deliberate — guessing
/// at the nearest quarter turn would silently move annotations on a
/// malformed page, whereas the identity leaves them exactly where the caller
/// asked for them.
#[inline]
fn norm_rotate(rotate: i32) -> i32 {
    rotate.rem_euclid(360)
}

/// Returns the on-screen (rotation-applied) width/height for a page whose
/// unrotated MediaBox width/height are `uw` x `uh`. This is the SIZE of the
/// display frame, not its coordinate range — the display frame's lower-left
/// corner is still `(mb.llx, mb.lly)` (see module docs).
pub fn rotated_dims(rotate: i32, uw: f64, uh: f64) -> (f64, f64) {
    match norm_rotate(rotate) {
        90 | 270 => (uh, uw),
        _ => (uw, uh),
    }
}

/// Map one point from "rotated page space" (MediaBox-absolute, y-up,
/// bottom-left origin, using rotated extents) to raw PDF user space
/// (MediaBox-absolute, y-up, bottom-left origin, unrotated extents).
///
/// `mb` is the page's `/MediaBox` in user space. See the module-level docs
/// for the four rotation formulas.
///
/// **Do not** call this on rectangles obtained from PDFKit
/// (`PDFSelection.bounds(for:)`, `PDFAnnotation.bounds`) — those are
/// already in user space. Use it for inputs authored in a
/// `/Rotate`-applied display frame.
pub fn page_space_to_user(rotate: i32, mb: UserSpaceRect, p: PageSpacePoint) -> UserSpacePoint {
    let mx0 = mb.0.llx;
    let my0 = mb.0.lly;
    let mx1 = mb.0.urx;
    let my1 = mb.0.ury;
    let (x, y) = (p.x, p.y);
    let (ux, uy) = match norm_rotate(rotate) {
        90 => (mx1 + my0 - y, my0 + x - mx0),
        180 => (mx1 + mx0 - x, my1 + my0 - y),
        270 => (mx0 + y - my0, my1 + mx0 - x),
        _ => (x, y),
    };
    UserSpacePoint { x: ux, y: uy }
}

/// Map an axis-aligned rectangle given in "rotated page space"
/// (MediaBox-absolute, rotated extents — see the module-level docs) to the
/// axis-aligned user-space rectangle it corresponds to. Transforms all four
/// corners under the rotation and takes their bounding box, which is exact
/// because the transformation is affine.
///
/// **Do not** call this on rectangles obtained from PDFKit — those are
/// already in user space (verified on device and on macOS). It
/// remains the correct call for rectangles authored in a `/Rotate`-applied
/// display frame (e.g. hand-computed layout at rotated extents).
pub fn rect_page_space_to_user(rotate: i32, mb: UserSpaceRect, r: PageSpaceRect) -> UserSpaceRect {
    let r = r.0;
    let corners = [
        page_space_to_user(rotate, mb, PageSpacePoint { x: r.llx, y: r.lly }),
        page_space_to_user(rotate, mb, PageSpacePoint { x: r.urx, y: r.ury }),
        page_space_to_user(rotate, mb, PageSpacePoint { x: r.llx, y: r.ury }),
        page_space_to_user(rotate, mb, PageSpacePoint { x: r.urx, y: r.lly }),
    ];
    let (min_x, max_x) = min_max_x(&corners);
    let (min_y, max_y) = min_max_y(&corners);
    UserSpaceRect(Rect {
        llx: min_x,
        lly: min_y,
        urx: max_x,
        ury: max_y,
    })
}

// Fixed-arity min/max over the four rotated corners. Named component
// helpers rather than an iterator-based fold because the input is always a
// `[UserSpacePoint; 4]` — no runtime length variance to guard against. An
// earlier version folded over a slice and carried an `.expect("unreachable")`
// for the empty case; that "unreachable" path was itself the smell.

fn min_max_x(corners: &[UserSpacePoint; 4]) -> (f64, f64) {
    let mut lo = corners[0].x;
    let mut hi = corners[0].x;
    for c in &corners[1..] {
        if c.x < lo {
            lo = c.x;
        }
        if c.x > hi {
            hi = c.x;
        }
    }
    (lo, hi)
}

fn min_max_y(corners: &[UserSpacePoint; 4]) -> (f64, f64) {
    let mut lo = corners[0].y;
    let mut hi = corners[0].y;
    for c in &corners[1..] {
        if c.y < lo {
            lo = c.y;
        }
        if c.y > hi {
            hi = c.y;
        }
    }
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.01
    }

    fn rect_approx(a: Rect, b: Rect) -> bool {
        approx(a.llx, b.llx) && approx(a.lly, b.lly) && approx(a.urx, b.urx) && approx(a.ury, b.ury)
    }

    fn origin_mb() -> UserSpaceRect {
        UserSpaceRect(Rect {
            llx: 0.0,
            lly: 0.0,
            urx: 612.0,
            ury: 792.0,
        })
    }

    // Non-origin-anchored MediaBox — Acrobat-OCR'd PDFs are typically shaped
    // this way. Every offset-MediaBox test below must succeed on this shape.
    fn offset_mb() -> UserSpaceRect {
        UserSpaceRect(Rect {
            llx: 9.0,
            lly: 9.0,
            urx: 621.0,
            ury: 801.0,
        })
    }

    // A MediaBox where mb.llx != mb.lly, to catch formulas that conflate
    // mx0/my0.
    fn asymmetric_offset_mb() -> UserSpaceRect {
        UserSpaceRect(Rect {
            llx: 9.0,
            lly: 5.0,
            urx: 621.0,
            ury: 797.0,
        })
    }

    fn ps_rect(llx: f64, lly: f64, urx: f64, ury: f64) -> PageSpaceRect {
        PageSpaceRect(Rect { llx, lly, urx, ury })
    }

    fn ps_point(x: f64, y: f64) -> PageSpacePoint {
        PageSpacePoint { x, y }
    }

    #[test]
    fn rotate_0_is_identity_at_origin() {
        let r = ps_rect(100.0, 200.0, 300.0, 240.0);
        let out = rect_page_space_to_user(0, origin_mb(), r);
        assert!(rect_approx(out.0, r.0), "expected identity, got {:?}", out);
    }

    #[test]
    fn rotate_0_is_identity_with_offset_mediabox() {
        // Regression: at rotate=0 the display frame equals the
        // unrotated MediaBox, so any input rect must round-trip unchanged
        // regardless of the MediaBox's origin.
        let r = ps_rect(100.0, 100.0, 200.0, 120.0);
        let out = rect_page_space_to_user(0, offset_mb(), r);
        assert!(rect_approx(out.0, r.0), "expected identity, got {:?}", out);
    }

    #[test]
    fn rotate_90_maps_correctly_at_origin() {
        // 200x40 rect at (100,200) in page space, uw=612 uh=792, rotate=90.
        // Corners: (100,200)->(412,100)  (300,240)->(372,300)
        //          (100,240)->(372,100)  (300,200)->(412,300)
        let r = ps_rect(100.0, 200.0, 300.0, 240.0);
        let out = rect_page_space_to_user(90, origin_mb(), r);
        let want = Rect {
            llx: 372.0,
            lly: 100.0,
            urx: 412.0,
            ury: 300.0,
        };
        assert!(rect_approx(out.0, want), "got {:?} want {:?}", out, want);
    }

    #[test]
    fn rotate_90_maps_correctly_with_offset_mediabox() {
        // MediaBox [9 9 621 801], rotate=90.
        // Point (dx=100, dy=200) → user (mx1+my0-dy, my0+dx-mx0)
        //                       = (621 + 9 - 200, 9 + 100 - 9) = (430, 100)
        let out = page_space_to_user(90, offset_mb(), ps_point(100.0, 200.0));
        assert!(approx(out.x, 430.0));
        assert!(approx(out.y, 100.0));
    }

    #[test]
    fn rotate_90_asymmetric_offset_regression() {
        // Formula must NOT conflate mx0/my0. mb.llx=9, mb.lly=5.
        // user_x = mx1 + my0 - dy = 621 + 5 - 200 = 426
        // user_y = my0 + dx - mx0 = 5 + 100 - 9 = 96
        let out = page_space_to_user(90, asymmetric_offset_mb(), ps_point(100.0, 200.0));
        assert!(approx(out.x, 426.0), "user_x = {}", out.x);
        assert!(approx(out.y, 96.0), "user_y = {}", out.y);
    }

    #[test]
    fn rotate_180_maps_correctly_at_origin() {
        let r = ps_rect(100.0, 200.0, 300.0, 240.0);
        let out = rect_page_space_to_user(180, origin_mb(), r);
        // Corners: (512,592) (312,552) (512,552) (312,592)
        let want = Rect {
            llx: 312.0,
            lly: 552.0,
            urx: 512.0,
            ury: 592.0,
        };
        assert!(rect_approx(out.0, want), "got {:?} want {:?}", out, want);
    }

    #[test]
    fn rotate_180_with_offset_mediabox() {
        // MediaBox [9 9 621 801], input point (100, 200) at rotate=180:
        // user_x = mx1 + mx0 - dx = 621 + 9 - 100 = 530
        // user_y = my1 + my0 - dy = 801 + 9 - 200 = 610
        let out = page_space_to_user(180, offset_mb(), ps_point(100.0, 200.0));
        assert!(approx(out.x, 530.0));
        assert!(approx(out.y, 610.0));
    }

    #[test]
    fn rotate_270_maps_correctly_at_origin() {
        let r = ps_rect(100.0, 200.0, 300.0, 240.0);
        let out = rect_page_space_to_user(270, origin_mb(), r);
        // Corners: (200,692) (240,492) (240,692) (200,492)
        let want = Rect {
            llx: 200.0,
            lly: 492.0,
            urx: 240.0,
            ury: 692.0,
        };
        assert!(rect_approx(out.0, want), "got {:?} want {:?}", out, want);
    }

    #[test]
    fn rotate_270_asymmetric_offset_regression() {
        // MediaBox [9 5 621 797], point (100, 200) at rotate=270:
        // user_x = mx0 + dy - my0 = 9 + 200 - 5 = 204
        // user_y = my1 + mx0 - dx = 797 + 9 - 100 = 706
        let out = page_space_to_user(270, asymmetric_offset_mb(), ps_point(100.0, 200.0));
        assert!(approx(out.x, 204.0));
        assert!(approx(out.y, 706.0));
    }

    #[test]
    fn corner_round_trip_at_offset_mediabox_r90() {
        // Display BL at (mx0, my0) must map to MediaBox BR = (mx1, my0).
        // Display BR at (mx0 + uh, my0) must map to MediaBox TR = (mx1, my1).
        let mb = offset_mb();
        let uw = mb.0.urx - mb.0.llx;
        let uh = mb.0.ury - mb.0.lly;
        let bl = page_space_to_user(90, mb, ps_point(mb.0.llx, mb.0.lly));
        assert!(approx(bl.x, mb.0.urx));
        assert!(approx(bl.y, mb.0.lly));
        let br = page_space_to_user(90, mb, ps_point(mb.0.llx + uh, mb.0.lly));
        assert!(approx(br.x, mb.0.urx));
        assert!(approx(br.y, mb.0.ury));
        // Display TR at (mx0 + uh, my0 + uw) → MediaBox TL = (mx0, my1).
        let tr = page_space_to_user(90, mb, ps_point(mb.0.llx + uh, mb.0.lly + uw));
        assert!(approx(tr.x, mb.0.llx));
        assert!(approx(tr.y, mb.0.ury));
    }

    #[test]
    fn negative_and_large_rotations_normalize() {
        let mb = origin_mb();
        let r = ps_rect(100.0, 200.0, 300.0, 240.0);
        let ref_out = rect_page_space_to_user(90, mb, r);
        for rot in [90, 450, -270, 810] {
            let out = rect_page_space_to_user(rot, mb, r);
            assert!(
                rect_approx(out.0, ref_out.0),
                "rotate={} got {:?}",
                rot,
                out
            );
        }
    }

    #[test]
    fn rotated_dims_swaps_for_90_and_270() {
        assert_eq!(rotated_dims(0, 612.0, 792.0), (612.0, 792.0));
        assert_eq!(rotated_dims(90, 612.0, 792.0), (792.0, 612.0));
        assert_eq!(rotated_dims(180, 612.0, 792.0), (612.0, 792.0));
        assert_eq!(rotated_dims(270, 612.0, 792.0), (792.0, 612.0));
    }
}
