//! Report sanitization. External-tool output quoted in the report can
//! contain local filesystem paths (work/sample copies, the engine binary,
//! the home directory); the sanitizer rewrites these to stable role names
//! so the generated markdown carries no machine-specific information.
//! Judgments always run on the raw strings — sanitization is applied only
//! when rendering.

pub struct Sanitizer {
    /// (from, to) pairs, applied in order (longest `from` first).
    pairs: Vec<(String, String)>,
}

impl Sanitizer {
    pub fn new() -> Self {
        Sanitizer { pairs: Vec::new() }
    }

    /// Registers a replacement. Degenerately short path strings (no
    /// separator and <= 3 chars) are ignored to avoid mangling prose.
    pub fn add_path(&mut self, from: &str, to: &str) {
        if from.is_empty() || from == to {
            return;
        }
        if !from.contains('/') && from.len() <= 3 {
            return;
        }
        self.pairs.push((from.to_string(), to.to_string()));
    }

    /// Registers a path replacement together with its canonicalized form
    /// (both spellings can appear in tool output).
    pub fn add_path_with_canonical(&mut self, from: &str, to: &str) {
        if let Ok(canon) = std::fs::canonicalize(from) {
            self.add_path(&canon.to_string_lossy(), to);
        }
        self.add_path(from, to);
    }

    /// Registers a literal (non-path) replacement.
    pub fn add_literal(&mut self, from: &str, to: &str) {
        if !from.is_empty() && from != to {
            self.pairs.push((from.to_string(), to.to_string()));
        }
    }

    /// Sorts pairs longest-first so nested paths resolve to the most
    /// specific role name. Call once after registering everything.
    pub fn finalize(&mut self) {
        self.pairs.sort_by_key(|p| std::cmp::Reverse(p.0.len()));
    }

    pub fn apply(&self, s: &str) -> String {
        let mut out = s.to_string();
        for (from, to) in &self.pairs {
            out = out.replace(from.as_str(), to);
        }
        out
    }
}

impl Default for Sanitizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_longest_first() {
        let mut s = Sanitizer::new();
        s.add_path("/home/u/project/work", "work");
        s.add_path("/home/u/project", "project");
        s.add_path("/home/u", "~");
        s.finalize();
        assert_eq!(
            s.apply("saved /home/u/project/work/S1/add.pdf under /home/u/project via /home/u"),
            "saved work/S1/add.pdf under project via ~"
        );
    }

    #[test]
    fn engine_path_becomes_basename() {
        let mut s = Sanitizer::new();
        s.add_path("/opt/tools/engine-cli", "engine-cli");
        s.finalize();
        assert_eq!(
            s.apply("/opt/tools/engine-cli add x.pdf: exit status 1"),
            "engine-cli add x.pdf: exit status 1"
        );
    }

    #[test]
    fn nm_prefix_redaction_is_literal() {
        let mut s = Sanitizer::new();
        s.add_literal("secret-prefix", "<nm>");
        s.finalize();
        assert_eq!(
            s.apply("present=[secret-prefix-hl-1]"),
            "present=[<nm>-hl-1]"
        );
    }

    #[test]
    fn short_relative_paths_are_ignored() {
        let mut s = Sanitizer::new();
        s.add_path("w", "work");
        s.finalize();
        assert_eq!(s.apply("word w"), "word w");
    }
}
