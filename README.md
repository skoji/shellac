# shellac

`shellac` is a verification harness and PDF corpus for testing
**incremental-save correctness** of PDF-writing tools and libraries.

## What this is about

Many PDF editors do not rewrite a file from scratch when a small change is
made (adding a highlight, editing a note, and so on). Instead they perform
an *incremental save*: the original bytes are left untouched and the change
is appended to the end of the file as a new revision, linked in by an
updated cross-reference section. This is efficient and, done correctly,
non-destructive — but it is easy to get subtly wrong in ways that are hard
to notice by eye (a byte that shouldn't have moved did, a cross-reference
chain that should be traversable isn't, an annotation that should still be
there silently disappears).

`shellac` exists to make those properties checkable, automatically and
repeatedly, against a range of real-world PDF shapes (different producers,
encodings, encryption, page structures, and prior save histories) rather
than against a single hand-picked sample file.

## How verification works, in outline

For each sample PDF in the corpus, `shellac` runs a fixed sequence of
edit scenarios (add an annotation, modify one, remove one, and repeated
incremental saves) through the tool under test, and checks the result
against a battery of structural and content-level properties: that bytes
which should be unchanged really are, that the file's revision structure
is well-formed, that the change is visible to independent PDF readers, and
that nothing else was disturbed along the way. Results are aggregated into
a single report across the whole corpus.

Further detail on the check catalogue, the corpus, and how to run the
harness will be added as those pieces land.

## Continuous integration

Running the full matrix means driving a save engine, so CI currently covers
the parts that need no save operation: the harness's own tests, the corpus
generators, and the properties of the committed fixtures. A full-matrix
workflow will follow once a public save CLI exists.

Each check is a script under `scripts/ci/`, so any CI failure can be
reproduced locally by running the same command:

```sh
bash scripts/ci/check-generated-samples.sh     # regenerate S3, S8..S12 and assert their structure
bash scripts/ci/audit-fixtures.sh              # committed fixtures carry no identifying content
bash scripts/ci/check-fixture-invariants.sh    # S4 prefix/revision/annotation and S5 xref invariants
bash scripts/ci/build-swift-helpers.sh         # compile the PDFKit helpers (macOS)
```

`check-generated-samples.sh` takes an optional directory to build the
samples in; passing one keeps them between runs, which is worth doing
locally because S3 is rendered and OCR'd from scratch.

## Known exceptions

`corpus/known-exceptions.json` records check outcomes that are understood
and accepted rather than treated as defects — for example a position check
whose two measurement paths disagree about vertical Japanese text. Each
entry names the samples, checks, and scenarios it covers and states why, so
that tolerating an outcome is a reviewable decision in the repository rather
than a special case buried in the harness.

## License

MIT. See [LICENSE](LICENSE).
