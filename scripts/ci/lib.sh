#!/usr/bin/env bash
# lib.sh — helpers shared by the scripts/ci checks. Source it, do not run it.
#
# Every check script is meant to be runnable by hand, so failures are
# collected and reported together instead of aborting on the first one:
# `ci_fail` records, `ci_finish` sets the exit status.
#
# Bash-style constraint (project convention, same as corpus/generators): no
# command substitution, no backticks, no heredocs. Values that would
# normally be captured with `$(...)` are written to a temp file and read
# back with `read`, so helpers hand their result back in a CI_* variable.

ci_failures=0
CI_LABEL="ci"
CI_TMP=""
CI_COUNT=0
CI_SIZE=0

ci_cleanup() {
    if [ -n "${CI_TMP}" ]; then
        rm -rf "${CI_TMP}"
    fi
}

# ci_init <label>: scratch dir, cleanup trap, and the label used in output.
# `mkdir` without -p is atomic and refuses an existing path, which is the
# property we want from a predictable name (mktemp would need command
# substitution to read its stdout).
ci_init() {
    CI_LABEL="$1"
    # PDFs are byte streams that are not valid text in any encoding. In a
    # UTF-8 locale, grep and sed interpret them as characters and can skip
    # or mis-split a line containing an invalid sequence, which differs
    # between the GNU and BSD tools. The C locale makes every match
    # byte-oriented and identical on both.
    export LC_ALL=C
    CI_TMP="${TMPDIR:-/tmp}/shellac-ci-${CI_LABEL}.$$"
    mkdir "${CI_TMP}"
    trap ci_cleanup EXIT
}

ci_pass() { printf 'ok    %s\n' "$*"; }

# Progress detail that is neither a pass nor a failure.
ci_info() { printf 'info  %s\n' "$*"; }

ci_fail() {
    printf 'FAIL  %s\n' "$*" >&2
    ci_failures=$((ci_failures + 1))
}

# ci_expect_eq <label> <want> <got>
ci_expect_eq() {
    if [ "$2" = "$3" ]; then
        ci_pass "$1 = $2"
    else
        ci_fail "$1: want [$2], got [$3]"
    fi
}

# ci_expect_contains <label> <file> <fixed string>
ci_expect_contains() {
    if grep -q -F -- "$3" "$2"; then
        ci_pass "$1: $3"
    else
        ci_fail "$1: expected to find [$3]"
    fi
}

# ci_count <file> <extended regex> -> CI_COUNT (occurrences, not lines)
ci_count() {
    { grep -a -o -E -- "$2" "$1" || true; } | wc -l > "${CI_TMP}/ci-count.txt"
    read -r CI_COUNT < "${CI_TMP}/ci-count.txt"
}

# ci_size <file> -> CI_SIZE. `wc -c` avoids the stat -f%z / -c%s split
# between macOS and Linux.
ci_size() {
    wc -c < "$1" > "${CI_TMP}/ci-size.txt"
    read -r CI_SIZE < "${CI_TMP}/ci-size.txt"
}

ci_require_tools() {
    for tool in "$@"; do
        if ! command -v "${tool}" >/dev/null 2>&1; then
            printf '%s: required tool not on PATH: %s\n' "${CI_LABEL}" "${tool}" >&2
            exit 1
        fi
    done
}

ci_require_file() {
    if [ ! -f "$1" ]; then
        printf '%s: required file not found: %s\n' "${CI_LABEL}" "$1" >&2
        exit 1
    fi
}

# ci_qdf <src pdf> <dst pdf>: expand to QDF form (streams uncompressed,
# object streams disabled) so plain-text greps see the whole document.
# qpdf exits 3 on warnings, which several fixtures produce and which is not
# a reason to fail a content audit; only >3 or 2 is a real error.
ci_qdf() {
    local rc=0
    if qpdf --qdf --object-streams=disable "$1" "$2" > "${CI_TMP}/qpdf.log" 2>&1; then
        rc=0
    else
        rc=$?
    fi
    if [ "${rc}" -ne 0 ] && [ "${rc}" -ne 3 ]; then
        ci_fail "qpdf --qdf failed on $1 (exit ${rc}); see ${CI_TMP}/qpdf.log"
        return 1
    fi
    return 0
}

# ci_json <src pdf> <dst json>: qpdf's structural JSON. Unlike the QDF text
# form this carries no stream payloads, so binary image data cannot produce
# spurious matches or hide content, and qpdf has already decoded every
# string (hex strings, escapes, and strings split across lines all arrive as
# one JSON value). Warnings (exit 3) are tolerated as in ci_qdf.
ci_json() {
    local rc=0
    if qpdf --json "$1" > "$2" 2> "${CI_TMP}/qpdf-json.log"; then
        rc=0
    else
        rc=$?
    fi
    if [ "${rc}" -ne 0 ] && [ "${rc}" -ne 3 ]; then
        ci_fail "qpdf --json failed on $1 (exit ${rc}); see ${CI_TMP}/qpdf-json.log"
        return 1
    fi
    return 0
}

ci_finish() {
    if [ "${ci_failures}" -ne 0 ]; then
        printf '%s: %d check(s) failed\n' "${CI_LABEL}" "${ci_failures}" >&2
        exit 1
    fi
    printf '%s: all checks passed\n' "${CI_LABEL}"
}
