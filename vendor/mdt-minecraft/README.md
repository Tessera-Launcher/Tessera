# MDT — Minecraft Download Toolkit

Resolve Minecraft versions and Fabric into an **aria2 download list**.

[![Crates.io](https://img.shields.io/crates/v/mdt-minecraft.svg?style=flat-square)](https://crates.io/crates/mdt-minecraft)
[![docs.rs](https://docs.rs/mdt-minecraft/badge.svg?style=flat-square)](https://docs.rs/mdt-minecraft)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg?style=flat-square)](https://opensource.org/licenses/MIT)

**语言 / Language:** [English](README.md) · [简体中文](README.zh_CN.md)

MDT parses the metadata a launcher needs — the version manifest, a version JSON,
the asset index, a Fabric profile — writes the profile back to
`versions/<id>/<id>.json`, and emits one file per version:

```
versions/26.3-fabric-0.20.1/26.3-fabric-0.20.1.txt
```

That file is an `aria2c --input-file` list: a URL, then its indented options.

```text
https://piston-data.mojang.com/v1/objects/e877…/client.jar
  dir=/game/versions/26.3-fabric-0.20.1
  out=26.3-fabric-0.20.1.jar
```

Every destination directory is created while the list is written, because aria2
does not create `dir=` and one missing directory fails the whole entry.

## Features

- Vanilla Minecraft (any version in the manifest) and the Fabric loader.
- One list per version, per-file SHA-1 included, deduplicated by destination.
- The version JSON is written too: the launcher reads it back to build a command
  line, and Fabric's profile is merged so the directory stands on its own.
- Mirrors: `Mirror::BMCLAPI`, `Mirror::Mojang`, `Mirror::Custom("<name>")`, with
  provider tables compiled in from `providers.compile.toml`.
- One URL per file — no fallback chains, no `checksum=` line. `plan.verify()` is
  the single place a file is judged, after the download
- `minimal` feature: version-manifest parsing and the plan core only.

MDT does **not** download game files, launch the game, or authenticate. It has no
CLI and no TUI; the only binary here is an example.

## Install

```toml
[dependencies]
mdt-minecraft = "0.7.0"
```

## Quick start

```rust
use mdt::{MdtClient, Mirror, ModLoader};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = MdtClient::builder()
        .mirror(Mirror::Mojang)
        .root_dir("./.minecraft")
        .build()?;

    // Vanilla: versions/1.20.1/1.20.1.{json,txt}
    let plan = client.plan_vanilla("1.20.1").await?;

    // Fabric: one list for the whole stack, including the loader's own libraries.
    let plan = client.plan_loader("1.20.1", ModLoader::Fabric, "0.15.11").await?;

    println!(
        "{} files, {} hashed, {:.1} MiB -> {}",
        plan.entries.len(),
        plan.hashed(),
        plan.total_bytes() as f64 / (1024.0 * 1024.0),
        plan.path(client.root_dir()).display()
    );
    Ok(())
}
```

Fabric's loader version is whatever the caller passes; resolve it with
`manifest::fetch_fabric_manifest(...)` and take the **first** entry to follow the
newest build (the meta list is ordered newest-first, betas included).

## Handing the list to aria2

```sh
aria2c -i versions/<id>/<id>.txt \
  -j64 -x1 -s1 \
  --no-conf=true --enable-color=false \
  --show-console-readout=false --summary-interval=0 \
  --console-log-level=notice \
  --auto-file-renaming=false --allow-overwrite=true \
  --stop-with-process=<launcher pid>
```

Notes from using it:

- The hash is not in the list. aria2's own check would be a second verification,
  and it leaves a failed file on disk anyway; verify once, in the launcher:

  ```rust
  let failed = plan.verify(client.root_dir()).await;   // missing, empty, wrong size or hash
  ```

- `-j` is the knob for thousands of small files; `-x`/`-s` only matter for files
  above `--min-split-size` (20 MiB default), so keep them at 1.
- aria2 exits 13 when the target already exists, and 32 when a transfer fails. Treat a non-zero exit as "re-check these paths", not as "the
  file is there".
- `--stop-with-process` keeps an aria2 child from outliving the launcher.
- `--save-session=<file>` writes the failures back out, so a second run only
  retries what is missing.

## Feature flags

| Feature | Default | Effect |
|---|---|---|
| `tokio` | ✅ | async API with a reqwest transport |
| `http2` | ❌ | enable HTTP/2 (measured slower for Minecraft's asset CDN) |
| `minimal` | ❌ | no loader and no asset module: version manifest + plan core |

## Errors

Everything returns `Result<T, MdtError>`; the enum is `#[non_exhaustive]`, so keep
a wildcard arm. The variants a plan can produce are `NetworkError`,
`ChecksumMismatch`, `InvalidManifest`, `VersionNotFound`, `LoaderInstallFailed`
and `Io`.

## License

MIT, same as upstream. This copy is vendored into Tessera; see `VENDORED.md` for
the local changes.
