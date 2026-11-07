# 更新日志

## [0.7.0] | 安全加固与可观测性重构

> [!IMPORTANT]
> 本次只有一处破坏性变更：`MdtError::PartialDownloadFailed` 的载荷由 `Vec<String>` 改为
> `Vec<FailedDownload>`（见第一条）。其余改动全部是增量式的。

- **破坏性变更——`PartialDownloadFailed` 现在携带失败原因**：变体改为 `PartialDownloadFailed(Vec<FailedDownload>)`，`FailedDownload { url, reason }` 为每个文件保留最后一次错误信息。上层终于可以区分「镜像回了 404」和「本地磁盘写满了」，而不再只看到一个光秃秃的 URL。原始 URL 仍然保留，供重试等程序化用途；`Display` 与 `Debug` 都会对其 userinfo 脱敏。

- **Zip Slip 防御**：`unzip_jar` 与原生库解压现在都会先过一遍 `safe_path::validate_extract_path`，之后才碰文件系统。绝对路径、任意分隔符下的 `..` 组件、Windows 盘符前缀、NTFS 备用数据流（`file.txt:hidden`）、NUL 字节，以及（仅在 Windows 上）`CON`、`COM1` 这类设备名一律拒绝；第一个非法条目就会以新增的 `MdtError::UnsafeArchivePath { entry, reason }` 中断整次解压，而不是悄悄跳过。目标路径若已存在且是软链接，同样拒绝写入。

- **物理拷贝 fallback 不再被软链接套走**：虚拟目录挂载从 hardlink/symlink 退化到物理拷贝时，目标路径改用 `symlink_metadata`（绝不用 `metadata`）检查，命中软链接直接拒绝并报 `Security: Symlink escape blocked`；文件创建改用 `create_new`（新增 `io::copy_create_new`），只要该路径上已有任何东西（包括软链接）就会原子失败。这条路径上不再使用 `fs::copy` / `File::create`。

- **凭证脱敏**：新增 `redact_credentials`（独立的 `security` 模块，并由 `customize` 重新导出，原有导入路径照旧可用），把 `scheme://user:pass@host` 变成 `scheme://***:***@host`。它幂等、对 UTF-8 安全、复杂度线性，且不碰路径与查询串（`?token=` 属于资源本身，无法与正常参数区分）。它被施加在唯一出口 `customize::debug_line` 以及 `FailedDownload` 的 `Display`/`Debug` 上——因此带口令的镜像 URL 不可能经由 MDT 落进启动器日志。

- **`ProgressReporter::debug`**：新增方法，默认空实现，现有 reporter 无需改动即可编译。`StderrReporter` 会以 `[mdt] …` 的形式把它打印在固定的进度行之上；库内所有 `eprintln!` 现在都走这条通道，启动器终于可以拦截或静默 MDT 的内部追踪，而不是被迫接受直接写 stderr 的日志。公共自由函数签名一律不变（内部改走新增的 `*_reported` 变体并传入 `NoopReporter`），需要接管日志请用增量的 `mount_all_with_reporter` 与 `VirtualDir::mount_with_reporter`。

- **运行时镜像覆盖可以热重载**：`RUNTIME_PROVIDERS` 由 `OnceLock` 换成 `RwLock<Option<…>>`，加载失败不再被永久缓存。新增 `reload_runtime_providers()`（失败返回带类型的 `MdtError::InvalidProviderConfig`）与 `runtime_providers_error()`，长生命周期的启动器改完 `providers.runtime.toml` 后无需重启即可生效。

- **修复——`providers.runtime.toml` 此前从未被正确读取**：旧实现用 `str::parse::<toml::Value>()` 解析，而它解析的是单个 TOML **值**而非文档，于是任何真实的 `[section]` 文件都会报 `unexpected content, expected nothing`，覆盖表被静默留空。现已改为 `toml::from_str::<toml::Table>`。

