use std::process::ExitCode;

use verify::{cli, matrix};

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
        Ok(cli::Command::Gate(_opts)) => todo!(),
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::from(2)
        }
    }
}
