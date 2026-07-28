//! Known-exception list: schema parsing and lookup, plus a compatibility
//! test that the committed `corpus/known-exceptions.json` still loads.

use verify::exceptions::KnownExceptions;

fn committed_list() -> KnownExceptions {
    let path = format!(
        "{}/../../corpus/known-exceptions.json",
        env!("CARGO_MANIFEST_DIR")
    );
    KnownExceptions::load(&path).unwrap_or_else(|e| panic!("loading {path}: {e}"))
}

const SAMPLE_JSON: &str = r#"{
  "version": 1,
  "description": "test list",
  "exceptions": [
    {
      "id": "all-scenarios",
      "samples": ["S1", "S8"],
      "checks": ["C11a", "C8"],
      "reason": "scenarios omitted means every scenario",
      "recorded_measurements": "none"
    },
    {
      "id": "loop-only",
      "samples": ["S7"],
      "checks": ["C8"],
      "scenarios": ["loop-*"],
      "reason": "loop scenarios only"
    }
  ]
}"#;

#[test]
fn parses_all_documented_fields() {
    let list = KnownExceptions::parse(SAMPLE_JSON).unwrap();
    assert_eq!(list.version, 1);
    assert_eq!(list.exceptions.len(), 2);

    let first = &list.exceptions[0];
    assert_eq!(first.id, "all-scenarios");
    assert_eq!(first.samples, vec!["S1", "S8"]);
    assert_eq!(first.checks, vec!["C11a", "C8"]);
    assert_eq!(first.scenarios, None);
    assert_eq!(first.reason, "scenarios omitted means every scenario");
    assert_eq!(first.recorded_measurements, "none");

    let second = &list.exceptions[1];
    assert_eq!(
        second.scenarios.as_deref(),
        Some(["loop-*".to_string()].as_slice())
    );
    // recorded_measurements is optional and defaults to empty.
    assert_eq!(second.recorded_measurements, "");
}

#[test]
fn omitted_scenarios_match_every_scenario() {
    let list = KnownExceptions::parse(SAMPLE_JSON).unwrap();
    for scenario in ["add", "modify-comment", "remove", "loop-01"] {
        let found = list.find("S1", "C11a", scenario).unwrap();
        assert_eq!(found.id, "all-scenarios");
    }
}

#[test]
fn scenario_wildcard_matches_only_the_named_family() {
    let list = KnownExceptions::parse(SAMPLE_JSON).unwrap();
    for scenario in ["loop-01", "loop-10"] {
        assert_eq!(list.find("S7", "C8", scenario).unwrap().id, "loop-only");
    }
    assert!(list.find("S7", "C8", "add").is_none());
    assert!(list.find("S7", "C8", "modify-comment").is_none());
}

#[test]
fn unlisted_sample_check_or_scenario_is_not_an_exception() {
    let list = KnownExceptions::parse(SAMPLE_JSON).unwrap();
    assert!(list.find("S2", "C11a", "add").is_none());
    assert!(list.find("S1", "C6", "add").is_none());
    assert!(list.find("S7", "C11a", "loop-01").is_none());
    assert!(!list.is_known("S2", "C11a", "add"));
    assert!(list.is_known("S1", "C8", "loop-01"));
}

#[test]
fn rejects_unknown_fields_and_unsupported_versions() {
    let unknown_field = r#"{"version":1,"exceptions":[
      {"id":"x","samples":["S1"],"checks":["C8"],"reason":"r","surprise":true}]}"#;
    assert!(KnownExceptions::parse(unknown_field).is_err());

    let future_version = r#"{"version":2,"exceptions":[]}"#;
    let err = KnownExceptions::parse(future_version).unwrap_err();
    assert!(err.contains("version"), "unexpected error: {err}");
}

#[test]
fn rejects_entries_missing_required_content() {
    let empty_samples = r#"{"version":1,"exceptions":[
      {"id":"x","samples":[],"checks":["C8"],"reason":"r"}]}"#;
    assert!(KnownExceptions::parse(empty_samples).is_err());

    let empty_reason = r#"{"version":1,"exceptions":[
      {"id":"x","samples":["S1"],"checks":["C8"],"reason":"  "}]}"#;
    assert!(KnownExceptions::parse(empty_reason).is_err());

    let duplicate_ids = r#"{"version":1,"exceptions":[
      {"id":"x","samples":["S1"],"checks":["C8"],"reason":"r"},
      {"id":"x","samples":["S2"],"checks":["C8"],"reason":"r"}]}"#;
    assert!(KnownExceptions::parse(duplicate_ids).is_err());
}

#[test]
fn committed_list_matches_the_current_schema() {
    let list = committed_list();
    assert_eq!(list.version, 1);
    assert_eq!(list.exceptions.len(), 2);
    for e in &list.exceptions {
        assert!(!e.reason.trim().is_empty(), "{} has no reason", e.id);
        assert!(
            !e.recorded_measurements.trim().is_empty(),
            "{} has no recorded measurements",
            e.id
        );
    }
}

#[test]
fn committed_list_covers_the_registered_cases() {
    let list = committed_list();

    // Vertical-text measurement difference: every scenario of the
    // vertical-text samples.
    for sample in ["S1", "S8", "S9-rc4-empty-user", "S10-aes256-empty-user"] {
        assert!(list.is_known(sample, "C11a", "add"), "{sample} C11a add");
        assert!(
            list.is_known(sample, "C8", "loop-01"),
            "{sample} C8 loop-01"
        );
    }
    assert!(!list.is_known("S2", "C11a", "add"));
    assert!(!list.is_known("S1", "C6", "add"));

    // Font-metric drift: loop scenarios only.
    assert!(list.is_known("S7", "C11a", "loop-01"));
    assert!(list.is_known("S7", "C8", "loop-10"));
    assert!(!list.is_known("S7", "C11a", "add"));
}
