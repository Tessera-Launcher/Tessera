# DTCG schema provenance

The files in this directory are unmodified JSON Schemas published by the
[Design Tokens Community Group](https://www.w3.org/community/design-tokens/).

- Version: `2025.10`
- Source: <https://designtokens.org/schemas/2025.10/format.json> and <https://designtokens.org/schemas/2025.10/resolver.json>
- Upstream repository: <https://github.com/design-tokens/community-group>
- License: [W3C Software and Document License](https://www.w3.org/Consortium/Legal/2015/copyright-software-and-document)
- Integrity: `manifest.json` records the download URL and SHA-256 of each file;
  `design/scripts/validate-tokens.mjs` refuses to run when a file no longer matches.

Do not edit these files. To upgrade, download the published version into a new
versioned directory, record its checksums, and point `design/scripts/validate-tokens.mjs`
at the new version.

## Why vendor instead of fetching

Validation must work offline and must not change when upstream publishes a new
version. Pinning the bytes keeps CI reproducible and makes an accidental local
edit fail loudly rather than silently relaxing the contract.

## Why it is separate from Terrazzo

Terrazzo parses and resolves tokens, and its lint rules cover value semantics.
Those rules intentionally allow some non-standard authoring, for example a
numeric `$description`. The official schema rejects it. Keeping both gates
means malformed DTCG documents fail, while Terrazzo still handles inheritance,
aliases and resolver contexts.
