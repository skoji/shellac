//! The known-exception gate: reducing a matrix run to a verdict.
//!
//! The matrix records what happened; the gate decides whether it is
//! acceptable. Keeping the two apart means a verdict can be re-derived — and
//! tested — without repeating a corpus run, and that tolerating an outcome
//! stays a data question (`corpus/known-exceptions.json`) rather than a
//! property of the run that produced it.

use serde::{Deserialize, Serialize};

use crate::consts::C8_MAX_DELTA;
use crate::encrypted::EncMode;
use crate::exceptions::KnownExceptions;
use crate::sample::SampleResult;
use crate::sanitize::Sanitizer;
use crate::scenario::Scenario;
use crate::util::trunc;

/// Check id for a failure that is not a check verdict: a save operation, a
/// baseline load, or a loop that stopped early.
pub const FATAL_CHECK: &str = "fatal";

/// Scenario id used when a sample failed before any scenario ran.
pub const BASELINE_SCENARIO: &str = "baseline";

/// Scenario id used when the repeated-save loop did not complete.
pub const LOOP_SCENARIO: &str = "loop";

/// Scenario id used for the refusal contract of a refused-mode fixture.
pub const REFUSED_SCENARIO: &str = "refused";

/// Schema version of the `--fails-out` document.
pub const FAILS_SCHEMA_VERSION: u32 = 1;

/// Fatal text prefix marking a scenario that was deliberately not run.
const SKIPPED_PREFIX: &str = "skipped:";

/// Longest cell detail kept in the machine-readable output.
const DETAIL_MAX: usize = 400;

/// One failing (sample, check, scenario) triple with the detail the matrix
/// recorded for it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailCell {
    pub sample: String,
    pub check: String,
    pub scenario: String,
    #[serde(default)]
    pub detail: String,
}

impl FailCell {
    pub fn new(sample: &str, check: &str, scenario: &str, detail: impl Into<String>) -> Self {
        FailCell {
            sample: sample.to_string(),
            check: check.to_string(),
            scenario: scenario.to_string(),
            detail: detail.into(),
        }
    }
}

/// The machine-readable failing-cell document exchanged between `matrix`
/// and `gate`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailCells {
    pub version: u32,
    pub cells: Vec<FailCell>,
}

impl FailCells {
    pub fn new(cells: Vec<FailCell>) -> Self {
        FailCells {
            version: FAILS_SCHEMA_VERSION,
            cells,
        }
    }

    pub fn to_json(&self) -> String {
        // The document is committed to CI logs and artifacts, so it is
        // written in a form a human can read in a diff.
        serde_json::to_string_pretty(self).unwrap_or_else(|e| panic!("encoding fail cells: {e}"))
    }

    pub fn parse(json: &str) -> Result<Self, String> {
        let doc: FailCells =
            serde_json::from_str(json).map_err(|e| format!("parsing fail cells: {e}"))?;
        if doc.version != FAILS_SCHEMA_VERSION {
            return Err(format!(
                "unsupported fail-cell schema version {} (this build understands {})",
                doc.version, FAILS_SCHEMA_VERSION
            ));
        }
        Ok(doc)
    }

    pub fn load(path: &str) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("reading {path}: {e}"))?;
        Self::parse(&text)
    }

    pub fn write(&self, path: &str) -> Result<(), String> {
        std::fs::write(path, format!("{}\n", self.to_json()))
            .map_err(|e| format!("writing {path}: {e}"))
    }
}

/// A failing cell together with the registry entry that excuses it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownFail {
    pub cell: FailCell,
    pub exception_id: String,
}

/// The gate's verdict over one run's failing cells.
#[derive(Clone, Debug, Default)]
pub struct GateOutcome {
    pub known: Vec<KnownFail>,
    pub unknown: Vec<FailCell>,
}

impl GateOutcome {
    pub fn passed(&self) -> bool {
        self.unknown.is_empty()
    }
}

