//! `shellac`: an lopdf-based **incremental** PDF annotation save engine.
//!
//! A full PDF rewrite is destructive for documents whose text layer was
//! produced by an OCR pass — the rewriting library re-encodes what it
//! understands and drops what it does not. This crate instead appends one
//! incremental update after the previous file's bytes, verbatim, so
//! everything it did not touch survives byte-for-byte.
//!
//! The crate is usable both as a Rust library and, through the C ABI below,
//! as a static library linked into a host application.
//!
//! # FFI contract
//!
//! Three symbols are exposed with C ABI (`cbindgen` generates the header at
//! build time from these; see `build.rs`):
//!
//! ```c
//! char* shellac_apply_ops(const char* path, const char* ops_json);
//! int32_t shellac_can_open(const char* path);
//! void shellac_free_string(char* ptr);
//! ```
//!
//! * `shellac_apply_ops(path, ops_json)` reads the PDF at `path`, applies
//!   the JSON-encoded op batch (see [`ops::OpsBatch`] and
//!   [`ops::AnnotationOp`]), and — if any op mutated the document — writes
//!   one incremental update back to `path` via `tmp file + rename`. Returns
//!   a heap-allocated NUL-terminated C string containing the JSON-encoded
//!   [`ops::ApplyResult`]. On invalid inputs (null pointers, non-UTF-8 path,
//!   malformed ops_json) it returns `NULL`. The caller MUST free the
//!   returned pointer with `shellac_free_string`.
//!
//!   Even a lopdf parse failure or IO error yields a well-formed
//!   `ApplyResult` (with `status: parse_failed` or `status: io_failed`);
//!   `NULL` is reserved for invalid-argument cases only.
//!
//! * `shellac_can_open(path)` tries to parse the PDF (filtered load).
//!   Returns:
//!
//!   - 0 = OK (parsed, not encrypted)
//!   - 1 = parse_failed
//!   - 2 = io_failed
//!   - 3 = invalid_arg
//!   - 4 = panic
//!   - 5 = password_required (encrypted, but empty user password did NOT
//!     open it — a real password is required)
//!   - 6 = encrypted (encrypted; empty user password DID open it, so the
//!     objects were decrypted in place and an incremental save can
//!     re-encrypt under the retained key)
//!   - 7 = annotations_restricted (encrypted; empty user password DID open
//!     it, but `/P` (permissions) has the ANNOTABLE bit clear — the
//!     document producer disallows annotation edits. Reported separately
//!     so a caller can refuse the annotate operation up front instead of
//!     letting the user believe a save succeeded)
//!
//! * `shellac_free_string(ptr)` frees a string previously returned from
//!   `shellac_apply_ops`. Safe to pass `NULL` (no-op).
//!
//! # Safety
//!
//! Every FFI entry point is wrapped in `std::panic::catch_unwind`. Panics
//! never cross the FFI boundary. `panic = "abort"` MUST NOT be set on the
//! release profile — it disables `catch_unwind`. See `Cargo.toml`.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

pub mod annots;
pub mod ops;
pub mod transform;

/// Return codes for [`shellac_can_open`].
const CAN_OPEN_OK: i32 = 0;
const CAN_OPEN_PARSE_FAILED: i32 = 1;
const CAN_OPEN_IO_FAILED: i32 = 2;
const CAN_OPEN_INVALID_ARG: i32 = 3;
const CAN_OPEN_PANIC: i32 = 4;
const CAN_OPEN_PASSWORD_REQUIRED: i32 = 5;
const CAN_OPEN_ENCRYPTED: i32 = 6;
const CAN_OPEN_ANNOTATIONS_RESTRICTED: i32 = 7;

/// Borrow a `&str` out of a possibly-null, NUL-terminated C string.
///
/// # Safety
/// Caller must ensure `ptr` is either NULL or a valid NUL-terminated string
/// that stays alive for the borrow.
unsafe fn borrow_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    CStr::from_ptr(ptr).to_str().ok()
}

fn into_raw_string(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Apply an op batch to `path`. See module-level docs for the full contract.
///
/// # Safety
/// `path` and `ops_json` must each be either NULL or a valid NUL-terminated
/// C string. The returned pointer, if non-NULL, must be freed with
/// [`shellac_free_string`].
#[no_mangle]
pub unsafe extern "C" fn shellac_apply_ops(
    path: *const c_char,
    ops_json: *const c_char,
) -> *mut c_char {
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let Some(path_str) = borrow_str(path) else {
            return std::ptr::null_mut();
        };
        let Some(json_str) = borrow_str(ops_json) else {
            return std::ptr::null_mut();
        };
        let batch: ops::OpsBatch = match serde_json::from_str(json_str) {
            Ok(b) => b,
            Err(_) => return std::ptr::null_mut(),
        };
        let apply_result = ops::apply_ops(Path::new(path_str), batch);
        let json = match serde_json::to_string(&apply_result) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };
        into_raw_string(json)
    }));
    match result {
        Ok(ptr) => ptr,
        Err(_) => std::ptr::null_mut(),
    }
}

