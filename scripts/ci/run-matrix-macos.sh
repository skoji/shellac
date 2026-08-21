#!/usr/bin/env bash
# run-matrix-macos.sh — the verification matrix with every check enabled.
#
# The PDFKit helpers are what C5, C7 and C11b are made of, and they need
# macOS with Xcode's Swift compiler. This mode is therefore the only one
# that covers the PDFKit side of the corpus: the text-extraction comparison,
# the reload check, and the PDFKit-side position re-verification that
# corroborates C11a through a second implementation.
#
# The run ends in a verdict rather than a report. `verify gate` matches the
# matrix's failing cells against corpus/known-exceptions.json, and a
# committed synthetic failure checks that the gate still rejects what the
# list does not cover.
#
# Usage: bash scripts/ci/run-matrix-macos.sh <samples dir> [out dir]
#   samples dir  the corpus samples to run over; the committed fixtures plus
#                the generated ones (see check-generated-samples.sh)
#   out dir      where matrix.md, fails.json and timing.txt land; defaults
#                to a temp dir that is removed on exit

set -euo pipefail

if [ "$#" -lt 1 ]; then
    printf 'usage: bash scripts/ci/run-matrix-macos.sh <samples dir> [out dir]\n' >&2
    exit 2
fi

script_dir="${BASH_SOURCE[0]%/*}"
# shellcheck source=scripts/ci/lib.sh
. "${script_dir}/lib.sh"
# shellcheck source=scripts/ci/matrix-lib.sh
. "${script_dir}/matrix-lib.sh"

ci_init "run-matrix-macos"
CI_MATRIX_REPO_ROOT="${script_dir}/../.."

# The helpers are compiled on demand by `verify matrix` itself.
ci_require_tools xcrun

ci_matrix_run "macOS (full)" "$1" "${2:-${CI_TMP}/out}"

ci_finish
