//! C7 (PDFKit reload) and C11b (PDFKit-side position re-verification),
//! backed by the pdfkit_check helper.

use serde::Deserialize;

use crate::consts::{SKIP_C11B, SKIP_C11B_UNEVALUATED};
use crate::proc::run;
use crate::util::{bracket_list, contains_all, contains_any, dedup};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PdfkitCheck {
    pub loaded: bool,
    pub page_count: i64,
    #[serde(rename = "foundNM")]
    pub found_nm: Vec<String>,
    pub annot_types: Vec<String>,
    #[serde(rename = "thumbnailOK")]
    pub thumbnail_ok: bool,
    pub c11b_checked: bool,
    pub c11b_pass: bool,
    pub c11b_detail: String,
}

/// Runs the pdfkit_check helper. `needle`/`anchor_nm` are the C11b
/// arguments; pass both non-empty to have the helper re-locate the needle
/// and compare against the named annotation's bounds. Returns the parsed
/// JSON plus the raw stdout (quoted in failure logs).
pub fn pdfkit_check(
    bin: &str,
    path: &str,
    expected_pages: i64,
    needle: &str,
    anchor_nm: &str,
) -> (Result<PdfkitCheck, String>, String) {
    let mut args = vec![
        path.to_string(),
        format!("{expected_pages}"),
        "-".to_string(),
    ];
    if !needle.is_empty() && !anchor_nm.is_empty() {
        args.push(needle.to_string());
        args.push(anchor_nm.to_string());
    }
    let r = run(bin, &args);
    let raw = r.stdout_str();
    if let Some(e) = &r.err {
        return (
            Err(format!("pdfkit_check: {} ({})", e, r.stderr_str().trim())),
            raw,
        );
    }
    match serde_json::from_slice::<PdfkitCheck>(&r.stdout) {
        Ok(c) => (Ok(c), raw),
        Err(e) => (Err(format!("pdfkit_check json: {e}")), raw),
    }
}

pub struct C7Outcome {
    pub pass: bool,
    pub detail: String,
    pub fail_log: Option<String>,
}

/// Evaluates C7 from a successful pdfkit_check run.
pub fn evaluate_c7(
    ck: &PdfkitCheck,
    raw: &str,
    pages: i64,
    expect_present: &[String],
    expect_absent: &[String],
) -> C7Outcome {
    let missing = contains_all(&ck.found_nm, expect_present);
    let unexpected = contains_any(&ck.found_nm, expect_absent);
    let pass = ck.loaded
        && ck.page_count == pages
        && missing.is_empty()
        && unexpected.is_empty()
        && ck.thumbnail_ok;
    let detail = format!(
        "loaded={} pages={}/{} missingNM={} unexpectedNM={} thumbnails={} annotTypes={}",
        ck.loaded,
        ck.page_count,
        pages,
        bracket_list(&missing),
        bracket_list(&unexpected),
        ck.thumbnail_ok,
        bracket_list(&dedup(&ck.annot_types))
    );
    let fail_log = if pass {
        None
    } else {
        Some(format!(
            "C7: {}\nraw json:\n{}",
            detail,
            crate::util::trunc(raw, 2000)
        ))
    };
    C7Outcome {
        pass,
        detail,
        fail_log,
    }
}

pub struct C11bOutcome {
    pub pass: bool,
    pub detail: String,
    pub fail_log: Option<String>,
}

/// Evaluates C11b. `requested` is false when the scenario carried no
/// C11b-eligible id (skip). `ck` is the pdfkit_check result (`Err` when the
/// helper failed).
pub fn evaluate_c11b(requested: bool, ck: Result<&PdfkitCheck, &str>) -> C11bOutcome {
    if !requested {
        return C11bOutcome {
            pass: true,
            detail: SKIP_C11B.to_string(),
            fail_log: None,
        };
    }
    match ck {
        Err(cerr) => C11bOutcome {
            pass: false,
            detail: format!("pdfkit_check failed: {cerr}"),
            fail_log: None,
        },
        Ok(c) if !c.c11b_checked => C11bOutcome {
            pass: true,
            detail: SKIP_C11B_UNEVALUATED.to_string(),
            fail_log: None,
        },
        Ok(c) => C11bOutcome {
            pass: c.c11b_pass,
            detail: c.c11b_detail.clone(),
            fail_log: if c.c11b_pass {
                None
            } else {
                Some(format!("C11b: {}", c.c11b_detail))
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ck(found: &[&str], pages: i64) -> PdfkitCheck {
        PdfkitCheck {
            loaded: true,
            page_count: pages,
            found_nm: found.iter().map(|s| s.to_string()).collect(),
            annot_types: vec!["Highlight".into(), "Underline".into(), "Highlight".into()],
            thumbnail_ok: true,
            ..Default::default()
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn c7_pass_dedups_annot_types() {
        let out = evaluate_c7(&ck(&["a", "b"], 1), "{}", 1, &ids(&["a", "b"]), &[]);
        assert!(out.pass);
        assert_eq!(
            out.detail,
            "loaded=true pages=1/1 missingNM=[] unexpectedNM=[] thumbnails=true annotTypes=[Highlight Underline]"
        );
        assert!(out.fail_log.is_none());
    }

    #[test]
    fn c7_fail_on_missing_nm() {
        let out = evaluate_c7(&ck(&["a"], 1), "{}", 1, &ids(&["a", "b"]), &[]);
        assert!(!out.pass);
        assert!(out.detail.contains("missingNM=[b]"));
        assert!(out.fail_log.as_deref().unwrap().starts_with("C7: "));
    }

    #[test]
    fn c7_fail_on_unexpected_nm() {
        let out = evaluate_c7(&ck(&["a", "x"], 1), "{}", 1, &[], &ids(&["x"]));
        assert!(!out.pass);
        assert!(out.detail.contains("unexpectedNM=[x]"));
    }

    #[test]
    fn c7_fail_on_page_count_mismatch() {
        let out = evaluate_c7(&ck(&[], 2), "{}", 1, &[], &[]);
        assert!(!out.pass);
        assert!(out.detail.contains("pages=2/1"));
    }

    #[test]
    fn c11b_skip_when_not_requested() {
        let out = evaluate_c11b(false, Err("unused"));
        assert!(out.pass);
        assert_eq!(out.detail, SKIP_C11B);
    }

    #[test]
    fn c11b_fail_when_helper_failed() {
        let out = evaluate_c11b(true, Err("exit status 1 ()"));
        assert!(!out.pass);
        assert_eq!(out.detail, "pdfkit_check failed: exit status 1 ()");
        assert!(out.fail_log.is_none());
    }

    #[test]
    fn c11b_skip_when_helper_did_not_evaluate() {
        let c = PdfkitCheck::default();
        let out = evaluate_c11b(true, Ok(&c));
        assert!(out.pass);
        assert_eq!(out.detail, SKIP_C11B_UNEVALUATED);
    }

    #[test]
    fn c11b_propagates_helper_verdict() {
        let c = PdfkitCheck {
            c11b_checked: true,
            c11b_pass: false,
            c11b_detail: "needle mismatch".into(),
            ..Default::default()
        };
        let out = evaluate_c11b(true, Ok(&c));
        assert!(!out.pass);
        assert_eq!(out.detail, "needle mismatch");
        assert_eq!(out.fail_log.as_deref(), Some("C11b: needle mismatch"));
    }
}
