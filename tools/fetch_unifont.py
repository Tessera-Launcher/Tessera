#!/usr/bin/env python3
"""Build the embedded Tessera Unifont asset.

The generated font is deliberately not stored in Git. This script downloads
GNU Unifont's outline OTF and BDF source, wraps native 16 px and 32 px bitmap
strikes around the outlines, renames the family to ``Tessera Unifont`` so the
host's installed Unifont can never be selected, and writes the gzipped result to
``assets/fonts/tessera-unifont.otf.gz``.

Requirements:
    pip install fonttools
    fonttosfnt on PATH (Debian package: fonttosfnt)

Usage:
    python3 tools/fetch_unifont.py
"""

from __future__ import annotations

import gzip
import hashlib
import shutil
import subprocess
import tempfile
import urllib.request
from pathlib import Path

from fontTools.ttLib import TTFont

VERSION = "16.0.04"
ROOT = Path(__file__).resolve().parents[1]
CACHE = ROOT / "target" / "unifont-src"
OUT = ROOT / "assets" / "fonts" / "tessera-unifont.otf.gz"
BASE_URL = f"https://unifoundry.com/pub/unifont/unifont-{VERSION}"
OTF_URL = f"{BASE_URL}/font-builds/unifont-{VERSION}.otf"
BDF_URL = f"{BASE_URL}/font-builds/unifont-{VERSION}.bdf.gz"


