//! The known-exception gate: reducing a matrix run to a verdict.
//!
//! The matrix records what happened; the gate decides whether it is
//! acceptable. Keeping the two apart means a verdict can be re-derived — and
//! tested — without repeating a corpus run, and that tolerating an outcome
//! stays a data question (`corpus/known-exceptions.json`) rather than a
//! property of the run that produced it.

use serde::{Deserialize, Serialize};

use crate::exceptions::KnownExceptions;
use crate::sample::SampleResult;
use crate::sanitize::Sanitizer;

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
    pub fn new(_cells: Vec<FailCell>) -> Self {
        todo!()
    }

    pub fn to_json(&self) -> String {
        todo!()
    }

    pub fn parse(_json: &str) -> Result<Self, String> {
        todo!()
    }

    pub fn load(_path: &str) -> Result<Self, String> {
        todo!()
    }

    pub fn write(&self, _path: &str) -> Result<(), String> {
        todo!()
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
pub fn collect_fail_cells(_results: &[SampleResult]) -> Vec<FailCell> {
    todo!()
}

/// Splits failing cells into the ones the registry accepts and the ones it
/// does not.
pub fn apply_exceptions(_cells: &[FailCell], _list: &KnownExceptions) -> GateOutcome {
    todo!()
}

/// Rewrites machine-specific strings out of cell details and shortens them
/// for the machine-readable output.
pub fn sanitize_cells(_cells: &[FailCell], _san: &Sanitizer) -> Vec<FailCell> {
    todo!()
}

/// Renders the report's gate section.
pub fn render_gate_section(_outcome: &GateOutcome) -> String {
    todo!()
}
