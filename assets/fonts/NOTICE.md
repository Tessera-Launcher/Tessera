# Embedded Unifont

Tessera embeds GNU Unifont 16.0.04 as its only UI font. The family is renamed to
`Tessera Unifont` so the host's installed Unifont can never be selected. The
font contains the original outlines plus 16 px and 32 px bitmap strikes.

- Upstream: <https://unifoundry.com/unifont/>
- File: `tessera-unifont.otf.gz` (decompressed name: `tessera-unifont.otf`)
- SHA-256 (uncompressed): `9e5c95874a014ce21cbda85655750ed77f504da0c190fbb0aeff17aed1b245be`
- SHA-256 (gzip): `0d098d59f6b3841e7b7a4ba592b18e81dfe014ce5c4898b0ee929c63d4a4f8c5`
- Copyright: 1998-2025 Roman Czyborra, Paul Hardy, Qianqian Fang, Andrew Miller,
  David Corbett, Johnnie Weaver, and others.
- License: SIL Open Font License 1.1 and GNU GPL 2.0 or later with the GNU font
  embedding exception. The full texts are in `OFL-1.1.txt` and `LICENSE.txt`.

The gzip file is compiled into the executable with `include_bytes!`; the OTF is
decompressed in memory at startup and is not read from the system font
directories at runtime.