def download(url: str, destination: Path) -> None:
    if destination.exists():
        print(f"using cached {destination}")
        return
    print(f"downloading {url}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with urllib.request.urlopen(url) as response, destination.open("wb") as output:
        shutil.copyfileobj(response, output)


def run(args: list[str]) -> None:
    print("+", " ".join(args))
    subprocess.run(args, check=True)


def scale_bdf_2x(source: Path, destination: Path) -> None:
    """Double a BDF font. A factor of two is pixel-exact and needs no resampler."""
    lines = source.read_text(encoding="latin-1").splitlines()
    output = []
    in_bitmap = False
    width = 0
    for line in lines:
        if in_bitmap:
            if line.startswith("ENDCHAR"):
                in_bitmap = False
                output.append(line)
                continue
            if not line.strip():
                output.append(line)
                continue
            bits = []
            for nibble in line.strip():
                value = int(nibble, 16)
                bits.extend((value >> shift) & 1 for shift in (3, 2, 1, 0))
            bits = bits[:width]
            doubled = []
            for bit in bits:
                doubled.extend((bit, bit))
            padded = doubled + [0] * (-len(doubled) % 4)
            row = "".join(
                f"{padded[index] * 8 + padded[index + 1] * 4 + padded[index + 2] * 2 + padded[index + 3]:X}"
                for index in range(0, len(padded), 4)
            )
            output.extend((row, row))
            continue

        if line.startswith("FONT "):
            line = line.replace("-16-160-", "-32-320-").replace("-c-80-", "-c-160-")
            output.append(line)
        elif line.startswith("SIZE "):
            parts = line.split()
            output.append(f"SIZE {int(parts[1]) * 2} {parts[2]} {parts[3]}")
        elif line.startswith("FONTBOUNDINGBOX "):
            values = [int(value) for value in line.split()[1:]]
            output.append("FONTBOUNDINGBOX " + " ".join(str(value * 2) for value in values))
        elif line.startswith("SWIDTH ") or line.startswith("DWIDTH "):
            parts = line.split()
            output.append(f"{parts[0]} {int(parts[1]) * 2} {parts[2]}")
        elif line.startswith("BBX "):
            values = [int(value) for value in line.split()[1:]]
            width = values[0] * 2
            output.append("BBX " + " ".join(str(value * 2) for value in values))
        elif line.startswith("PIXEL_SIZE "):
            output.append(f"PIXEL_SIZE {int(line.split()[1]) * 2}")
        elif line.startswith("POINT_SIZE "):
            output.append(f"POINT_SIZE {int(line.split()[1]) * 2}")
        elif line.startswith("AVERAGE_WIDTH "):
            output.append(f"AVERAGE_WIDTH {int(line.split()[1]) * 2}")
        elif line.startswith("FONT_ASCENT ") or line.startswith("FONT_DESCENT "):
            output.append(f"{line.split()[0]} {int(line.split()[1]) * 2}")
        else:
            output.append(line)
        if line == "BITMAP":
            in_bitmap = True
    destination.write_text("\n".join(output) + "\n", encoding="latin-1")

def remap_bitmap_strikes(otf_path: Path, otb_path: Path) -> TTFont:
    """Attach the BDF bitmap strikes to the OTF's outline glyph names."""
    otf = TTFont(otf_path)
    bitmap = TTFont(otb_path)
    # Force decompilation before renaming the EBLC index entries: EBDT uses
    # those names to associate each bitmap with its glyph.
    ebdt = bitmap["EBDT"]
    eblc = bitmap["EBLC"]
    bitmap_cmap = bitmap.getBestCmap()
    outline_cmap = otf.getBestCmap()
    codepoint_by_name = {}
    for codepoint, name in bitmap_cmap.items():
        codepoint_by_name.setdefault(name, codepoint)
    outline_names = set(otf.getGlyphOrder())

    new_strike_data = []
    for strike_data in ebdt.strikeData:
        remapped = {}
        for bitmap_name, glyph in strike_data.items():
            codepoint = codepoint_by_name.get(bitmap_name)
            outline_name = outline_cmap.get(codepoint) if codepoint is not None else None
            if outline_name and outline_name in outline_names and outline_name not in remapped:
                remapped[outline_name] = glyph
        new_strike_data.append(remapped)
    ebdt.strikeData = new_strike_data

    for strike, remapped in zip(eblc.strikes, new_strike_data):
        for index_subtable in strike.indexSubTables:
            names = []
            seen = set()
            for bitmap_name in index_subtable.names:
                codepoint = codepoint_by_name.get(bitmap_name)
                outline_name = outline_cmap.get(codepoint) if codepoint is not None else None
                if (
                    outline_name
                    and outline_name in outline_names
                    and outline_name in remapped
                    and outline_name not in seen
                ):
                    names.append(outline_name)
                    seen.add(outline_name)
            index_subtable.names = sorted(names, key=otf.getGlyphID)

    otf["EBDT"] = ebdt
    otf["EBLC"] = eblc

    for record in otf["name"].names:
        if record.nameID in (1, 4, 16):
            record.string = "Tessera Unifont"
        elif record.nameID in (2, 17):
            record.string = "Regular"
        elif record.nameID == 6:
            record.string = "TesseraUnifont-Regular"
    return otf


def main() -> None:
    if shutil.which("fonttosfnt") is None:
        raise SystemExit(
            "missing fonttosfnt\nInstall the fonttosfnt package, then re-run this script."
        )

    CACHE.mkdir(parents=True, exist_ok=True)
    otf_path = CACHE / f"unifont-{VERSION}.otf"
    bdf_gz = CACHE / f"unifont-{VERSION}.bdf.gz"
    bdf16 = CACHE / "unifont-16.bdf"
    bdf32 = CACHE / "unifont-32.bdf"
    otb_path = CACHE / "unifont-16-32.otb"

    download(OTF_URL, otf_path)
    download(BDF_URL, bdf_gz)
    if not bdf16.exists():
        with gzip.open(bdf_gz, "rb") as source, bdf16.open("wb") as target:
            shutil.copyfileobj(source, target)
    if not bdf32.exists():
        print("doubling", bdf16, "->", bdf32)
        scale_bdf_2x(bdf16, bdf32)
    run(
        [
            "fonttosfnt",
            "-o",
            str(otb_path),
            "-b",
            "-c",
            str(bdf16),
            str(bdf32),
        ]
    )

    otf = remap_bitmap_strikes(otf_path, otb_path)
    with tempfile.NamedTemporaryFile(suffix=".otf", delete=False) as temporary:
        temporary_path = Path(temporary.name)
    try:
        otf.save(temporary_path)
        data = temporary_path.read_bytes()
    finally:
        temporary_path.unlink(missing_ok=True)

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(gzip.compress(data, compresslevel=9, mtime=0))
    digest = hashlib.sha256(OUT.read_bytes()).hexdigest()
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes, sha256 {digest})")


if __name__ == "__main__":
    main()