/// Attempt to parse the PDF at `path` (filtered load).
///
/// Return codes:
/// * 0 — OK, the PDF parsed successfully and is not encrypted
/// * 1 — parse_failed (malformed PDF or lopdf parse error)
/// * 2 — io_failed (read error, e.g. file not found or permission denied)
/// * 3 — invalid_arg (NULL pointer or non-UTF-8 path)
/// * 4 — panic (a Rust panic was caught at the FFI boundary)
/// * 5 — password_required (encrypted; the empty user password did not
///   authenticate — a real user password is required to read the file)
/// * 6 — encrypted (encrypted; empty user password authenticated so lopdf
///   was able to decrypt the objects, and an incremental save can
///   re-encrypt what it appends under the retained key)
/// * 7 — annotations_restricted (encrypted; empty user password
///   authenticated, but `/P` (permissions integer) has the `ANNOTABLE` bit
///   clear so adding annotations is disallowed by the document producer.
///   Reported separately so a caller can refuse the annotate operation up
///   front rather than let the user think a save succeeded)
///
/// # Encryption detection
///
/// After a successful load, lopdf leaves the document in one of two states:
///
/// * `was_encrypted() == true`: the PDF was encrypted, and lopdf successfully
///   authenticated with the empty user password. Decrypted objects live in
///   `document.objects`; the `/Encrypt` reference was removed from the
///   trailer. `is_encrypted()` returns false in this state.
/// * `is_encrypted() == true`: the PDF was encrypted and the empty user
///   password did NOT authenticate. lopdf preserved the `/Encrypt` trailer
///   entry so callers can inspect it, but `document.objects` was NOT
///   decrypted (a load with a password would be required for real use).
///
/// Only the first state can produce a safe incremental update: lopdf keeps
/// the encryption state it authenticated with, so newly-written objects are
/// re-encrypted under the same key. In the second state there is no key to
/// encrypt under, and an increment would leave the trailer's `/Encrypt`
/// pointing at cleartext bytes — hence the separate return code.
///
/// # Annotation permissions
///
/// When `was_encrypted() == true`, `document.encryption_state.permissions()`
/// exposes the `/P` bitfield in its parsed form. We check
/// `Permissions::ANNOTABLE` (bit 6 of `/P`) and demote code `6` to `7` when
/// it is clear, so a caller can distinguish "encrypted but writable" from
/// "encrypted AND the producer disallows annotations".
///
/// # Safety
/// `path` must be NULL or a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn shellac_can_open(path: *const c_char) -> i32 {
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let Some(path_str) = borrow_str(path) else {
            return CAN_OPEN_INVALID_ARG;
        };
        let bytes = match std::fs::read(Path::new(path_str)) {
            Ok(b) => b,
            Err(_) => return CAN_OPEN_IO_FAILED,
        };
        match lopdf::Document::load_mem_with_options(&bytes, annots::load_options()) {
            Ok(doc) => {
                if doc.was_encrypted() {
                    // Empty user password auto-decrypted the objects.
                    // Read `/P` from the parsed encryption state and
                    // downgrade the class to `annotations_restricted`
                    // when ANNOTABLE is clear. Fall back to `.encrypted`
                    // on the defensive nil branch (`was_encrypted()` is
                    // documented to imply `encryption_state.is_some()`
                    // but we don't want to panic if that invariant ever
                    // changes upstream).
                    match doc.encryption_state.as_ref() {
                        Some(state)
                            if !state.permissions().contains(lopdf::Permissions::ANNOTABLE) =>
                        {
                            CAN_OPEN_ANNOTATIONS_RESTRICTED
                        }
                        _ => CAN_OPEN_ENCRYPTED,
                    }
                } else if doc.is_encrypted() {
                    CAN_OPEN_PASSWORD_REQUIRED
                } else {
                    CAN_OPEN_OK
                }
            }
            Err(_) => CAN_OPEN_PARSE_FAILED,
        }
    }));
    match result {
        Ok(code) => code,
        Err(_) => CAN_OPEN_PANIC,
    }
}

/// Free a string previously returned from [`shellac_apply_ops`]. Safe to
/// call with NULL (no-op).
///
/// # Safety
/// `ptr` must either be NULL or a pointer previously returned by
/// [`shellac_apply_ops`], and must not be freed twice.
#[no_mangle]
pub unsafe extern "C" fn shellac_free_string(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    let _ = CString::from_raw(ptr);
}

#[cfg(test)]
mod tests;
