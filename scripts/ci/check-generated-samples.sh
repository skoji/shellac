#!/usr/bin/env bash
# check-generated-samples.sh — run the corpus generators and assert the
# structural properties the harness relies on.
#
# S3 and S8..S12 are not committed: they are produced from the generators
# (and, for all but S3, from the committed S1). That keeps the repository
# small, but it also means a generator that silently starts emitting
# something different would go unnoticed. This script regenerates them and
# asserts what each sample exists to exercise:
#
#   S3   10 pages; page 1 has no text layer, page 2 does (the "no anchor on
#        this page" path)
#   S8   /Rotate 90 (the rotation-aware coordinate transform)
#   S9   RC4 128-bit (V2 R3), opens with an empty user password
#   S10  AES-256 (V5 R6, AESv3), opens with an empty user password
#   S11  a user password is required, so it cannot be opened without one
#   S12  AES-256 with the annotate/modify bits cleared in /P
#
# Encryption parameters are read from `qpdf --json`, whose `encrypt` section
# reports the same information as `--show-encryption` in a form that can be
# asserted key by key. Whitespace is stripped first so the assertions do not
# depend on how qpdf indents its output.
#
# Which key means what (from `qpdf --json-help`): `method` is the overall
# encryption method and `streammethod` / `stringmethod` are the methods for
# streams and strings. `filemethod` is *not* the document's method -- it
# applies to embedded attachments -- so it is not what these assertions use.
#
# Whether a password is needed is read from `qpdf --requires-password`,
# which exists for the question and answers it by exit code:
#   0 = a password other than the one supplied is required
#   2 = the file is not encrypted
#   3 = encrypted, and the supplied password (here: none) is correct
# Reading the code rather than "did some command fail" keeps a corrupt file
# from being mistaken for a password-protected one.
#
# Usage: bash scripts/ci/check-generated-samples.sh [work_dir] [s3_pages]
#   work_dir  where samples are generated; defaults to a temp dir that is
#             removed on exit. Pass a path to keep them (the generators are
#             idempotent, so a rerun skips what is already there -- useful
#             locally, where S3 takes minutes to build).
#   s3_pages  total S3 page count including the cover, defaults to 10

set -euo pipefail

script_dir="${BASH_SOURCE[0]%/*}"
# shellcheck source=scripts/ci/lib.sh
. "${script_dir}/lib.sh"

ci_init "check-generated-samples"
ci_require_tools qpdf pdfinfo pdftotext img2pdf ocrmypdf tesseract

repo_root="${script_dir}/../.."
generators="${repo_root}/corpus/generators"

work_dir="${1:-${CI_TMP}/samples}"
s3_pages="${2:-10}"
mkdir -p "${work_dir}"

# Everything except S3 is derived from S1, so the generators need it here.
ci_require_file "${repo_root}/corpus/fixtures/S1.pdf"
if [ ! -f "${work_dir}/S1.pdf" ]; then
    cp "${repo_root}/corpus/fixtures/S1.pdf" "${work_dir}/S1.pdf"
fi

bash "${generators}/generate_s3.sh" "${work_dir}" "${s3_pages}"
bash "${generators}/generate_s8.sh" "${work_dir}"
bash "${generators}/generate_encrypted_fixtures.sh" "${work_dir}"

# encrypt_json <sample file> <output file>: qpdf's encryption report,
# whitespace removed.
encrypt_json() {
    qpdf --json --json-key=encrypt "$1" > "${CI_TMP}/enc-raw.json"
    tr -d ' \t\n' < "${CI_TMP}/enc-raw.json" > "$2"
}

# password_status <sample file> -> CI_PW_STATUS (the --requires-password
# exit code; see the header for what each value means).
password_status() {
    local rc=0
    if qpdf --requires-password "$1" > /dev/null 2>&1; then
        rc=0
    else
        rc=$?
    fi
    CI_PW_STATUS="${rc}"
}

# expect_encryption <label> <json> <V> <R> <bits> <method>
expect_encryption() {
    ci_expect_contains "$1: encrypted" "$2" '"encrypted":true'
    ci_expect_contains "$1: V" "$2" "\"V\":$3"
    ci_expect_contains "$1: R" "$2" "\"R\":$4"
    ci_expect_contains "$1: key bits" "$2" "\"bits\":$5"
    ci_expect_contains "$1: overall method" "$2" "\"method\":\"$6\""
    ci_expect_contains "$1: stream method" "$2" "\"streammethod\":\"$6\""
    ci_expect_contains "$1: string method" "$2" "\"stringmethod\":\"$6\""
    # Which password matched: --requires-password reports that the supplied
    # credentials opened the file, not which of the two they matched, so the
    # empty *user* password is asserted here.
    ci_expect_contains "$1: empty user password matches" "$2" '"userpasswordmatched":true'
}

# --- S3 -------------------------------------------------------------------
s3="${work_dir}/S3.pdf"
ci_require_file "${s3}"

