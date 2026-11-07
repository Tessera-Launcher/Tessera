# Changelog

## [0.7.0] | Security hardening & observability

> [!IMPORTANT]
> One breaking change: `MdtError::PartialDownloadFailed` now carries `Vec<FailedDownload>` instead of
> `Vec<String>` (first bullet below). Everything else in this release is additive.

- **Breaking — `PartialDownloadFailed` carries reasons** — the variant is now
  `PartialDownloadFailed(Vec<FailedDownload>)`, where `FailedDownload { url, reason }` records the
  last error for each file. An upper layer can finally tell "the mirror answered 404" from "the disk
  is full" instead of seeing a bare URL. The raw URL stays available for a retry; both `Display` and
  `Debug` redact its userinfo.

- **Zip Slip defence** — every archive entry now passes `safe_path::validate_extract_path` before
  anything touches the filesystem, in both `unzip_jar` and native extraction. Absolute paths, `..`
  components (either separator), Windows drive prefixes and NTFS alternate data streams
  (`file.txt:hidden`), NUL bytes, and — on Windows — device names such as `CON` or `COM1` are
  rejected, and the first offending entry aborts the whole extraction with the new
  `MdtError::UnsafeArchivePath { entry, reason }` instead of being skipped. A destination that
  already exists as a symbolic link is refused rather than written through.

- **No symlink escape on the copy fallback** — when a virtual mount falls back from hardlink/symlink
  to a physical copy, the target is inspected with `symlink_metadata` (never `metadata`), a planted
  link is refused with `Security: Symlink escape blocked`, and the file is created with
  `create_new` (new `io::copy_create_new`), which fails atomically when anything — including a
  link — already exists at that path. `fs::copy` / `File::create` are no longer used there.

- **Credential redaction** — new `redact_credentials` (own `security` module, re-exported from
  `customize` so existing import paths keep working) turns `scheme://user:pass@host` into
  `scheme://***:***@host`. It is idempotent, UTF-8 safe, linear in the input, and leaves the path and
  query alone (a `?token=` value is part of the resource and cannot be told apart from a legitimate
  parameter). It is applied at one chokepoint, `customize::debug_line`, and by `FailedDownload`'s
  `Display`/`Debug`, so a mirror URL with an embedded password cannot reach a launcher's log.

- **`ProgressReporter::debug`** — new method with an empty default, so existing reporters compile
  unchanged. `StderrReporter` renders it as `[mdt] …` above the pinned progress line, and every
  `eprintln!` in the library now goes through it: a launcher can intercept or silence MDT's internal
  trace instead of having it written straight to stderr. Public free functions keep their signatures
  (they delegate to the new `*_reported` internals with `NoopReporter`);
  `mount_all_with_reporter` and `VirtualDir::mount_with_reporter` are the additive entry points that
  accept your reporter.

- **Runtime mirror overrides can be reloaded** — `RUNTIME_PROVIDERS` is now
  `RwLock<Option<…>>` rather than a `OnceLock`, so a failed load is no longer cached forever. New
  `reload_runtime_providers()` (typed `MdtError::InvalidProviderConfig` on failure) and
  `runtime_providers_error()` let a long-lived launcher pick up an edited `providers.runtime.toml`
  without restarting.

- **Fixed — `providers.runtime.toml` was never actually read** — the file used to be parsed with
  `str::parse::<toml::Value>()`, which parses a single TOML *value* rather than a document, so every
  real `[section]` file failed with "unexpected content, expected nothing" and the override table
  stayed silently empty. It is now parsed with `toml::from_str::<toml::Table>`.

- **Prefix-based mirror rewriting** — `apply_mirror` / `apply_maven_mirror` no longer use
  `String::replace`. A rule fires only when the URL starts with its `from` prefix *on a URL
  boundary*, so `https://piston-meta.mojang.com` no longer rewrites
  `https://piston-meta.mojang.com.evil/…` or a Mojang URL embedded in another site's query string.
  The join keeps exactly one separator (no `//`, no `mavencom/x.jar` glue), and `maven_exclude` is
  matched against the URL's host — exact or subdomain — instead of arbitrary text in the URL.

