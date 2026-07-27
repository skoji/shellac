//! C3: increment audit. Scans the appended bytes for forbidden tokens,
//! object headers, and structural markers.

use crate::consts::FORBIDDEN_TOKENS;
use crate::util::contains_subslice;

#[derive(Clone, Debug, Default)]
pub struct IncAudit {
    pub forbidden_hits: Vec<String>,
    pub obj_nrs: Vec<String>,
    pub has_obj_stm: bool,
    pub has_flate: bool,
    pub has_startxref: bool,
    pub size: usize,
}

impl IncAudit {
    pub fn pass(&self) -> bool {
        self.forbidden_hits.is_empty() && self.has_startxref
    }

    pub fn summary(&self) -> String {
        let mut s = format!(
            "{} bytes, objs=[{}], ObjStm={}, FlateDecode={}, startxref={}",
            self.size,
            self.obj_nrs.join(" "),
            self.has_obj_stm,
            self.has_flate,
            self.has_startxref
        );
        if !self.forbidden_hits.is_empty() {
            s.push_str(", FORBIDDEN=");
            s.push_str(&self.forbidden_hits.join(","));
        }
        s
    }
}

fn is_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

/// The whitespace class used by the object-header scanner: tab, newline,
/// form feed, carriage return, space (vertical tab intentionally excluded).
fn is_ws(b: u8) -> bool {
    matches!(b, b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Scans `inc` for object headers of the shape `<digits> <digits> obj` with
/// a word boundary after `obj`, returning the object numbers (first digit
/// runs) in order of appearance. Matches are non-overlapping.
fn scan_obj_numbers(inc: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < inc.len() {
        if let Some((nr, end)) = try_match_obj(inc, i) {
            out.push(nr);
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// Attempts to match `(\d+)[ws]+(\d+)[ws]+obj\b` starting exactly at `pos`.
/// Returns the captured object number and the match end offset.
fn try_match_obj(inc: &[u8], pos: usize) -> Option<(String, usize)> {
    let mut i = pos;
    let nr_start = i;
    while i < inc.len() && is_digit(inc[i]) {
        i += 1;
    }
    if i == nr_start {
        return None;
    }
    let nr_end = i;
    let ws1 = i;
    while i < inc.len() && is_ws(inc[i]) {
        i += 1;
    }
    if i == ws1 {
        return None;
    }
    let gen_start = i;
    while i < inc.len() && is_digit(inc[i]) {
        i += 1;
    }
    if i == gen_start {
        return None;
    }
    let ws2 = i;
    while i < inc.len() && is_ws(inc[i]) {
        i += 1;
    }
    if i == ws2 {
        return None;
    }
    if !inc[i..].starts_with(b"obj") {
        return None;
    }
    i += 3;
    if i < inc.len() && is_word_byte(inc[i]) {
        return None;
    }
    let nr = String::from_utf8_lossy(&inc[nr_start..nr_end]).into_owned();
    Some((nr, i))
}

/// Audits one increment (the bytes appended after the previous revision).
pub fn audit_increment(inc: &[u8]) -> IncAudit {
    let mut a = IncAudit {
        size: inc.len(),
        ..Default::default()
    };
    for tok in FORBIDDEN_TOKENS {
        if contains_subslice(inc, tok.as_bytes()) {
            a.forbidden_hits.push(tok.to_string());
        }
    }
    a.obj_nrs = scan_obj_numbers(inc);
    a.has_obj_stm = contains_subslice(inc, b"ObjStm");
    a.has_flate = contains_subslice(inc, b"FlateDecode");
    a.has_startxref = contains_subslice(inc, b"startxref");
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_object_numbers_in_order() {
        let inc = b"9 0 obj\n<<>>\nendobj\n27 0 obj\n<<>>\nendobj\nstartxref\n123\n%%EOF";
        let a = audit_increment(inc);
        assert_eq!(a.obj_nrs, vec!["9", "27"]);
        assert!(a.has_startxref);
        assert!(a.pass());
    }

    #[test]
    fn obj_requires_word_boundary() {
        let a = audit_increment(b"12 0 object");
        assert!(a.obj_nrs.is_empty());
        let b = audit_increment(b"12 0 obj<< >>");
        assert_eq!(b.obj_nrs, vec!["12"]);
        let c = audit_increment(b"12 0 obj");
        assert_eq!(c.obj_nrs, vec!["12"]);
    }

    #[test]
    fn obj_scanner_accepts_mixed_whitespace_but_not_vertical_tab() {
        let a = audit_increment(b"3\t\r\n0\x0cobj ");
        assert_eq!(a.obj_nrs, vec!["3"]);
        let b = audit_increment(b"3\x0b0 obj ");
        assert!(b.obj_nrs.is_empty());
    }

    #[test]
    fn obj_scanner_survives_invalid_utf8() {
        let mut inc: Vec<u8> = vec![0xff, 0xfe, b'\n'];
        inc.extend_from_slice(b"5 0 obj\nstream\n");
        inc.extend_from_slice(&[0x80, 0x81, 0x82]);
        inc.extend_from_slice(b"\nendstream\nstartxref\n");
        let a = audit_increment(&inc);
        assert_eq!(a.obj_nrs, vec!["5"]);
        assert!(a.has_startxref);
    }

    #[test]
    fn forbidden_tokens_fail_the_audit() {
        let a = audit_increment(b"9 0 obj /ToUnicode /Type/Font endobj startxref");
        assert_eq!(a.forbidden_hits, vec!["/ToUnicode", "/Type/Font"]);
        assert!(!a.pass());
        assert!(a.summary().ends_with(", FORBIDDEN=/ToUnicode,/Type/Font"));
    }

    #[test]
    fn missing_startxref_fails() {
        let a = audit_increment(b"9 0 obj endobj");
        assert!(!a.pass());
    }

    #[test]
    fn summary_format_matches_report_expectations() {
        let inc = b"9 0 obj\n27 0 obj\n28 0 obj\nstartxref\n";
        let mut a = audit_increment(inc);
        a.size = 1397;
        assert_eq!(
            a.summary(),
            "1397 bytes, objs=[9 27 28], ObjStm=false, FlateDecode=false, startxref=true"
        );
    }

    #[test]
    fn flate_and_objstm_are_recorded_but_do_not_fail() {
        let a = audit_increment(b"1 0 obj /Filter /FlateDecode /Type /ObjStm startxref");
        assert!(a.has_flate);
        assert!(a.has_obj_stm);
        assert!(a.pass());
    }
}
