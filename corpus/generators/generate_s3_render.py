#!/usr/bin/env python3
"""Render the S3 page images: one cover + N horizontal Japanese body pages.

Called by corpus/generators/generate_s3.sh. Emits page-01.png .. page-NN.png
into the output directory at a fixed DPI so that img2pdf lands on A4-sized
pages.

Page 1 is the cover: title/author drawn as pixels only, and generate_s3.sh
excludes it from OCR. The resulting PDF therefore has a page 1 with no text
layer — the "no text anchor on page 1" property the harness exercises as
the C11a/C11b skip path.

Usage:
    generate_s3_render.py --text <excerpt.txt> --out <dir> [options]
"""

from __future__ import annotations

import argparse
import os
import platform
import sys

from PIL import Image, ImageDraw, ImageFont

# Font candidates, most preferred first. Kept as a search list rather than a
# hardcoded path so the same script runs on a macOS workstation and on a Linux
# CI box; --font overrides everything.
FONT_CANDIDATES_DARWIN = [
    ("/System/Library/Fonts/ヒラギノ明朝 ProN.ttc", 0),
    ("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", 0),
    ("/Library/Fonts/Osaka.ttf", 0),
]

FONT_CANDIDATES_LINUX = [
    ("/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc", 0),
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
    ("/usr/share/fonts/truetype/noto/NotoSerifCJK-Regular.ttc", 0),
    ("/usr/share/fonts/opentype/noto/NotoSerifCJKjp-Regular.otf", 0),
    ("/usr/share/fonts/opentype/noto/NotoSansCJKjp-Regular.otf", 0),
    ("/usr/share/fonts/truetype/fonts-japanese-mincho.ttf", 0),
    ("/usr/share/fonts/truetype/fonts-japanese-gothic.ttf", 0),
    ("/usr/share/fonts/google-noto-cjk/NotoSerifCJK-Regular.ttc", 0),
    ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 0),
]

# Characters that must not start a line (行頭禁則). Minimal set: enough to keep
# the rendered page looking like real typesetting, which is what Tesseract has
# to segment.
NO_LINE_START = "、。，．)）」』】〉》’”ぁぃぅぇぉっゃゅょァィゥェォッャュョーヽヾ！？"

# Characters that must not end a line (行末禁則).
NO_LINE_END = "(（「『【〈《‘“"


def resolve_font(explicit: str | None, index: int) -> tuple[str, int]:
    if explicit:
        if not os.path.exists(explicit):
            raise SystemExit("generate_s3_render: font not found: %s" % explicit)
        return explicit, index

    candidates = (
        FONT_CANDIDATES_DARWIN
        if platform.system() == "Darwin"
        else FONT_CANDIDATES_LINUX
    )
    for path, idx in candidates:
        if os.path.exists(path):
            return path, idx

    raise SystemExit(
        "generate_s3_render: no Japanese font found. Install one "
        "(Linux: fonts-noto-cjk) or pass --font <path>."
    )


def load_source_text(path: str) -> list[str]:
    """Read the excerpt, dropping provenance comment lines (leading '#')."""
    paragraphs = []
    with open(path, encoding="utf-8") as fh:
        for raw in fh:
            line = raw.rstrip("\n")
            if line.startswith("#"):
                continue
            line = line.rstrip()
            if line.strip():
                # The leading ideographic space of each paragraph is kept: it
                # is part of the typesetting Tesseract has to segment.
                paragraphs.append(line)
    if not paragraphs:
        raise SystemExit("generate_s3_render: source text is empty: %s" % path)
    return paragraphs


def wrap_paragraph(text: str, chars_per_line: int) -> list[str]:
    """Greedy fixed-width wrap with minimal kinsoku handling.

    Japanese text is effectively monospaced at this font size, so a character
    count is a good enough measure and keeps the output deterministic across
    font versions.
    """
    lines: list[str] = []
    buf = ""
    for ch in text:
        if len(buf) < chars_per_line:
            buf += ch
            continue
        # Line is full. Pull the next character up only if it may not start a
        # line (hanging punctuation), otherwise break here.
        if ch in NO_LINE_START:
            buf += ch
            continue
        while buf and buf[-1] in NO_LINE_END:
            ch = buf[-1] + ch
            buf = buf[:-1]
        lines.append(buf)
        buf = ch
    if buf:
        lines.append(buf)
    return lines


def layout_lines(paragraphs: list[str], chars_per_line: int) -> list[str]:
    lines: list[str] = []
    for para in paragraphs:
        lines.extend(wrap_paragraph(para, chars_per_line))
    return lines


