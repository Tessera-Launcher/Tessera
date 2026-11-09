# Vendored copy — local changes

Upstream: [Lavaver/Minecraft-Download-Toolkit](https://github.com/Lavaver/Minecraft-Download-Toolkit)
at `85f1b0d` (v0.7.0, 2026-11-05). License: MIT (see `LICENSE`), which is
compatible with the GPL-3.0-or-later this project ships under.

This copy is the download/install layer only: it never launches the game and
never authenticates. Auth lives in `../minecraft-auth`.

## Why an in-tree copy

Every other Rust Minecraft library hardcodes the Mojang endpoint constants. MDT
is the only one with declarative mirror providers (`providers.compile.toml`) plus
a runtime override file, and both edits below are inside code that no upstream
version exposes as configuration. Vendoring beats wrapping here.

## Local changes

### 1. Legacy asset indexes are materialised (old-version fix)

Upstream synced every asset index into `assets/objects` and stopped there. Two
index generations are read by the game from somewhere else, so both produced a
version that started but had no sounds or language files:

| Versions | Index id | Marker in the index JSON | Layout the game reads |
|---|---|---|---|
| 1.6.1 – 1.7.2 | `legacy` | `"virtual": true` | `assets/virtual/legacy/` |
| alpha – 1.5.2 | `pre-1.6` | `"map_to_resources": true` | `assets/virtual/pre-1.6/` and `<root>/resources/` |

- `downloader::AssetIndex` now keeps `is_virtual` and `map_to_resources` instead
  of discarding them, plus `needs_virtual_layout()`.
- `downloader::VirtualAssets` reports the resulting directories.
- `asset::materialize_virtual_layout` links every object (hardlink → symlink →
  copy, 32-way concurrent, reusing `virtual_dir::mount_all`) to
  `assets/virtual/<index id>/<logical name>`. Objects that failed to download are
  skipped rather than left as dangling links.
- `AssetRegistry::fetch_and_sync` returns `Option<VirtualAssets>` and runs the
  materialisation after the sync; `AssetRegistry::objects()` exposes the table.
- `MdtClient::install_vanilla_with_assets` returns `VanillaInstall { version,
  virtual_assets }`. `install_vanilla` keeps its signature and drops the extra.

The decision is driven by the JSON flags, not by the index id. Id-based checks
miss `legacy` entirely (it is not `pre-1.6`), which is the bug this replaces.

### 2. Native classifier selection (old-version + macOS fix)

Upstream matched classifiers with `key.starts_with("natives-" + std::env::consts::OS)`.
That broke in three ways:

- **macOS had no natives at all** for any version older than 1.19: those
  manifests key the classifier and the `natives` map as `osx`, while
  `consts::OS` is `macos`. Result: `UnsatisfiedLinkError` at start-up.
- `${arch}` was never substituted, so Twitch's 1.8.x libraries
  (`natives-windows-${arch}`) never resolved.
- `HashMap` iteration order decided between `-32` and `-64` builds.

`downloader::select_native_artifact` now honours the library's `natives` map
first (both `macos` and `osx` spellings, `${arch}` expanded), then falls back to
prefixes preferring unsuffixed, then `-64`, then `-32`. `Library` gained the
`natives` field it was dropping.

### 3. Duplicate destinations are deduplicated

Beta-era manifests list the same library twice (`jinput-platform:2.0.5` appears
twice in beta 1.7.3), and loader profiles override a vanilla library by
re-listing it. Both produced two tasks writing one path, which the downloader
runs concurrently — the pre-check and the write can interleave. Tasks are now
deduplicated by destination, last declaration wins (matching launch-time merge
order).

### 4. Hot-path fixes: hash passes, concurrency, per-task setup

Measured on this machine with a 256 MB random file (page cache warm, `sha1` crate):

| read buffer | throughput |
|---|---|
| 8 KiB | 379 MB/s |
| 64 KiB | 424 MB/s |
| 256 KiB | 432 MB/s |

- **`AssetRegistry::sync` hashed on the async worker.** The post-download filter that decides
  what to link into `assets/objects` ran `cache_has` (metadata + full SHA-1) inline in an async
  fn. An asset set is ~600 MB, i.e. seconds of one runtime worker pinned with every download
  stalled behind it. It now runs in `spawn_blocking`, like `missing_locally` already did.
- **`HASH_BUF_SIZE = 256 KiB`** replaces the 8 KiB buffer in `sha1_file` (both cfg branches) and
  `sha1_of_file`: same hashes, 12% less time, and through `tokio::fs` also one fewer blocking-pool
  round trip per 8 KiB.
- **Asset concurrency is a caller knob.** `ASSET_CONCURRENCY` (16) is now only the default;
  `MdtClientBuilder::asset_concurrency()` and `MdtClient::set_asset_concurrency()` set it, and
  `AssetRegistry::sync` / `fetch_and_sync` take the effective value. `concurrency` (default 3)
  still governs the few large files. The HTTP pool is sized `asset_concurrency + 4` (floor 20) so
  raising the knob does not make the pool the new ceiling.
- **`MirrorChain`** resolves both mirrors once per batch instead of once per task.
  `fallback_urls` used to call `ManifestUrls::try_new` twice per file — 13 `String` clones each,
  ~96k allocations for a 3.7k-object install — plus, for `Mirror::Custom`, an `RwLock` read and a
  runtime provider table clone per URL rewrite.

### 7. Progress cadence is time-based, not ten-per-batch

`execute_downloads` used to report after every `total / 10` files. For 1.8.9's 722
assets that is one update every 72 files, and for a 3 700-asset version one update
per ~370 files — a progress bar that looks hung for seconds at a time.

It now reports at most every `REPORT_INTERVAL` (100 ms) plus one guaranteed update
when the batch finishes. Measured on a cold install of 1.8.9 through the official
source: 722 assets, 109.4 MiB, 7.6 MiB/s, and the longest gap between progress
updates is under 900 ms (asserted by `live_install_1_8_9...`).

The decision lives in `should_report(finished, since_last_report)` so the cadence
is testable without waiting on a clock.

### 6. HTTP/1.1 by default (HTTP/2 multiplexing was half speed)

Measured with `examples/download_bench` on 1.8.9's 722 asset objects (109.5 MiB), one machine,
one network, cold registry, five arrangements:

| transport | asset concurrency | assets phase | rate |
|---|---|---|---|
| HTTP/2 multiplexed (was the default) | 16 | 46.7 s | 2.3 MiB/s |
| HTTP/2 + adaptive window | 64 | 30.2 s | 3.6 MiB/s |
| HTTP/2 multiplexed | 64 | 32.2 s | 3.3 MiB/s |
| `curl`, one connection per object | 64 | 19.9 s | 5.2 MiB/s |
| **HTTP/1.1 pooled** | 16 | 27.4 s | 4.0 MiB/s |
| **HTTP/1.1 pooled** | 64 | **15.3 s** | **6.8 MiB/s** |

`resources.download.minecraft.net` speaks HTTP/2, so reqwest put all 64 streams on one
connection. A pool of HTTP/1.1 connections is twice as fast for this workload and also beats
`curl` (which pays a fresh connection per object). The whole 1.8.9 install went from 62.6 s to
39.7 s at defaults, and to 28.4 s at `--asset-concurrency 64`.

`build_client` now pins HTTP/1.1 unless [`MdtClientBuilder::http1_only`] is set to `false`, which
keeps multiplexing and enables the adaptive flow-control window (the h2 path is still ~10%
faster with it).

### 5. Duplicate asset objects and cross-device mounts

Found by running `examples/download_bench` with the registry on one filesystem and the install
target on another (`~/.mdt` on `/home`, target on `/tmp`). Two defects, one cause: an index names
the same object more than once (1.8.9: 734 names, 722 hashes).

- Nothing downstream keyed off the hash, only off the name list, so one object became two
  download tasks writing one destination *and one temp file*, and then two concurrent mounts
  onto one `assets/objects/<hash>`. `missing_locally` now returns one entry per hash, and
  `build_tasks_from_assets` deduplicates defensively for external callers.
- With two mounts racing and `hard_link` unavailable (registry and target on different volumes),
  the loser reached the copy fallback, saw the winner's symlink, and aborted the whole install
  with `Security: Symlink escape blocked`. `copy_into_target` now treats "the target is a symlink
  already pointing at this source" as success — that is the state it was creating, and nothing is
  written. A link pointing anywhere else is still refused.

### 8. HTTP/2 is compiled out (opt-in via this crate's `http2` feature)

Upstream's manifest took `reqwest`'s default features, and those include `http2`. Combined with
`minecraft-auth`'s explicit `features = ["…", "http2"]`, that linked the `h2` crate into every
build even though section 6 makes HTTP/1.1 the transport actually used — the h2 code path was
unreachable, and `http1_only` existed only to turn it *off*.

- `Cargo.toml`: `reqwest` is now `default-features = false` with
  `["json", "stream", "charset", "system-proxy"]`. `charset` stays because the code uses
  `Response::text()`; `system-proxy` stays so the ambient proxy configuration keeps working.
- New `http2` feature (`["reqwest?/http2"]`), off by default. `http.rs` gates the adaptive-window
  call on it, so `http1_only(false)` still compiles (and harmlessly stays on HTTP/1.1) when the
  feature is off.
- `minecraft-auth` drops its own `http2` too: auth is a handful of JSON POSTs, so multiplexing
  never paid for itself.

Measured on the same 219-object sample used for the concurrency work, with the whole tree rebuilt
from scratch: the release binary went from 4.8 MB to 4.6 MB (`h2` plus `hyper`'s http2 plumbing),
and install speed is unchanged because the h2 path was dead code at `http1_only(true)`.

### 9. The launcher's own JVM can run the official loader installers

`detect_java()` looked at `JAVA_HOME` then `PATH`, so a Forge/NeoForge install silently depended
on a system Java — while the launcher that vendors this crate refuses to launch a game on anything
but the JREs it downloaded itself. A user with no system Java, or an old one, could not install
Forge at all.

- `MdtClientBuilder::java(path)` (and `MdtClient::java()`) reserves a runtime for official
  installers; `InstallRequest` carries it as `java: Option<PathBuf>`.
- `install_via_official_installer` takes `java: Option<&Path>` and uses it instead of
  `detect_java()`. `None` keeps upstream's detection, so the crate still works standalone.
- Forge and NeoForge pass `req.java`; Fabric, Quilt and LiteLoader never start a JVM and ignore it.

### 10. Rebuilt legacy Forge installers (no `maven/`, no `version.json`)

Forge re-cut its pre-1.13 installers in 2021. Those archives carry neither the
old top-level `maven/` directory nor `version.json`; they hold the universal jar
at the root plus `install_profile.json`, whose `versionInfo` is the version JSON
and whose `libraries` list names every dependency. The legacy unpack path
required both removed entries and failed with
"legacy Forge installer is missing `maven/`".

`installer::version_json::install_legacy_forge` now:

- falls back to `install_profile.json` when `maven/` is absent;
- copies `install.filePath` (the root universal jar) to the Maven path named by
  `install.path`, which is where the version JSON and the launcher expect it
  (a byte copy: the unpack tree is deleted right after, so a hardlink/symlink
  would dangle);
- downloads `versionInfo.libraries` into `libraries/` when missing, using each
  entry's `url` and defaulting to `https://libraries.minecraft.net/` (all of
  them: `clientreq: false` only tells the official installer to skip a jar, but
  it stays in the version JSON and the launcher still needs it);
- synthesizes the version JSON from `install_profile.versionInfo` when
  `version.json` is absent, keeping the existing mirror rewrite;
- names **every** loader install `<mc>-<loader>-<ver>` (e.g.
  `<mc>-forge-<ver>`, `<mc>-fabric-<ver>`), dropping upstream's
  `<mc>+<Loader><ver>` convention. Forge 1.8.9's FML URL-decodes the version
  name, which turns `+` into a space, then looks for
  `versions/<mc> Forge<ver>/<mc>.jar` and refuses to start ("CRITICAL
  TAMPERING WITH MINECRAFT"). Hyphenated ids are also what Forge/NeoForge's
  official installers write. `installer_version_id` is now an alias of
  `loader_version_id`.

Installers that still ship `maven/` and `version.json` take the original path
unchanged.

The copy-on-write step in `MdtClient::install_modloader` pre-creates the
installer-id directory and copies vanilla into it. If the finished install lands
elsewhere (or fails), that copy is now removed, so the launcher no longer lists
a phantom version with no JSON.

### 11. Install activity is reported as status lines

Tessera's install screen shows a live log, so the phases that used to be silent
now emit `status` lines:

- vanilla: version manifest, game files (`jar` / libraries / natives, with a
  count), then the asset index by name and object count;
- every download batch: `开始下载 N 个文件（M MiB）`;
- Forge/NeoForge: running the official installer, then checking its output;
- legacy Forge: downloading/unpacking the installer, deploying the universal jar
  and bundled libraries, checking the dependency list, writing the version JSON;
- Fabric/Quilt: fetching the profile, then the dependency count; LiteLoader: its
  core file.

Asset objects still skip per-file lines (thousands of them); the batch start and
summary already say how many and how much.

## Tests added

`src/asset.rs`: `legacy_index_flags_are_parsed`,
`virtual_index_links_named_paths_under_virtual_dir`,
`pre_1_6_index_also_maps_into_resources_dir`,
`virtual_layout_skips_objects_that_failed_to_download`.

`src/virtual_dir.rs`: `copy_into_target_accepts_the_link_it_was_trying_to_make`.

`src/asset.rs`: `missing_locally_reports_one_entry_per_hash_not_per_name`.

`src/installer/version_json.rs`: `rebuilt_legacy_installer_is_deployed_from_install_profile`.

`src/downloader.rs`: `asset_tasks_are_built_once_per_hash`,
`progress_reports_on_a_time_cadence_not_a_file_count`,
`native_classifier_prefers_the_declared_natives_map`,
`native_classifier_accepts_the_pre_1_19_osx_spelling`,
`native_classifier_expands_the_arch_placeholder`,
`native_classifier_falls_back_to_prefixes_without_a_natives_map`,
`dedupe_keeps_the_last_declaration_for_a_destination`,
`version_json_tasks_are_deduped_and_natives_go_to_the_legacy_dir`.

## Example

`examples/download_bench.rs` installs 1.8.9 from a chosen mirror into a directory passed on the
command line and times each phase, counting bytes through a `Downloader` wrapper rather than
trusting manifest sizes. It exists to make the asset-concurrency knob measurable:

```sh
cargo run --release --example download_bench -- /tmp/mdt-1.8.9
cargo run --release --example download_bench -- /tmp/mdt-1.8.9 --asset-concurrency 64
```

## Updating from upstream

The three changes touch `src/downloader.rs` (struct fields, classifier
selection, task dedupe), `src/asset.rs` (materialisation), `src/client.rs`
(`VanillaInstall`), `src/lib.rs` (exports) and one test fixture in
`tests/downloader.rs`. They do not touch the mirror/URL/provider table, so
upstream mirror work applies cleanly; re-check the two helpers after a rebase.
