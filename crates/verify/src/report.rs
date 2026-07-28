//! Matrix report generation: the summary matrix with its data-driven
//! legend, per-sample scenario tables, and the encrypted-fixture section.

use crate::consts::{CELL_VOCAB_NOTE, CHECK_DEFS};
use crate::encrypted::EncMode;
use crate::sample::SampleResult;
use crate::sanitize::Sanitizer;
use crate::scenario::Scenario;
use crate::util::{bracket_list, trunc};

pub fn mark(pass: bool) -> &'static str {
    if pass { "pass" } else { "**FAIL**" }
}

/// Produces the Encrypted-column cell for a sample. This column reflects
/// the encrypted path itself:
///   - refused fixtures: expected refusal status and byte-identical file;
///   - roundtrip fixtures: every C-enc-qpdf check passed and every save
///     operation actually ran (a non-skipped fatal or an incomplete loop
///     means the encrypted save path is broken);
///   - all other samples: n/a.
///
/// Failures of unrelated checks (e.g. position checks) do not leak in.
pub fn encrypted_column(r: &SampleResult) -> String {
    match r.enc_mode {
        EncMode::RefusedUnchanged => {
            if r.enc_got_status == r.enc_want_status && r.enc_bytes_equal {
                "refused-unchanged".to_string()
            } else {
                "**FAIL**".to_string()
            }
        }
        EncMode::RoundtripPass => {
            if r.loop_scenarios.len() < 10 {
                return "**FAIL**".to_string();
            }
            let mut slots: Vec<(&Scenario, bool)> = vec![
                (&r.add, false),
                (&r.modified, r.modified_skipped),
                (&r.add_multiline, r.add_multiline_skipped),
                (&r.removed, false),
            ];
            slots.extend(r.loop_scenarios.iter().map(|sc| (sc, false)));
            for (sc, skipped) in slots {
                if skipped {
                    continue;
                }
                // Any non-skipped fatal — including a loop iteration —
                // means the encrypted save path is broken.
                if !sc.fatal.is_empty() {
                    return "**FAIL**".to_string();
                }
                for c in &sc.checks {
                    if c.id == "C-enc-qpdf" && !c.pass {
                        return "**FAIL**".to_string();
                    }
                }
            }
            "roundtrip-pass".to_string()
        }
        EncMode::None => "n/a".to_string(),
    }
}

/// Summary cell over all instances of a check id in a sample: "pass",
/// "**FAIL**" (a check failed; takes precedence), "**FAIL (op)**" (an
/// operation failed so the check never ran), or "n/a".
pub fn aggregate_check(res: &SampleResult, id: &str) -> String {
    // Refused-mode samples run none of the standard checks by design.
    if res.enc_mode == EncMode::RefusedUnchanged {
        return "n/a".to_string();
    }
    let mut pass = true;
    let mut found = false;
    let mut op_fail = false;
    let mut scan = |sc: &Scenario| {
        if !sc.fatal.is_empty() {
            op_fail = true;
            return;
        }
        for c in &sc.checks {
            if c.id == id {
                found = true;
                if !c.pass {
                    pass = false;
                }
            }
        }
    };
    scan(&res.add);
    if !res.modified_skipped {
        scan(&res.modified);
    }
    if !res.add_multiline_skipped {
        scan(&res.add_multiline);
    }
    scan(&res.removed);
    for sc in &res.loop_scenarios {
        scan(sc);
    }
    match (found, pass, op_fail) {
        (true, false, _) => "**FAIL**".to_string(),
        (_, _, true) => "**FAIL (op)**".to_string(),
        (true, _, _) => "pass".to_string(),
        _ => "n/a".to_string(),
    }
}

/// Sanitizes tool-derived text, then truncates for a table cell. The
/// sanitizer must run before truncation so a path cut in half cannot
/// escape replacement.
fn one_line(s: &str, san: &Sanitizer) -> String {
    trunc(&san.apply(s), 400).replace('\n', " ")
}