- **Manifest trust boundary** — download tasks are built only from entries whose SHA-1 is a real
  40-char digest, whose URL is `http(s)` and whose Maven-relative path stays inside `libraries/`
  (`is_sha1_hex`, `is_safe_http_url`, `validate_extract_path`); anything else is dropped instead of
  being fetched unverified. The asset-index descriptor (`id`, `sha1`, `url`) is validated before it
  becomes a filename or a cache key, `AssetRegistry::load` rejects an `index_id` that names a path,
  and a loader profile whose library `url` is not `http(s)` is rejected outright (SSRF).

- **A cache hit is no longer "the file exists"** — the LiteLoader JAR, `libraries/` copies and
  `download_artifact_verified`'s fast path all require a matching persisted SHA-1 sidecar; an
  unverifiable file is re-downloaded and re-hashed instead of being mounted. The fast path also
  returns early when the destination already matches, skipping the cache lookup and the copy.

- **No symlink following outside extraction** — `copy_dir_sync` refuses a symlink in the source or
  at the destination and writes copies with `create_new` (never `fs::copy`);
  `swap_loader_json_in_place` and `read_vanilla_launch_args` refuse to touch a version JSON that is
  a symlink; `object_path` quarantines a short/non-hex hash under `invalid/` instead of panicking on
  `&hash[..2]`; and `sync` relies on the downloader's single size+SHA-1 pre-check rather than
  hashing the registry copy a second time.

- **Internal** — `tempfile` joins the dev-dependencies. New negative tests cover zip-slip entries,
  planted and dangling symlinks, `create_new` overwrite attempts, `..`/ADS/device-name paths, the
  credential-leak shapes and reload-after-fix. MSRV is unchanged at 1.85.

## [0.6.10] | Minimal builds & graceful bare-engine errors

- **`minimal` feature** — if you want to use MDT in a resource-constrained embedded environment,
  the minimal subset is an excellent choice. It contains only an extremely streamlined core-engine
  experience: the `asset`, `registry`, `virtual_dir`, and `dual_loader` modules are stripped,
  `install_modloader` / `full_install` / `setup_dual_loader` are disabled, and native extraction in
  the download pipeline is skipped. In exchange, you implement the environment-specific pieces
  yourself — for example, inject a custom `Downloader` via `.downloader()`.

- **`MdtError::MissingDownloader`** — `MdtClientBuilder::build()` now returns this error instead of
  panicking when neither `tokio` nor `blocking` is enabled and no custom `Downloader` was provided.

## [0.6.9] | Blocking mode & three transport forms

- **Synchronous `BlockingClient`** — turn on the `blocking` feature and call the same API from a
  plain `std::thread`, no async runtime needed. It drives the core with `futures::executor::block_on`
  and swaps `tokio::fs` → `std::fs`, `tokio::time` → `std::thread` at compile time via an internal
  I/O shim.

- **Three transport forms** — `default = ["tokio"]` keeps `tokio` + `reqwest`; `blocking` uses
  `ureq` (a new `UreqDownloader`); with no features you get a bare engine with no default downloader,
  so you inject your own `Downloader`. The `blocking` runtime dependency tree pulls in no `tokio` or
  `reqwest`.

- **New error** — `MdtError::UreqError` for the blocking transport.

## [0.6.7 & 0.6.8] | Network, disk & mirror infrastructure

> [!NOTE]
> The `mdt-macros` macro library has been updated to v0.4.2. This update is solely to reflect the renaming of related configuration files; the API has not been modified and will not affect existing projects.

