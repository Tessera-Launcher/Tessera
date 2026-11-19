# Embedded Unifont

Tessera embeds GNU Unifont 16.0.04 as its only UI font. The family is renamed to
`Tessera Unifont` so the host's installed Unifont can never be selected. The
font contains the original outlines plus 12 px, 14 px, 16 px, 21 px and 32 px bitmap strikes.

- Upstream: <https://unifoundry.com/unifont/>
- File: `tessera-unifont.otf.gz` (decompressed name: `tessera-unifont.otf`)
- SHA-256 (uncompressed): `305e8d94f87eb4d8c08ba04c366f45ed2b50cda92e3ee4d8ccf3f37eef129d04`
- SHA-256 (gzip): `2b00e66a2577fa5ab7c47723872961bb82435e95d7162d0a7bc2339e0d3de01c`
- Copyright: 1998-2025 Roman Czyborra, Paul Hardy, Qianqian Fang, Andrew Miller,
  David Corbett, Johnnie Weaver, and others.
- License: SIL Open Font License 1.1 and GNU GPL 2.0 or later with the GNU font
  embedding exception. The full texts are in `OFL-1.1.txt` and `LICENSE.txt`.

The gzip file is compiled into the executable with `include_bytes!`; the OTF is
decompressed in memory at startup and is not read from the system font
directories at runtime.
