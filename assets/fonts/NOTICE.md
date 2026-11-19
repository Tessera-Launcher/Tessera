# Embedded Unifont

Tessera embeds GNU Unifont 16.0.04 as its only UI font. The family is renamed to
`Tessera Unifont` so the host's installed Unifont can never be selected. The
font contains the original outlines plus 16 px and 32 px bitmap strikes.

The generated `tessera-unifont.otf.gz` is not tracked in Git. Rebuild it with:

```sh
python3 tools/fetch_unifont.py
```

- Upstream: <https://unifoundry.com/unifont/>
- Copyright: 1998-2025 Roman Czyborra, Paul Hardy, Qianqian Fang, Andrew Miller,
  David Corbett, Johnnie Weaver, and others.
- License: SIL Open Font License 1.1 and GNU GPL 2.0 or later with the GNU font
  embedding exception. The full texts are in `OFL-1.1.txt` and `LICENSE.txt`.
