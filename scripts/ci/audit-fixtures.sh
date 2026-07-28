#!/usr/bin/env bash
# audit-fixtures.sh — content audit of the committed corpus fixtures.
#
# The fixtures are binary blobs that ship in the repository, so what they
# carry cannot be reviewed by reading the diff. This script asserts the
# properties a reviewer would otherwise have to take on trust:
#
#   1. document metadata (Title / Author / Creator) is empty
#   2. every string the document carries is on a published allowlist --
#      fixtures must not carry author-identifying strings, and asserting
#      against an allowlist states that as a property of the file rather
#      than as a list of specific names someone thought to look for
#   3. no local filesystem paths are embedded anywhere in the bytes
#   4. XMP metadata carries no identity properties
#
# Check 2 reads `qpdf --json` and covers every string in the document's
# object structure, whatever key or object it sits under. That is what makes
# it complete: qpdf has already parsed the file, so a value arrives as one
# JSON value however it was written -- hex string, escaped, split across
# lines, or reached through an indirect reference -- and there is no key
# list to omit something from. In that JSON a string is exactly a value
# prefixed `u:` (text) or `b:` (bytes); names, numbers and references such
# as "5 0 R" carry no prefix and are not strings.
#
# Stream payloads are absent from that JSON, so binary image data can
# neither hide a string nor forge a key. Page text lives in streams and is
# deliberately not audited -- the corpus exists to carry real book text --
# but the two byte-level checks below still cover streams for the things
# that must not appear anywhere.
#
# Runs on any platform with qpdf and poppler. Generated samples (S3, S8..S12)
# are not audited here: they are not committed, and their generators derive
# them from S1, which is.
#
# Usage: bash scripts/ci/audit-fixtures.sh

set -euo pipefail

script_dir="${BASH_SOURCE[0]%/*}"
# shellcheck source=scripts/ci/lib.sh
. "${script_dir}/lib.sh"

ci_init "audit-fixtures"
ci_require_tools qpdf pdfinfo grep sed

repo_root="${script_dir}/../.."
fixtures_dir="${repo_root}/corpus/fixtures"
allowlist="${script_dir}/fixture-string-allowlist.txt"
ci_require_file "${allowlist}"

# The committed fixtures, named explicitly so a stray locally-generated
# sample in the same directory neither joins nor masks the audit.
committed="S1 S2 S4 S5 S7"

# A PDF string in qpdf's JSON: the `u:`/`b:` prefix, then the value with
# JSON escapes.
string_re='"(u|b):(\\.|[^"\\])*"'

# Strip comments and blank lines once, so the per-value lookup is a plain
# fixed-string line match.
{ grep -v -E '^[[:space:]]*(#|$)' "${allowlist}" || true; } > "${CI_TMP}/allowed.txt"

for name in ${committed}; do
    pdf="${fixtures_dir}/${name}.pdf"
    ci_require_file "${pdf}"

    # (1) Empty document metadata. pdfinfo omits a key entirely when it is
    # absent, so "no line with a non-blank value" covers both absent and
    # present-but-empty.
    pdfinfo "${pdf}" > "${CI_TMP}/info.txt" 2>/dev/null
    ci_count "${CI_TMP}/info.txt" '^(Title|Author|Creator):[[:space:]]*[^[:space:]]'
    ci_expect_eq "${name}: non-empty Title/Author/Creator entries" "0" "${CI_COUNT}"

    # (2) Every string the document carries must be allowlisted.
    doc_json="${CI_TMP}/${name}.json"
    if ! ci_json "${pdf}" "${doc_json}"; then
        continue
    fi

    { grep -a -o -E -- "${string_re}" "${doc_json}" || true; } \
        | sed -E 's/^"//; s/"$//' \
        | sort -u > "${CI_TMP}/${name}-strings.txt"
    { grep -c '' "${CI_TMP}/${name}-strings.txt" || true; } > "${CI_TMP}/nvalues.txt"
    read -r n_strings < "${CI_TMP}/nvalues.txt"

    # A document with no strings at all would pass check 2 vacuously. Every
    # PDF has at least a trailer /ID or a /Producer, so treat an empty
    # extraction as a broken scan rather than a clean document.
    if [ "${n_strings}" -eq 0 ]; then
        ci_fail "${name}: no strings were extracted from qpdf --json; the scan is not working"
    fi
    ci_info "${name}: ${n_strings} distinct string(s) to check"

    failures_before="${ci_failures}"
    while IFS= read -r value; do
        if [ -z "${value}" ]; then
            continue
        fi
        if grep -q -x -F -- "${value}" "${CI_TMP}/allowed.txt"; then
            continue
        fi
        # The value itself is not printed: if this trips, the string is
        # already committed and the point is to get it removed, not to
        # reprint it in a build log. Inspect it locally with
        #   qpdf --json corpus/fixtures/<name>.pdf
        ci_fail "${name}: carries a string that is not on the allowlist (${#value} characters, including the type prefix); inspect it locally, then either fix the fixture or extend scripts/ci/fixture-string-allowlist.txt"
    done < "${CI_TMP}/${name}-strings.txt"

    if [ "${ci_failures}" -eq "${failures_before}" ]; then
        ci_pass "${name}: all ${n_strings} strings are allowlisted"
    fi

    # The QDF expansion is still needed below: unlike the JSON, it contains
    # the stream payloads, which is where an embedded path or an XMP packet
    # would live.
    expanded="${CI_TMP}/${name}-qdf.pdf"
    if ! ci_qdf "${pdf}" "${expanded}"; then
        continue
    fi

    # (3) No local filesystem paths, in the raw bytes or the expansion.
    for target in "${pdf}" "${expanded}"; do
        ci_count "${target}" '/(Users|home)/[A-Za-z0-9._-]'
        ci_expect_eq "${name}: local path strings in ${target##*/}" "0" "${CI_COUNT}"
    done

    # (4) XMP metadata carries no identity properties.
    ci_count "${expanded}" '<(dc:creator|dc:title|dc:rights|xmp:CreatorTool|pdf:Author)'
    ci_expect_eq "${name}: identity properties in XMP" "0" "${CI_COUNT}"
done

ci_finish
