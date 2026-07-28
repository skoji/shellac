//! `shellac-cli`: a thin CLI wrapper around [`shellac::ops::apply_ops`],
//! used as the save engine of the corpus regression harness.
//!
//! It owns no PDF-writing logic of its own. Every save routes through the
//! *same* engine an application links against, so a corpus regression run
//! doubles as a smoke test of the production save path — a behavior drift
//! between the two is not possible, because there is only one.
//!
//! The subcommands exist to script the harness's scenarios: add a fixed
//! highlight + underline, remove them again, add one highlight per
//! iteration for repeated-save runs, change a comment, add a multi-quad
//! highlight, and assert the appearance stream the engine generates.

use std::path::Path;
use std::process::ExitCode;

use lopdf::{Document, Object};
use shellac::ops::{apply_ops, AnnotationOp, ApplyResult, OpsBatch, Status, UserPoint, UserRect};

// ---------------------------------------------------------------------------
// Fixed constants. These are frozen: the corpus regression harness compares
// runs across engines and across time, so the contract-visible fields
// (subtype, /NM, contents string, colors, /CA) must not drift.
// ---------------------------------------------------------------------------

const HIGHLIGHT_ID: &str = "shelff-verify-hl-1";
const UNDERLINE_ID: &str = "shelff-verify-ul-1";
const MULTILINE_HL_ID: &str = "shelff-verify-hl-multiline-1";
const HIGHLIGHT_CONTENTS: &str = "shelff 検証コメント (highlight)";
const UNDERLINE_CONTENTS: &str = "shelff 検証コメント (underline)";
const MULTILINE_CONTENTS: &str = "shelff 検証コメント (multi-line highlight)";
const HIGHLIGHT_COLOR: [f32; 3] = [1.0, 0.8, 0.0];
const UNDERLINE_COLOR: [f32; 3] = [0.9, 0.2, 0.3];
const LOOP_COLOR: [f32; 3] = [0.2, 0.6, 1.0];
const DEFAULT_OPACITY: f32 = 0.5;

fn loop_id(i: u32) -> String {
    format!("shelff-verify-loop-{i}")
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  \
         shellac-cli add <pdf> [--hl-rect llx,lly,urx,ury] [--ul-rect llx,lly,urx,ury]\n  \
         \x20   add highlight+underline to page 1 as one increment via\n  \
         \x20   shellac::ops::apply_ops; rects default to the legacy\n  \
         \x20   fixed coordinates when the flags are absent.\n  \
         shellac-cli remove <pdf>         remove the two verify annotations as one increment\n  \
         shellac-cli loop-one <i> <pdf> [--rect llx,lly,urx,ury]\n  \
         \x20   add one highlight (shelff-verify-loop-<i>) as one increment.\n  \
         shellac-cli modify-comment <new-contents> <pdf>\n  \
         \x20   change the /Contents of shelff-verify-hl-1 as one increment.\n  \
         shellac-cli add-multiline <pdf> --hl-quads \"x0,y0,x1,y1,x2,y2,x3,y3;...\"\n  \
         \x20   add one Highlight (shelff-verify-hl-multiline-1) whose\n  \
         \x20   /QuadPoints spans N ≥ 2 rectangles (semicolon-separated,\n  \
         \x20   each with 4 TL, TR, BL, BR points in user space) —\n  \
         \x20   the per-line marker regression.\n  \
         shellac-cli check-ap <pdf>\n  \
         \x20   inspect <pdf> and assert: the Highlight named\n  \
         \x20   shelff-verify-hl-1 (or -multiline-1) carries /AP /N\n  \
         \x20   pointing at a Form XObject, and the Underline named\n  \
         \x20   shelff-verify-ul-1 does NOT carry /AP — the\n  \
         \x20   self-generated Highlight AP regression."
    );
    std::process::exit(1);
}

// ---------------------------------------------------------------------------
// Rects (CLI-facing = user space) → UserRect (engine-facing = user space)
// ---------------------------------------------------------------------------

/// The rect shape the harness passes on the CLI. Lives in raw PDF user
/// space — macOS PDFKit's `selection.bounds(for: page)` returns the
/// selection rect in the page's unrotated user space, so the harness hands
/// those bounds over as-is. [`shellac::ops`] also takes its `UserRect` in
/// **raw user space**, so the CLI rect is forwarded verbatim — there is no
/// rotation inverse anywhere on this path.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    llx: f64,
    lly: f64,
    urx: f64,
    ury: f64,
}