fn scenario_section(out: &mut String, sc: &Scenario, san: &Sanitizer) {
    out.push_str(&format!("#### Scenario: {}\n\n", sc.name));
    if !sc.fatal.is_empty() {
        out.push_str(&format!("**FATAL**: {}\n\n", san.apply(&sc.fatal)));
        return;
    }
    out.push_str("| Check | Result | Detail |\n|---|---|---|\n");
    for c in &sc.checks {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            c.id,
            mark(c.pass),
            one_line(&c.detail, san)
        ));
    }
    out.push('\n');
    if let Some(a) = &sc.audit {
        out.push_str(&format!("- increment audit: {}\n", a.summary()));
    }
    out.push_str(&format!("- C4: exact={}\n", sc.c4_exact));
    out.push_str(&format!(
        "- C9 Producer: pdfinfo={:?} exiftool={:?}\n",
        sc.producer, sc.exif_prod
    ));
    if !sc.ap_by_id.is_empty() {
        let parts: Vec<String> = sc
            .ap_by_id
            .iter()
            .map(|(k, v)| format!("{k}: /AP={v}"))
            .collect();
        out.push_str(&format!("- C10 /AP: {}\n", parts.join(", ")));
    }
    if let Some(sel) = &sc.c11a_selection
        && sel.candidates >= 2
    {
        out.push_str(&format!(
                "- C11a needle selection: {} candidates on page 1; selected {} (center distance {:.2}pt from PDFKit anchor bounds)\n",
                sel.candidates, sel.chosen, sel.center_distance
            ));
    }
    out.push('\n');
}

fn collect_fail_logs(r: &SampleResult) -> Vec<String> {
    let mut logs = Vec::new();
    let mut collect = |sc: &Scenario| {
        if !sc.fatal.is_empty() {
            logs.push(format!("[{}] FATAL: {}", sc.name, sc.fatal));
        }
        for l in &sc.fail_logs {
            logs.push(format!("[{}] {}", sc.name, l));
        }
    };
    collect(&r.add);
    if !r.modified_skipped {
        collect(&r.modified);
    }
    if !r.add_multiline_skipped {
        collect(&r.add_multiline);
    }
    collect(&r.removed);
    for sc in &r.loop_scenarios {
        collect(sc);
    }
    logs
}