- **Configurable network** — `MdtClientBuilder` now takes `.proxy(...)`, `.connect_timeout(...)`,
  and `.read_timeout(...)`. The shared client negotiates HTTP/2 (falling back to HTTP/1.1) and keeps
  an aggressive connection pool, so bursts of asset downloads reuse warm connections.

- **Disk-full defense** — a new `MdtError::DiskFull` is surfaced when a write hits the OS disk-full
  error (Windows 39/112, Linux 28) instead of a generic I/O error, so launchers can report "out of
  disk space" precisely.

- **Byte-aware progress & summary** — `ProgressReporter` gains `progress_bytes` and `summary` (with
  empty defaults, so existing reporters keep compiling). Downloads now report remaining bytes as
  they go and print a `Downloaded N component(s) (... in ... largest was ...)` summary at the end.
  `format_bytes` / `format_duration` do the human-friendly formatting.

- **Runtime mirror overrides** — the static config is now `providers.compile.toml`; a
  `providers.runtime.toml` (or `MDT_RUNTIME_PROVIDERS`) can override `Mirror::Custom` endpoints and
  rewrite rules at runtime, no recompile needed.

## [0.6.6] | Dual-loader support & in-place naming

- **In-place installs now launch** — with `copy_vanilla = false` the loader JSON takes over the
  vanilla JSON's filename (`1.16.5.json`), so the launcher sees a real mod-loader version instead
  of ignoring it. The pristine vanilla JSON is backed up as `1.16.5.json.mcvanila` for the
  uninstall flow. `read_vanilla_version_id` resolves the base version via `inheritsFrom`, and
  LiteLoader's launch-arg reader prefers the `.mcvanila` backup, so install and uninstall keep
  working after the rename.

- **Dual loader (Fabric + Quilt)** — `MdtClient::setup_dual_loader` installs both loaders into one
  version directory and keeps three JSONs (`<mc_version>.json.mcvanila`,
  `<mc_version>+Fabric<ver>.json`, `<mc_version>+Quilt<ver>.json`) plus the active
  `<mc_version>.json`. `switch_dual_loader_context` (or `mdt::dual_loader::switch_loader_context`)
  flips between them without touching the backup. State is detected by parsing each JSON's
  `mainClass` and `libraries` — never `id` or filenames — so a renamed folder can't fool it.

- **Latest stable loader version** — `latest_stable_loader_version` returns the newest *stable*
  Fabric/Quilt version (`LoaderVersionEntry.stable` is now parsed); `setup_dual_loader` uses it when
  no explicit version is given.

- **Windows file-lock retry on context switch** — the JSON swap retries up to 3 times (100 ms apart)
  on a Windows sharing violation (error 32), so Defender or a running launcher can't abort a
  context switch.

## [0.6.5] | Security Update

> [!NOTE]
> **v0.6.3 & v0.6.4 has not been “yanked” because, under certain conditions, it may not trigger OS error 32 (file lock exception)**. If you encounter this issue, updating to v0.6.5 will resolve it without requiring any changes to your existing code. Linux and macOS users can choose to remain on v0.6.3/4 or update to v0.6.5; the security patch II in v0.6.5 applies only to the Windows platform.

- **Cargo-style blocking wait** — when a mount hits a transient Windows file lock (sharing
  violation, raw OS error 32, or access denied), `mount_all` no longer gives up after three quick
  50 ms retries. Failed mounts are deferred to a polling queue that logs a Cargo-style
  `Blocking waiting for file lock on: ... (N files, retrying in 2s...)` line and retries every
  2 seconds for up to 30 seconds total. A locked file is never deleted or truncated to work around
  the lock — it's skipped and retried. Non-lock errors still abort immediately, and if the lock is
  still held after 30 seconds, `mount_all` returns `MdtError::VirtualDirMountFailed` with the
  blocked files listed.

## [0.6.3 & 0.6.4] | Virtual directory mounting