fn legacy_highlight_rect() -> Rect {
    Rect {
        llx: 100.0,
        lly: 100.0,
        urx: 300.0,
        ury: 120.0,
    }
}
fn legacy_underline_rect() -> Rect {
    Rect {
        llx: 100.0,
        lly: 140.0,
        urx: 300.0,
        ury: 160.0,
    }
}
fn legacy_loop_rect(i: u32) -> Rect {
    Rect {
        llx: 100.0,
        lly: (180 + 20 * i) as f64,
        urx: 300.0,
        ury: (196 + 20 * i) as f64,
    }
}

fn parse_rect(s: &str) -> Option<Rect> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut v = [0.0_f64; 4];
    for (i, p) in parts.iter().enumerate() {
        v[i] = p.trim().parse().ok()?;
    }
    Some(Rect {
        llx: v[0],
        lly: v[1],
        urx: v[2],
        ury: v[3],
    })
}

fn rect_flag(args: &[String], flag: &str, default: Rect) -> Rect {
    for (i, a) in args.iter().enumerate() {
        if a == flag {
            let Some(value) = args.get(i + 1) else {
                usage();
            };
            let Some(rect) = parse_rect(value) else {
                usage();
            };
            return rect;
        }
    }
    default
}

/// Parse the wire format of `--hl-quads`: semicolon-separated quads, each
/// quad is 4 (x, y) points in TL, TR, BL, BR order = 8 comma-separated
/// floats. Returns the flattened `Vec<[f64; 2]>` that
/// `AnnotationOp::Add.quad_points` expects. The engine writes those points
/// verbatim into /QuadPoints in the order given, which is what makes the
/// per-line marker scenario reproducible.
fn parse_quads(s: &str) -> Option<Vec<[f64; 2]>> {
    let quads: Vec<&str> = s.split(';').filter(|q| !q.trim().is_empty()).collect();
    if quads.is_empty() {
        return None;
    }
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(quads.len() * 4);
    for q in quads {
        let nums: Vec<&str> = q.split(',').collect();
        if nums.len() != 8 {
            return None;
        }
        let mut v = [0.0_f64; 8];
        for (i, n) in nums.iter().enumerate() {
            v[i] = n.trim().parse().ok()?;
        }
        out.push([v[0], v[1]]);
        out.push([v[2], v[3]]);
        out.push([v[4], v[5]]);
        out.push([v[6], v[7]]);
    }
    Some(out)
}

/// Look up the value of `flag` and parse it as a multi-quad list. Exits via
/// `usage()` when the flag is missing or the payload is malformed — this is
/// only called from `add-multiline`, where quads are mandatory.
fn required_quads_flag(args: &[String], flag: &str) -> Vec<[f64; 2]> {
    for (i, a) in args.iter().enumerate() {
        if a == flag {
            let Some(value) = args.get(i + 1) else {
                usage();
            };
            let Some(quads) = parse_quads(value) else {
                usage();
            };
            return quads;
        }
    }
    usage();
}

impl Rect {
    fn to_user_rect(self) -> UserRect {
        UserRect {
            llx: self.llx,
            lly: self.lly,
            urx: self.urx,
            ury: self.ury,
        }
    }
}

/// Bounding box of a flattened `Vec<[f64; 2]>` — used to derive `/Rect`
/// when the caller specifies /QuadPoints but not an explicit rect.
fn bbox_of_quads(quads: &[[f64; 2]]) -> UserRect {
    let (mut llx, mut lly) = (quads[0][0], quads[0][1]);
    let (mut urx, mut ury) = (quads[0][0], quads[0][1]);
    for p in &quads[1..] {
        if p[0] < llx {
            llx = p[0];
        }
        if p[0] > urx {
            urx = p[0];
        }
        if p[1] < lly {
            lly = p[1];
        }
        if p[1] > ury {
            ury = p[1];
        }
    }
    UserRect { llx, lly, urx, ury }
}

// ---------------------------------------------------------------------------
// OpsBatch builders
// ---------------------------------------------------------------------------