- **镜像改写改为严格前缀匹配**：`apply_mirror` / `apply_maven_mirror` 不再使用 `String::replace`。只有当 URL 以规则的 `from` 为前缀、且前缀在 **URL 边界**上收尾时才会改写，因此 `https://piston-meta.mojang.com` 不会再误伤 `https://piston-meta.mojang.com.evil/…`，也不会改写别的站点查询串里嵌着的 Mojang URL。拼接时保证只留一个分隔符（不会出现 `//`，也不会粘成 `mavencom/x.jar`）；`maven_exclude` 改为按 URL 的 host（精确或子域）匹配，而不是按整条 URL 的文本匹配。

- **清单信任边界**：只有 SHA-1 是真正的 40 位摘要、URL 为 `http(s)`、且 Maven 相对路径不会逃出 `libraries/` 的条目才会生成下载任务（`is_sha1_hex`、`is_safe_http_url`、`validate_extract_path`），其余条目直接丢弃而不是「不校验就下载」。asset index 描述符（`id`、`sha1`、`url`）在变成文件名或缓存键之前先校验，`AssetRegistry::load` 拒绝把路径当 `index_id`；加载器 profile 里库的 `url` 不是 `http(s)` 时直接报错（SSRF）。

- **缓存命中不再等于「文件存在」**：LiteLoader 的 JAR、`libraries/` 下的副本以及 `download_artifact_verified` 的快速路径，都必须匹配持久化的 SHA-1 sidecar；无法验证的文件会被重新下载并重新计算哈希，而不是直接挂载。快速路径在目标已匹配时直接返回，省掉缓存查询与拷贝。

- **解压之外的路径也不再跟随软链接**：`copy_dir_sync` 拒绝源或目标处的软链接，并用 `create_new` 写副本（不再用 `fs::copy`）；`swap_loader_json_in_place` 与 `read_vanilla_launch_args` 遇到软链接的版本 JSON 直接拒绝；`object_path` 把过短/非法的哈希隔离到 `invalid/` 下，而不是在 `&hash[..2]` 上 panic；`sync` 改为依赖下载器那一次 size+SHA-1 预检，不再对 registry 副本重复全量哈希。

- **内部**：`tempfile` 加入 dev-dependencies。新增一批负向测试，覆盖 zip-slip 条目、被预置的与悬空的软链接、`create_new` 覆盖尝试、`..`/ADS/设备名路径、凭证泄漏的各种形状以及「修好文件后重载」。MSRV 保持 1.85 不变。

## [0.6.10] | 极简构建与纯引擎错误

- **`minimal` feature**：如果你想要在环境受限的嵌入式开发中使用 MDT，那么只使用最小子集会是你不错的绝佳选择。最小子集仅包含一套极度精简化的核心引擎体验——`asset`、`registry`、`virtual_dir` 与 `dual_loader` 模块被剥离，`install_modloader` / `full_install` / `setup_dual_loader` 被禁用，下载管线中的原生库解压被跳过。这也意味着你需要自行实现一套适用于嵌入式受限环境的特定功能，例如通过 `.downloader()` 注入自定义 `Downloader`。

- **`MdtError::MissingDownloader`**：当 `tokio` 与 `blocking` 均未开启、且没有注入自定义 `Downloader` 时，`MdtClientBuilder::build()` 现在返回该错误，而不再 panic。

## [0.6.9] | 阻塞模式与三种传输形态

- **同步 `BlockingClient`**：开启 `blocking` 即可从普通 `std::thread` 调用同名 API，无需任何异步运行时。它用 `futures::executor::block_on` 驱动内核，并在编译期把 `tokio::fs` → `std::fs`、`tokio::time` → `std::thread`（内部 I/O shim）。

- **三种传输形态**：`default = ["tokio"]` 保留 `tokio` + `reqwest`；`blocking` 用 `ureq`（新增 `UreqDownloader`）；不开任何 feature 则是纯引擎，没有默认下载器（自行注入 `Downloader`）。`blocking` 的运行时依赖树不含 `tokio` / `reqwest`。

- **新错误**：`MdtError::UreqError`（阻塞传输）。

## [0.6.7 & 0.6.8] | 网络、磁盘与镜像基础设施

> [!NOTE]
> `mdt-macros` 宏库已更新至 v0.4.2 版本。此次更新仅是为了反映相关配置文件的更名，API 未作修改，也不会影响现有项目。

