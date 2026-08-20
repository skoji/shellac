//! The known-exception gate: which matrix outcomes become failing cells,
//! and which cells the committed registry accepts.
//!
//! The registry test at the bottom pins both halves of the CI contract: the
//! measured distribution of this corpus must not fail the run, and a
//! failure outside that distribution must.

use verify::encrypted::EncMode;
use verify::exceptions::KnownExceptions;
use verify::gate::{
    FATAL_CHECK, FailCell, FailCells, GateOutcome, apply_exceptions, collect_fail_cells,
    render_gate_section, sanitize_cells,
};
use verify::sample::SampleResult;
use verify::sanitize::Sanitizer;
use verify::scenario::Scenario;

// ---- builders -------------------------------------------------------------

fn scenario(name: &str, checks: &[(&str, bool)]) -> Scenario {
    let mut sc = Scenario {
        name: name.to_string(),
        ..Default::default()
    };
    for (id, pass) in checks {
        sc.add(id, *pass, "detail");
    }
    sc
}

fn passing(name: &str) -> Scenario {
    scenario(name, &[("C1", true), ("C11a", true)])
}

/// A sample whose every scenario passes, with a complete ten-iteration loop.
fn passing_sample(name: &str) -> SampleResult {
    let mut r = SampleResult {
        name: name.to_string(),
        add: passing("add"),
        modified: passing("modify-comment"),
        add_multiline: passing("add-multiline"),
        removed: passing("remove"),
        c8_pass: true,
        c8_detail: "10/10 iterations pass, max increment 900 bytes".to_string(),
        ..Default::default()
    };
    for i in 1..=10 {
        r.loop_scenarios.push(passing(&format!("loop-{i:02}")));
        r.loop_sizes.push(1000 + i as usize);
        r.loop_deltas.push(900);
    }
    r
}

fn cells_of(r: SampleResult) -> Vec<FailCell> {
    collect_fail_cells(&[r])
}

fn triples(cells: &[FailCell]) -> Vec<(String, String, String)> {
    cells
        .iter()
        .map(|c| (c.sample.clone(), c.check.clone(), c.scenario.clone()))
        .collect()
}

fn triple(sample: &str, check: &str, scenario: &str) -> (String, String, String) {
    (sample.to_string(), check.to_string(), scenario.to_string())
}

// ---- collection -----------------------------------------------------------

#[test]
fn a_wholly_passing_sample_produces_no_cells() {
    assert!(cells_of(passing_sample("S2")).is_empty());
}

#[test]
fn each_failing_check_becomes_one_cell_carrying_its_detail() {
    let mut r = passing_sample("S2");
    r.add.add("C6", false, "hl-1: object=false (want present)");
    let cells = cells_of(r);
    assert_eq!(cells.len(), 1);
    assert_eq!(cells[0].sample, "S2");
    assert_eq!(cells[0].check, "C6");
    assert_eq!(cells[0].scenario, "add");
    assert_eq!(cells[0].detail, "hl-1: object=false (want present)");
}

#[test]
fn a_scenario_fatal_becomes_a_fatal_cell_named_for_the_scenario() {
    let mut r = passing_sample("S2");
    r.removed = Scenario::fatal("remove", "remove: engine exited 1");
    let cells = cells_of(r);
    assert_eq!(
        triples(&cells),
        vec![triple("S2", FATAL_CHECK, "remove")],
        "a failed save operation is a fatal, not a check failure"
    );
    assert_eq!(cells[0].detail, "remove: engine exited 1");
}

#[test]
fn deliberately_skipped_scenarios_are_not_failures() {
    let mut r = passing_sample("S3");
    r.add_multiline = Scenario::fatal("add-multiline", "skipped: no text anchor for this sample");
    r.add_multiline_skipped = true;
    r.modified = Scenario::fatal("modify-comment", "skipped: add failed");
    assert!(cells_of(r).is_empty());
}

#[test]
fn a_sample_that_never_reached_a_scenario_is_one_fatal_cell() {
    let mut r = passing_sample("S5");
    r.fatal_err = "baseline load: no such file".to_string();
    let cells = cells_of(r);
    assert_eq!(triples(&cells), vec![triple("S5", FATAL_CHECK, "baseline")]);
}

// ---- C8 -------------------------------------------------------------------

#[test]
fn c8_emits_one_cell_per_failing_iteration() {
    let mut r = passing_sample("S7");
    r.loop_scenarios[0] = scenario("loop-01", &[("C11a", false)]);
    r.c8_pass = false;
    r.c8_detail = "iter 1 has failing checks".to_string();
    let cells = cells_of(r);
    assert_eq!(
        triples(&cells),
        vec![
            triple("S7", "C11a", "loop-01"),
            triple("S7", "C8", "loop-01"),
        ],
        "the iteration's own check and the C8 aggregate are separate cells"
    );
}

