#!/usr/bin/env bash
# run-matrix-linux.sh — the verification matrix in its PDFKit-free mode.
#
# This is the mode for environments where PDFKit does not exist. C5, C7 and
# C11b are not evaluated; the page count, page geometry and text anchor come
# from qpdf and poppler instead of the Swift helpers. Everything else --
# C1..C4, C6, C6-quads, C8, C11a, C9/C10 and the encrypted paths -- runs as
# it does on macOS.
#
# Nothing here is actually Linux-specific, which is deliberate: running this
# script on macOS is how the mode is checked before CI depends on it.
#
# The run ends in a verdict rather than a report. `verify gate` matches the
# matrix's failing cells against corpus/known-exceptions.json, and a
# committed synthetic failure checks that the gate still rejects what the
# list does not cover.
#
# Usage: bash scripts/ci/run-matrix-linux.sh <samples dir> [out dir]
#   samples dir  the corpus samples to run over; the committed fixtures plus
#                the generated ones (see check-generated-samples.sh)
#   out dir      where matrix.md, fails.json and timing.txt land; defaults
#                to a temp dir that is removed on exit

set -euo pipefail

if [ "$#" -lt 1 ]; then
    printf 'usage: bash scripts/ci/run-matrix-linux.sh <samples dir> [out dir]\n' >&2
    exit 2
fi

script_dir="${BASH_SOURCE[0]%/*}"
# shellcheck source=scripts/ci/lib.sh
. "${script_dir}/lib.sh"
# shellcheck source=scripts/ci/matrix-lib.sh
. "${script_dir}/matrix-lib.sh"

ci_init "run-matrix-linux"
CI_MATRIX_REPO_ROOT="${script_dir}/../.."

ci_matrix_run "linux (no PDFKit)" "$1" "${2:-${CI_TMP}/out}" --no-pdfkit

ci_finish