fn build_add_op(
    index: u32,
    id: &str,
    subtype: &str,
    user_rect: Rect,
    color: [f32; 3],
    opacity: f32,
    contents: &str,
) -> AnnotationOp {
    AnnotationOp::Add {
        index,
        page_index: 0,
        annot_id: id.to_string(),
        subtype: subtype.to_string(),
        rect: user_rect.to_user_rect(),
        quad_points: None,
        color,
        opacity,
        contents: contents.to_string(),
    }
}

fn build_remove_op(index: u32, id: &str, subtype: &str) -> AnnotationOp {
    // user_point is only consulted when the /NM lookup fails; since every
    // /NM we ever emit ("shelff-verify-*") round-trips through the same
    // batch, the fallback is unreachable in the verification harness. Pass
    // (0, 0) rather than plumbing rects the CLI does not have.
    AnnotationOp::Remove {
        index,
        page_index: 0,
        annot_id: Some(id.to_string()),
        subtype: subtype.to_string(),
        user_point: UserPoint { x: 0.0, y: 0.0 },
    }
}

fn build_modify_op(index: u32, id: &str, subtype: &str, new_comment: &str) -> AnnotationOp {
    AnnotationOp::ModifyComment {
        index,
        page_index: 0,
        annot_id: Some(id.to_string()),
        subtype: subtype.to_string(),
        user_point: UserPoint { x: 0.0, y: 0.0 },
        new_comment: Some(new_comment.to_string()),
    }
}

