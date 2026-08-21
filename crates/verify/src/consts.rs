//! Verdict-bearing string constants and the check-definition table that
//! drives the report legend. Keeping these in one place makes the strings
//! that judgments and report cells depend on auditable at a glance.

/// Tokens that must never appear in an incremental update (font subset /
/// re-embedding markers). C3 fails when any of these is found in the
/// appended bytes.
pub const FORBIDDEN_TOKENS: [&str; 5] = [
    "/ToUnicode",
    "/FontDescriptor",
    "/FontFile",
    "/Type /Font",
    "/Type/Font",
];

/// C11a detail when the scenario has no positioned annotation to verify or
/// the sample has no text anchor at all (a pass, not a fail).
pub const SKIP_NO_ANCHOR: &str = "skip: no text anchor for this sample";

/// C11b detail when the scenario carries no C11b-eligible id (a pass).
pub const SKIP_C11B: &str = "skip: no anchor for this sample, or scenario has no C11b-eligible id";

/// C11b detail when the helper unexpectedly skipped the evaluation (a pass).
pub const SKIP_C11B_UNEVALUATED: &str = "skip: pdfkit_check did not evaluate c11b (unexpected)";

/// Fatal marker filled into every scenario slot of a refused-mode
/// encrypted sample.
pub const SKIP_REFUSED: &str = "skipped: encrypted-refused expected";

/// Per-iteration incremental growth limit for C8 (fail at >= 50 KiB).
pub const C8_MAX_DELTA: i64 = 50 * 1024;

/// Incremental saves per sample in the C8 endurance scenario.
///
/// The loop that runs them, the completeness check that decides a run
/// stopped early, and the sentences that report both all state this number.
/// Split across those places it is four chances for the report to describe a
/// run that did not happen.
pub const LOOP_ITERATIONS: usize = 10;

/// One check definition: the id used in tables plus the legend meaning.
pub struct CheckDef {
    pub id: &'static str,
    pub meaning: &'static str,
}

/// The full check catalogue, in summary-matrix column order. The report
/// legend is generated from this table.
pub const CHECK_DEFS: &[CheckDef] = &[
    CheckDef {
        id: "C1",
        meaning: "forward byte immutability: the previous revision is a byte-identical prefix of the new file",
    },
    CheckDef {
        id: "C2",
        meaning: "%%EOF marker count grows by exactly 1 per incremental save",
    },
    CheckDef {
        id: "C3",
        meaning: "increment audit: appended bytes end in startxref and contain no font re-embedding tokens",
    },
    CheckDef {
        id: "C4",
        meaning: "pdftotext extraction unchanged (whitespace-normalized comparison)",
    },
    CheckDef {
        id: "C5",
        meaning: "PDFKit text extraction unchanged (exact comparison)",
    },
    CheckDef {
        id: "C6",
        meaning: "annotation presence/absence via qpdf JSON: the /NM object exists and is attached to a page's /Annots",
    },
    CheckDef {
        id: "C6-quads",
        meaning: "/QuadPoints shape survives the round-trip (add-multiline only: >= 16 floats, multiple of 8)",
    },
    CheckDef {
        id: "C7",
        meaning: "PDFKit reload: document loads, page count and expected /NM set match, thumbnails render",
    },
    CheckDef {
        id: "C8",
        meaning: "repeated-save endurance: 10 loop iterations all pass and each increment stays under 50KB",
    },
    CheckDef {
        id: "C11a",
        meaning: "position verification, independent path: pdftotext -bbox-layout needle bounds vs the /Rect read back via qpdf (tolerance 3pt)",
    },
    CheckDef {
        id: "C11b",
        meaning: "position verification, PDFKit path: re-located needle bounds vs annotation bounds (tolerance 3pt)",
    },
    CheckDef {
        id: "C-enc-qpdf",
        meaning: "encrypted round-trip integrity: qpdf --check --password= after each scenario (encrypted roundtrip fixtures only)",
    },
    CheckDef {
        id: "Encrypted",
        meaning: "encrypted-path aggregate: C-enc-qpdf results for roundtrip fixtures, or the refusal contract (expected status + file unchanged) for refused fixtures",
    },
];

/// One-line vocabulary note for summary-matrix cells.
pub const CELL_VOCAB_NOTE: &str = "Cell values: `pass` / `**FAIL**` (at least one instance of the check failed) / \
`**FAIL (op)**` (a save operation itself failed, so its checks never ran) / `n/a` (not applicable). \
A `pass` whose detail starts with `skip:` means there was nothing to verify (e.g. no text anchor), \
not a verified position match. Encrypted-column values: `roundtrip-pass` / `refused-unchanged` / `**FAIL**` / `n/a`.";

#[cfg(test)]
mod tests {
    use super::*;

    // The legend states the loop count in prose, and a reader takes it for
    // what the run did. Pin the sentence to the constant the run reads.
    #[test]
    fn the_c8_legend_states_the_loop_iteration_count() {
        let c8 = CHECK_DEFS
            .iter()
            .find(|d| d.id == "C8")
            .expect("C8 is in the check catalogue");
        assert!(
            c8.meaning
                .contains(&format!("{LOOP_ITERATIONS} loop iterations")),
            "the C8 legend does not state {LOOP_ITERATIONS} loop iterations: {}",
            c8.meaning
        );
    }
}