- **可配置网络**：`MdtClientBuilder` 新增 `.proxy(...)`、`.connect_timeout(...)`、`.read_timeout(...)`。共享客户端会协商 HTTP/2（回退 HTTP/1.1）并维护一个积极的连接池，批量下载资源时能复用热连接。

- **磁盘满防御**：新增 `MdtError::DiskFull`，当写入命中系统磁盘满错误（Windows 39/112、Linux 28）时，不再抛一个笼统的 I/O 错误，而是精确地上报「磁盘空间不足」，启动器可以据此给出明确提示。

- **字节级进度与汇总**：`ProgressReporter` 新增 `progress_bytes` 与 `summary`（默认空实现，旧 reporter 无需改动即可编译）。下载过程现在会实时报告剩余字节，结束后打印 `Downloaded N component(s) (... in ... largest was ...)` 汇总；`format_bytes` / `format_duration` 负责人类可读的格式化。

- **运行时镜像覆盖**：静态配置改名为 `providers.compile.toml`；通过 `providers.runtime.toml`（或环境变量 `MDT_RUNTIME_PROVIDERS`）可以在运行时覆盖 `Mirror::Custom` 的端点与重写规则，无需重新编译。

## [0.6.6] | 双加载器支持与就地命名

- **就地安装现在能正常启动了**。`copy_vanilla = false` 时，加载器 JSON 会顶替原版 JSON 的文件名（`1.16.5.json`），启动器就能识别出这是个模组加载器版本，而不是视而不见。纯净的原版 JSON 备份为 `1.16.5.json.mcvanila` 供卸载流程使用。`read_vanilla_version_id` 通过 `inheritsFrom` 解析基础版本，LiteLoader 的启动参数读取也优先用 `.mcvanila` 备份，所以改名后安装与卸载依然正常。

- **双加载器（Fabric + Quilt）**：`MdtClient::setup_dual_loader` 把两个加载器装进同一个版本目录，并保留三份 JSON（`<mc_version>.json.mcvanila`、`<mc_version>+Fabric<ver>.json`、`<mc_version>+Quilt<ver>.json`）外加活跃的 `<mc_version>.json`。`switch_dual_loader_context`（或 `mdt::dual_loader::switch_loader_context`）在两者之间切换，不碰备份。状态检测靠解析每个 JSON 的 `mainClass` 与 `libraries`——绝不看 `id` 或文件名，改文件夹名也骗不过它。

- **最新稳定版加载器**：`latest_stable_loader_version` 从清单里挑最新的*稳定* Fabric/Quilt 版本（现在会解析 `LoaderVersionEntry.stable`）；没传显式版本时 `setup_dual_loader` 就用它。

- **切换上下文时的 Windows 文件锁重试**。JSON 交换在遇到 Windows 共享冲突（错误 32）时最多重试 3 次（间隔 100ms），Defender 或正在运行的启动器不会再打断切换。

## [0.6.5] | 安全更新

> [!NOTE]
> **v0.6.3 与 v0.6.4 均未被撤回，因为在特定条件下，它们可能不会触发 OS 错误 32（文件锁异常）**。如果你遇到了这个问题，更新到 v0.6.5 即可解决，无需对现有代码做任何改动。Linux 与 macOS 用户可以选择停留在 v0.6.3/4 或更新至 v0.6.5；v0.6.5 中的「安全补丁 II」仅针对 Windows 平台。

- **Cargo 风格阻塞等待**——当挂载碰到瞬时的 Windows 文件锁（共享冲突，原始 OS 错误 32，或拒绝访问）时，`mount_all` 不再三次 50ms 快速重试后就放弃。失败的挂载会被放入轮询队列，打印 Cargo 风格的
  `Blocking waiting for file lock on: ...（N files, retrying in 2s...）` 行，每 2 秒重试一次，最多等 30 秒。被锁定的文件绝不会被删除或截断来绕过锁——只会跳过并稍后重试。非锁错误仍然立即中止；若 30 秒后锁仍未释放，`mount_all` 会以 `MdtError::VirtualDirMountFailed` 返回，并列明受阻文件。

