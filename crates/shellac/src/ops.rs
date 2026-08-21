//! Annotation-op application engine.
//!
//! Consumes a batch of add / remove / modify_comment ops (JSON on the wire),
//! applies them against a filtered-loaded lopdf `Document`, and writes
//! exactly one incremental update back to disk (or skips the save entirely
//! if nothing changed).
//!
//! # Coordinate space
//!
//! Every coordinate that arrives on the wire — `rect`, `quad_points`,
//! `user_point` — is interpreted as **raw PDF user space**: the same
//! MediaBox-absolute, y-up, unrotated space that PDF files store in
//! `/Rect`, and the space Apple's PDFKit reports through
//! `PDFSelection.bounds(for:)` / `PDFAnnotation.bounds` (verified on
//! device and on macOS; see `crate::transform`). The engine writes them
//! verbatim into `/Rect` / `/QuadPoints` and passes them straight to the
//! `/Rect contains(point)` fallback resolver — no rotation transform is
//! applied here.
//!
//! # Semantics
//!
//! * `add`: the engine checks these conditions **in this order** (matching
//!   the implementation in `apply_ops`):
//!     1. `add_page_not_found` if `page_index` is out of range for the doc.
//!     2. `add_duplicate_nm` if an annotation with the same `/NM` already
//!        exists in `prev` **or** was queued in an earlier op of this same
//!        batch.
//!     3. Otherwise a new annotation object is created, added to
//!        `new_document`, and its ref appended to the page's `/Annots`.
//!
//! * `remove`:
//!     - `remove_page_not_found` if `page_index` is out of range.
//!     - `remove_target_not_found` if `/NM` did not match AND the
//!       `subtype + /Rect contains(user_point)` fallback found no
//!       annotation.
//!
//! * `modify_comment`:
//!     - `modify_page_not_found` / `modify_target_not_found` mirror the
//!       remove taxonomy.
//!     - Otherwise the annotation dict is cloned from `prev`, `/Contents`
//!       is replaced (or removed if `new_comment` is `null`), and the
//!       object is written back at the same `ObjectId`.
//!
//! All skipped ops keep their input `index` for downstream diagnostics and
//! the overall response status stays `ok` (skipped ops are not failures).
//! On lopdf parse failure the status is `parse_failed`; on IO error at
//! save time it is `io_failed`. When the input PDF is encrypted **and**
//! lopdf's empty-password auto-decrypt failed (i.e. `is_encrypted()` is
//! true on load — a password is required and the crate has no key), the
//! status is `encrypted_refused` and the file is left byte-for-byte
//! untouched. When the input PDF is empty-password auto-decrypted but
//! `/P` clears the `ANNOTABLE` permission bit, the status is
//! `annotations_restricted` and the file is likewise left untouched.
//!
//! The `annotations_restricted` refusal is defense in depth. A caller that
//! probes the file up front (via [`crate::shellac_can_open`]) can present
//! the right UI and never enqueue the batch at all — but probing is
//! typically asynchronous, so there is a window in which a save can slip
//! past the caller's own guard. Refusing here makes the "annotation edits
//! are rejected" contract hold end-to-end regardless of the caller's
//! classification timing, and the distinct status lets the caller upgrade
//! whatever encryption class it cached.
//!
//! Other empty-password auto-decrypted PDFs (`was_encrypted()` is true,
//! `is_encrypted()` is false, and `/P` allows annotation edits) take the
//! incremental save path: lopdf#522 preserves the /Encrypt trailer entry
//! and re-encrypts the incremental section under the loaded key, so the
//! round-trip is safe for those files.
//!
//! # Batch semantics (out of scope in this crate)
//!
//! This engine applies ops **in the order given** with two intra-batch
//! effects only: `add` deduplication (against previously-queued adds in
//! this batch) and last-write-wins for `modify_comment` at the same
//! `ObjectId`. It does NOT coalesce cross-op interactions such as
//! `remove → add` of the same `/NM`, `add → remove`, or double `remove`.
//! Producing a coalesced batch is the caller's responsibility.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use lopdf::{Document, IncrementalDocument, Object, ObjectId, Permissions};
use serde::{Deserialize, Serialize};

use crate::annots::{
    append_annot_refs, build_highlight_ap_stream, find_by_nm, find_by_rect_point, has_nm,
    load_options, remove_annot_refs, save_incremental, text_markup_dict, update_contents,
};
use crate::transform::{Rect, UserSpacePoint, UserSpaceRect};

