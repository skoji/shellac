# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] - 2026-08-21

### Fixed

- `build.rs` wrote the cbindgen-generated header into `include/shellac.h`
  under `CARGO_MANIFEST_DIR`, which for a crates.io dependency is the
  extracted registry source. It now writes only to `OUT_DIR`; the committed
  `include/shellac.h` ships unchanged and is refreshed explicitly with
  `SHELLAC_UPDATE_HEADER=1 cargo build -p shellac`. A test checks that the
  committed header is byte-identical to the generated one, so a stale header
  fails CI. `SHELLAC_SKIP_CBINDGEN` keeps its meaning. (#7)

## [0.1.0] - 2026-08-21

### Added

- `shellac`, an lopdf-based incremental-save annotation engine for PDF. Adds,
  removes and re-comments text-markup annotations by appending one
  incremental update, so bytes it did not touch are left exactly as they
  were. Usable as a Rust library (`shellac::ops::apply_ops`) and, through
  `shellac_apply_ops` / `shellac_can_open` / `shellac_free_string`, as a C
  ABI static library with a cbindgen-generated header. Encrypted documents
  that the empty user password opens are re-encrypted under the retained
  key; documents needing a real password, and documents whose `/P` forbids
  annotation edits, are refused and left untouched. Every FFI entry point
  catches unwinding panics.
- `shellac-cli`, a command-line wrapper that routes every save through the
  same engine entry point.
- `verify`, a harness that drives a save engine over a PDF corpus and checks
  byte immutability, revision structure, text-extraction stability,
  annotation presence and placement, repeated-save endurance and the
  encrypted round-trip.
- A corpus of eleven PDF samples spanning vertical and horizontal Japanese
  text, scanned books with invisible OCR text layers, both cross-reference
  formats, page rotation, prior incremental and full-rewrite save histories,
  and four encryption configurations. Five samples are committed; the rest
  are rebuilt by the scripts in `corpus/generators/`.
- A known-exception gate (`verify gate`) that reduces a matrix run to a
  verdict against `corpus/known-exceptions.json`, so an accepted measurement
  difference is a reviewable entry rather than a special case in the
  harness. Failed save operations can never be excused, and every run feeds
  the gate a failure no entry covers to prove it still rejects.
- Continuous integration over the whole corpus on two platforms: Linux for
  everything reachable with qpdf and poppler, macOS for the PDFKit-based
  checks.

[0.1.1]: https://github.com/skoji/shellac/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/skoji/shellac/releases/tag/v0.1.0
