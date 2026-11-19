# Embedded Unifont

Tessera embeds GNU Unifont 16.0.04 as its only UI font.

- Upstream: <https://unifoundry.com/unifont/>
- File: `unifont-16.0.04.otf`
- SHA-256: `0e3981ab552231b5a2a870f2b61741903a4bf25c23ef5aeb05fdced1b3c7af4d`
- Copyright: 1998-2025 Roman Czyborra, Paul Hardy, Qianqian Fang, Andrew Miller,
  David Corbett, Johnnie Weaver, and others.
- License: SIL Open Font License 1.1 and GNU GPL 2.0 or later with the GNU font
  embedding exception. The full texts are in `OFL-1.1.txt` and `LICENSE.txt`.

The font is compiled into the executable with `include_bytes!`; it is not read
from the system font directories at runtime.