/// Builds the complete matrix markdown. `env_lines` are pre-rendered
/// environment bullet lines; `generated_at` is an RFC3339 timestamp.
/// `san` rewrites machine-specific strings in quoted tool output; it is
/// applied before any truncation.
pub fn build_report(
    results: &[SampleResult],
    env_lines: &str,
    generated_at: &str,
    san: &Sanitizer,
) -> String {
    let mut sb = String::new();
    sb.push_str("# Incremental-save verification matrix\n\n");
    sb.push_str(&format!("Generated: {generated_at}\n\n"));
    sb.push_str("## Environment\n\n");
    sb.push_str(&san.apply(env_lines));
    sb.push_str("\n## Summary matrix\n\n");

    // Data-driven legend.
    sb.push_str("| Check | Meaning |\n|---|---|\n");
    for def in CHECK_DEFS {
        sb.push_str(&format!("| {} | {} |\n", def.id, def.meaning));
    }
    sb.push('\n');
    sb.push_str(CELL_VOCAB_NOTE);
    sb.push_str(
        " C6-quads and C-enc-qpdf run in specific scenarios only and appear in the per-sample scenario tables.\n\n",
    );

    sb.push_str("| Sample | C1 | C2 | C3 | C4 | C5 | C6 | C7 | C8 | C11a | C11b | Encrypted |\n");
    sb.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for r in results {
        if !r.fatal_err.is_empty() {
            let mut row = vec![
                r.name.clone(),
                format!("**FATAL**: {}", one_line(&r.fatal_err, san)),
            ];
            row.extend(std::iter::repeat_n(String::new(), 10));
            sb.push_str(&format!("| {} |\n", row.join(" | ")));
            continue;
        }
        let mut row = vec![r.name.clone()];
        for id in ["C1", "C2", "C3", "C4", "C5", "C6", "C7"] {
            row.push(aggregate_check(r, id));
        }
        if r.enc_mode == EncMode::RefusedUnchanged {
            row.push("n/a".to_string());
        } else {
            row.push(mark(r.c8_pass).to_string());
        }
        row.push(aggregate_check(r, "C11a"));
        row.push(aggregate_check(r, "C11b"));
        row.push(encrypted_column(r));
        sb.push_str(&format!("| {} |\n", row.join(" | ")));
    }

    // Text anchors.
    sb.push_str("\n## Text anchors\n\n");
    sb.push_str(
        "The needle chosen by pdfkit_textbbox on each sample's page 1. Samples with found=false \
skip C11a/C11b in every scenario and fall back to the fixed-coordinate placement \
(100,100)-(300,120) etc.\n\n",
    );
    sb.push_str("| Sample | found | needle | pageRotation | MediaBox |\n|---|---|---|---|---|\n");
    for r in results {
        if !r.fatal_err.is_empty() || r.enc_mode == EncMode::RefusedUnchanged {
            continue;
        }
        let needle = if r.anchor.found {
            r.anchor.needle.clone()
        } else if !r.anchor_err.is_empty() {
            format!("(error: {})", one_line(&r.anchor_err, san))
        } else {
            "(no anchor text on this page)".to_string()
        };
        sb.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            r.name, r.anchor.found, needle, r.anchor.page_rotation, r.anchor.media_box
        ));
    }

    // Recorded items.
    sb.push_str("\n## Recorded items (C9 / C10, after add)\n\n");
    sb.push_str("| Sample | C9 Producer (pdfinfo) | C9 Producer (exiftool) | C10 /AP |\n|---|---|---|---|\n");
    for r in results {
        if !r.fatal_err.is_empty() || r.enc_mode == EncMode::RefusedUnchanged {
            continue;
        }
        let ap: Vec<String> = r
            .add
            .ap_by_id
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let ap_str = if ap.is_empty() {
            "n/a".to_string()
        } else {
            ap.join(", ")
        };
        sb.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            r.name, r.add.producer, r.add.exif_prod, ap_str
        ));
    }

    // Encrypted fixtures.
    build_encrypted_section(&mut sb, results, san);

    // Per-sample sections.
    for r in results {
        sb.push_str(&format!("\n## {}\n\n", r.name));
        if !r.fatal_err.is_empty() {
            sb.push_str(&format!("**FATAL**: {}\n", san.apply(&r.fatal_err)));
            continue;
        }
        if r.enc_mode == EncMode::RefusedUnchanged {
            sb.push_str("### Baseline (refused-mode fixture)\n\n");
            sb.push_str(&format!(
                "- size: {} bytes / %%EOF: {}\n",
                r.base_size, r.base_eof
            ));
            sb.push_str(&format!(
                "- refused: {}\n\n",
                san.apply(&r.enc_refused_note)
            ));
            if !r.enc_add_fatal_msg.is_empty() {
                sb.push_str("### Raw log (engine add stderr)\n\n```\n");
                sb.push_str(&trunc(&san.apply(&r.enc_add_fatal_msg), 1500));
                sb.push_str("\n```\n");
            }
            continue;
        }
        sb.push_str("### Baseline\n\n");
        sb.push_str(&format!(
            "- size: {} bytes / pages: {} / %%EOF: {}\n",
            r.base_size, r.pages, r.base_eof
        ));
        sb.push_str(&format!(
            "- Producer: pdfinfo={:?} exiftool={:?}\n",
            r.base_producer, r.base_exif_producer
        ));
        sb.push_str(&format!(
            "- existing annotation types: {} / thumbnails: {}\n\n",
            bracket_list(&r.base_annot_types),
            r.base_thumb_ok
        ));

        sb.push_str("### add / modify-comment / add-multiline / remove\n\n");
        scenario_section(&mut sb, &r.add, san);
        if !r.modified_skipped {
            scenario_section(&mut sb, &r.modified, san);
        } else {
            sb.push_str("#### Scenario: modify-comment\n\n");
            sb.push_str(&format!("{}\n\n", san.apply(&r.modified.fatal)));
        }
        if !r.add_multiline_skipped {
            scenario_section(&mut sb, &r.add_multiline, san);
        } else {
            sb.push_str("#### Scenario: add-multiline\n\n");
            sb.push_str(&format!("{}\n\n", san.apply(&r.add_multiline.fatal)));
        }
        scenario_section(&mut sb, &r.removed, san);

        sb.push_str("### loop (10 incremental saves)\n\n");
        sb.push_str(&format!(
            "C8: {} — {}\n\n",
            mark(r.c8_pass),
            san.apply(&r.c8_detail)
        ));
        sb.push_str(
            "| iter | size (bytes) | delta (bytes) | checks | objects in increment (recorded, not asserted) |\n|---|---|---|---|---|\n",
        );
        for (i, sc) in r.loop_scenarios.iter().enumerate() {
            let objs = sc
                .audit
                .as_ref()
                .map(|a| a.obj_nrs.join(" "))
                .unwrap_or_default();
            sb.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                i + 1,
                r.loop_sizes.get(i).copied().unwrap_or(0),
                r.loop_deltas.get(i).copied().unwrap_or(0),
                mark(sc.all_pass()),
                objs
            ));
        }
        sb.push('\n');
        // Aggregated needle-ambiguity note for the loop iterations (the
        // per-scenario detail column is not rendered for loop iters).
        let ambiguous: Vec<(usize, &crate::checks::bbox::NeedleSelection)> = r
            .loop_scenarios
            .iter()
            .enumerate()
            .filter_map(|(i, sc)| {
                sc.c11a_selection
                    .as_ref()
                    .filter(|s| s.candidates >= 2)
                    .map(|s| (i + 1, s))
            })
            .collect();
        if !ambiguous.is_empty() {
            let max_d = ambiguous
                .iter()
                .map(|(_, s)| s.center_distance)
                .fold(f64::NEG_INFINITY, f64::max);
            let n = ambiguous[0].1.candidates;
            sb.push_str(&format!(
                "- C11a needle selection (loop): needle matched {} times on page 1; nearest-to-anchor candidate selected in {} iteration(s), max center distance {:.2}pt\n\n",
                n,
                ambiguous.len(),
                max_d
            ));
        }

        let logs = collect_fail_logs(r);
        if !logs.is_empty() {
            sb.push_str("### Failure logs\n\n```\n");
            for l in logs {
                sb.push_str(&san.apply(&l));
                sb.push_str("\n\n");
            }
            sb.push_str("```\n");
        }
    }
    sb
}

