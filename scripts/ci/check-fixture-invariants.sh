#!/usr/bin/env bash
# check-fixture-invariants.sh — assert the structural properties that make
# S4 and S5 worth committing.
#
# S4 is the corpus's incremental-save reference and was produced by a
# generator that is not part of this repository (see corpus/README.md), so
# its provenance cannot be re-derived here. What can be verified is what it
# claims to be, using qpdf alone:
#
#   S2 is a true byte prefix of S4, %%EOF appears 3 times, and the two
#   append-mode saves contributed exactly one each of Highlight, Underline,
#   Text and Popup.
#
# S5 is the corpus's only classic-xref sample, so the assertions cover the
# cross-reference form as well as its single seeded annotation.
#
# Needs only qpdf; runs on any platform.
#
# Usage: bash scripts/ci/check-fixture-invariants.sh

set -euo pipefail

script_dir="${BASH_SOURCE[0]%/*}"
# shellcheck source=scripts/ci/lib.sh
. "${script_dir}/lib.sh"

ci_init "check-fixture-invariants"
ci_require_tools qpdf grep sed cmp head

repo_root="${script_dir}/../.."
fixtures="${repo_root}/corpus/fixtures"

s2="${fixtures}/S2.pdf"
s4="${fixtures}/S4.pdf"
s5="${fixtures}/S5.pdf"
for f in "${s2}" "${s4}" "${s5}"; do
    ci_require_file "${f}"
done

# annot_subtypes <expanded pdf> <output file>: one annotation subtype name
# per line. Extracting the whole token and matching it on its own line keeps
# /Text from also counting a /TrueType or /Type1 entry.
annot_subtypes() {
    { grep -a -o -E '/Subtype */[A-Za-z0-9]+' "$1" || true; } \
        | sed -E 's|.*/||' > "$2"
}

# count_exact <file> <line> -> CI_COUNT
count_exact() {
    { grep -c -x -F -- "$2" "$1" || true; } > "${CI_TMP}/exact.txt"
    read -r CI_COUNT < "${CI_TMP}/exact.txt"
}

# --- S4: S2 is a true byte prefix ----------------------------------------
ci_size "${s2}"
s2_size="${CI_SIZE}"
ci_size "${s4}"
s4_size="${CI_SIZE}"

if [ "${s4_size}" -gt "${s2_size}" ]; then
    ci_pass "S4 is larger than S2 (${s2_size} -> ${s4_size} bytes)"
else
    ci_fail "S4 (${s4_size} bytes) is not larger than S2 (${s2_size} bytes)"
fi

head -c "${s2_size}" "${s4}" > "${CI_TMP}/s4-prefix.pdf"
if cmp -s "${s2}" "${CI_TMP}/s4-prefix.pdf"; then
    ci_pass "S4: the leading ${s2_size} bytes are byte-identical to S2"
else
    ci_fail "S4: the leading ${s2_size} bytes differ from S2"
fi

# --- S4: revision count and annotation set -------------------------------
ci_count "${s4}" '%%EOF'
ci_expect_eq "S4: %%EOF markers" "3" "${CI_COUNT}"

ci_qdf "${s4}" "${CI_TMP}/s4-qdf.pdf"
annot_subtypes "${CI_TMP}/s4-qdf.pdf" "${CI_TMP}/s4-subtypes.txt"
for subtype in Highlight Underline Text Popup; do
    count_exact "${CI_TMP}/s4-subtypes.txt" "${subtype}"
    ci_expect_eq "S4: ${subtype} annotations" "1" "${CI_COUNT}"
done

# --- S5: classic cross-reference table ------------------------------------
ci_count "${s5}" '^xref$'
ci_expect_eq "S5: classic xref sections" "1" "${CI_COUNT}"
ci_count "${s5}" '^trailer$'
ci_expect_eq "S5: trailer dictionaries" "1" "${CI_COUNT}"
ci_count "${s5}" '/Type */XRef'
ci_expect_eq "S5: cross-reference streams" "0" "${CI_COUNT}"

ci_count "${s5}" '%%EOF'
ci_expect_eq "S5: %%EOF markers" "1" "${CI_COUNT}"

# --- S5: the seeded annotation --------------------------------------------
ci_qdf "${s5}" "${CI_TMP}/s5-qdf.pdf"
annot_subtypes "${CI_TMP}/s5-qdf.pdf" "${CI_TMP}/s5-subtypes.txt"
count_exact "${CI_TMP}/s5-subtypes.txt" "Highlight"
ci_expect_eq "S5: Highlight annotations" "1" "${CI_COUNT}"

ci_count "${CI_TMP}/s5-qdf.pdf" '/T \(shellac-corpus\)'
ci_expect_eq "S5: annotations authored by shellac-corpus" "1" "${CI_COUNT}"

ci_finish
