//! Annotation id (/NM) construction. The id prefix is injected via the
//! `--nm-prefix` CLI flag so the harness carries no fixed identity of its
//! own; the save engine under test must emit the same ids.

pub struct NmIds {
    prefix: String,
}

impl NmIds {
    pub fn new(prefix: &str) -> Self {
        NmIds {
            prefix: prefix.to_string(),
        }
    }

    pub fn highlight(&self) -> String {
        format!("{}-hl-1", self.prefix)
    }

    pub fn underline(&self) -> String {
        format!("{}-ul-1", self.prefix)
    }

    pub fn multiline_highlight(&self) -> String {
        format!("{}-hl-multiline-1", self.prefix)
    }

    pub fn loop_id(&self, i: i64) -> String {
        format!("{}-loop-{}", self.prefix, i)
    }

    /// The replacement /Contents text passed to the engine's modify-comment
    /// subcommand.
    pub fn modified_contents(&self) -> String {
        format!("{} modified content", self.prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_derive_from_prefix() {
        let ids = NmIds::new("test-verify");
        assert_eq!(ids.highlight(), "test-verify-hl-1");
        assert_eq!(ids.underline(), "test-verify-ul-1");
        assert_eq!(ids.multiline_highlight(), "test-verify-hl-multiline-1");
        assert_eq!(ids.loop_id(7), "test-verify-loop-7");
        assert_eq!(ids.modified_contents(), "test-verify modified content");
    }
}