> [!NOTE]
> **v0.6.3 has not been “yanked” because, under certain conditions, it may not trigger OS error 32 (file lock exception)**. If you encounter this issue, updating to v0.6.4 will resolve it without requiring any changes to your existing code. Linux and macOS users can choose to remain on v0.6.3 or update to v0.6.4; the security patch in v0.6.4 applies only to the Windows platform.

- **Virtual directory** — the global registry is now *mounted* into the local `.minecraft` layout
  instead of physically copied. For every file MDT tries **hardlink → symlink → copy**, so same-volume
  installs are instant and free, cross-volume installs use a symlink, and restricted filesystems
  (FAT32, Windows without Developer Mode) fall back to a copy with a warning. `AssetRegistry`,
  `download_library`, and the LiteLoader core JAR all mount through the new `mdt::VirtualDir`
  (`mdt::virtual_dir::mount_all`), and `MdtError::VirtualDirMountFailed` reports mount failures.
  Transient Windows file locks (sharing violation, raw OS error 32) are retried up to 3 times with a
  50 ms backoff before falling through to the next strategy.

## [0.6.2] | The minimum compatible version of Rust has been determined

> [!NOTE]
> This update only lowers the minimum required version of the Rust toolchain; there are no breaking changes to the API contracts, so please rest assured.

After careful consideration and observation of the editions commonly used within the Rust community, we have decided to lower MDT’s minimum compatible Rust version to 1.85.0 (Rust 2021 Edition) to make it easier for more projects and developers to adopt this toolkit.

