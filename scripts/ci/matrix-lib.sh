#!/usr/bin/env bash
# matrix-lib.sh — the shared body of run-matrix-linux.sh and
# run-matrix-macos.sh. Source it, do not run it.
#
# The two platform scripts differ only in which checks their environment can
# evaluate. The steps around that -- build, run, gate, negative canary,
# timing -- are the same, and are kept here rather than duplicated so the
# two cannot drift into asserting different things.
#
# Bash-style constraint (project convention, same as lib.sh): no command
# substitution, no backticks, no heredocs.

# The /NM prefix shellac-cli emits. Frozen together with the engine's
# annotation ids.
CI_MATRIX_NM_PREFIX="shellac-verify"

# Repository root; set by the sourcing script before ci_matrix_run.
CI_MATRIX_REPO_ROOT="."

# Wall-clock seconds the matrix itself took, set by ci_matrix_run.
CI_MATRIX_SECONDS=0

# ci_matrix_record_timing <label> <out dir>: the number CI exists to report.
# It lands in the job log, in an artifact, and in the run summary.
ci_matrix_record_timing() {
    printf '%s matrix seconds: %s\n' "$1" "${CI_MATRIX_SECONDS}" > "$2/timing.txt"
    ci_info "$1: matrix took ${CI_MATRIX_SECONDS}s (recorded in $2/timing.txt)"
    if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
        printf -- '- %s matrix: %ss\n' "$1" "${CI_MATRIX_SECONDS}" >> "${GITHUB_STEP_SUMMARY}"
    fi
}

# ci_matrix_gate <label> <verify bin> <fails json> <exceptions json>
#
# Exit 3 is the gate's verdict that the run produced failures the registry
# does not cover. Any other non-zero status means the gate could not reach a
# verdict at all, which is a different problem and is reported as one.
ci_matrix_gate() {
    local rc=0
    "$2" gate --fails "$3" --exceptions "$4" || rc=$?
    if [ "${rc}" -eq 0 ]; then
        ci_pass "$1: every failing cell is covered by the known-exception list"
    elif [ "${rc}" -eq 3 ]; then
        ci_fail "$1: the matrix produced failures outside the known-exception list"
    else
        ci_fail "$1: gate could not reach a verdict (exit ${rc})"
    fi
}

# ci_matrix_canary <label> <verify bin> <exceptions json>
#
# A gate that accepted everything would be indistinguishable from a clean
# run, so every run also feeds it a committed failure that no entry covers
# and requires the rejection.
ci_matrix_canary() {
    local canary="${CI_MATRIX_REPO_ROOT}/scripts/ci/testdata/unknown-fail-cells.json"
    ci_require_file "${canary}"
    local rc=0
    "$2" gate --fails "${canary}" --exceptions "$3" > "${CI_TMP}/canary.log" 2>&1 || rc=$?
    if [ "${rc}" -ne 3 ]; then
        # The log lives in CI_TMP, which the EXIT trap deletes, so a canary
        # that misbehaves would otherwise leave nothing behind but its exit
        # code -- and the exit code is the one thing already known to be
        # wrong. Emit it while the file still exists.
        printf '%s: canary gate exited %s; its output follows\n' "$1" "${rc}" >&2
        cat "${CI_TMP}/canary.log" >&2
    fi
    ci_expect_eq "$1: a failure outside the list exits 3" "3" "${rc}"
}

# ci_matrix_run <label> <samples dir> <out dir> [extra `verify matrix` flags...]
ci_matrix_run() {
    local label="$1"
    local samples="$2"
    local out_dir="$3"
    shift 3

    ci_require_tools cargo qpdf pdftotext pdfinfo
    # C9 records the Producer metadata and never judges it, so a missing
    # exiftool leaves a column blank rather than failing the run.
    if ! command -v exiftool > /dev/null 2>&1; then
        ci_info "${label}: exiftool not on PATH; the C9 exiftool column will be empty"
    fi
    if [ ! -d "${samples}" ]; then
        printf '%s: samples directory not found: %s\n' "${CI_LABEL}" "${samples}" >&2
        exit 1
    fi
    mkdir -p "${out_dir}"

    local manifest="${CI_MATRIX_REPO_ROOT}/Cargo.toml"
    local target_dir="${CARGO_TARGET_DIR:-${CI_MATRIX_REPO_ROOT}/target}"
    local engine="${target_dir}/release/shellac-cli"
    local verify="${target_dir}/release/verify"
    local exceptions="${CI_MATRIX_REPO_ROOT}/corpus/known-exceptions.json"
    ci_require_file "${manifest}"
    ci_require_file "${exceptions}"

    ci_info "${label}: building the save engine and the harness"
    cargo build --release --manifest-path "${manifest}" -p shellac --bin shellac-cli
    cargo build --release --manifest-path "${manifest}" -p verify

    local report="${out_dir}/matrix.md"
    local fails="${out_dir}/fails.json"
    ci_info "${label}: running the matrix over ${samples}"
    local started="${SECONDS}"
    "${verify}" matrix \
        --samples "${samples}" \
        --work "${CI_TMP}/work" \
        --scripts "${CI_MATRIX_REPO_ROOT}/scripts" \
        --bin "${CI_TMP}/bin" \
        --out "${report}" \
        --engine-cmd "${engine}" \
        --nm-prefix "${CI_MATRIX_NM_PREFIX}" \
        --exceptions "${exceptions}" \
        --fails-out "${fails}" \
        "$@"
    CI_MATRIX_SECONDS=$((SECONDS - started))
    ci_pass "${label}: the matrix ran to completion and wrote ${report}"

    ci_matrix_gate "${label}" "${verify}" "${fails}" "${exceptions}"
    ci_matrix_canary "${label}" "${verify}" "${exceptions}"
    ci_matrix_record_timing "${label}" "${out_dir}"
}