def render_cover(size: tuple[int, int], font_path: str, font_index: int) -> Image.Image:
    width, height = size
    img = Image.new("RGB", size, "white")
    draw = ImageDraw.Draw(img)

    title_font = ImageFont.truetype(font_path, int(height * 0.055), index=font_index)
    author_font = ImageFont.truetype(font_path, int(height * 0.026), index=font_index)
    sub_font = ImageFont.truetype(font_path, int(height * 0.018), index=font_index)

    # A plain cover: a rule, the title, the author. No text layer will be
    # attached to this page, so these are pixels and nothing else.
    margin = int(width * 0.14)
    draw.rectangle(
        [margin, int(height * 0.16), width - margin, int(height * 0.62)],
        outline="black",
        width=max(2, int(width * 0.004)),
    )
    draw.text((width / 2, height * 0.31), "こ こ ろ", font=title_font, fill="black", anchor="mm")
    draw.text((width / 2, height * 0.45), "夏 目 漱 石", font=author_font, fill="black", anchor="mm")
    draw.text(
        (width / 2, height * 0.54),
        "上　先生と私（抄）",
        font=sub_font,
        fill="black",
        anchor="mm",
    )
    draw.text(
        (width / 2, height * 0.80),
        "青空文庫 / パブリックドメイン",
        font=sub_font,
        fill="black",
        anchor="mm",
    )
    return img


def render_body(
    size: tuple[int, int],
    lines: list[str],
    font_path: str,
    font_index: int,
    font_px: int,
    line_px: int,
    margin_px: int,
    page_no: int,
) -> Image.Image:
    width, height = size
    img = Image.new("RGB", size, "white")
    draw = ImageDraw.Draw(img)
    font = ImageFont.truetype(font_path, font_px, index=font_index)
    folio_font = ImageFont.truetype(font_path, int(font_px * 0.6), index=font_index)

    y = margin_px
    for line in lines:
        draw.text((margin_px, y), line, font=font, fill="black")
        y += line_px

    draw.text(
        (width / 2, height - margin_px / 2),
        "- %d -" % page_no,
        font=folio_font,
        fill="black",
        anchor="mm",
    )
    return img


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--text", required=True, help="UTF-8 source text (excerpt)")
    ap.add_argument("--out", required=True, help="output directory for PNGs")
    ap.add_argument("--pages", type=int, default=10, help="total pages incl. cover")
    ap.add_argument("--dpi", type=int, default=150)
    ap.add_argument("--font", default=None, help="path to a Japanese TTF/OTF/TTC")
    ap.add_argument("--font-index", type=int, default=0, help="face index in a TTC")
    args = ap.parse_args()

    if args.pages < 2:
        raise SystemExit("generate_s3_render: --pages must be >= 2 (cover + body)")

    font_path, font_index = resolve_font(args.font, args.font_index)

    # A4 at the requested DPI. img2pdf reads the PNG's pHYs chunk (written from
    # the dpi= argument below) and derives the PDF page box from it.
    width = int(round(210.0 / 25.4 * args.dpi))
    height = int(round(297.0 / 25.4 * args.dpi))
    size = (width, height)

    font_px = int(round(args.dpi * 0.213))  # ~ 11pt at 150dpi
    line_px = int(round(font_px * 1.55))
    margin_px = int(round(args.dpi * 0.83))  # ~ 21mm

    chars_per_line = (width - 2 * margin_px) // font_px
    lines_per_page = (height - 2 * margin_px) // line_px

    body_pages = args.pages - 1
    lines = layout_lines(load_source_text(args.text), chars_per_line)
    needed = body_pages * lines_per_page
    if len(lines) < needed:
        raise SystemExit(
            "generate_s3_render: source text too short: have %d lines, "
            "need %d for %d body pages" % (len(lines), needed, body_pages)
        )

    os.makedirs(args.out, exist_ok=True)

    cover = render_cover(size, font_path, font_index)
    cover.save(os.path.join(args.out, "page-01.png"), dpi=(args.dpi, args.dpi))

    for i in range(body_pages):
        chunk = lines[i * lines_per_page:(i + 1) * lines_per_page]
        page = render_body(
            size, chunk, font_path, font_index, font_px, line_px, margin_px, i + 2
        )
        page.save(
            os.path.join(args.out, "page-%02d.png" % (i + 2)),
            dpi=(args.dpi, args.dpi),
        )

    sys.stderr.write(
        "generate_s3_render: font=%s index=%d %dx%d px, %d chars/line, "
        "%d lines/page, %d pages\n"
        % (font_path, font_index, width, height, chars_per_line, lines_per_page, args.pages)
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
