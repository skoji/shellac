#!/usr/bin/env bash
# assemble-samples.sh — put the corpus the matrix runs over into one
# directory: the generated samples plus the committed fixtures.
#
# The matrix takes a single samples directory, but the corpus arrives from
# two places: six samples are built by the generators (see
# check-generated-samples.sh) and five are committed under corpus/fixtures.
# Both sets are named here rather than globbed, which is the point of this
# script. A glob over the generated directory would have taken S1 -- the
# input every generator except S3 derives from, and a copy of it is left
# behind there -- as though it were generated output, and would have gone
# on succeeding while a generator quietly stopped producing something. What
# the corpus consists of is a decision, so it is written down and checked
# rather than inferred from a directory listing.
#
# The two directories are kept apart -- CI caches the generated one, and the
# committed fixtures are tens of megabytes that come from the checkout on
# every run, so putting them in the cache would pay to move bytes that are
# already there.
#
# Usage: bash scripts/ci/assemble-samples.sh <generated dir> <samples dir>
#   generated dir  where check-generated-samples.sh built S3 and S8..S12
#   samples dir    created if absent; receives every corpus sample

set -euo pipefail

if [ "$#" -ne 2 ]; then
    printf 'usage: bash scripts/ci/assemble-samples.sh <generated dir> <samples dir>\n' >&2
    exit 2
fi

script_dir="${BASH_SOURCE[0]%/*}"
# shellcheck source=scripts/ci/lib.sh
. "${script_dir}/lib.sh"

ci_init "assemble-samples"

repo_root="${script_dir}/../.."
fixtures_dir="${repo_root}/corpus/fixtures"

# What the corpus is, by name. The one list; both CI jobs reach it through
# this script. The generated names are the ones the generators actually
# write, so a rename there fails here rather than silently shrinking the
# corpus the matrix runs over.
GENERATED_SAMPLES=(
    S3.pdf
    S8.pdf
    S9-rc4-empty-user.pdf
    S10-aes256-empty-user.pdf
    S11-password-required.pdf
    S12-annotations-restricted.pdf
)
COMMITTED_FIXTURES=(S1.pdf S2.pdf S4.pdf S5.pdf S7.pdf)

generated="$1"
samples="$2"

if [ ! -d "${generated}" ]; then
    printf '%s: generated samples directory not found: %s\n' "${CI_LABEL}" "${generated}" >&2
    exit 1
fi
if [ "${generated%/}" = "${samples%/}" ]; then
    printf '%s: the generated and samples directories must differ\n' "${CI_LABEL}" >&2
    exit 1
fi

mkdir -p "${samples}"

# Check the generated set before copying anything. `cp` of a name that is
# not there aborts the script under `set -e` with the shell's own wording,
# which names one file and stops; collecting the whole list first says which
# generator did not run.
for name in "${GENERATED_SAMPLES[@]}"; do
    if [ ! -f "${generated}/${name}" ]; then
        ci_fail "generated sample not in ${generated}: ${name}"
    fi
done
for name in "${COMMITTED_FIXTURES[@]}"; do
    if [ ! -f "${fixtures_dir}/${name}" ]; then
        ci_fail "committed fixture not in ${fixtures_dir}: ${name}"
    fi
done
if [ "${ci_failures}" -ne 0 ]; then
    ci_finish
fi

for name in "${GENERATED_SAMPLES[@]}"; do
    cp "${generated}/${name}" "${samples}/${name}"
done
for name in "${COMMITTED_FIXTURES[@]}"; do
    cp "${fixtures_dir}/${name}" "${samples}/${name}"
done

for name in "${GENERATED_SAMPLES[@]}" "${COMMITTED_FIXTURES[@]}"; do
    if [ -f "${samples}/${name}" ]; then
        ci_pass "in place: ${name}"
    else
        ci_fail "did not reach ${samples}: ${name}"
    fi
done

# An exact count, not a lower bound: a samples directory reused across runs
# can hold a PDF from an older corpus, and the matrix would run over it as
# if it belonged.
expected_count=$((${#GENERATED_SAMPLES[@]} + ${#COMMITTED_FIXTURES[@]}))
{ ls "${samples}"/*.pdf || true; } | wc -l > "${CI_TMP}/sample-count.txt"
read -r sample_count < "${CI_TMP}/sample-count.txt"
ci_expect_eq "PDFs in ${samples}" "${expected_count}" "${sample_count}"

ci_finish