At the same time, the [macro library](https://crates.io/crates/mdt-macros) will be updated to version v0.4.1 in conjunction with this lowering of the minimum Rust version requirement. We sincerely apologize for any inconvenience this may cause.

## [0.6.1] | Routine Feature Updates

- **Customization traits** — a set of public extension traits (`ProgressReporter`, `Downloader`,
  `RegistryCache`) with working defaults (`StderrReporter`, `HttpDownloader`, `FsRegistryCache`).
  `ProgressReporter` and `Downloader` are wired into the hot path and injectable via
  `MdtClientBuilder::progress_reporter` / `::downloader`; `Downloader` now covers both file
  (`download`) and metadata (`fetch`) so the entire transport — not just progress — is swappable.
  `RegistryCache` ships as a content-addressed byte-store building block (`FsRegistryCache`).

## [0.6.0] | Registry & Security Update

- **Resilient downloads** — a single asset timing out or dropping the connection no longer aborts
  the batch. Each file is retried up to 3 times with exponential backoff + jitter, then soft-failed
  and reported via `MdtError::PartialDownloadFailed`, so the rest of the queue still finishes.
- **Cargo-registry flavor asset system** — a *global* registry rooted at `$HOME/.mdt/registry` (override
  `MDT_HOME`), shared across `.minecraft` directories, following the standard layout:
  `src/mojang/<version>/main/` for vanilla assets (content-addressed by SHA-1),
  `index/mojang/.cache/<sha1>` for the `assetIndex` JSON, and `<Modloader>/<version>/` for loader
  libraries and Method A installer JARs. The index is loaded first; every file is downloaded into
  the registry (temp + checksum verified + moved) and then *copied* — never hardlinked — into the
  local `assets/objects/` or `libraries/` dir. Modloader artifacts (libraries and installer JARs)
  are checksum-verified via their Maven `.sha1` sidecar (falling back to `.sha256` then `.md5`),
  and the hash is persisted next to the cached file so warm cache hits stay verifiable offline.

## [0.5.3] | Feature Update: We made a minor tweak to the terminal experience

- **Cargo-style progress** — installs and downloads now print friendly status lines to stderr:
  `Updating <source> index`, `Downloaded <file> by <Minecraft/Loader> <version>`, and a throttled
  `Downloading x/y Component(s) ...` tracker that stays pinned to the bottom of the terminal
  (rewritten in place with `\r` on a TTY). `stdout` stays clean for downstream launchers.

## [0.5.2] | hotfix patch 2: don't kill slow downloads (lol)

0.5.1 was yanked: the global 30s *total* request timeout aborted large-but-healthy downloads
(client.jar, assets) mid-stream. The client now uses a 15s connect timeout plus a 30s *read*
timeout (which resets as bytes arrive), so slow downloads survive while a stalled mirror still
times out. Small metadata fetches (manifests, loader profiles) get a strict 15s per-request
timeout.

## [0.5.1] | hotfix patch 1: We're happy to tell you that we've written a bug :D

> [!CAUTION]
> **This version has been yanked.** For existing projects, continuing to use this version can cause unpredictable and serious problems. For new projects, you normally cannot install this version via Cargo.

0.5.0 was yanked: Fix Modloader Installation Deadlock & Add Resilient Network Fallbacks

## [0.5.0] | Modloader (Un)Install Mechanism Update

> [!CAUTION]
> **This version has been yanked.** For existing projects, continuing to use this version can cause unpredictable and serious problems. For new projects, you normally cannot install this version via Cargo.

- **Uninstall** — `MdtClient::uninstall_modloader` reverts a loader back to pure vanilla using a
  SHA-1 manifest comparison. It pre-loads the manifest table into memory (no N+1 network queries),
  scans on disk with a multithreaded bucket-sorted scanner, and applies MCR markers
  (`[Hash].pinned` modified, `[Hash].pinned.add` created, `[Filename].pinned.remove` removed).
  The `mods/` folder is preserved. Re-running it is safe.
- **1 Pub Function(s) Breaking** — `MdtClient::install_modloader` now takes a vanilla version directory (`&Path`)
  plus a `copy_vanilla: bool` instead of a `&str` version. The Minecraft version is read from the
  directory's `version.json` `id` field and fails fast with `MdtError::InvalidManifest` if that
  JSON is missing or malformed.
- **Copy-on-write** — with `copy_vanilla == true` the vanilla directory is copied into the
  isolated `versions/<mc>+<Loader><ver>` (or the official installer's `-forge-`/`-neoforge-`)
  directory first; with `false` the loader is installed in place, mutating the vanilla directory.
- **New error** — `MdtError::UninstallFailed`.
- **Resilient downloads** — every loader download now follows a mirror-first, official-as-fallback
  policy, and the shared HTTP client carries connect/request timeouts (15s/30s), so a stalled or
  403ing mirror falls back to the official source instead of hanging.
- **Macro Name & Mechanism Breaking** — the `mdt_install!` macro is renamed to `full_install!`, and it now
  installs vanilla and the loader in one shot (with a five-argument form for the copy-on-write
  flag). It expands to the new `MdtClient::full_install`.

## [0.4.0] | custom mirror providers

Mirror endpoints and URL-rewrite rules moved out of the code into `providers.toml`. They're
compiled into static lookup tables at build time by the `mdt-macros::providers!` proc macro
(MDT's take on `rust-i18n`'s `i18n!`), so nothing is parsed at runtime.

- **Custom mirrors** — `Mirror::Custom("name")` picks a `[name]` section from `providers.toml`,
  with its own endpoints and ordered `mirror` / `maven` rewrite rules (plus a `maven_exclude` list).
- **Fallible lookup** — `ManifestUrls::new` split into `try_new` (returns `MdtError::UnknownProvider`)
  and `new` (panics).
- **New error** — `MdtError::UnknownProvider`.
- **Breaking** — `Mirror` is now `Clone` (not `Copy`), and mirrors are passed by reference (`&Mirror`).
- **Docs** — `docs/mirror_toml.md` and `docs/mirror_uri.md`.
- **Workspace** — `mdt-macros` is now a separate member crate.

## [0.3.1] | Dependencies Update

Added the README badge and bumped Tokio.

## [0.3.0] | First Released

First release of MDT as a library.
