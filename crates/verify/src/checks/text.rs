//! C4 (pdftotext) and C5 (PDFKit text) comparison judgments.
//!
//! Extraction errors are folded into the compared strings upstream (see
//! `state::load_state`): both sides erroring identically compares equal, so
//! C4/C5 pass; a one-sided error shows up as a mismatch.

use crate::util::normalize_ws;

pub struct C4Result {
    pub exact: bool,
    pub pass: bool,
    pub detail: String,
}

/// C4: pdftotext comparison. The verdict uses the whitespace-normalized
/// comparison; the exact-equality bit is recorded separately.
pub fn check_c4(prev: &str, cur: &str) -> C4Result {
    let exact = cur == prev;
    let norm = normalize_ws(cur) == normalize_ws(prev);
    C4Result {
        exact,
        pass: norm,
        detail: format!(
            "exact={} normalized={} (len {} -> {})",
            exact,
            norm,
            prev.len(),
            cur.len()
        ),
    }
}

/// C5: PDFKit text comparison (exact equality, no normalization).
pub fn check_c5(prev: &str, cur: &str) -> (bool, String) {
    let pass = cur == prev;
    (
        pass,
        format!("exact={} (len {} -> {})", pass, prev.len(), cur.len()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c4_pass_on_whitespace_only_difference() {
        let r = check_c4("a b\nc", "a  b c");
        assert!(r.pass);
        assert!(!r.exact);
        assert_eq!(r.detail, "exact=false normalized=true (len 5 -> 6)");
    }

    #[test]
    fn c4_fail_on_content_difference() {
        let r = check_c4("abc", "abd");
        assert!(!r.pass);
        assert_eq!(r.detail, "exact=false normalized=false (len 3 -> 3)");
    }

    #[test]
    fn c4_passes_when_both_sides_hold_the_same_error_string() {
        let e = "PDFTOTEXT-ERROR: pdftotext: exit status 1 (bad file)";
        let r = check_c4(e, e);
        assert!(r.pass);
        assert!(r.exact);
    }

    #[test]
    fn c4_fails_on_one_sided_error() {
        let r = check_c4("real text", "PDFTOTEXT-ERROR: pdftotext: exit status 1 (bad)");
        assert!(!r.pass);
    }

    #[test]
    fn c5_exact_only() {
        let (pass, detail) = check_c5("same", "same");
        assert!(pass);
        assert_eq!(detail, "exact=true (len 4 -> 4)");
        let (pass2, _) = check_c5("a b", "a  b");
        assert!(!pass2);
    }
}