/// Collects every failing cell of a matrix run.
pub fn collect_fail_cells(results: &[SampleResult]) -> Vec<FailCell> {
    let mut cells = Vec::new();
    for r in results {
        collect_sample(r, &mut cells);
    }
    cells
}

fn collect_sample(r: &SampleResult, out: &mut Vec<FailCell>) {
    if !r.fatal_err.is_empty() {
        out.push(FailCell::new(
            &r.name,
            FATAL_CHECK,
            BASELINE_SCENARIO,
            r.fatal_err.clone(),
        ));
        return;
    }
    let mut scenarios: Vec<&Scenario> = vec![&r.add, &r.modified, &r.add_multiline, &r.removed];
    scenarios.extend(r.loop_scenarios.iter());
    for sc in scenarios {
        collect_scenario(&r.name, sc, out);
    }
    collect_c8(r, out);
    collect_refusal_contract(r, out);
}

fn collect_scenario(sample: &str, sc: &Scenario, out: &mut Vec<FailCell>) {
    if !sc.fatal.is_empty() {
        if !sc.fatal.starts_with(SKIPPED_PREFIX) {
            out.push(FailCell::new(
                sample,
                FATAL_CHECK,
                &sc.name,
                sc.fatal.clone(),
            ));
        }
        return;
    }
    for c in &sc.checks {
        if !c.pass {
            out.push(FailCell::new(sample, &c.id, &sc.name, c.detail.clone()));
        }
    }
}

/// C8 is an aggregate over the loop, not a per-scenario check, so its cells
/// are reconstructed from the same facts `run_sample` aggregated. The walk
/// mirrors that computation; if it ever fails to account for a C8 failure,
/// the fallback below keeps the gate from passing a run the matrix failed.
fn collect_c8(r: &SampleResult, out: &mut Vec<FailCell>) {
    if r.enc_mode == EncMode::RefusedUnchanged || r.c8_pass {
        return;
    }
    let before = out.len();
    for (i, sc) in r.loop_scenarios.iter().enumerate() {
        if !sc.all_pass() {
            out.push(FailCell::new(
                &r.name,
                "C8",
                &sc.name,
                "loop iteration has failing checks",
            ));
        }
        let delta = r.loop_deltas.get(i).copied().unwrap_or(0);
        if delta >= C8_MAX_DELTA {
            out.push(FailCell::new(
                &r.name,
                "C8",
                &sc.name,
                format!("increment {delta} bytes >= {C8_MAX_DELTA}"),
            ));
        }
    }
    if r.loop_scenarios.len() < 10 {
        // A loop that stopped early means a save operation failed, which no
        // `loop-*` entry should be able to excuse.
        out.push(FailCell::new(
            &r.name,
            FATAL_CHECK,
            LOOP_SCENARIO,
            format!("only {}/10 iterations completed", r.loop_scenarios.len()),
        ));
    }
    if out.len() == before {
        out.push(FailCell::new(
            &r.name,
            "C8",
            LOOP_SCENARIO,
            r.c8_detail.clone(),
        ));
    }
}

fn collect_refusal_contract(r: &SampleResult, out: &mut Vec<FailCell>) {
    if r.enc_mode != EncMode::RefusedUnchanged {
        return;
    }
    if r.enc_got_status == r.enc_want_status && r.enc_bytes_equal {
        return;
    }
    out.push(FailCell::new(
        &r.name,
        "Encrypted",
        REFUSED_SCENARIO,
        r.enc_refused_note.clone(),
    ));
}

/// Splits failing cells into the ones the registry accepts and the ones it
/// does not. Fatal cells bypass the registry: an engine that failed to save
/// is a defect whatever the list says.
pub fn apply_exceptions(cells: &[FailCell], list: &KnownExceptions) -> GateOutcome {
    let mut outcome = GateOutcome::default();
    for cell in cells {
        let excused = if cell.check == FATAL_CHECK {
            None
        } else {
            list.find(&cell.sample, &cell.check, &cell.scenario)
        };
        match excused {
            Some(e) => outcome.known.push(KnownFail {
                cell: cell.clone(),
                exception_id: e.id.clone(),
            }),
            None => outcome.unknown.push(cell.clone()),
        }
    }
    outcome
}

