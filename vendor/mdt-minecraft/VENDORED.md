# Vendored copy — local changes

Upstream: [Lavaver/Minecraft-Download-Toolkit](https://github.com/Lavaver/Minecraft-Download-Toolkit)
at `85f1b0d` (v0.7.0, 2026-11-05). License: MIT (see `LICENSE`), which is
compatible with the GPL-3.0-or-later this project ships under.

This copy is **metadata only**: it resolves a version and a loader profile and
writes a download list. It never transfers a game file, never launches the game
and never authenticates. Auth lives in `../minecraft-auth`; the bytes are moved
by aria2 (see the launcher's `aria2` module) or by whatever the front-end uses.

## Why an in-tree copy

Every other Rust Minecraft library hardcodes the Mojang endpoint constants. MDT
is the only one with declarative mirror providers (`providers.compile.toml`) plus
a runtime override file, and both are inside code that no upstream version
exposes as configuration. Vendoring beats wrapping here.

## The plan-only change set

Upstream downloads everything itself. This copy does not:

| Removed | Why |
|---|---|
| `downloader::execute_downloads*`, `DownloadBatch`, segmented Range downloads | aria2 does this, with more connections and per-file hashes |
| `asset::AssetRegistry` (`sync`, `fetch_and_sync`, registry seeding, `missing_locally`) | assets are entries in the list now |
| `virtual_dir` (hardlink / symlink / copy mounts) | it existed to feed the download cache |
| `MdtClient::install_vanilla*`, `install_loader*`, `install_modloader`, `full_install`, `uninstall_modloader` | replaced by `plan_vanilla` / `plan_loader` |
| `installer::mcr` (post-uninstall manifest scan), `installer::uninstall` | uninstall needs the downloader to restore files |
| `jar` (JAR re-packing), `cli`, `tui`, `blocking`, `macros` | download-side conveniences; this crate has one consumer |
| `MdtError::{PartialDownloadFailed, UninstallFailed}` | nothing can produce them now |

Added:

- `plan::DownloadPlan` / `PlanEntry` — `<list_dir>/<id>.txt` (`列表/` by
  default, `MdtClientBuilder::list_dir` to move it) in the `aria2c --input-file`
  format (`dir=`, `out=`; one URL per file, no checksum).
- `DownloadPlan::verify` — the single verification step: missing, empty, wrong
  size or wrong SHA-1. The list stays a download list, not a proof.
- `DownloadPlan::create_dirs` — called by `write`. aria2 does not create `dir=`,
  and one missing directory fails the whole entry.
- `plan::fill_missing_hashes` — Fabric's profile names its own artifacts without
  a digest, so `<artifact>.sha1` is fetched (metadata) and the line becomes
  verifiable.
- `asset::plan_objects` — fetches the asset index (metadata, cached in
  `assets/indexes/<id>.json`, SHA-1 checked) and lists every object.
- `lwjgl3::plan_swap` — the 1.8.9 LWJGL 3 exchange, planned instead of fetched.
- `examples/plan.rs` — `cargo run --example plan -- <root> <mc> [loader]`.

## Local changes kept from earlier

- **HTTP/1.1 by default** (`http2` is opt-in). Measured on Minecraft assets:
  h2 put all streams on one TCP connection and lost to multiple h1.1 streams.
- **Asset index caching** in `$HOME/.mdt/registry/index/…`: the index is the same
  file every install of that version reads, so it is fetched once and re-verified
  by SHA-1 before use.
- **Native selection** honours the library's `natives` map and the profile's
  per-OS rules (`evaluate_rules`), which upstream got slightly wrong for a few
  1.7-era profiles.
- **Mirror-aware Maven and asset URLs** (`urls::ManifestUrls`), including
  `maven_exclude` host lists, so BMCLAPI clients never get linked to
  `libraries.minecraft.net`.
- **Security checks on remote metadata**: `security::is_safe_http_url`,
  `is_safe_path_component`, `is_sha1_hex` — a version JSON is remote input, and a
  task is admitted only when its hash, URL and Maven-relative path are all sane.
- **Version JSON written before any list**: the launcher reads it back to build a
  command line, and a plan that points at a missing profile is useless.

## Verifying

```sh
cd vendor/mdt-minecraft
cargo test                       # 17 unit tests + doc tests
cargo test --lib plan_ -- --ignored   # live: writes a 1.20.1 (and Fabric) list
cargo run --example plan -- /tmp/mdt 26.3 0.20.1
```

The two `--ignored` tests assert that a list is written, that every line carries
a hash, and that `libraries/` and `assets/objects/` contain **no files** — only
the directories aria2 needs.

## Updating

Re-apply the plan-only change set on top of the new upstream tag. The public
surface this project depends on is small:

```text
MdtClient::{builder, plan_vanilla, plan_loader, root_dir, mirror, downloader}
DownloadPlan::{entries, hashed, total_bytes, path, write, to_aria2}
Mirror, ModLoader, VersionType, MdtError, ProgressReporter, manifest::*
```