/// Build an `AnnotationOp::Add` whose `/QuadPoints` will span multiple
/// rectangles (the per-line marker case). `/Rect` is
/// derived as the bbox of all quads so PDF viewers that fall back to /Rect
/// (no /QuadPoints support) still show something reasonable, and `apply_ops`
/// writes the quads verbatim in the given order into /QuadPoints.
fn build_add_op_with_quads(
    index: u32,
    id: &str,
    subtype: &str,
    quads: Vec<[f64; 2]>,
    color: [f32; 3],
    opacity: f32,
    contents: &str,
) -> AnnotationOp {
    let rect = bbox_of_quads(&quads);
    AnnotationOp::Add {
        index,
        page_index: 0,
        annot_id: id.to_string(),
        subtype: subtype.to_string(),
        rect,
        quad_points: Some(quads),
        color,
        opacity,
        contents: contents.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

/// Apply `batch` to `path` via the engine, log the ApplyResult to stderr,
/// and translate to a POSIX exit code. Anything other than
/// `status: ok, applied > 0` is a failure — the verification harness never
/// legitimately batches an all-skipped op set, so treating "applied == 0"
/// as failure catches misuse (e.g. remove against a PDF that lacks the
/// expected /NM) at the CLI boundary instead of pushing the ambiguity into
/// the harness's error paths.
fn run_and_report(path: &Path, subcommand: &str, batch: OpsBatch) -> ExitCode {
    let result: ApplyResult = apply_ops(path, batch);
    let json = serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string());
    eprintln!("shellac-cli {}: {}", subcommand, json);
    match (&result.status, result.applied) {
        (Status::Ok, applied) if applied > 0 => ExitCode::SUCCESS,
        (Status::Ok, _) => {
            eprintln!(
                "shellac-cli {}: no ops applied (all skipped); refusing to succeed",
                subcommand
            );
            ExitCode::FAILURE
        }
        _ => ExitCode::FAILURE,
    }
}

// ---------------------------------------------------------------------------
// check-ap: post-save regression for the self-generated Highlight AP
// ---------------------------------------------------------------------------
//
// After `shellac-cli add` we expect:
//   - shelff-verify-hl-1 (Highlight) to carry `/AP /N <ref>` pointing at
//     a Form XObject (the self-generated appearance stream).
//   - shelff-verify-ul-1 (Underline) to NOT carry `/AP` — the AP is
//     Highlight-only.
//
// The Highlight name is checked against both HIGHLIGHT_ID and
// MULTILINE_HL_ID so that both the plain `add` corpus check and the
// `add-multiline` per-line marker corpus check reuse the same command.
// If the encrypted-roundtrip shell script wants to verify both after a
// remove, the caller should re-add and then re-check.
fn find_by_nm(doc: &Document, nm: &str) -> Option<lopdf::ObjectId> {
    for (_pn, page_id) in doc.get_pages() {
        if let Some(id) = shellac::annots::find_by_nm(doc, page_id, nm) {
            return Some(id);
        }
    }
    None
}

fn ap_form_ref(doc: &Document, annot_id: lopdf::ObjectId) -> Option<lopdf::ObjectId> {
    let dict = doc.get_object(annot_id).ok()?.as_dict().ok()?;
    let ap = dict.get(b"AP").ok()?.as_dict().ok()?;
    ap.get(b"N").ok()?.as_reference().ok()
}

fn check_ap(path: &Path) -> ExitCode {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("shellac-cli check-ap: cannot read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    // Plain load (no strip filter) so we can inspect the AP stream's
    // decrypted content. Empty user password auto-decrypts encrypted
    // fixtures the shell script produces.
    let doc = match Document::load_mem(&bytes) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("shellac-cli check-ap: parse failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut failures: Vec<String> = Vec::new();

    // Highlight: /AP /N → Form XObject required.
    let hl_annot = find_by_nm(&doc, HIGHLIGHT_ID).or_else(|| find_by_nm(&doc, MULTILINE_HL_ID));
    match hl_annot {
        None => failures.push(format!(
            "Highlight (/NM {} or {}) not found",
            HIGHLIGHT_ID, MULTILINE_HL_ID
        )),
        Some(id) => match ap_form_ref(&doc, id) {
            None => failures.push("Highlight is missing /AP /N reference".to_string()),
            Some(ap_ref) => {
                let ok = doc
                    .get_object(ap_ref)
                    .ok()
                    .and_then(|o| o.as_stream().ok())
                    .and_then(|s| s.dict.get(b"Subtype").ok())
                    .and_then(|n| n.as_name().ok())
                    .map(|n| n == b"Form")
                    .unwrap_or(false);
                if !ok {
                    failures.push("Highlight /AP /N does not point at a Form XObject".to_string());
                }
            }
        },
    }

    // Underline: /AP must NOT be present.
    match find_by_nm(&doc, UNDERLINE_ID) {
        // Underline may legitimately be absent (e.g. after remove); only
        // flag when it exists but carries an /AP.
        None => {}
        Some(id) => {
            if let Ok(dict) = doc.get_object(id).and_then(Object::as_dict) {
                if dict.get(b"AP").is_ok() {
                    failures.push("Underline carries an unexpected /AP".to_string());
                }
            }
        }
    }

    if failures.is_empty() {
        eprintln!("shellac-cli check-ap: ok");
        ExitCode::SUCCESS
    } else {
        for f in &failures {
            eprintln!("shellac-cli check-ap: {}", f);
        }
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
    }

    match args[1].as_str() {
        "add" => {
            if args.len() < 3 {
                usage();
            }
            let path = Path::new(&args[2]);
            let hl_rect = rect_flag(&args, "--hl-rect", legacy_highlight_rect());
            let ul_rect = rect_flag(&args, "--ul-rect", legacy_underline_rect());
            let batch = OpsBatch {
                ops: vec![
                    build_add_op(
                        0,
                        HIGHLIGHT_ID,
                        "Highlight",
                        hl_rect,
                        HIGHLIGHT_COLOR,
                        DEFAULT_OPACITY,
                        HIGHLIGHT_CONTENTS,
                    ),
                    build_add_op(
                        1,
                        UNDERLINE_ID,
                        "Underline",
                        ul_rect,
                        UNDERLINE_COLOR,
                        DEFAULT_OPACITY,
                        UNDERLINE_CONTENTS,
                    ),
                ],
            };
            run_and_report(path, "add", batch)
        }
        "remove" => {
            if args.len() != 3 {
                usage();
            }
            let path = Path::new(&args[2]);
            let batch = OpsBatch {
                ops: vec![
                    build_remove_op(0, HIGHLIGHT_ID, "Highlight"),
                    build_remove_op(1, UNDERLINE_ID, "Underline"),
                ],
            };
            run_and_report(path, "remove", batch)
        }
        "loop-one" => {
            if args.len() < 4 {
                usage();
            }
            let Ok(i) = args[2].parse::<u32>() else {
                usage();
            };
            if i < 1 {
                usage();
            }
            let path = Path::new(&args[3]);
            let rect = rect_flag(&args, "--rect", legacy_loop_rect(i));
            let contents = format!("shelff loop 検証 {i}");
            let batch = OpsBatch {
                ops: vec![build_add_op(
                    0,
                    &loop_id(i),
                    "Highlight",
                    rect,
                    LOOP_COLOR,
                    DEFAULT_OPACITY,
                    &contents,
                )],
            };
            run_and_report(path, "loop-one", batch)
        }
        "modify-comment" => {
            if args.len() != 4 {
                usage();
            }
            let new_contents = &args[2];
            let path = Path::new(&args[3]);
            let batch = OpsBatch {
                ops: vec![build_modify_op(0, HIGHLIGHT_ID, "Highlight", new_contents)],
            };
            run_and_report(path, "modify-comment", batch)
        }
        "check-ap" => {
            if args.len() != 3 {
                usage();
            }
            let path = Path::new(&args[2]);
            check_ap(path)
        }
        "add-multiline" => {
            if args.len() < 3 {
                usage();
            }
            let path = Path::new(&args[2]);
            let quads = required_quads_flag(&args, "--hl-quads");
            // Reject single-quad payloads at the CLI boundary — a lone quad
            // is what the plain `add` subcommand already exercises; the
            // point of add-multiline is the N ≥ 2 case.
            if quads.len() < 8 {
                eprintln!(
                    "shellac-cli add-multiline: --hl-quads must have at least 2 rects (8 points)"
                );
                return ExitCode::FAILURE;
            }
            let batch = OpsBatch {
                ops: vec![build_add_op_with_quads(
                    0,
                    MULTILINE_HL_ID,
                    "Highlight",
                    quads,
                    HIGHLIGHT_COLOR,
                    DEFAULT_OPACITY,
                    MULTILINE_CONTENTS,
                )],
            };
            run_and_report(path, "add-multiline", batch)
        }
        _ => usage(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }
    fn rect_approx(a: Rect, b: Rect) -> bool {
        approx(a.llx, b.llx) && approx(a.lly, b.lly) && approx(a.urx, b.urx) && approx(a.ury, b.ury)
    }

    #[test]
    fn parse_rect_accepts_four_floats() {
        let r = parse_rect("100,200,300,240").expect("parses");
        assert!(rect_approx(
            r,
            Rect {
                llx: 100.0,
                lly: 200.0,
                urx: 300.0,
                ury: 240.0
            }
        ));
    }

    #[test]
    fn parse_rect_rejects_wrong_arity() {
        assert!(parse_rect("1,2,3").is_none());
        assert!(parse_rect("1,2,3,4,5").is_none());
        assert!(parse_rect("").is_none());
    }

    #[test]
    fn parse_quads_single_rect() {
        let q = parse_quads("100,240,300,240,100,200,300,200").expect("parses");
        assert_eq!(q.len(), 4, "one rect = 4 points");
        assert_eq!(q[0], [100.0, 240.0]); // TL
        assert_eq!(q[3], [300.0, 200.0]); // BR
    }

    #[test]
    fn parse_quads_multi_rect() {
        let q = parse_quads("100,240,300,240,100,220,300,220;100,220,300,220,100,200,300,200")
            .expect("parses");
        assert_eq!(q.len(), 8, "two rects = 8 points");
        assert_eq!(q[4], [100.0, 220.0]); // second rect TL
    }

    #[test]
    fn parse_quads_rejects_wrong_arity() {
        // Missing points inside a rect
        assert!(parse_quads("1,2,3,4,5,6,7").is_none());
        assert!(parse_quads("").is_none());
        // Empty rect after a valid one
        assert!(parse_quads("1,2,3,4,5,6,7,8;").is_some()); // trailing empty is trimmed
        assert!(parse_quads("1,2,3,4,5,6,7,8;;1,2,3,4,5,6,7,8").is_some()); // internal empties trimmed
    }

    #[test]
    fn bbox_of_quads_covers_all_points() {
        let quads = vec![
            [100.0, 240.0],
            [300.0, 240.0],
            [100.0, 220.0],
            [300.0, 220.0],
            [90.0, 220.0],
            [280.0, 220.0],
            [90.0, 190.0],
            [280.0, 190.0],
        ];
        let b = bbox_of_quads(&quads);
        assert!((b.llx - 90.0).abs() < 1e-6);
        assert!((b.lly - 190.0).abs() < 1e-6);
        assert!((b.urx - 300.0).abs() < 1e-6);
        assert!((b.ury - 240.0).abs() < 1e-6);
    }
}
