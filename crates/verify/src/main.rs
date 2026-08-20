use std::process::ExitCode;

use verify::{cli, gate, matrix};

/// Exit status for a run whose matrix produced failures the known-exception
/// list does not cover. Distinct from 1 so CI can tell a verification
/// failure from a broken setup.
const EXIT_UNKNOWN_FAILURES: u8 = 3;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&args) {
        Ok(cli::Command::Matrix(opts)) => match matrix::run_matrix(&opts) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("verify matrix: {e}");
                ExitCode::FAILURE
            }
        },
        Ok(cli::Command::Gate(opts)) => match gate::run_gate(&opts) {
            Ok(outcome) => {
                print!("{}", gate::render_gate_section(&outcome));
                if outcome.passed() {
                    ExitCode::SUCCESS
                } else {
                    eprintln!(
                        "verify gate: {} failing cell(s) are not covered by the known-exception list",
                        outcome.unknown.len()
                    );
                    ExitCode::from(EXIT_UNKNOWN_FAILURES)
                }
            }
            Err(e) => {
                eprintln!("verify gate: {e}");
                ExitCode::FAILURE
            }
        },
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::from(2)
        }
    }
}
