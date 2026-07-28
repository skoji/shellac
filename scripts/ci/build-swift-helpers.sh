#!/usr/bin/env bash
# build-swift-helpers.sh — compile the PDFKit helpers the harness shells out
# to.
#
# The helpers are single-file Swift programs built on demand by a matrix
# run. Building them on their own keeps a compile error from being
# discovered only once a full run is already under way, and it is the part
# of the macOS side that can be checked without a save CLI.
#
# macOS only: the helpers link against PDFKit.
#
# Usage: bash scripts/ci/build-swift-helpers.sh [out_dir]
#   out_dir  where the binaries are written; defaults to a temp dir that is
#            removed on exit.

set -euo pipefail

script_dir="${BASH_SOURCE[0]%/*}"
# shellcheck source=scripts/ci/lib.sh
. "${script_dir}/lib.sh"

ci_init "build-swift-helpers"

uname -s > "${CI_TMP}/uname.txt"
read -r kernel < "${CI_TMP}/uname.txt"
if [ "${kernel}" != "Darwin" ]; then
    printf 'build-swift-helpers: PDFKit is macOS-only; nothing to build on %s\n' "${kernel}" >&2
    exit 1
fi

ci_require_tools swiftc

helpers_dir="${script_dir}/.."
out_dir="${1:-${CI_TMP}/bin}"
mkdir -p "${out_dir}"

for helper in pdfkit_text pdfkit_check pdfkit_textbbox generate_s5; do
    src="${helpers_dir}/${helper}.swift"
    ci_require_file "${src}"
    if swiftc -O -o "${out_dir}/${helper}" "${src}" > "${CI_TMP}/${helper}.log" 2>&1; then
        ci_pass "${helper}: built"
    else
        ci_fail "${helper}: swiftc failed"
        cat "${CI_TMP}/${helper}.log" >&2
    fi
done

ci_finish
