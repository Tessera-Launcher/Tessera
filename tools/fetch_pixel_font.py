#!/usr/bin/env python3
"""Build the embedded Tessera Pixel 8 asset.

Pixel Operator 8 is a CC0 pixel typeface designed on an 8 px grid. Minecraft's
own Latin face has a property no ordinary TTF has: a glyph's advance is exactly
its ink width plus one pixel, with no side bearings. That is what makes its text
sit so tightly, so this script rewrites the metrics the same way before the font
is embedded:

    advance = round(ink width in px) + 1 px      (space: 4 px)
    left side bearing = 0                        (outline moved to xMin = 0)

It also drops the extra line gap and renames the family to ``Tessera Pixel 8``,
so the host's copy of Pixel Operator can never be selected by accident.

Requirements:
    pip install fonttools

Usage:
    python3 tools/fetch_pixel_font.py
"""

from __future__ import annotations

import gzip
import hashlib
import urllib.request
from pathlib import Path

from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.recordingPen import DecomposingRecordingPen
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTFont

# Pinned to a commit: the mirror is a copy of the upstream repository
# (notabug.org/HarvettFox96/ttf-pixeloperator), which needs a login to fetch.
COMMIT = "496045526d00b66e37ca11762c446b7423145822"
URL = f"https://raw.githubusercontent.com/bauripalash/ttf-pixeloperator-fork/{COMMIT}/ttf/PixelOperator8.ttf"
SHA256 = "5cccb9ef6cf18977b6e5721d49a1a6e78dd6a6f1c4f69537470f7dc1dc829ffc"

FAMILY = "Tessera Pixel 8"
DESIGN_PX = 8
SPACE_ADVANCE = 4

ROOT = Path(__file__).resolve().parents[1]
CACHE = ROOT / "target" / "font-src" / "PixelOperator8.ttf"
OUT = ROOT / "assets" / "fonts" / "tessera-pixel8.ttf.gz"


def download(destination: Path) -> None:
    if destination.exists():
        return
    destination.parent.mkdir(parents=True, exist_ok=True)
    print(f"downloading {URL}")
    with urllib.request.urlopen(URL) as response, destination.open("wb") as file:
        file.write(response.read())


def verify(path: Path) -> None:
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    if digest != SHA256:
        raise SystemExit(f"unexpected sha256 for {path}: {digest}")
    print(f"Pixel Operator 8 matches sha256 {SHA256[:16]}…")


class _Decomposed:
    """Draws a glyph with components resolved, without needing a glyph set."""

    def __init__(self, glyph, glyf):
        self.glyph = glyph
        self.glyf = glyf

    def draw(self, pen):
        self.glyph.draw(pen, self.glyf)

    @property
    def numberOfContours(self):
        return self.glyph.numberOfContours


class _GlyphSet:
    def __init__(self, glyf):
        self.glyf = glyf

    def __getitem__(self, name):
        return _Decomposed(self.glyf[name], self.glyf)


def tight_metrics(font: TTFont) -> None:
    """Give every glyph Minecraft's advance: ink width plus one pixel."""
    unit = font["head"].unitsPerEm // DESIGN_PX
    glyf, hmtx = font["glyf"], font["hmtx"]
    glyphs = _GlyphSet(glyf)

    for name in font.getGlyphOrder():
        recorded = DecomposingRecordingPen(glyphs)
        glyf[name].draw(recorded, glyf)
        bounds = BoundsPen(None)
        recorded.replay(bounds)

        if bounds.bounds is None:
            if name == "space":
                hmtx[name] = (SPACE_ADVANCE * unit, 0)
            continue

        x_min, _, x_max, _ = bounds.bounds
        ink_pixels = max(1, round((x_max - x_min) / unit))
        pen = TTGlyphPen(None)
        recorded.replay(TransformPen(pen, (1, 0, 0, 1, -x_min, 0)))
        glyf[name] = pen.glyph()
        glyf[name].recalcBounds(glyf)
        hmtx[name] = ((ink_pixels + 1) * unit, 0)


def rename(font: TTFont) -> None:
    family = FAMILY
    full = FAMILY
    postscript = FAMILY.replace(" ", "")
    records = {
        1: family,      # family
        2: "Regular",   # subfamily
        4: full,        # full name
        6: postscript,  # PostScript name
        16: family,     # typographic family
        17: "Regular",  # typographic subfamily
    }
    name_table = font["name"]
    for record in list(name_table.names):
        if record.nameID in records:
            name_table.removeNames(nameID=record.nameID)
    for name_id, value in records.items():
        name_table.setName(value, name_id, 3, 1, 0x409)
        name_table.setName(value, name_id, 1, 0, 0)


def metrics(font: TTFont) -> None:
    """No line gap: the layout in Minecraft stacks lines on a 9 px box."""
    unit = font["head"].unitsPerEm // DESIGN_PX
    ascent, descent = 7 * unit, -1 * unit
    font["hhea"].ascent = ascent
    font["hhea"].descent = descent
    font["hhea"].lineGap = 0
    font["OS/2"].sTypoAscender = ascent
    font["OS/2"].sTypoDescender = descent
    font["OS/2"].sTypoLineGap = 0
    font["OS/2"].usWinAscent = ascent
    font["OS/2"].usWinDescent = -descent
    font["head"].modified = 0


def main() -> None:
    download(CACHE)
    verify(CACHE)

    font = TTFont(CACHE, recalcTimestamp=False)
    tight_metrics(font)
    rename(font)
    metrics(font)

    OUT.parent.mkdir(parents=True, exist_ok=True)
    temporary = OUT.with_suffix(".tmp")
    font.save(temporary)
    with temporary.open("rb") as source, gzip.GzipFile(
        filename="", mode="wb", fileobj=OUT.open("wb"), compresslevel=9, mtime=0
    ) as archive:
        archive.write(source.read())
    temporary.unlink()

    print(f"wrote {OUT.relative_to(ROOT)} ({OUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