#[test]
fn c8_reports_an_oversized_increment_even_when_every_check_passed() {
    let mut r = passing_sample("S4");
    r.loop_deltas[3] = 50 * 1024;
    r.c8_pass = false;
    r.c8_detail = "iter 4 increment 51200 bytes >= 50KB".to_string();
    let cells = cells_of(r);
    assert_eq!(triples(&cells), vec![triple("S4", "C8", "loop-04")]);
    assert!(cells[0].detail.contains("51200"));
}

#[test]
fn an_incomplete_loop_is_a_fatal_cell_not_a_c8_cell() {
    // A loop that stops early means the save operation failed. Registering
    // `S*/C8/loop-*` must not excuse a broken engine, so the cell is fatal.
    let mut r = passing_sample("S1");
    r.loop_scenarios.truncate(3);
    r.loop_sizes.truncate(3);
    r.loop_deltas.truncate(3);
    r.c8_pass = false;
    r.c8_detail = "iter 4 loopOne: exit status 1; only 3/10 iterations completed".to_string();
    let cells = cells_of(r);
    assert_eq!(triples(&cells), vec![triple("S1", FATAL_CHECK, "loop")]);
    assert!(cells[0].detail.contains("3/10"));
}

#[test]
fn a_c8_failure_with_no_reconstructable_reason_still_produces_a_cell() {
    // The gate must never report success for a run the matrix marked FAIL.
    let mut r = passing_sample("S5");
    r.c8_pass = false;
    r.c8_detail = "something the walk cannot attribute".to_string();
    let cells = cells_of(r);
    assert_eq!(triples(&cells), vec![triple("S5", "C8", "loop")]);
    assert_eq!(cells[0].detail, "something the walk cannot attribute");
}

// ---- encrypted ------------------------------------------------------------

fn refused_sample(name: &str, got: &str, bytes_equal: bool) -> SampleResult {
    SampleResult {
        name: name.to_string(),
        enc_mode: EncMode::RefusedUnchanged,
        enc_want_status: "encrypted_refused".to_string(),
        enc_got_status: got.to_string(),
        enc_bytes_equal: bytes_equal,
        enc_refused_note: format!("status={got} (want encrypted_refused)"),
        add: Scenario::fatal("add", "skipped: encrypted-refused expected"),
        modified: Scenario::fatal("modify-comment", "skipped: encrypted-refused expected"),
        modified_skipped: true,
        add_multiline: Scenario::fatal("add-multiline", "skipped: encrypted-refused expected"),
        add_multiline_skipped: true,
        removed: Scenario::fatal("remove", "skipped: encrypted-refused expected"),
        c8_pass: true,
        c8_detail: "skipped: encrypted-refused expected".to_string(),
        ..Default::default()
    }
}

#[test]
fn a_refused_fixture_that_holds_the_contract_produces_no_cells() {
    let r = refused_sample("S11-password-required", "encrypted_refused", true);
    assert!(cells_of(r).is_empty(), "an empty loop is expected here");
}

#[test]
fn a_broken_refusal_contract_becomes_an_encrypted_cell() {
    let wrong_status = refused_sample("S11-password-required", "parse_failed", true);
    assert_eq!(
        triples(&cells_of(wrong_status)),
        vec![triple("S11-password-required", "Encrypted", "refused")]
    );

    let mutated = refused_sample("S12-annotations-restricted", "encrypted_refused", false);
    assert_eq!(
        triples(&cells_of(mutated)),
        vec![triple("S12-annotations-restricted", "Encrypted", "refused")]
    );
}

// ---- exception matching ---------------------------------------------------

const TEST_LIST: &str = r#"{
  "version": 1,
  "exceptions": [
    {"id": "everything", "samples": ["*"], "checks": ["*"], "reason": "test wildcard"}
  ]
}"#;

#[test]
fn a_matching_entry_moves_a_cell_to_known_and_names_the_entry() {
    let list = KnownExceptions::parse(TEST_LIST).unwrap();
    let cells = vec![FailCell::new("S1", "C11a", "add", "mismatch")];
    let out = apply_exceptions(&cells, &list);
    assert!(out.passed());
    assert_eq!(out.known.len(), 1);
    assert_eq!(out.known[0].exception_id, "everything");
    assert!(out.unknown.is_empty());
}

#[test]
fn a_fatal_cell_is_unknown_even_under_a_wildcard_entry() {
    let list = KnownExceptions::parse(TEST_LIST).unwrap();
    let cells = vec![FailCell::new("S1", FATAL_CHECK, "loop", "engine exited 1")];
    let out = apply_exceptions(&cells, &list);
    assert!(!out.passed());
    assert!(out.known.is_empty());
    assert_eq!(out.unknown, cells);
}

#[test]
fn an_unmatched_cell_is_unknown() {
    let list = KnownExceptions::parse(
        r#"{"version":1,"exceptions":[
          {"id":"only-s1","samples":["S1"],"checks":["C11a"],"reason":"r"}]}"#,
    )
    .unwrap();
    let cells = vec![
        FailCell::new("S1", "C11a", "add", "d"),
        FailCell::new("S2", "C11a", "add", "d"),
    ];
    let out = apply_exceptions(&cells, &list);
    assert_eq!(out.known.len(), 1);
    assert_eq!(out.unknown, vec![FailCell::new("S2", "C11a", "add", "d")]);
}

