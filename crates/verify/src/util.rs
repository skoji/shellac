//! Small string/byte helpers shared across the harness.

/// Truncates `s` to at most `n` bytes, cutting at the nearest char boundary
/// at or below `n`, and appends a truncation marker when anything was cut.
pub fn trunc(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut k = n;
    while k > 0 && !s.is_char_boundary(k) {
        k -= 1;
    }
    format!("{}...(truncated)", &s[..k])
}

/// Removes every Unicode whitespace character from `s`.
pub fn normalize_ws(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Counts non-overlapping occurrences of `%%EOF` in `b`.
pub fn count_eof(b: &[u8]) -> usize {
    count_subslice(b, b"%%EOF")
}

/// Counts non-overlapping occurrences of `needle` in `hay`.
pub fn count_subslice(hay: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || hay.len() < needle.len() {
        return 0;
    }
    let mut count = 0;
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if &hay[i..i + needle.len()] == needle {
            count += 1;
            i += needle.len();
        } else {
            i += 1;
        }
    }
    count
}

/// Reports whether `hay` contains `needle` as a contiguous subslice.
pub fn contains_subslice(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Removes duplicates while preserving first-occurrence order.
pub fn dedup(items: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for s in items {
        if seen.insert(s.clone()) {
            out.push(s.clone());
        }
    }
    out
}

/// Returns the part of `s` before the first newline (or all of `s`).
pub fn first_line(s: &str) -> &str {
    match s.find('\n') {
        Some(i) => &s[..i],
        None => s,
    }
}

/// Returns the needles missing from `haystack` (set semantics), in needle order.
pub fn contains_all(haystack: &[String], needles: &[String]) -> Vec<String> {
    let set: std::collections::HashSet<&str> = haystack.iter().map(|s| s.as_str()).collect();
    needles
        .iter()
        .filter(|n| !set.contains(n.as_str()))
        .cloned()
        .collect()
}

/// Returns the needles present in `haystack` (set semantics), in needle order.
pub fn contains_any(haystack: &[String], needles: &[String]) -> Vec<String> {
    let set: std::collections::HashSet<&str> = haystack.iter().map(|s| s.as_str()).collect();
    needles
        .iter()
        .filter(|n| set.contains(n.as_str()))
        .cloned()
        .collect()
}

/// Formats a string list as `[a b c]` (empty list renders as `[]`).
pub fn bracket_list<S: AsRef<str>>(items: &[S]) -> String {
    let joined: Vec<&str> = items.iter().map(|s| s.as_ref()).collect();
    format!("[{}]", joined.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trunc_short_string_unchanged() {
        assert_eq!(trunc("hello", 10), "hello");
        assert_eq!(trunc("hello", 5), "hello");
    }

    #[test]
    fn trunc_cuts_and_marks() {
        assert_eq!(trunc("hello world", 5), "hello...(truncated)");
    }

    #[test]
    fn trunc_respects_utf8_boundaries() {
        // "あ" is 3 bytes; cutting at 4 must not split the second char.
        let s = "ああ"; // 6 bytes
        assert_eq!(trunc(s, 4), "あ...(truncated)");
        assert_eq!(trunc(s, 1), "...(truncated)");
    }

    #[test]
    fn normalize_ws_removes_all_whitespace() {
        assert_eq!(normalize_ws(" a\tb\nc\u{00a0}d "), "abcd");
    }

    #[test]
    fn count_eof_non_overlapping() {
        assert_eq!(count_eof(b"%%EOF"), 1);
        assert_eq!(count_eof(b"%%EOF%%EOF"), 2);
        assert_eq!(count_eof(b"xx%%EOF\n%%EOFtrailer"), 2);
        assert_eq!(count_eof(b"%%EO"), 0);
        // A partial marker glued to a full one must not double count.
        assert_eq!(count_eof(b"%%%%EOF"), 1);
    }

    #[test]
    fn dedup_preserves_first_occurrence_order() {
        let v: Vec<String> = ["b", "a", "b", "c", "a"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(dedup(&v), vec!["b", "a", "c"]);
    }

    #[test]
    fn first_line_splits_on_newline() {
        assert_eq!(first_line("one\ntwo"), "one");
        assert_eq!(first_line("only"), "only");
        assert_eq!(first_line("cr kept\r\nnext"), "cr kept\r");
    }

    #[test]
    fn contains_all_reports_missing_in_needle_order() {
        let hay: Vec<String> = ["x", "y"].iter().map(|s| s.to_string()).collect();
        let needles: Vec<String> = ["a", "y", "b"].iter().map(|s| s.to_string()).collect();
        assert_eq!(contains_all(&hay, &needles), vec!["a", "b"]);
        assert!(contains_all(&hay, &[]).is_empty());
    }

    #[test]
    fn contains_any_reports_present_in_needle_order() {
        let hay: Vec<String> = ["x", "y"].iter().map(|s| s.to_string()).collect();
        let needles: Vec<String> = ["a", "y", "x"].iter().map(|s| s.to_string()).collect();
        assert_eq!(contains_any(&hay, &needles), vec!["y", "x"]);
    }

    #[test]
    fn bracket_list_formats_like_a_space_joined_vector() {
        assert_eq!(bracket_list::<&str>(&[]), "[]");
        assert_eq!(bracket_list(&["a", "b"]), "[a b]");
    }
}
