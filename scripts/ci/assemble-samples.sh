#!/usr/bin/env bash
# assemble-samples.sh — put the corpus the matrix runs over into one
# directory: the generated samples plus the committed fixtures.
#
# The matrix takes a single samples directory, but the corpus arrives from
# two places. Six samples are built by the generators (see
# check-generated-samples.sh) and five are committed under corpus/fixtures.
# Naming the committed ones here, rather than relying on whatever a
# generator happened to leave behind in its work directory, is the point of
# this script: S1 is the input every generator except S3 derives from, so it
# turns up in the generated directory as a by-product, and a corpus that
# depends on that by-product loses a sample the day the generators stop
# needing a copy of it.
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

# The committed corpus fixtures. The one list; both CI jobs reach it through
# this script.
COMMITTED_FIXTURES=(S1 S2 S4 S5 S7)

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

# Generated first, committed second: the generators work on a copy of S1, so
# whatever the work directory holds under that name is overwritten here by
# the fixture that is under version control.
cp "${generated}/"*.pdf "${samples}/"
for name in "${COMMITTED_FIXTURES[@]}"; do
    ci_require_file "${fixtures_dir}/${name}.pdf"
    cp "${fixtures_dir}/${name}.pdf" "${samples}/${name}.pdf"
done

for name in "${COMMITTED_FIXTURES[@]}"; do
    if [ -f "${samples}/${name}.pdf" ]; then
        ci_pass "committed fixture in place: ${name}.pdf"
    else
        ci_fail "committed fixture missing from ${samples}: ${name}.pdf"
    fi
done

{ ls "${samples}"/*.pdf || true; } | wc -l > "${CI_TMP}/sample-count.txt"
read -r sample_count < "${CI_TMP}/sample-count.txt"
if [ "${sample_count}" -gt "${#COMMITTED_FIXTURES[@]}" ]; then
    ci_pass "assembled ${sample_count} samples in ${samples}"
else
    ci_fail "no generated samples reached ${samples} (${sample_count} files total)"
fi

ci_finish