// ---- the committed registry against the measured distribution -------------

fn committed_list() -> KnownExceptions {
    let path = format!(
        "{}/../../corpus/known-exceptions.json",
        env!("CARGO_MANIFEST_DIR")
    );
    KnownExceptions::load(&path).unwrap_or_else(|e| panic!("loading {path}: {e}"))
}

/// A sample whose anchor text is vertical: C11a fails in `add` and in every
/// loop iteration, so C8 aggregates as failing too.
fn vertical_text_sample(name: &str) -> SampleResult {
    let mut r = passing_sample(name);
    r.add = scenario("add", &[("C1", true), ("C11a", false)]);
    for (i, sc) in r.loop_scenarios.iter_mut().enumerate() {
        *sc = scenario(&format!("loop-{:02}", i + 1), &[("C11a", false)]);
    }
    r.c8_pass = false;
    r.c8_detail = "iter 1 has failing checks".to_string();
    r
}

/// The distribution measured on this corpus: four vertical-text samples,
/// S7 drifting in its first loop iteration only, everything else passing.
fn measured_results() -> Vec<SampleResult> {
    let mut results: Vec<SampleResult> = ["S1", "S8", "S9-rc4-empty-user", "S10-aes256-empty-user"]
        .iter()
        .map(|n| vertical_text_sample(n))
        .collect();

    let mut s7 = passing_sample("S7");
    s7.loop_scenarios[0] = scenario("loop-01", &[("C11a", false)]);
    s7.c8_pass = false;
    s7.c8_detail = "iter 1 has failing checks".to_string();
    results.push(s7);

    for n in ["S2", "S3", "S4", "S5"] {
        results.push(passing_sample(n));
    }
    results.push(refused_sample(
        "S11-password-required",
        "encrypted_refused",
        true,
    ));
    results.push(refused_sample(
        "S12-annotations-restricted",
        "encrypted_refused",
        true,
    ));
    results
}

#[test]
fn the_committed_registry_accepts_the_measured_distribution() {
    let cells = collect_fail_cells(&measured_results());
    assert!(!cells.is_empty(), "the distribution has failing cells");
    let out = apply_exceptions(&cells, &committed_list());
    assert!(
        out.passed(),
        "known failures must not fail the run; unknown: {:?}",
        out.unknown
    );
    assert_eq!(out.known.len(), cells.len());
}

#[test]
fn one_failure_outside_the_measured_distribution_fails_the_run() {
    let mut results = measured_results();
    let s2 = results
        .iter_mut()
        .find(|r| r.name == "S2")
        .expect("S2 is in the distribution");
    s2.add.add("C1", false, "prefix diverges at byte 12");

    let cells = collect_fail_cells(&results);
    let out = apply_exceptions(&cells, &committed_list());
    assert!(!out.passed());
    assert_eq!(triples(&out.unknown), vec![triple("S2", "C1", "add")]);
}

// ---- machine-readable output and rendering --------------------------------

#[test]
fn fail_cells_json_round_trips() {
    let cells = vec![FailCell::new("S1", "C11a", "add", "mismatch")];
    let doc = FailCells::new(cells.clone());
    assert_eq!(doc.version, 1);
    let json = doc.to_json();
    let back = FailCells::parse(&json).unwrap();
    assert_eq!(back.cells, cells);
}

#[test]
fn fail_cells_rejects_an_unsupported_version() {
    let err = FailCells::parse(r#"{"version":99,"cells":[]}"#).unwrap_err();
    assert!(err.contains("version"), "unexpected error: {err}");
}

#[test]
fn cell_details_are_sanitized_before_they_are_shortened() {
    let long_path = format!("/private/workdir-{}", "x".repeat(420));
    let cells = vec![FailCell::new(
        "S1",
        "C6",
        "add",
        format!("qpdf warning:\n{long_path}/S1/add.pdf oddity"),
    )];
    let mut san = Sanitizer::new();
    san.add_path(&long_path, "work");
    san.finalize();
    let out = sanitize_cells(&cells, &san);
    assert!(!out[0].detail.contains("/private/workdir"));
    assert!(out[0].detail.contains("work/S1/add.pdf oddity"));
    assert!(!out[0].detail.contains('\n'));
}

#[test]
fn the_gate_section_summarizes_known_entries_and_lists_unknown_cells() {
    let cells = collect_fail_cells(&measured_results());
    let md = render_gate_section(&apply_exceptions(&cells, &committed_list()));
    assert!(md.starts_with("## Known-exception gate"));
    assert!(md.contains("vertical-text-bbox-measurement"));
    assert!(md.contains("font-metric-drift-in-loop"));
    assert!(md.contains("(none)"), "no unknown cells to list");

    let unknown = GateOutcome {
        known: Vec::new(),
        unknown: vec![FailCell::new("S2", "C1", "add", "prefix diverges")],
    };
    let md2 = render_gate_section(&unknown);
    assert!(md2.contains("| S2 | C1 | add | prefix diverges |"));
}