// ---------------------------------------------------------------------------
// Wire format
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct OpsBatch {
    pub ops: Vec<AnnotationOp>,
}

/// A point on the page in raw PDF **user space** (MediaBox-absolute, y-up,
/// unrotated). See module docs.
#[derive(Debug, Deserialize, Clone, Copy)]
pub struct UserPoint {
    pub x: f64,
    pub y: f64,
}

impl UserPoint {
    fn into_user_space_point(self) -> UserSpacePoint {
        UserSpacePoint {
            x: self.x,
            y: self.y,
        }
    }
}

/// An axis-aligned rectangle on the page in raw PDF **user space**
/// (MediaBox-absolute, y-up, unrotated). See module docs.
#[derive(Debug, Deserialize, Clone, Copy)]
pub struct UserRect {
    pub llx: f64,
    pub lly: f64,
    pub urx: f64,
    pub ury: f64,
}

impl UserRect {
    fn into_user_space_rect(self) -> UserSpaceRect {
        UserSpaceRect(Rect {
            llx: self.llx,
            lly: self.lly,
            urx: self.urx,
            ury: self.ury,
        })
    }
}

/// The op input shape. `type` distinguishes the variant (snake_case in the
/// JSON: `add`, `remove`, `modify_comment`).
///
/// `annot_id` is the caller's stable identity for an annotation and is
/// written to / matched against the annotation's `/NM` entry. `shelff_id` is
/// accepted as a deserialization alias for it, so a writer that emits the
/// alternative key keeps working without a flag day. New callers should emit
/// `annot_id`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnnotationOp {
    Add {
        index: u32,
        page_index: u32,
        #[serde(alias = "shelff_id")]
        annot_id: String,
        subtype: String,
        rect: UserRect,
        #[serde(default)]
        quad_points: Option<Vec<[f64; 2]>>,
        color: [f32; 3],
        opacity: f32,
        contents: String,
    },
    Remove {
        index: u32,
        page_index: u32,
        #[serde(default, alias = "shelff_id")]
        annot_id: Option<String>,
        subtype: String,
        user_point: UserPoint,
    },
    ModifyComment {
        index: u32,
        page_index: u32,
        #[serde(default, alias = "shelff_id")]
        annot_id: Option<String>,
        subtype: String,
        user_point: UserPoint,
        #[serde(default)]
        new_comment: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Result JSON
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    ParseFailed,
    IoFailed,
    /// Encrypted PDF that still requires a password after load
    /// (`is_encrypted()` returned true — empty-password auto-decrypt
    /// failed). The incremental save is refused as a safety net: without
    /// an encryption key lopdf cannot re-encrypt newly-written objects,
    /// so an incremental round-trip would leave the trailer `/Encrypt`
    /// pointing at cleartext bytes. The file is left byte-for-byte
    /// untouched.
    ///
    /// The sibling case — `was_encrypted() && !ANNOTABLE`, i.e.
    /// empty-password auto-decrypt succeeded but the `/P` bit forbids
    /// annotation edits — is reported as [`Status::AnnotationsRestricted`]
    /// instead: the crate does hold the key material to re-encrypt, so
    /// that failure is a policy refusal rather than a safety-net bail-out.
    EncryptedRefused,
    /// Empty-password auto-decrypted PDF whose `/P` (permissions integer)
    /// clears the `ANNOTABLE` bit — the document producer disallows
    /// annotation edits. The file is left byte-for-byte untouched.
    ///
    /// A caller that probes encryption up front (see
    /// [`crate::shellac_can_open`], return code 7) can refuse the edit
    /// before a batch is ever queued. Because such a probe is normally
    /// asynchronous, a save can still race past it; this status makes the
    /// refusal independent of the caller's timing, and being distinct from
    /// [`Status::EncryptedRefused`] it tells the caller *why*, so a cached
    /// encryption class can be upgraded rather than mislabeled as
    /// "password required".
    AnnotationsRestricted,
}