## [0.6.3 & 0.6.4] | 虚拟目录挂载

> [!NOTE]
> **v0.6.3 未被撤回，因为它可能会在特定条件下不会导致 OS 错误 32（文件锁异常）**。如果你遇到了，更新到 v0.6.4 即可解决此问题，无需对你的现有代码进行修改。Linux / MacOS 可以选择停留在 v0.6.3 或更新至 v0.6.4，v0.6.4 的安全更新补丁仅针对于 Windows 平台。*顺带说一句：Windows Defender 你的胆子真的是肥嘟嘟的*（怒）

- **虚拟目录**——全局注册表现在是「挂载」进本地 `.minecraft` 目录结构，而不是物理复制。对每个文件，MDT 依次尝试 **硬链接 → 符号链接 → 复制**：同盘安装即时且零额外占用，跨盘安装用符号链接，受限文件系统（FAT32、没有开发者模式的 Windows）则以复制回退并打印警告。`AssetRegistry`、`download_library` 和 LiteLoader 核心 JAR 都通过新的 `mdt::VirtualDir`（`mdt::virtual_dir::mount_all`）挂载；挂载失败由 `MdtError::VirtualDirMountFailed` 上报。瞬时的 Windows 文件锁（共享冲突，原始 OS 错误 32）会在落到下一策略前最多重试 3 次（间隔 50ms）。

## [0.6.2] | 确定了最低兼容的 Rust 版本

> [!NOTE]
> 本次更新只是降低了最低要求的 Rust 工具链版本，API 合约没有任何破坏性变更，请放心。

经过慎重考虑并观察 Rust 社区常用 edition 后，我们决定把 MDT 的最低兼容 Rust 版本降到 1.85.0（Rust 2021 Edition），让更多项目和开发者能更轻松地采用这套工具。