pdfinfo "${s3}" > "${CI_TMP}/s3-info.txt"
{ grep -E '^Pages:' "${CI_TMP}/s3-info.txt" || true; } | sed -E 's/[^0-9]//g' > "${CI_TMP}/s3-pages.txt"
read -r s3_actual_pages < "${CI_TMP}/s3-pages.txt"
ci_expect_eq "S3: page count" "${s3_pages}" "${s3_actual_pages}"

pdftotext -f 1 -l 1 "${s3}" - > "${CI_TMP}/s3-p1.txt"
tr -d '[:space:]' < "${CI_TMP}/s3-p1.txt" > "${CI_TMP}/s3-p1-stripped.txt"
ci_size "${CI_TMP}/s3-p1-stripped.txt"
ci_expect_eq "S3: extractable characters on page 1" "0" "${CI_SIZE}"

pdftotext -f 2 -l 2 "${s3}" - > "${CI_TMP}/s3-p2.txt"
tr -d '[:space:]' < "${CI_TMP}/s3-p2.txt" > "${CI_TMP}/s3-p2-stripped.txt"
ci_size "${CI_TMP}/s3-p2-stripped.txt"
if [ "${CI_SIZE}" -gt 0 ]; then
    ci_pass "S3: page 2 has a text layer (${CI_SIZE} characters)"
else
    ci_fail "S3: page 2 has no text layer"
fi

# --- S8 -------------------------------------------------------------------
s8="${work_dir}/S8.pdf"
ci_require_file "${s8}"
qpdf --json "${s8}" > "${CI_TMP}/s8-raw.json"
tr -d ' \t\n' < "${CI_TMP}/s8-raw.json" > "${CI_TMP}/s8.json"
# Count every /Rotate entry whatever its value, then require them all to be
# 90: enumerating the values that would be wrong would miss the ones nobody
# listed (-90, 45, ...). Whitespace is already stripped, so a JSON number
# ends at the following comma or closing brace; matching that terminator is
# what keeps 90 from also accepting 900 or 90.5.
ci_count "${CI_TMP}/s8.json" '"/Rotate":-?[0-9.]+'
s8_rotate_total="${CI_COUNT}"
ci_count "${CI_TMP}/s8.json" '"/Rotate":90[,}]'
s8_rotate_90="${CI_COUNT}"
if [ "${s8_rotate_total}" -ge 1 ]; then
    ci_pass "S8: /Rotate entries present (${s8_rotate_total})"
else
    ci_fail "S8: no /Rotate entry at all"
fi
ci_expect_eq "S8: /Rotate entries that are 90" "${s8_rotate_total}" "${s8_rotate_90}"

# --- S9 -------------------------------------------------------------------
s9="${work_dir}/S9-rc4-empty-user.pdf"
ci_require_file "${s9}"
encrypt_json "${s9}" "${CI_TMP}/s9.json"
expect_encryption "S9" "${CI_TMP}/s9.json" 2 3 128 "RC4"
ci_expect_contains "S9: annotations permitted" "${CI_TMP}/s9.json" '"modifyannotations":true'
# No password was supplied, so "correct password" means the empty one.
password_status "${s9}"
ci_expect_eq "S9: --requires-password status (3 = opens as supplied)" "3" "${CI_PW_STATUS}"

# --- S10 ------------------------------------------------------------------
s10="${work_dir}/S10-aes256-empty-user.pdf"
ci_require_file "${s10}"
encrypt_json "${s10}" "${CI_TMP}/s10.json"
expect_encryption "S10" "${CI_TMP}/s10.json" 5 6 256 "AESv3"
ci_expect_contains "S10: annotations permitted" "${CI_TMP}/s10.json" '"modifyannotations":true'
password_status "${s10}"
ci_expect_eq "S10: --requires-password status (3 = opens as supplied)" "3" "${CI_PW_STATUS}"

# --- S11 ------------------------------------------------------------------
# Asserting the status code, not merely that something failed: status 0 says
# a different password is required, which a damaged file would not report.
s11="${work_dir}/S11-password-required.pdf"
ci_require_file "${s11}"
password_status "${s11}"
ci_expect_eq "S11: --requires-password status (0 = a password is required)" "0" "${CI_PW_STATUS}"

# --- S12 ------------------------------------------------------------------
s12="${work_dir}/S12-annotations-restricted.pdf"
ci_require_file "${s12}"
encrypt_json "${s12}" "${CI_TMP}/s12.json"
expect_encryption "S12" "${CI_TMP}/s12.json" 5 6 256 "AESv3"
password_status "${s12}"
ci_expect_eq "S12: --requires-password status (3 = opens as supplied)" "3" "${CI_PW_STATUS}"
ci_expect_contains "S12: annotations not permitted" "${CI_TMP}/s12.json" '"modifyannotations":false'
ci_expect_contains "S12: modification not permitted" "${CI_TMP}/s12.json" '"modify":false'

ci_finish