/// Per-op skip reason. Named per-op so a caller's diagnostic layer can
/// bucket without carrying the op type separately.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// `add` op targeted a `page_index` beyond the document's page count.
    AddPageNotFound,
    /// `add` op's `/NM` collides with an existing annotation (in `prev`
    /// or earlier in this batch).
    AddDuplicateNm,
    /// `remove` op targeted a `page_index` beyond the document's page count.
    RemovePageNotFound,
    /// `remove` op's target annotation was not found by either `/NM` or by
    /// `subtype + /Rect contains(user_point)` fallback.
    RemoveTargetNotFound,
    /// `modify_comment` op targeted a `page_index` beyond the page count.
    ModifyPageNotFound,
    /// `modify_comment` op's target annotation was not found.
    ModifyTargetNotFound,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Skipped {
    pub index: u32,
    pub reason: SkipReason,
}

#[derive(Debug, Serialize)]
pub struct ApplyResult {
    pub status: Status,
    pub applied: u32,
    pub skipped: Vec<Skipped>,
}

impl ApplyResult {
    fn parse_failed() -> Self {
        Self {
            status: Status::ParseFailed,
            applied: 0,
            skipped: Vec::new(),
        }
    }
    fn io_failed() -> Self {
        Self {
            status: Status::IoFailed,
            applied: 0,
            skipped: Vec::new(),
        }
    }
    /// Encrypted PDF with no usable key — safety net. The file is left
    /// byte-for-byte untouched.
    pub(crate) fn encrypted_refused() -> Self {
        Self {
            status: Status::EncryptedRefused,
            applied: 0,
            skipped: Vec::new(),
        }
    }
    /// Empty-password auto-decrypted PDF whose `/P` disallows annotation
    /// edits — defense-in-depth refusal. The file is left byte-for-byte
    /// untouched.
    pub(crate) fn annotations_restricted() -> Self {
        Self {
            status: Status::AnnotationsRestricted,
            applied: 0,
            skipped: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

/// Outcome of the two-stage `remove` / `modify_comment` resolver.
enum ResolveOutcome {
    Found(ObjectId),
    NotFound,
}

/// Apply an ops batch to the PDF at `path`. Returns an [`ApplyResult`]
/// classifying the outcome. Any parse or IO failure yields a well-formed
/// error result (never panics; the caller relies on catch_unwind at the
/// FFI boundary but this function does not rely on unwinding).
pub fn apply_ops(path: &Path, batch: OpsBatch) -> ApplyResult {
    // Read + parse.
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(_) => return ApplyResult::io_failed(),
    };
    let prev = match Document::load_mem_with_options(&bytes, load_options()) {
        Ok(d) => d,
        Err(_) => return ApplyResult::parse_failed(),
    };

    // lopdf#522 (shipped in lopdf 0.44.0) makes the incremental writer
    // preserve the /Encrypt trailer entry and re-encrypt any newly-added
    // objects under the loaded key, so the `was_encrypted()` path
    // (empty-password auto-decrypt succeeded — lopdf holds the
    // encryption_state) can safely take the incremental save route. The
    // `is_encrypted()` path (auto-decrypt failed — no encryption_state is
    // retained) still has nothing to re-encrypt under, so we continue to
    // refuse those files as a safety net.
    if prev.is_encrypted() {
        return ApplyResult::encrypted_refused();
    }
    // Defense in depth: even when the empty-password auto-decrypt succeeded,
    // `/P` (the encrypted permissions integer) may still clear the
    // `ANNOTABLE` bit, meaning the document producer disallows annotation
    // edits. A caller that probes the file up front normally stops the batch
    // before it reaches this engine, but such a probe is typically async —
    // there is a window in which a save can slip past it. Refusing here makes
    // the "annotation edits are rejected" contract independent of the
    // caller's timing. We inspect `encryption_state.permissions()` directly
    // rather than trusting an out-of-band signal. Defensive fallback: if
    // `was_encrypted()` is true but `encryption_state` is `None` (an upstream
    // invariant change), we take the safe branch and refuse — mirrors the
    // `can_open` FFI's handling in `lib.rs`.
    if prev.was_encrypted() {
        let annotations_allowed = prev
            .encryption_state
            .as_ref()
            .map(|state| state.permissions().contains(Permissions::ANNOTABLE))
            .unwrap_or(false);
        if !annotations_allowed {
            return ApplyResult::annotations_restricted();
        }
    }

    let mut idoc = IncrementalDocument::create_from(bytes, prev);

    // Resolve pages up front (0-based → lopdf 1-based).
    let pages_by_index_0based: HashMap<u32, ObjectId> = idoc
        .get_prev_documents()
        .get_pages()
        .into_iter()
        .map(|(n, id)| (n.saturating_sub(1), id))
        .collect();

    // Pending mutations, grouped by page. modify is applied directly via
    // `set_object`; add/remove are batched per page and flushed at the end.
    let mut pending_add: HashMap<u32, Vec<ObjectId>> = HashMap::new();
    let mut pending_remove: HashMap<u32, Vec<ObjectId>> = HashMap::new();
    // (page_index → list of add /NMs queued in *this* batch, to catch
    // intra-batch duplicates).
    let mut batch_added_nms: HashMap<u32, Vec<String>> = HashMap::new();

    let mut applied: u32 = 0;
    let mut skipped: Vec<Skipped> = Vec::new();

    for op in batch.ops.into_iter() {
        match op {
            AnnotationOp::Add {
                index,
                page_index,
                annot_id,
                subtype,
                rect,
                quad_points,
                color,
                opacity,
                contents,
            } => {
                let Some(&page_id) = pages_by_index_0based.get(&page_index) else {
                    skipped.push(Skipped {
                        index,
                        reason: SkipReason::AddPageNotFound,
                    });
                    continue;
                };

                // Duplicate /NM check: both in prev and in this batch's
                // pending adds for the same page.
                let duplicate_in_prev = has_nm(idoc.get_prev_documents(), page_id, &annot_id);
                let duplicate_in_batch = batch_added_nms
                    .get(&page_index)
                    .map(|v| v.iter().any(|n| n == &annot_id))
                    .unwrap_or(false);
                if duplicate_in_prev || duplicate_in_batch {
                    skipped.push(Skipped {
                        index,
                        reason: SkipReason::AddDuplicateNm,
                    });
                    continue;
                }

                // Rect / quad_points are already in raw user space per this
                // engine's coordinate contract — write them verbatim.
                let user_rect = rect.into_user_space_rect();
                let user_quads: Option<Vec<UserSpacePoint>> = quad_points.map(|pts| {
                    pts.iter()
                        .map(|p| UserSpacePoint { x: p[0], y: p[1] })
                        .collect()
                });
                let user_quads_ref: Option<&[UserSpacePoint]> = user_quads.as_deref();

                let mut dict = text_markup_dict(
                    &subtype,
                    &annot_id,
                    &contents,
                    user_rect,
                    (color[0], color[1], color[2]),
                    opacity,
                    user_quads_ref,
                );

                // Generate a self-crafted `/AP /N` for Highlight
                // annotations so Acrobat / Preview thumbnails do not fall
                // back to the built-in QuadPoints-polygon rasterizer
                // (which draws a bow-tie for vertical quads even when the
                // QuadPoints ordering is correct). The AP fills each quad
                // as an independent Rect under `/BM Multiply`, so vertical
                // and horizontal Highlights render identically in PDFKit,
                // Preview (body + thumbnail), Acrobat, and poppler.
                //
                // Reconstruction from point-order in the AP builder
                // means we can pass the pre-reorder `user_quads` here —
                // both orderings resolve to the same per-quad bbox.
                // When no explicit quads were provided, fall back to
                // the four corners of `/Rect` so single-line Highlights
                // still get an AP.
                if subtype == "Highlight" {
                    let ap_quads_owned: Vec<UserSpacePoint> = match user_quads.as_ref() {
                        Some(pts) => pts.clone(),
                        None => {
                            let r = user_rect.0;
                            // Corners in the same conceptual order as the
                            // PDFKit Z-order callers send — reconstruction
                            // is min/max, so any 4 distinct corners work.
                            vec![
                                UserSpacePoint { x: r.llx, y: r.ury }, // TL
                                UserSpacePoint { x: r.urx, y: r.ury }, // TR
                                UserSpacePoint { x: r.llx, y: r.lly }, // BL
                                UserSpacePoint { x: r.urx, y: r.lly }, // BR
                            ]
                        }
                    };
                    let ap_stream = build_highlight_ap_stream(
                        &ap_quads_owned,
                        user_rect,
                        (color[0], color[1], color[2]),
                    );
                    let ap_id = idoc.new_document.add_object(ap_stream);
                    let mut ap_dict = lopdf::Dictionary::new();
                    ap_dict.set("N", Object::Reference(ap_id));
                    dict.set("AP", Object::Dictionary(ap_dict));
                }

                let new_id = idoc.new_document.add_object(Object::Dictionary(dict));
                pending_add.entry(page_index).or_default().push(new_id);
                batch_added_nms
                    .entry(page_index)
                    .or_default()
                    .push(annot_id);
                applied += 1;
            }
            AnnotationOp::Remove {
                index,
                page_index,
                annot_id,
                subtype,
                user_point,
            } => {
                let Some(&page_id) = pages_by_index_0based.get(&page_index) else {
                    skipped.push(Skipped {
                        index,
                        reason: SkipReason::RemovePageNotFound,
                    });
                    continue;
                };
                match resolve_target(&idoc, page_id, annot_id.as_deref(), &subtype, user_point) {
                    ResolveOutcome::Found(rid) => {
                        pending_remove.entry(page_index).or_default().push(rid);
                        applied += 1;
                    }
                    ResolveOutcome::NotFound => {
                        skipped.push(Skipped {
                            index,
                            reason: SkipReason::RemoveTargetNotFound,
                        });
                    }
                }
            }
            AnnotationOp::ModifyComment {
                index,
                page_index,
                annot_id,
                subtype,
                user_point,
                new_comment,
            } => {
                let Some(&page_id) = pages_by_index_0based.get(&page_index) else {
                    skipped.push(Skipped {
                        index,
                        reason: SkipReason::ModifyPageNotFound,
                    });
                    continue;
                };
                match resolve_target(&idoc, page_id, annot_id.as_deref(), &subtype, user_point) {
                    ResolveOutcome::Found(rid) => {
                        // Apply the modify immediately: clone from prev,
                        // replace or remove /Contents, set_object at the
                        // same ObjectId. No per-page batching needed.
                        if update_contents(&mut idoc, rid, new_comment.as_deref()).is_ok() {
                            applied += 1;
                        } else {
                            skipped.push(Skipped {
                                index,
                                reason: SkipReason::ModifyTargetNotFound,
                            });
                        }
                    }
                    ResolveOutcome::NotFound => {
                        skipped.push(Skipped {
                            index,
                            reason: SkipReason::ModifyTargetNotFound,
                        });
                    }
                }
            }
        }
    }

    // If nothing was applied (either the batch was empty or every op
    // skipped), don't touch the file. `applied` is the single source of
    // truth here — it counts adds queued into `pending_add`, removes
    // queued into `pending_remove`, and modifies already written into
    // `new_document`, so it is 0 iff no bytes need to change.
    if applied == 0 {
        return ApplyResult {
            status: Status::Ok,
            applied,
            skipped,
        };
    }

    // Flush add/remove per page in a deterministic order (helps byte-level
    // comparison in tests).
    let mut touched_pages: Vec<u32> = pending_add
        .keys()
        .chain(pending_remove.keys())
        .copied()
        .collect();
    touched_pages.sort();
    touched_pages.dedup();

    for page_index in touched_pages {
        let Some(&page_id) = pages_by_index_0based.get(&page_index) else {
            continue;
        };
        // `append_annot_refs` / `remove_annot_refs` only fail on structural
        // problems (a page dict that isn't actually a dict, an /Annots key
        // whose type is bogus, etc.) — those are parse-level defects, not
        // IO, so we report them as `parse_failed`. IO from here on is only
        // the tmp-file write + rename in `save_incremental`.
        if let Some(new_ids) = pending_add.get(&page_index) {
            if !new_ids.is_empty() && append_annot_refs(&mut idoc, page_id, new_ids).is_err() {
                return ApplyResult::parse_failed();
            }
        }
        if let Some(remove_ids) = pending_remove.get(&page_index) {
            if !remove_ids.is_empty() && remove_annot_refs(&mut idoc, page_id, remove_ids).is_err()
            {
                return ApplyResult::parse_failed();
            }
        }
    }

    match save_incremental(path, &mut idoc) {
        Ok(()) => ApplyResult {
            status: Status::Ok,
            applied,
            skipped,
        },
        Err(_) => ApplyResult::io_failed(),
    }
}

fn resolve_target(
    idoc: &IncrementalDocument,
    page_id: ObjectId,
    annot_id: Option<&str>,
    subtype: &str,
    user_point: UserPoint,
) -> ResolveOutcome {
    let prev = idoc.get_prev_documents();
    if let Some(id) = annot_id {
        if let Some(rid) = find_by_nm(prev, page_id, id) {
            return ResolveOutcome::Found(rid);
        }
    }
    match find_by_rect_point(prev, page_id, subtype, user_point.into_user_space_point()) {
        Some(rid) => ResolveOutcome::Found(rid),
        None => ResolveOutcome::NotFound,
    }
}
