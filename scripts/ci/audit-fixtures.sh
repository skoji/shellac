#!/usr/bin/env bash
# audit-fixtures.sh — content audit of the committed corpus fixtures.
#
# The fixtures are binary blobs that ship in the repository, so what they
# carry cannot be reviewed by reading the diff. This script asserts the
# properties a reviewer would otherwise have to take on trust.
#
# It works in two layers, which answer different questions.
#
# Layer 1, identity: the committed fixtures hash to the values recorded in
# fixture-checksums.txt. This freezes them byte for byte and subsumes every
# structural property the content checks derive from the bytes, since none
# of those can change while a hash stays the same.
#
# Layer 2, content: what a fixture actually carries. These are the checks
# that matter when a fixture is legitimately regenerated -- the point at
# which layer 1 has to be updated and so cannot vouch for anything. Run
# them, read what they report, then update the manifest.
#
#   1. document metadata (Title / Author / Creator) is empty
#   2. every string the document carries, in every revision it ships, is on
#      a published allowlist -- fixtures must not carry author-identifying
#      strings, and asserting against an allowlist states that as a property
#      of the file rather than as a list of specific names someone thought
#      to look for
#   3. the file ships the number of revisions it is declared to ship, and
#      nothing but whitespace follows the last one
#   4. no local filesystem paths are embedded anywhere in the bytes
#   5. XMP metadata carries no identity properties
#
# What this is for: keeping personal information out of the corpus by
# accident -- an account name a tool wrote into a file, a local path baked
# into a generated sample. It is not a defence against someone who can edit
# this script, since anything here can be edited away; that is outside what
# CI can decide.
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
revisions="${script_dir}/fixture-revisions.txt"
ci_require_file "${revisions}"
checksums="${script_dir}/fixture-checksums.txt"
ci_require_file "${checksums}"

# --- Layer 1: the committed fixtures are the ones that were reviewed ------
# GNU coreutils ships sha256sum; macOS ships shasum. Comments are stripped
# first because the two disagree about non-checksum lines.
checksum_tool=""
if command -v sha256sum > /dev/null 2>&1; then
    checksum_tool="sha256sum"
elif command -v shasum > /dev/null 2>&1; then
    checksum_tool="shasum -a 256"
else
    printf 'audit-fixtures: no sha256sum or shasum on PATH\n' >&2
    exit 1
fi

{ grep -v -E '^[[:space:]]*(#|$)' "${checksums}" || true; } > "${CI_TMP}/checksums.txt"
if ( cd "${repo_root}" && ${checksum_tool} --check "${CI_TMP}/checksums.txt" ) \
    > "${CI_TMP}/checksum.log" 2>&1; then
    ci_pass "committed fixtures match fixture-checksums.txt"
else
    ci_fail "committed fixtures do not match fixture-checksums.txt; a fixture changed, or the manifest was not updated after a deliberate regeneration"
    cat "${CI_TMP}/checksum.log" >&2
fi

# The committed fixtures, named explicitly so a stray locally-generated
# sample in the same directory neither joins nor masks the audit.
committed="S1 S2 S4 S5 S7"

# A PDF string in qpdf's JSON: the `u:`/`b:` prefix, then the value with
# JSON escapes.
string_re='"(u|b):(\\.|[^"\\])*"'

# Strip comments and blank lines once, so the per-value lookup is a plain
# fixed-string line match.
{ grep -v -E '^[[:space:]]*(#|$)' "${allowlist}" || true; } > "${CI_TMP}/allowed.txt"
{ grep -v -E '^[[:space:]]*(#|$)' "${revisions}" || true; } > "${CI_TMP}/revisions.txt"

