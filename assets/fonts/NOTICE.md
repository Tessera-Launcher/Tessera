# Embedded fonts

Tessera ships two faces. Latin text uses the pixel face; every glyph it lacks
(Chinese above all) falls through to Unifont. Both are compiled into the
executable and renamed, so the host's installed copies can never be selected by
accident, and neither asset is tracked in Git.

## Tessera Pixel 8

Pixel Operator 8 with Minecraft's metrics: a glyph's advance is its ink width
plus one pixel, side bearings are zero, and the line gap is dropped. That is what
makes Latin text sit as tightly as the game's own font.

The generated `tessera-pixel8.ttf.gz` is not tracked in Git. Rebuild it with:

```sh
python3 -m pip install fonttools
python3 tools/fetch_pixel_font.py
```

- Upstream: <https://notabug.org/HarvettFox96/ttf-pixeloperator> (needs a login
  to fetch; the script pins a commit of the GitHub mirror)
- Designer: Jayvee Enaguas (HarvettFox96)
- License: CC0 1.0 Universal — no rights reserved, no attribution required. The
  full text is in `CC0-1.0.txt`.
- Changes: outlines moved to `xMin = 0`, advances rewritten to ink + 1 px (space
  fixed at 4 px), line gap cleared, family renamed to `Tessera Pixel 8`.

## Tessera Unifont

GNU Unifont 16.0.04, used for everything the pixel face does not cover. The
family is renamed to `Tessera Unifont` so the host's installed Unifont can never
be selected. The font contains the original outlines plus 16 px and 32 px bitmap
strikes.

The generated `tessera-unifont.otf.gz` is not tracked in Git. Rebuild it with:

```sh
python3 -m pip install fonttools
python3 tools/fetch_unifont.py
```

- Upstream: <https://unifoundry.com/unifont/>
- Copyright: 1998-2025 Roman Czyborra, Paul Hardy, Qianqian Fang, Andrew Miller,
  David Corbett, Johnnie Weaver, and others.
- License: SIL Open Font License 1.1 and GNU GPL 2.0 or later with the GNU font
  embedding exception. The full texts are in `OFL-1.1.txt` and `LICENSE.txt`.
