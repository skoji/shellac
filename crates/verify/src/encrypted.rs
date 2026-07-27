//! Encrypted-fixture classification and engine-status extraction.
//!
//! Fixtures whose basename starts with `S9-`/`S10-` are expected to
//! round-trip (auto-decryptable, annotation-permitting); `S11-`/`S12-`
//! fixtures are expected to be refused by the save engine with a specific
//! status and left byte-for-byte unchanged.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EncMode {
    #[default]
    None,
    RoundtripPass,
    RefusedUnchanged,
}

/// Classifies a sample by filename prefix (basename without extension).
/// Returns the mode plus the expected engine status string (empty when
/// unused).
pub fn classify_encrypted(name: &str) -> (EncMode, &'static str) {
    if name.starts_with("S9-") || name.starts_with("S10-") {
        (EncMode::RoundtripPass, "")
    } else if name.starts_with("S11-") {
        (EncMode::RefusedUnchanged, "encrypted_refused")
    } else if name.starts_with("S12-") {
        (EncMode::RefusedUnchanged, "annotations_restricted")
    } else {
        (EncMode::None, "")
    }
}

/// Extracts the last `"status":"<snake_case>"` value from the engine's
/// combined output. Later matches win so an operation-level status emitted
/// at the end is not shadowed by wrapper-level ones.
pub fn extract_status(msg: &str) -> String {
    const NEEDLE: &str = "\"status\":\"";
    let mut last = String::new();
    let mut rest = msg;
    let mut base = 0usize;
    while let Some(pos) = rest.find(NEEDLE) {
        let start = base + pos + NEEDLE.len();
        let tail = &msg[start..];
        let end = tail
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_lowercase() || *c == '_'))
            .map(|(i, _)| i)
            .unwrap_or(tail.len());
        // The value must be non-empty and closed by a quote to count.
        if end > 0 && tail[end..].starts_with('"') {
            last = tail[..end].to_string();
        }
        base = start;
        rest = &msg[base..];
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_requires_hyphenated_prefix() {
        assert_eq!(classify_encrypted("S9-rc4-empty-user").0, EncMode::RoundtripPass);
        assert_eq!(classify_encrypted("S10-aes256-empty-user").0, EncMode::RoundtripPass);
        assert_eq!(
            classify_encrypted("S11-password-required"),
            (EncMode::RefusedUnchanged, "encrypted_refused")
        );
        assert_eq!(
            classify_encrypted("S12-annotations-restricted"),
            (EncMode::RefusedUnchanged, "annotations_restricted")
        );
        assert_eq!(classify_encrypted("S9").0, EncMode::None);
        assert_eq!(classify_encrypted("S1").0, EncMode::None);
        assert_eq!(classify_encrypted("S90-x").0, EncMode::None);
    }

    #[test]
    fn extract_status_takes_last_match() {
        let msg = r#"wrapper {"status":"ok","applied":0} engine {"status":"encrypted_refused","applied":0}"#;
        assert_eq!(extract_status(msg), "encrypted_refused");
    }

    #[test]
    fn extract_status_empty_when_absent() {
        assert_eq!(extract_status("no status here"), "");
    }

    #[test]
    fn extract_status_ignores_non_snake_case_values() {
        assert_eq!(extract_status(r#""status":"OK""#), "");
        assert_eq!(extract_status(r#""status":"ok""#), "ok");
    }
}
