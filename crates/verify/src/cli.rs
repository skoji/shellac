//! Hand-rolled CLI parsing for the `matrix` subcommand.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MatrixOpts {
    pub samples: String,
    pub work: String,
    pub scripts: String,
    pub bin: String,
    pub out: String,
    pub engine_cmd: String,
    pub nm_prefix: String,
    pub redact_nm_prefix: bool,
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Matrix(MatrixOpts),
}

pub const USAGE: &str = "usage: verify matrix --samples <dir> --work <dir> --scripts <dir> --bin <dir> \
--out <path> --engine-cmd <path> --nm-prefix <str> [--redact-nm-prefix]
  --samples          directory containing sample PDFs (*.pdf)
  --work             working directory (PDF copies land here)
  --scripts          directory with the PDFKit helper Swift sources
  --bin              directory for compiled helper binaries
  --out              output markdown path
  --engine-cmd       path to the save-engine CLI under test
  --nm-prefix        annotation id (/NM) prefix the engine emits
  --redact-nm-prefix replace the nm prefix with <nm> in the report";

/// Parses command-line arguments (without the program name). Returns a
/// usage error string for unknown/missing input; the caller exits 2.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut it = args.iter();
    match it.next().map(|s| s.as_str()) {
        Some("matrix") => {}
        Some(other) => return Err(format!("verify: unknown subcommand {other:?}\n{USAGE}")),
        None => return Err(USAGE.to_string()),
    }
    let mut opts = MatrixOpts::default();
    while let Some(arg) = it.next() {
        let (flag, inline_value) = match arg.split_once('=') {
            Some((f, v)) => (f, Some(v.to_string())),
            None => (arg.as_str(), None),
        };
        if flag == "--redact-nm-prefix" {
            if inline_value.is_some() {
                return Err(format!("verify matrix: {flag} takes no value\n{USAGE}"));
            }
            opts.redact_nm_prefix = true;
            continue;
        }
        let slot = match flag {
            "--samples" => &mut opts.samples,
            "--work" => &mut opts.work,
            "--scripts" => &mut opts.scripts,
            "--bin" => &mut opts.bin,
            "--out" => &mut opts.out,
            "--engine-cmd" => &mut opts.engine_cmd,
            "--nm-prefix" => &mut opts.nm_prefix,
            _ => return Err(format!("verify matrix: unknown flag {flag:?}\n{USAGE}")),
        };
        let value = match inline_value {
            Some(v) => v,
            None => match it.next() {
                Some(v) => v.clone(),
                None => {
                    return Err(format!("verify matrix: {flag} requires a value\n{USAGE}"));
                }
            },
        };
        *slot = value;
    }
    let required = [
        ("--samples", &opts.samples),
        ("--work", &opts.work),
        ("--scripts", &opts.scripts),
        ("--bin", &opts.bin),
        ("--out", &opts.out),
        ("--engine-cmd", &opts.engine_cmd),
        ("--nm-prefix", &opts.nm_prefix),
    ];
    let missing: Vec<&str> = required
        .iter()
        .filter(|(_, v)| v.is_empty())
        .map(|(f, _)| *f)
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "verify matrix: missing required flag(s): {}\n{USAGE}",
            missing.join(", ")
        ));
    }
    Ok(Command::Matrix(opts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    fn full_args() -> Vec<String> {
        argv(&[
            "matrix",
            "--samples",
            "samples",
            "--work",
            "work",
            "--scripts",
            "scripts",
            "--bin",
            "bin",
            "--out",
            "out.md",
            "--engine-cmd",
            "/x/engine",
            "--nm-prefix",
            "test-verify",
        ])
    }

    #[test]
    fn parses_all_flags() {
        let Command::Matrix(o) = parse(&full_args()).unwrap();
        assert_eq!(o.samples, "samples");
        assert_eq!(o.engine_cmd, "/x/engine");
        assert_eq!(o.nm_prefix, "test-verify");
        assert!(!o.redact_nm_prefix);
    }

    #[test]
    fn parses_redact_flag_and_equals_form() {
        let mut a = full_args();
        a.push("--redact-nm-prefix".to_string());
        let Command::Matrix(o) = parse(&a).unwrap();
        assert!(o.redact_nm_prefix);

        let b = argv(&[
            "matrix",
            "--samples=s",
            "--work=w",
            "--scripts=sc",
            "--bin=b",
            "--out=o",
            "--engine-cmd=e",
            "--nm-prefix=p",
        ]);
        let Command::Matrix(o2) = parse(&b).unwrap();
        assert_eq!(o2.scripts, "sc");
    }

    #[test]
    fn missing_required_flag_is_usage_error() {
        let a = argv(&["matrix", "--samples", "s"]);
        let err = parse(&a).unwrap_err();
        assert!(err.contains("missing required flag(s)"));
        assert!(err.contains("--work"));
    }

    #[test]
    fn unknown_flag_and_subcommand_are_usage_errors() {
        let mut a = full_args();
        a.push("--bogus".to_string());
        assert!(parse(&a).unwrap_err().contains("unknown flag"));
        assert!(parse(&argv(&["frobnicate"]))
            .unwrap_err()
            .contains("unknown subcommand"));
        assert!(parse(&[]).unwrap_err().contains("usage:"));
    }

    #[test]
    fn flag_without_value_is_usage_error() {
        let a = argv(&["matrix", "--samples"]);
        assert!(parse(&a).unwrap_err().contains("requires a value"));
    }
}
