# shellac

An **incremental-save annotation engine for PDF**, built on
[lopdf](https://crates.io/crates/lopdf). A full rewrite re-encodes what the
writing library understands and drops what it does not — the invisible OCR
text layer of a scanned book, stored as Form XObjects, is the usual
casualty. `shellac` instead appends one incremental update after the
previous file's bytes, verbatim, so everything it did not touch survives
byte-for-byte. It adds, removes and re-comments text-markup annotations,
takes its op batch as JSON, and is usable as a Rust library, as a C ABI
static library, or through the `shellac-cli` binary.

## Rust API

```rust
use std::path::Path;
use shellac::ops::{apply_ops, OpsBatch, Status};

let batch: OpsBatch = serde_json::from_str(ops_json)?;
let result = apply_ops(Path::new("book.pdf"), batch);
assert_eq!(result.status, Status::Ok);
```

`shellac::ops::apply_ops` is the entry point. `shellac::ops` also carries the
wire types (`OpsBatch`, `AnnotationOp`, `UserRect`, `UserPoint`,
`ApplyResult`, `Status`, `SkipReason`), and `shellac::transform` documents
the coordinate contract.

## C ABI

Three symbols, declared in the cbindgen-generated `include/shellac.h`:

```c
char*   shellac_apply_ops(const char* path, const char* ops_json);
int32_t shellac_can_open(const char* path);
void    shellac_free_string(char* ptr);
```

`shellac_apply_ops` returns a heap-allocated NUL-terminated C string holding
the JSON-encoded `ApplyResult`; the caller MUST free it with
`shellac_free_string`. It returns `NULL` when no result could be produced or
handed back: an invalid argument (a null pointer or a non-UTF-8 path),
malformed `ops_json`, a panic caught at the boundary, or a result that failed
to serialize. A parse failure or an IO error is none of those — each still
yields a well-formed `ApplyResult`, with `status` set to `parse_failed` or
`io_failed`. `shellac_free_string(NULL)` is a no-op.

## Ops JSON

A batch is `{"ops": [...]}`, applied in the order given. Every op carries an
`index`, which is echoed back in `skipped` so a caller can attribute a skip;
a zero-based `page_index`; and a `subtype` (`Highlight`, `Underline`, ...).

`annot_id` is the caller's stable identity for an annotation: it is written
to the annotation's `/NM` entry by `add`, and matched against `/NM` by
`remove` and `modify_comment`. When `/NM` does not match, those two fall
back to `subtype` plus `/Rect contains(user_point)`. The key `shelff_id` is
accepted as an alias for `annot_id`.

Rectangles (`rect`), quad points (`quad_points`) and points (`user_point`)
are all in raw PDF **user space**: MediaBox-absolute, y-up, unrotated — the
same space the file itself stores in `/Rect`. No coordinate value is
transformed on the way in, and no rotation transform is applied, so a
`/Rotate`-bearing page needs no adjustment from the caller.

Point *order* is the one thing normalized. A `Highlight` quad that is taller
than it is wide — a run of vertical Japanese text — is written in Acrobat's
`[BL, TL, BR, TR]` order regardless of the order it arrived in, because
Acrobat's built-in QuadPoints rasterizer draws a bow-tie for such a quad
otherwise. Wide quads, and `Underline` / `StrikeOut` / `Squiggly` at any
aspect ratio, keep the caller's order.

### add

`color` is RGB in 0..1, `opacity` becomes `/CA`. `quad_points` is optional;
it is a flat list of `[x, y]` pairs, four per marked rectangle, and when it
is absent the four corners of `rect` are used.

```json
{"ops": [{
  "type": "add",
  "index": 0,
  "page_index": 0,
  "annot_id": "note-8f3a",
  "subtype": "Highlight",
  "rect": {"llx": 100.0, "lly": 100.0, "urx": 300.0, "ury": 120.0},
  "quad_points": [[100.0, 120.0], [300.0, 120.0], [100.0, 100.0], [300.0, 100.0]],
  "color": [1.0, 0.8, 0.0],
  "opacity": 0.5,
  "contents": "a comment"
}]}
```

### remove

`annot_id` may be omitted, in which case only the `/Rect`-contains fallback
resolves the target.

```json
{"ops": [{
  "type": "remove",
  "index": 0,
  "page_index": 0,
  "annot_id": "note-8f3a",
  "subtype": "Highlight",
  "user_point": {"x": 150.0, "y": 110.0}
}]}
```

### modify_comment

`new_comment` replaces `/Contents`; `null` (or an absent key) removes it.

```json
{"ops": [{
  "type": "modify_comment",
  "index": 0,
  "page_index": 0,
  "annot_id": "note-8f3a",
  "subtype": "Highlight",
  "user_point": {"x": 150.0, "y": 110.0},
  "new_comment": "an edited comment"
}]}
```

### ApplyResult

```json
{
  "status": "ok",
  "applied": 1,
  "skipped": [{"index": 1, "reason": "add_duplicate_nm"}]
}
```

`status` is one of `ok`, `parse_failed`, `io_failed`, `encrypted_refused`,
`annotations_restricted`. `applied` counts the ops that changed the
document. `skipped` lists the ops that did not, each with a `reason` of
`add_page_not_found`, `add_duplicate_nm`, `remove_page_not_found`,
`remove_target_not_found`, `modify_page_not_found` or
`modify_target_not_found`. A skipped op is not a failure, so the status
stays `ok`; if nothing was applied, the file is not written at all.

`add` rejects a duplicate `/NM` — as `add_duplicate_nm` — if the same
`annot_id` is already on that page in the document, or was queued for that
page earlier in the same batch. Both checks are per page: the same
`annot_id` used on two different pages is not a duplicate and both adds are
applied, so an `annot_id` a caller wants to be unique document-wide has to
be made so by the caller.

Beyond that dedup and last-write-wins for `modify_comment` at the same
object, the engine has no intra-batch effects. It does not coalesce cross-op
interactions such as `remove` then `add` of the same `/NM`; producing a
coalesced batch is the caller's job.

## can_open

`shellac_can_open` classifies a document before any batch is queued:

| Code | Meaning |
|---:|---|
| 0 | ok — parsed, not encrypted |
| 1 | parse_failed — malformed PDF or lopdf parse error |
| 2 | io_failed — read error, e.g. file not found or permission denied |
| 3 | invalid_arg — NULL pointer or non-UTF-8 path |
| 4 | panic — a Rust panic was caught at the FFI boundary |
| 5 | password_required — encrypted, and the empty user password did not authenticate |
| 6 | encrypted — encrypted, and the empty user password did authenticate |
| 7 | annotations_restricted — as 6, but `/P` has the `ANNOTABLE` bit clear |

## Encryption

When the empty user password authenticates, lopdf decrypts the objects and
retains the encryption state, so the appended increment is re-encrypted
under the same key and the round-trip is safe. That is code 6, and
`apply_ops` proceeds normally.

When it does not authenticate, there is no key to encrypt an increment
under — an incremental save would leave the trailer's `/Encrypt` pointing at
cleartext bytes. `apply_ops` refuses with `encrypted_refused` and leaves the
file byte-for-byte untouched.

When the document decrypts but `/P` clears the `ANNOTABLE` bit, its producer
disallows annotation edits, and `apply_ops` refuses with
`annotations_restricted` — again leaving the file untouched. This refusal is
defense in depth: a caller that probes with `shellac_can_open` normally
never enqueues such a batch, but the probe is typically asynchronous, so a
save can race past the caller's own guard.

## Safety

Every FFI entry point is wrapped in `std::panic::catch_unwind`, so a panic
never unwinds across the C ABI boundary — which would be undefined
behaviour. `panic = "abort"` disables `catch_unwind` and therefore MUST NOT
be set on any profile that includes this crate.

## shellac-cli

The crate ships `shellac-cli`, a thin wrapper that owns no PDF-writing logic
of its own: every save routes through `apply_ops`. It is what the
repository's corpus harness drives, and each subcommand is one of the
harness's scenarios.

```sh
shellac-cli add <pdf> [--hl-rect llx,lly,urx,ury] [--ul-rect llx,lly,urx,ury]
shellac-cli remove <pdf>
shellac-cli loop-one <i> <pdf> [--rect llx,lly,urx,ury]
shellac-cli modify-comment <new-contents> <pdf>
shellac-cli add-multiline <pdf> --hl-quads "x0,y0,x1,y1,x2,y2,x3,y3;..."
shellac-cli check-ap <pdf>
```

## Verification

The engine's behaviour is checked against a corpus of eleven PDFs — varying
producer, encoding, encryption, page structure and prior save history — by a
harness that asserts byte immutability, revision structure, text-extraction
stability, annotation placement and repeated-save endurance, on both Linux
and macOS. Both live in the repository:
<https://github.com/skoji/shellac>.

## License

MIT.
