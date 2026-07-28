//! Known-exception list: matrix outcomes that are understood and accepted
//! rather than treated as defects of the save path.
//!
//! The list is data (`corpus/known-exceptions.json`), not code, so that the
//! reason an outcome is tolerated is reviewable next to the corpus it
//! describes. Entries carry the measurements observed when they were
//! registered; those are a record of how large the effect was and are never
//! used as a threshold — an entry either covers a (sample, check, scenario)
//! triple or it does not.

use serde::Deserialize;

/// Schema version this build understands. Bumping the file's `version`
/// without teaching the loader about it is a load error rather than a
/// silently different interpretation.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnownExceptions {
    pub version: u32,
    #[serde(default)]
    pub description: String,
    pub exceptions: Vec<Exception>,
}

/// One registered exception. `samples`, `checks`, and `scenarios` are all
/// matched with the same `*` wildcard rule, so `loop-*` covers the loop
/// family while an explicit name covers exactly itself.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exception {
    pub id: String,
    pub samples: Vec<String>,
    pub checks: Vec<String>,
    /// Omitted means every scenario.
    #[serde(default)]
    pub scenarios: Option<Vec<String>>,
    pub reason: String,
    /// Observations recorded at registration time. Documentation only.
    #[serde(default)]
    pub recorded_measurements: String,
}

impl KnownExceptions {
    pub fn parse(json: &str) -> Result<Self, String> {
        let list: KnownExceptions =
            serde_json::from_str(json).map_err(|e| format!("parsing known exceptions: {e}"))?;
        list.validate()?;
        Ok(list)
    }

    pub fn load(path: &str) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("reading {path}: {e}"))?;
        Self::parse(&text)
    }

    /// The first entry covering this triple, if any.
    pub fn find(&self, sample: &str, check: &str, scenario: &str) -> Option<&Exception> {
        self.exceptions
            .iter()
            .find(|e| e.applies_to(sample, check, scenario))
    }

    pub fn is_known(&self, sample: &str, check: &str, scenario: &str) -> bool {
        self.find(sample, check, scenario).is_some()
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported known-exception schema version {} (this build understands {})",
                self.version, SCHEMA_VERSION
            ));
        }
        let mut seen: Vec<&str> = Vec::new();
        for e in &self.exceptions {
            if e.id.trim().is_empty() {
                return Err("known exception has an empty id".to_string());
            }
            if seen.contains(&e.id.as_str()) {
                return Err(format!("duplicate known-exception id {:?}", e.id));
            }
            seen.push(&e.id);
            if e.samples.is_empty() {
                return Err(format!("known exception {:?} lists no samples", e.id));
            }
            if e.checks.is_empty() {
                return Err(format!("known exception {:?} lists no checks", e.id));
            }
            if e.scenarios.as_ref().is_some_and(|s| s.is_empty()) {
                return Err(format!(
                    "known exception {:?} has an empty scenario list; omit the field to mean every scenario",
                    e.id
                ));
            }
            if e.reason.trim().is_empty() {
                return Err(format!("known exception {:?} has no reason", e.id));
            }
        }
        Ok(())
    }
}

impl Exception {
    pub fn applies_to(&self, sample: &str, check: &str, scenario: &str) -> bool {
        any_match(&self.samples, sample)
            && any_match(&self.checks, check)
            && match &self.scenarios {
                None => true,
                Some(patterns) => any_match(patterns, scenario),
            }
    }
}

fn any_match(patterns: &[String], value: &str) -> bool {
    patterns.iter().any(|p| glob_match(p, value))
}

/// Wildcard match where `*` stands for any run of characters (including
/// none). Nothing else is special, so plain names match exactly.
fn glob_match(pattern: &str, value: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == value;
    }
    let last = parts.len() - 1;
    let Some(mut rest) = value.strip_prefix(parts[0]) else {
        return false;
    };
    for mid in &parts[1..last] {
        if mid.is_empty() {
            continue;
        }
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    rest.ends_with(parts[last])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_match_without_wildcard_is_equality() {
        assert!(glob_match("loop-01", "loop-01"));
        assert!(!glob_match("loop-0", "loop-01"));
        assert!(!glob_match("loop-01", "loop-0"));
    }

    #[test]
    fn glob_match_handles_leading_trailing_and_inner_wildcards() {
        assert!(glob_match("loop-*", "loop-01"));
        assert!(glob_match("loop-*", "loop-"));
        assert!(!glob_match("loop-*", "add"));
        assert!(glob_match("*-comment", "modify-comment"));
        assert!(glob_match("S9-*-user", "S9-rc4-empty-user"));
        assert!(!glob_match("S9-*-user", "S9-rc4-empty-owner"));
        assert!(glob_match("*", "anything"));
    }

    #[test]
    fn glob_match_does_not_reuse_consumed_input() {
        assert!(!glob_match("a*a", "a"));
        assert!(glob_match("a*a", "aa"));
    }

    #[test]
    fn applies_to_requires_all_three_dimensions() {
        let e = Exception {
            id: "e".to_string(),
            samples: vec!["S7".to_string()],
            checks: vec!["C8".to_string()],
            scenarios: Some(vec!["loop-*".to_string()]),
            reason: "r".to_string(),
            recorded_measurements: String::new(),
        };
        assert!(e.applies_to("S7", "C8", "loop-03"));
        assert!(!e.applies_to("S1", "C8", "loop-03"));
        assert!(!e.applies_to("S7", "C11a", "loop-03"));
        assert!(!e.applies_to("S7", "C8", "add"));
    }
}
