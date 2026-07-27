#!/bin/bash
# generate_s8.sh: derive S8 from S1 via qpdf --rotate=+90. Idempotent:
# no-op when the samples dir already contains an S8.pdf, so a matrix
# rerun does not repeatedly rewrite the same file (and to avoid
# clobbering hand-curated variants).
#
# Usage: bash corpus/generators/generate_s8.sh <samples_dir>
#
# <samples_dir> must contain S1.pdf. On success, <samples_dir>/S8.pdf
# exists and is a /Rotate 90 spin of S1 — used to exercise the
# rotation-aware coordinate transform on both the harness and the tool
# under test without needing a hand-authored rotated PDF.

set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "usage: bash corpus/generators/generate_s8.sh <samples_dir>" >&2
    exit 1
fi

samples_dir="$1"
src="$samples_dir/S1.pdf"
dst="$samples_dir/S8.pdf"

if [ ! -d "$samples_dir" ]; then
    echo "generate_s8: samples dir does not exist: $samples_dir" >&2
    exit 1
fi

if [ ! -f "$src" ]; then
    echo "generate_s8: source PDF not found: $src" >&2
    exit 1
fi

if [ -f "$dst" ]; then
    echo "generate_s8: $dst already exists, skipping"
    exit 0
fi

if ! command -v qpdf >/dev/null 2>&1; then
    echo "generate_s8: qpdf not on PATH (brew install qpdf)" >&2
    exit 1
fi

echo "generate_s8: qpdf --rotate=+90 $src -> $dst"
qpdf --rotate=+90 "$src" "$dst"
echo "generate_s8: done"
