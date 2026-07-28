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
#   S9   RC4 128-bit, opens with an empty user password
#   S10  AES-256 (AESV3 R6), opens with an empty user password
#   S11  a user password is required, so it cannot be opened without one
#   S12  AES-256 with the annotate/modify bits cleared in /P
#
# Encryption parameters are read from `qpdf --json`, whose `encrypt` section
# reports the same information as `--show-encryption` in a form that can be
# asserted key by key. Whitespace is stripped first so the assertions do not
# depend on how qpdf indents its output.
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
ci_count "${CI_TMP}/s8.json" '"/Rotate":90'
ci_expect_eq "S8: /Rotate 90 entries" "1" "${CI_COUNT}"
ci_count "${CI_TMP}/s8.json" '"/Rotate":(0|180|270)'
ci_expect_eq "S8: other /Rotate values" "0" "${CI_COUNT}"

# --- S9 -------------------------------------------------------------------
s9="${work_dir}/S9-rc4-empty-user.pdf"
ci_require_file "${s9}"
encrypt_json "${s9}" "${CI_TMP}/s9.json"
ci_expect_contains "S9: encrypted" "${CI_TMP}/s9.json" '"encrypted":true'
ci_expect_contains "S9: RC4" "${CI_TMP}/s9.json" '"filemethod":"RC4"'
ci_expect_contains "S9: 128-bit key" "${CI_TMP}/s9.json" '"bits":128'
ci_expect_contains "S9: revision 3" "${CI_TMP}/s9.json" '"R":3'
# No password was supplied, so a match means the user password is empty.
ci_expect_contains "S9: opens with an empty user password" "${CI_TMP}/s9.json" '"userpasswordmatched":true'

# --- S10 ------------------------------------------------------------------
s10="${work_dir}/S10-aes256-empty-user.pdf"
ci_require_file "${s10}"
encrypt_json "${s10}" "${CI_TMP}/s10.json"
ci_expect_contains "S10: AESv3" "${CI_TMP}/s10.json" '"filemethod":"AESv3"'
ci_expect_contains "S10: 256-bit key" "${CI_TMP}/s10.json" '"bits":256'
ci_expect_contains "S10: revision 6" "${CI_TMP}/s10.json" '"R":6'
ci_expect_contains "S10: opens with an empty user password" "${CI_TMP}/s10.json" '"userpasswordmatched":true'
ci_expect_contains "S10: annotations permitted" "${CI_TMP}/s10.json" '"modifyannotations":true'

# --- S11 ------------------------------------------------------------------
s11="${work_dir}/S11-password-required.pdf"
ci_require_file "${s11}"
if qpdf --check "${s11}" > "${CI_TMP}/s11-check.txt" 2>&1; then
    ci_fail "S11: qpdf --check succeeded without a password; the user password is not required"
else
    ci_pass "S11: cannot be opened without the user password"
fi

# --- S12 ------------------------------------------------------------------
s12="${work_dir}/S12-annotations-restricted.pdf"
ci_require_file "${s12}"
encrypt_json "${s12}" "${CI_TMP}/s12.json"
ci_expect_contains "S12: AESv3" "${CI_TMP}/s12.json" '"filemethod":"AESv3"'
ci_expect_contains "S12: 256-bit key" "${CI_TMP}/s12.json" '"bits":256'
ci_expect_contains "S12: opens with an empty user password" "${CI_TMP}/s12.json" '"userpasswordmatched":true'
ci_expect_contains "S12: annotations not permitted" "${CI_TMP}/s12.json" '"modifyannotations":false'
ci_expect_contains "S12: modification not permitted" "${CI_TMP}/s12.json" '"modify":false'

ci_finish