与此同时，[宏库](https://crates.io/crates/mdt-macros)将随本次最低 Rust 版本下调同步更新到 v0.4.1。由此带来的不便，我们深表歉意。

## [0.6.1] | 例行功能更新

- **定制化 trait**——一组公开的扩展 trait（`ProgressReporter`、`Downloader`、`RegistryCache`），各自带有可用的默认实现（`StderrReporter`、`HttpDownloader`、`FsRegistryCache`）。`ProgressReporter` 和 `Downloader` 已接入热路径，可通过 `MdtClientBuilder::progress_reporter` / `::downloader` 注入；`Downloader` 现在同时覆盖文件（`download`）与元数据（`fetch`），因此整个传输层——不只是进度——都可替换。`RegistryCache` 以内容寻址字节存储构建块（`FsRegistryCache`）的形式提供。

## [0.6.0] | 注册表 & 安全更新

- **韧性下载**——单个资源超时或断连不再中断整批任务。每个文件最多重试 3 次（指数退避 + 抖动），之后软失败并通过 `MdtError::PartialDownloadFailed` 上报，其余队列仍继续完成。
- **Cargo 注册表风格资源系统**——以 `$HOME/.mdt/registry` 为根（可用 `MDT_HOME` 覆盖）、跨 `.minecraft` 目录共享的*全局*注册表，遵循标准布局：原版资源在 `src/mojang/<version>/main/`（按 SHA-1 内容寻址），`assetIndex` JSON 在 `index/mojang/.cache/<sha1>`，加载器库与方式 A 的 installer JAR 在 `<Modloader>/<version>/`。先加载索引；每个文件先下载进注册表（临时文件 + 校验和验证 + 移动），再*复制*——绝不硬链接——到本地 `assets/objects/` 或 `libraries/` 目录。模组加载器产物（库与 installer JAR）通过其 Maven `.sha1` sidecar 做校验（依次回退 `.sha256`、`.md5`），哈希持久化到缓存文件旁，使热缓存命中离线仍可验证。

## [0.5.3] | 功能更新：我们小小调了下终端体验

- **Cargo 风格进度**——安装与下载现在会向 stderr 打印友好的状态行：`Updating <source> index`、`Downloaded <file> by <Minecraft/Loader> <version>`，以及节流的 `Downloading x/y Component(s) ...` 追踪器，该追踪器始终钉在终端底部（在 TTY 上用 `\r` 原地刷新）。`stdout` 保持干净，供下游启动器使用。

## [0.5.2] | 热修复补丁 2：别再杀掉慢下载了（笑）

0.5.1 已撤回：全局 30 秒*总*请求超时会在中途掐断大而健康的下载（client.jar、资源）。现在客户端改用 15 秒连接超时 + 30 秒*读*超时（每收到字节就重置），慢下载得以存活，而彻底卡死的镜像仍会超时。小的元数据抓取（清单、加载器 profile）保留 15 秒的单请求超时。

## [0.5.1] | 热修复补丁 1：很高兴告诉你，我们写了个 bug :D

> [!CAUTION]
> **该版本遭到撤回**。对于已有项目，继续使用该版本会导致不可预测的严重问题。对于新项目，你通常不能通过 Cargo 安装该版本。

0.5.0 已撤回：修复加载器安装死锁，并加入韧性网络回退。

## [0.5.0] | 加载器（卸载）安装机制更新

> [!CAUTION]
> **该版本遭到撤回**。对于已有项目，继续使用该版本会导致不可预测的严重问题。对于新项目，你通常不能通过 Cargo 安装该版本。

- **卸载**——`MdtClient::uninstall_modloader` 通过 SHA-1 清单比对把加载器还原回纯净原版。它先把清单表预载入内存（无 N+1 网络查询），再用多线程桶排序扫描器扫描磁盘，并应用 MCR 标记（`[Hash].pinned` 已修改、`[Hash].pinned.add` 已新建、`[Filename].pinned.remove` 已删除）。`mods/` 目录会被保留。重复运行是安全的。
- **1 个公开函数破坏性变更**——`MdtClient::install_modloader` 现在接收原版版本目录（`&Path`）加一个 `copy_vanilla: bool`，取代原来的 `&str` 版本号。Minecraft 版本从该目录 `version.json` 的 `id` 字段读取；JSON 缺失或格式错误时以 `MdtError::InvalidManifest` 快速失败。
- **写时复制**——`copy_vanilla == true` 时，先把原版目录复制进隔离的 `versions/<mc>+<Loader><ver>`（或官方安装器的 `-forge-`/`-neoforge-`）目录；`false` 时加载器原地安装，改动原版目录。
- **新错误**——`MdtError::UninstallFailed`。
- **韧性下载**——每个加载器下载现在都遵循「镜像优先、官方兜底」策略，共享 HTTP 客户端带连接/请求超时（15s/30s），卡死或 403 的镜像会回退到官方源而不是挂起。
- **宏名称与机制破坏性变更**——`mdt_install!` 宏更名为 `full_install!`，现在一次装完原版和加载器（带一个控制写时复制的五参形式）。它展开为新的 `MdtClient::full_install`。

## [0.4.0] | 自定义镜像 provider

镜像端点与 URL 重写规则从代码里移到了 `providers.toml`。它们在编译期由 `mdt-macros::providers!` 过程宏编译成静态查找表（MDT 对 `rust-i18n` 里 `i18n!` 的借鉴），所以运行时什么都不解析。

- **自定义镜像**——`Mirror::Custom("name")` 选取 `providers.toml` 里的一个 `[name]` 段，拥有自己的端点与有序的 `mirror` / `maven` 重写规则（外加一个 `maven_exclude` 列表）。
- **可失败查找**——`ManifestUrls::new` 拆分为 `try_new`（返回 `MdtError::UnknownProvider`）和 `new`（panic）。
- **新错误**——`MdtError::UnknownProvider`。
- **破坏性变更**——`Mirror` 现在是 `Clone`（不再是 `Copy`），镜像按引用（`&Mirror`）传递。
- **文档**——`docs/mirror_toml.md` 和 `docs/mirror_uri.md`。
- **工作区**——`mdt-macros` 现在是独立的 member crate。

## [0.3.1] | 依赖更新

加了 README 徽章，并升级了 Tokio。

## [0.3.0] | 首次发布

MDT 作为库的首次发布。