fn build_encrypted_section(sb: &mut String, results: &[SampleResult], san: &Sanitizer) {
    sb.push_str("\n## Encrypted fixtures\n\n");
    sb.push_str(
        "Samples whose filename starts with an `S9-`/`S10-`/`S11-`/`S12-` prefix exercise the \
encrypted-save paths (fixtures generated by corpus/generators/generate_encrypted_fixtures.sh).\n\n\
- **Roundtrip fixtures** (`S9-`/`S10-` prefix; auto-decryptable, annotations permitted): the normal \
scenario flow runs end-to-end and each scenario appends a `C-enc-qpdf` check \
(`qpdf --check --password=`) so a structural break introduced by re-encryption fails visibly. \
The Encrypted column is `roundtrip-pass` when every save succeeded and every C-enc-qpdf check passed; \
failures of unrelated checks do not affect this column.\n\
- **Refused fixtures** (`S11-` password-protected / `S12-` annotations-restricted): the engine must \
refuse the save with the expected status and leave the file byte-for-byte unchanged. The Encrypted \
column is `refused-unchanged` when both hold. The standard check columns are n/a by design.\n\n",
    );

    let roundtrip: Vec<&SampleResult> = results
        .iter()
        .filter(|r| r.enc_mode == EncMode::RoundtripPass && r.fatal_err.is_empty())
        .collect();
    let refused: Vec<&SampleResult> = results
        .iter()
        .filter(|r| r.enc_mode == EncMode::RefusedUnchanged && r.fatal_err.is_empty())
        .collect();
    if roundtrip.is_empty() && refused.is_empty() {
        sb.push_str("(no encrypted fixtures in this corpus run)\n");
        return;
    }

    if !roundtrip.is_empty() {
        sb.push_str("### C-enc-qpdf results (roundtrip fixtures)\n\n");
        sb.push_str("| Sample | Scenario | Result | Detail |\n|---|---|---|---|\n");
        for r in &roundtrip {
            let mut rows: Vec<(&str, &Scenario)> = vec![
                ("add", &r.add),
                ("modify-comment", &r.modified),
                ("add-multiline", &r.add_multiline),
                ("remove", &r.removed),
            ];
            if let Some(last) = r.loop_scenarios.last() {
                rows.push(("loop-10", last));
            }
            for (label, sc) in rows {
                if !sc.fatal.is_empty() {
                    sb.push_str(&format!(
                        "| {} | {} | n/a | {} |\n",
                        r.name,
                        label,
                        one_line(&sc.fatal, san)
                    ));
                    continue;
                }
                match sc.checks.iter().find(|c| c.id == "C-enc-qpdf") {
                    Some(c) => sb.push_str(&format!(
                        "| {} | {} | {} | {} |\n",
                        r.name,
                        label,
                        mark(c.pass),
                        one_line(&c.detail, san)
                    )),
                    None => sb.push_str(&format!(
                        "| {} | {} | n/a | (check not attached) |\n",
                        r.name, label
                    )),
                }
            }
        }
        sb.push('\n');
    }

    if !refused.is_empty() {
        sb.push_str("### Refusal contract (refused fixtures)\n\n");
        sb.push_str(
            "| Sample | Expected status | Got status | Bytes | Result |\n|---|---|---|---|---|\n",
        );
        for r in &refused {
            let got = if r.enc_got_status.is_empty() {
                "<none>"
            } else {
                &r.enc_got_status
            };
            let bytes = if r.enc_bytes_equal {
                "unchanged"
            } else {
                "CHANGED"
            };
            sb.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                r.name,
                r.enc_want_status,
                got,
                bytes,
                encrypted_column(r)
            ));
        }
        sb.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::Scenario;

    fn passing_scenario(name: &str, ids: &[&str]) -> Scenario {
        let mut sc = Scenario {
            name: name.to_string(),
            ..Default::default()
        };
        for id in ids {
            sc.add(id, true, "ok");
        }
        sc
    }

    fn base_sample() -> SampleResult {
        SampleResult {
            name: "S1".to_string(),
            c8_pass: true,
            c8_detail: "10/10 iterations pass, max increment 1 bytes".to_string(),
            add: passing_scenario("add", &["C1", "C11a"]),
            modified: passing_scenario("modify-comment", &["C1"]),
            add_multiline: passing_scenario("add-multiline", &["C1"]),
            removed: passing_scenario("remove", &["C1"]),
            ..Default::default()
        }
    }

    #[test]
    fn aggregate_pass_fail_and_na() {
        let mut r = base_sample();
        assert_eq!(aggregate_check(&r, "C1"), "pass");
        assert_eq!(aggregate_check(&r, "C99"), "n/a");
        r.removed.add("C1", false, "bad");
        assert_eq!(aggregate_check(&r, "C1"), "**FAIL**");
    }

    #[test]
    fn aggregate_fail_takes_precedence_over_op_fail() {
        let mut r = base_sample();
        r.modified = Scenario::fatal("modify-comment", "modify-comment: boom");
        r.add.add("C1", false, "bad");
        assert_eq!(aggregate_check(&r, "C1"), "**FAIL**");
        // Without the failing check, the fatal shows as FAIL (op).
        let mut r2 = base_sample();
        r2.modified = Scenario::fatal("modify-comment", "modify-comment: boom");
        assert_eq!(aggregate_check(&r2, "C1"), "**FAIL (op)**");
    }

    #[test]
    fn aggregate_ignores_skipped_scenarios() {
        let mut r = base_sample();
        r.add_multiline =
            Scenario::fatal("add-multiline", "skipped: no text anchor for this sample");
        r.add_multiline_skipped = true;
        assert_eq!(aggregate_check(&r, "C1"), "pass");
    }

    #[test]
    fn aggregate_refused_is_na() {
        let mut r = base_sample();
        r.enc_mode = EncMode::RefusedUnchanged;
        assert_eq!(aggregate_check(&r, "C1"), "n/a");
    }

    fn roundtrip_sample() -> SampleResult {
        let mut r = base_sample();
        r.name = "S9-x".to_string();
        r.enc_mode = EncMode::RoundtripPass;
        for sc in [
            &mut r.add,
            &mut r.modified,
            &mut r.add_multiline,
            &mut r.removed,
        ] {
            sc.add("C-enc-qpdf", true, "qpdf --check: clean");
        }
        for i in 0..10 {
            let mut sc = passing_scenario(&format!("loop-{:02}", i + 1), &["C1"]);
            if i == 9 {
                sc.add("C-enc-qpdf", true, "qpdf --check: clean");
            }
            r.loop_scenarios.push(sc);
            r.loop_sizes.push(100 + i);
            r.loop_deltas.push(1);
        }
        r
    }

    #[test]
    fn encrypted_column_roundtrip_pass_ignores_unrelated_failures() {
        let mut r = roundtrip_sample();
        // A C11a failure in add must NOT leak into the Encrypted column.
        r.add.add("C11a", false, "position mismatch");
        r.c8_pass = false;
        assert_eq!(encrypted_column(&r), "roundtrip-pass");
    }

    #[test]
    fn encrypted_column_fails_on_enc_qpdf_failure() {
        let mut r = roundtrip_sample();
        if let Some(c) = r.loop_scenarios[9]
            .checks
            .iter_mut()
            .find(|c| c.id == "C-enc-qpdf")
        {
            c.pass = false;
        }
        assert_eq!(encrypted_column(&r), "**FAIL**");
    }

    #[test]
    fn encrypted_column_fails_on_save_fatal_or_short_loop() {
        let mut r = roundtrip_sample();
        r.removed = Scenario::fatal("remove", "remove: engine exploded");
        assert_eq!(encrypted_column(&r), "**FAIL**");

        let mut r2 = roundtrip_sample();
        r2.loop_scenarios.truncate(5);
        assert_eq!(encrypted_column(&r2), "**FAIL**");
    }

    #[test]
    fn encrypted_column_fails_on_loop_iteration_fatal() {
        let mut r = roundtrip_sample();
        r.loop_scenarios[9] = Scenario::fatal("loop-10", "load state: boom");
        assert_eq!(encrypted_column(&r), "**FAIL**");
    }

    #[test]
    fn encrypted_column_roundtrip_allows_skipped_slots() {
        let mut r = roundtrip_sample();
        r.add_multiline =
            Scenario::fatal("add-multiline", "skipped: no text anchor for this sample");
        r.add_multiline_skipped = true;
        assert_eq!(encrypted_column(&r), "roundtrip-pass");
    }

    #[test]
    fn encrypted_column_refused_and_none() {
        let mut r = base_sample();
        assert_eq!(encrypted_column(&r), "n/a");
        r.enc_mode = EncMode::RefusedUnchanged;
        r.enc_want_status = "encrypted_refused".to_string();
        r.enc_got_status = "encrypted_refused".to_string();
        r.enc_bytes_equal = true;
        assert_eq!(encrypted_column(&r), "refused-unchanged");
        r.enc_bytes_equal = false;
        assert_eq!(encrypted_column(&r), "**FAIL**");
        r.enc_bytes_equal = true;
        r.enc_got_status = "parse_failed".to_string();
        assert_eq!(encrypted_column(&r), "**FAIL**");
    }

    #[test]
    fn report_contains_summary_row_and_sections() {
        let mut r = base_sample();
        r.base_size = 1234;
        r.pages = 1;
        r.base_eof = 2;
        r.c8_pass = true;
        for i in 0..10 {
            r.loop_scenarios
                .push(passing_scenario(&format!("loop-{:02}", i + 1), &["C1"]));
            r.loop_sizes.push(1234 + (i + 1) * 10);
            r.loop_deltas.push(10);
        }
        let md = build_report(
            &[r],
            "- qpdf version x\n- save engine: cli: engine\n",
            "2026-01-01T00:00:00Z",
            &Sanitizer::new(),
        );
        assert!(md.contains("# Incremental-save verification matrix"));
        assert!(md.contains("| S1 | pass |"));
        assert!(md.contains(
            "| Sample | C1 | C2 | C3 | C4 | C5 | C6 | C7 | C8 | C11a | C11b | Encrypted |"
        ));
        assert!(md.contains("## Text anchors"));
        assert!(md.contains("### loop (10 incremental saves)"));
        assert!(md.contains("objects in increment (recorded, not asserted)"));
        // Legend is data-driven from CHECK_DEFS.
        assert!(md.contains("| C11a | position verification"));
        assert!(md.contains("(no encrypted fixtures in this corpus run)"));
    }

    #[test]
    fn report_summary_c8_cell_is_na_for_refused() {
        let mut r = base_sample();
        r.name = "S11-password-required".to_string();
        r.enc_mode = EncMode::RefusedUnchanged;
        r.enc_want_status = "encrypted_refused".to_string();
        r.enc_got_status = "encrypted_refused".to_string();
        r.enc_bytes_equal = true;
        r.enc_refused_note =
            "status=encrypted_refused (want encrypted_refused), bytes=unchanged".to_string();
        r.enc_add_fatal_msg = "engine: refused".to_string();
        let md = build_report(&[r], "", "2026-01-01T00:00:00Z", &Sanitizer::new());
        assert!(md.contains("| S11-password-required | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | refused-unchanged |"));
        assert!(md.contains("### Baseline (refused-mode fixture)"));
        assert!(md.contains("### Raw log (engine add stderr)"));
        assert!(md.contains("### Refusal contract (refused fixtures)"));
    }

    #[test]
    fn report_fatal_sample_row() {
        let r = SampleResult {
            name: "S1".to_string(),
            fatal_err: "baseline load: nope".to_string(),
            ..Default::default()
        };
        let md = build_report(&[r], "", "2026-01-01T00:00:00Z", &Sanitizer::new());
        assert!(md.contains("| S1 | **FATAL**: baseline load: nope |"));
    }

    #[test]
    fn detail_is_sanitized_before_truncation() {
        // A long path near the 400-byte cell boundary must not survive as
        // a partial (unsanitizable) fragment: sanitize runs first.
        let long_path = format!("/very/long/private/workdir-{}", "x".repeat(360));
        let mut r = base_sample();
        r.add.add(
            "C6",
            true,
            format!("qpdf warning (exit 3, processing completed): WARNING: {long_path}/S1/add.pdf oddity"),
        );
        let mut san = crate::sanitize::Sanitizer::new();
        san.add_path(&long_path, "work");
        san.finalize();
        let md = build_report(&[r], "", "2026-01-01T00:00:00Z", &san);
        assert!(!md.contains("/very/long/private"), "path fragment leaked");
        assert!(md.contains("WARNING: work/S1/add.pdf oddity"));
    }

    #[test]
    fn scenario_section_renders_ambiguity_bullet() {
        use crate::checks::bbox::NeedleSelection;
        use crate::geom::Rect;
        let mut r = base_sample();
        r.add.c11a_selection = Some(NeedleSelection {
            candidates: 3,
            chosen: Rect::new(1.0, 2.0, 3.0, 4.0),
            center_distance: 1.234,
        });
        let md = build_report(&[r], "", "2026-01-01T00:00:00Z", &Sanitizer::new());
        assert!(md.contains(
            "- C11a needle selection: 3 candidates on page 1; selected (1.00,2.00)-(3.00,4.00) (center distance 1.23pt from PDFKit anchor bounds)"
        ));
    }
}
