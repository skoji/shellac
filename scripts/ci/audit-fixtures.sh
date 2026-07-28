#!/usr/bin/env bash
# audit-fixtures.sh — content audit of the committed corpus fixtures.
#
# The fixtures are binary blobs that ship in the repository, so what they
# carry cannot be reviewed by reading the diff. This script asserts the
# properties a reviewer would otherwise have to take on trust:
#
#   1. document metadata (Title / Author / Creator) is empty
#   2. every string under an identity-bearing key is on a published
#      allowlist -- fixtures must not carry author-identifying strings, and
#      asserting against an allowlist states that as a property of the file
#      rather than as a list of specific names someone thought to look for
#   3. no local filesystem paths are embedded anywhere in the bytes
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

# Keys whose string values identify a person or a tool. Dates (/M, /ModDate,
# /CreationDate), font CID system info (/Registry, /Ordering) and appearance
# strings (/DA) are deliberately out of scope: they cannot carry a name.
id_keys='(T|Contents|Author|Title|Creator|Producer|Subject|Keywords)'

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

    # (2) Allowlisted strings only. The audit runs over the QDF expansion so
    # that strings inside compressed object streams are visible as text.
    expanded="${CI_TMP}/${name}-qdf.pdf"
    if ! ci_qdf "${pdf}" "${expanded}"; then
        continue
    fi

    # Literal strings: `\(` and `\)` are escapes inside the value, so the
    # value pattern accepts any escaped character or any non-escape,
    # non-closing character.
    value_re="/${id_keys} *\\((\\\\.|[^\\\\)])*\\)"

    # Completeness guards for the extraction below. A hex string, or a
    # literal string broken across lines, would not match the value pattern
    # and would otherwise pass unseen; requiring every use of an identity
    # key to yield exactly one readable value turns any such encoding into a
    # failure a human has to look at.
    ci_count "${expanded}" "/${id_keys} *<[^<]"
    hex_values="${CI_COUNT}"
    ci_expect_eq "${name}: hex-encoded values under identity keys" "0" "${hex_values}"

    ci_count "${expanded}" "/${id_keys} *[(<]"
    key_uses="${CI_COUNT}"
    ci_count "${expanded}" "${value_re}"
    ci_expect_eq "${name}: readable values per identity-key use" "${key_uses}" "${CI_COUNT}"

    { grep -a -o -E -- "${value_re}" "${expanded}" || true; } \
        | sort -u > "${CI_TMP}/${name}-strings.txt"
    { grep -c '' "${CI_TMP}/${name}-strings.txt" || true; } > "${CI_TMP}/nvalues.txt"
    read -r literal_values < "${CI_TMP}/nvalues.txt"
    ci_info "${name}: checking ${literal_values} distinct identity-key value(s) against the allowlist"

    failures_before="${ci_failures}"
    while IFS= read -r entry; do
        if [ -z "${entry}" ]; then
            continue
        fi
        key="${entry%%\(*}"
        key="${key% }"
        value="${entry#*\(}"
        value="${value%\)}"
        if [ -z "${value}" ]; then
            continue
        fi
        if grep -q -x -F -- "${value}" "${CI_TMP}/allowed.txt"; then
            continue
        fi
        # The value itself is not printed: if this trips, the string is
        # already committed and the point is to get it removed, not to
        # reprint it in a build log. Inspect it locally with
        #   qpdf --qdf --object-streams=disable corpus/fixtures/<name>.pdf -
        ci_fail "${name}: ${key} carries a value that is not on the allowlist (${#value} characters); inspect it locally, then either fix the fixture or extend scripts/ci/fixture-string-allowlist.txt"
    done < "${CI_TMP}/${name}-strings.txt"

    if [ "${ci_failures}" -eq "${failures_before}" ]; then
        ci_pass "${name}: every identity-key value is allowlisted"
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