for name in ${committed}; do
    pdf="${fixtures_dir}/${name}.pdf"
    ci_require_file "${pdf}"

    # (1) Empty document metadata. pdfinfo omits a key entirely when it is
    # absent, so "no line with a non-blank value" covers both absent and
    # present-but-empty.
    pdfinfo "${pdf}" > "${CI_TMP}/info.txt" 2>/dev/null
    ci_count "${CI_TMP}/info.txt" '^(Title|Author|Creator):[[:space:]]*[^[:space:]]'
    ci_expect_eq "${name}: non-empty Title/Author/Creator entries" "0" "${CI_COUNT}"

    # (2) Every string the document carries must be allowlisted -- in every
    # revision it ships, not only the current one.
    #
    # An incrementally saved PDF keeps its earlier revisions in its leading
    # bytes, and qpdf --json reports the document as currently resolved: a
    # string that a later save superseded (the base file's /Producer, an
    # intermediate trailer /ID) is invisible there while still sitting in
    # the bytes that ship. So each revision is cut at its %%EOF and audited
    # as a document in its own right, and anything trailing the final marker
    # is checked separately below.
    : > "${CI_TMP}/${name}-raw-strings.txt"

    scan_strings() { # <pdf or prefix> <json out>
        { grep -a -o -E -- "${string_re}" "$2" || true; } \
            | sed -E 's/^"//; s/"$//' >> "${CI_TMP}/${name}-raw-strings.txt"
    }

    if ! ci_json "${pdf}" "${CI_TMP}/${name}.json"; then
        continue
    fi
    scan_strings "${pdf}" "${CI_TMP}/${name}.json"

    { grep -a -b -o '%%EOF' "${pdf}" || true; } | cut -d: -f1 > "${CI_TMP}/eof.txt"
    rev=0
    revs_scanned=0
    while read -r offset; do
        rev=$((rev + 1))
        # The revision ends with its end-of-file marker, five bytes long.
        end=$((offset + 5))
        prefix="${CI_TMP}/${name}-rev${rev}.pdf"
        head -c "${end}" "${pdf}" > "${prefix}"
        rc=0
        if qpdf --json "${prefix}" > "${CI_TMP}/rev.json" 2> "${CI_TMP}/rev.err"; then
            rc=0
        else
            rc=$?
        fi
        if [ "${rc}" -ne 0 ] && [ "${rc}" -ne 3 ]; then
            # A prefix qpdf cannot open is not a revision: an incremental
            # save always leaves a complete, openable document behind. The
            # one such boundary in this corpus is the first %%EOF of a
            # linearized file, which ends the first-page cross-reference
            # section rather than a saved revision.
            if [ "${rev}" -eq 1 ] && grep -q -a '/Linearized' "${pdf}"; then
                ci_info "${name}: boundary 1 is a linearization artifact, not a revision"
                continue
            fi
            ci_fail "${name}: revision ${rev} (${end} bytes) could not be opened by qpdf (exit ${rc}), so its strings cannot be audited"
            continue
        fi
        scan_strings "${prefix}" "${CI_TMP}/rev.json"
        revs_scanned=$((revs_scanned + 1))
    done < "${CI_TMP}/eof.txt"

    # The markers decide both where the revisions are and where the file
    # ends, so their number is pinned rather than taken from the file.
    # Otherwise appending a string and a further %%EOF would move "the end
    # of the file" past the appended bytes, which belong to no revision and
    # show up in no qpdf view.
    expected_eof=""
    while read -r declared_name declared_count; do
        if [ "${declared_name}" = "${name}" ]; then
            expected_eof="${declared_count}"
        fi
    done < "${CI_TMP}/revisions.txt"
    if [ -z "${expected_eof}" ]; then
        ci_fail "${name}: no expected %%EOF count is declared in scripts/ci/fixture-revisions.txt"
    else
        ci_expect_eq "${name}: %%EOF markers" "${expected_eof}" "${rev}"
    fi

    # Bytes after the final %%EOF belong to no revision, so nothing above
    # would look at them: qpdf reports the document, and appended bytes are
    # not part of it. Anything but trailing whitespace there is content this
    # audit cannot account for.
    if [ "${rev}" -gt 0 ]; then
        tail -1 "${CI_TMP}/eof.txt" > "${CI_TMP}/last-eof.txt"
        read -r last_offset < "${CI_TMP}/last-eof.txt"
        last_end=$((last_offset + 5))
        ci_size "${pdf}"
        trailing=$((CI_SIZE - last_end))
        if [ "${trailing}" -gt 0 ]; then
            tail -c "${trailing}" "${pdf}" > "${CI_TMP}/tail.bin"
            tr -d '[:space:]' < "${CI_TMP}/tail.bin" > "${CI_TMP}/tail-stripped.bin"
            ci_size "${CI_TMP}/tail-stripped.bin"
            ci_expect_eq "${name}: non-whitespace bytes after the final %%EOF" "0" "${CI_SIZE}"
        else
            ci_pass "${name}: nothing follows the final %%EOF"
        fi
    fi

    sort -u "${CI_TMP}/${name}-raw-strings.txt" > "${CI_TMP}/${name}-strings.txt"
    { grep -c '' "${CI_TMP}/${name}-strings.txt" || true; } > "${CI_TMP}/nvalues.txt"
    read -r n_strings < "${CI_TMP}/nvalues.txt"

    # A document with no strings at all would pass check 2 vacuously. Every
    # PDF has at least a trailer /ID or a /Producer, so treat an empty
    # extraction as a broken scan rather than a clean document.
    if [ "${n_strings}" -eq 0 ]; then
        ci_fail "${name}: no strings were extracted from qpdf --json; the scan is not working"
    fi
    if [ "${revs_scanned}" -eq 0 ]; then
        ci_fail "${name}: no revision could be audited"
    fi
    ci_info "${name}: ${revs_scanned} revision(s), ${n_strings} distinct string(s) to check"

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