/// Rewrites machine-specific strings out of cell details and shortens them
/// for the machine-readable output. Sanitization runs before truncation so
/// a path cut in half cannot escape replacement.
pub fn sanitize_cells(cells: &[FailCell], san: &Sanitizer) -> Vec<FailCell> {
    cells
        .iter()
        .map(|c| FailCell {
            detail: trunc(&san.apply(&c.detail), DETAIL_MAX).replace('\n', " "),
            ..c.clone()
        })
        .collect()
}

/// Renders the report's gate section: a per-entry summary of what the
/// registry accepted, and the full list of what it did not.
pub fn render_gate_section(outcome: &GateOutcome) -> String {
    let mut sb = String::from("## Known-exception gate\n\n");
    sb.push_str(
        "Every failing cell of this run, matched against `corpus/known-exceptions.json`. \
A known cell is one the registry accepts, for the reason recorded there; an UNKNOWN cell is \
what makes the run fail. Failures of a save operation itself are never matched against the \
registry.\n\n",
    );
    sb.push_str(&format!(
        "- failing cells: {} (known {}, unknown {})\n\n",
        outcome.known.len() + outcome.unknown.len(),
        outcome.known.len(),
        outcome.unknown.len()
    ));

    sb.push_str("### Known\n\n");
    let groups = group_known(outcome);
    if groups.is_empty() {
        sb.push_str("(none)\n\n");
    } else {
        sb.push_str("| Exception | Cells | Samples |\n|---|---|---|\n");
        for (id, count, samples) in groups {
            sb.push_str(&format!(
                "| {} | {} | {} |\n",
                id,
                count,
                samples.join(", ")
            ));
        }
        sb.push('\n');
    }

    sb.push_str("### Unknown\n\n");
    if outcome.unknown.is_empty() {
        sb.push_str("(none)\n");
    } else {
        sb.push_str("| Sample | Check | Scenario | Detail |\n|---|---|---|---|\n");
        for c in &outcome.unknown {
            sb.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                c.sample,
                c.check,
                c.scenario,
                c.detail.replace('\n', " ")
            ));
        }
    }
    sb
}

/// Known cells grouped by entry id, in first-appearance order, each with
/// its cell count and the samples it covered.
fn group_known(outcome: &GateOutcome) -> Vec<(String, usize, Vec<String>)> {
    let mut groups: Vec<(String, usize, Vec<String>)> = Vec::new();
    for k in &outcome.known {
        match groups.iter_mut().find(|(id, _, _)| *id == k.exception_id) {
            Some((_, count, samples)) => {
                *count += 1;
                if !samples.contains(&k.cell.sample) {
                    samples.push(k.cell.sample.clone());
                }
            }
            None => groups.push((k.exception_id.clone(), 1, vec![k.cell.sample.clone()])),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_groups_keep_first_appearance_order_and_deduplicate_samples() {
        let outcome = GateOutcome {
            known: vec![
                KnownFail {
                    cell: FailCell::new("S8", "C11a", "add", ""),
                    exception_id: "b".to_string(),
                },
                KnownFail {
                    cell: FailCell::new("S1", "C11a", "add", ""),
                    exception_id: "a".to_string(),
                },
                KnownFail {
                    cell: FailCell::new("S8", "C8", "loop-01", ""),
                    exception_id: "b".to_string(),
                },
            ],
            unknown: Vec::new(),
        };
        assert_eq!(
            group_known(&outcome),
            vec![
                ("b".to_string(), 2, vec!["S8".to_string()]),
                ("a".to_string(), 1, vec!["S1".to_string()]),
            ]
        );
    }
}
