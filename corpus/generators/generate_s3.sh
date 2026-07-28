#!/usr/bin/env bash
# generate_s3.sh — build S3.pdf from scratch: rendered page images bundled by
# img2pdf, then a Tesseract text layer grafted on by ocrmypdf with page 1
# excluded.
#
# S3's role in the corpus is "multi-page, image-first scanned book whose page 1
# is a cover with no text layer". Page 1 intentionally has no text layer: the
# missing page-1 anchor is what drives the harness's C11a/C11b skip path and
# the fixed-coordinate annotation fallback. Everything here is copyright-clean
# and fully automatic:
#   page 1      cover image (title/author drawn as pixels), never OCR'd
#   page 2..N   horizontal Japanese body pages typeset from
#               assets/kokoro-excerpt.txt (Aozora Bunko, public domain)
#
# Unlike S9..S12 or S8 this is not a derivative of S1 — nothing in fixtures/
# is read.
#
# Idempotent: no-op when <samples_dir>/S3.pdf already exists, so a matrix rerun
# can call it freely.
#
# Usage: bash corpus/generators/generate_s3.sh [samples_dir] [pages]
#   samples_dir  defaults to corpus/fixtures
#   pages        total page count including the cover, defaults to 10
#
# Environment overrides:
#   S3_FONT        path to a Japanese TTF/OTF/TTC. Default: the renderer probes
#                  Hiragino (macOS) then Noto CJK / fonts-japanese (Linux).
#   S3_FONT_INDEX  face index inside a .ttc (default 0)
#   S3_PYTHON      python interpreter that already has Pillow. Setting it is the
#                  whole interpreter story: no python3 on PATH is required.
#                  Unset, the script uses python3 if it can import PIL, and
#                  otherwise creates a throwaway venv in the build dir and
#                  pip-installs Pillow into it (the global environment is never
#                  touched).
#
# Bash-style constraints (project convention: no command substitution,
# backticks, or heredocs — see generate_encrypted_fixtures.sh for the same
# rule): values are captured to temp files and read back with `read`. The
# Python side has no such constraint.

set -euo pipefail

script_dir="${BASH_SOURCE[0]%/*}"
renderer="${script_dir}/generate_s3_render.py"
source_text="${script_dir}/assets/kokoro-excerpt.txt"

samples_dir="${1:-corpus/fixtures}"
pages="${2:-10}"
dst="${samples_dir}/S3.pdf"

if [ ! -d "${samples_dir}" ]; then
    echo "generate_s3: samples dir does not exist: ${samples_dir}" >&2
    exit 1
fi

if [ -f "${dst}" ]; then
    echo "generate_s3: ${dst} already exists, skipping"
    exit 0
fi

for f in "${renderer}" "${source_text}"; do
    if [ ! -f "${f}" ]; then
        echo "generate_s3: missing required file: ${f}" >&2
        exit 1
    fi
done

for tool in img2pdf ocrmypdf tesseract; do
    if ! command -v "${tool}" >/dev/null 2>&1; then
        echo "generate_s3: ${tool} not on PATH" >&2
        echo "  macOS: brew install ocrmypdf img2pdf tesseract-lang" >&2
        echo "  Debian/Ubuntu: apt install ocrmypdf img2pdf tesseract-ocr-jpn fonts-noto-cjk" >&2
        exit 1
    fi
done

# The interpreter is picked here, next to the other dependency checks, so that
# what we demand matches what we will actually run: an explicit S3_PYTHON is the
# whole interpreter story and needs no python3 on PATH, whereas both fallbacks
# (system Pillow / throwaway venv) do. Materialising the venv happens further
# down, once the build dir exists.
python_bin="${S3_PYTHON:-}"
if [ -n "${python_bin}" ]; then
    if ! command -v "${python_bin}" >/dev/null 2>&1; then
        echo "generate_s3: S3_PYTHON is not an executable command: ${python_bin}" >&2
        exit 1
    fi
elif ! command -v python3 >/dev/null 2>&1; then
    echo "generate_s3: python3 not on PATH" >&2
    echo "  install python3, or point S3_PYTHON at a python that has Pillow" >&2
    exit 1
fi

# Scratch space. `mkdir` without -p is atomic and refuses to follow an existing
# path, which is the property we want from a predictable name; going through
# mktemp would need command substitution to read its stdout.
build_dir="${TMPDIR:-/tmp}/generate_s3.$$"
mkdir "${build_dir}"

cleanup() {
    rm -rf "${build_dir}"
}
trap cleanup EXIT

# The Japanese traineddata is what makes the text layer meaningful; without it
# ocrmypdf would fall back to a language we did not ask for.
tesseract --list-langs > "${build_dir}/langs.txt" 2>&1
if ! grep -q -x "jpn" "${build_dir}/langs.txt"; then
    echo "generate_s3: tesseract has no 'jpn' traineddata" >&2
    echo "  macOS: brew install tesseract-lang" >&2
    echo "  Debian/Ubuntu: apt install tesseract-ocr-jpn" >&2
    exit 1
fi

# Pillow resolution, continued from the dependency check above: python_bin is
# already set when the caller supplied S3_PYTHON. Otherwise prefer a system
# python3 that has Pillow, and only then pay for a throwaway venv. The venv
# lives in the build dir so it disappears with the trap above.
if [ -z "${python_bin}" ]; then
    if python3 -c "import PIL" >/dev/null 2>&1; then
        python_bin="python3"
    else
        echo "generate_s3: Pillow not available, creating a temporary venv"
        python3 -m venv "${build_dir}/venv"
        "${build_dir}/venv/bin/pip" install --quiet --upgrade pip
        "${build_dir}/venv/bin/pip" install --quiet Pillow
        python_bin="${build_dir}/venv/bin/python"
    fi
fi

# Seeded with --font-index so the array is never empty: `set -u` on bash 3.2
# (the macOS default) errors on the expansion of an empty array.
font_args=(--font-index "${S3_FONT_INDEX:-0}")
if [ -n "${S3_FONT:-}" ]; then
    font_args+=(--font "${S3_FONT}")
fi

echo "generate_s3: rendering ${pages} page images"
"${python_bin}" "${renderer}" \
    --text "${source_text}" \
    --out "${build_dir}/pages" \
    --pages "${pages}" \
    "${font_args[@]}"

# img2pdf derives each PDF page box from the PNG pHYs (150 dpi), landing on A4.
# It embeds the PNG bitstreams losslessly, so the pages stay image-only.
echo "generate_s3: img2pdf -> ${build_dir}/no-text-layer.pdf"
img2pdf --output "${build_dir}/no-text-layer.pdf" "${build_dir}"/pages/page-*.png

# Skipping page 1 is the whole point: it gets copied through untouched and
# ends up with no text layer, which is the C11 skip path this sample exists for.
# The range is written out in full rather than as "2-end", which ocrmypdf only
# learned to accept after 15.2.0; the page count is known here anyway.
# --output-type pdf keeps ocrmypdf from running the PDF/A (Ghostscript) rewrite,
# so the output stays a plain image+text-layer PDF like a scanner would emit.
# --optimize 0 keeps the embedded images byte-identical to what img2pdf wrote.
echo "generate_s3: ocrmypdf -l jpn --pages 2-${pages} -> ${dst}"
ocrmypdf \
    --language jpn \
    --pages "2-${pages}" \
    --output-type pdf \
    --optimize 0 \
    --quiet \
    "${build_dir}/no-text-layer.pdf" \
    "${build_dir}/S3.pdf"

mv "${build_dir}/S3.pdf" "${dst}"
echo "generate_s3: done -> ${dst}"
