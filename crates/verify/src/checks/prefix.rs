//! C1: forward byte immutability.

/// Reports whether `cur` begins with exactly the bytes of `prev` and grew.
pub fn check_prefix(prev: &[u8], cur: &[u8]) -> (bool, String) {
    if cur.len() <= prev.len() {
        return (
            false,
            format!(
                "file did not grow: prev={} cur={} bytes",
                prev.len(),
                cur.len()
            ),
        );
    }
    if &cur[..prev.len()] != prev {
        let mut off: i64 = -1;
        for i in 0..prev.len() {
            if cur[i] != prev[i] {
                off = i as i64;
                break;
            }
        }
        return (
            false,
            format!(
                "prefix mismatch at byte offset {} (prev={} bytes)",
                off,
                prev.len()
            ),
        );
    }
    (
        true,
        format!(
            "first {} bytes identical, appended {} bytes",
            prev.len(),
            cur.len() - prev.len()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_when_grown_with_identical_prefix() {
        let (p, d) = check_prefix(b"abc", b"abcdef");
        assert!(p);
        assert_eq!(d, "first 3 bytes identical, appended 3 bytes");
    }

    #[test]
    fn fail_when_not_grown() {
        let (p, d) = check_prefix(b"abc", b"abc");
        assert!(!p);
        assert_eq!(d, "file did not grow: prev=3 cur=3 bytes");
        let (p2, d2) = check_prefix(b"abc", b"ab");
        assert!(!p2);
        assert_eq!(d2, "file did not grow: prev=3 cur=2 bytes");
    }

    #[test]
    fn fail_reports_first_differing_offset() {
        let (p, d) = check_prefix(b"abcd", b"abXdEXTRA");
        assert!(!p);
        assert_eq!(d, "prefix mismatch at byte offset 2 (prev=4 bytes)");
    }
}
