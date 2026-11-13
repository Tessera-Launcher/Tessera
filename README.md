# Tessera

桌面版 Minecraft: Java Edition 启动器。界面基于 `gpui-kit`，无 WebView、无 GTK 依赖；
启动逻辑与界面分离，核心在 `tessera-core`。数据全部放在可执行文件旁的 `data/`，
不写用户主目录。

## 状态

已可用：扫描并启动本地版本、微软 device code 登录（官方 / 自定义 app id /
`azure_app_ids.toml` 清单）、离线登录、版本与 5 种 Mod Loader 下载、Adoptium JRE 下载、
令牌刷新、卸载与共享库/资源回收、原版 AOT 缓存训练（含 1.0–1.5.2 的 legacy 路径）、
唱片与背景音乐播放、日志落盘、按键全键盘操作。

未做：mods / 资源包管理、服务器列表、实例设置页、设置界面（仍手改 `配置.toml`）、
自动更新、主题与 Unifont。

## 运行

```sh
cargo run              # 默认成员就是 tessera-gui
TESSERA_DATA=/tmp/t cargo run   # 临时换数据目录
```

Linux 编译需要 ALSA 头文件（Debian/Ubuntu `libasound2-dev`、Fedora `alsa-lib-devel`）；
Windows / macOS 无需额外系统包。音乐播放不依赖 FFmpeg 或系统编解码器
（rodio + symphonia 纯 Rust 解码 Vorbis，cpal 输出）。

## 数据布局

```
<exe>/data/
├── 游戏/          versions / libraries / assets
├── 运行时/        下载的 JRE，一个版本一个文件夹（21.0.13+9、8u512-b09）
├── 凭据/账户/     一个账户一个 json，会话和离线账户都在这里
│                 （玩家名.json / 玩家名 (1).json …）
├── 凭据/启动器/   azure_app_ids.toml：可选的启动器 app id 清单
├── 缓存/          下载缓存（MDT 的内容寻址注册表 + fastmenu-agent.jar）
├── 日志/          tessera.log：每次启动追加
└── 配置.toml      堆大小 / 窗口尺寸 / GitHub Proxy（首次改动时才写）
```

`MDT_HOME` 在启动时指向 `data/缓存`（环境里已有则不覆盖）；`assets/objects/` 里的文件
是注册表的硬链接，同一份数据只占一次磁盘空间。

## 键位

| 键 | 作用 |
|---|---|
| `↑` `↓` `j` `k` / `Home` `g` / `End` `G` | 选择 |
| `Enter` / 双击行 | 启动（列表里是切换/确认） |
| `x` `Delete` | 卸载，先问一次再按一次才删 |
| `d` / `a` / `v` / `m` / `l` / `r` / `h` | 下载 / 账户 / Java / 音乐 / 登录 / 重扫 / 折叠基础版本 |
| `PageUp` `PageDown` | 翻日志（主页） |
| `Tab` | 切换焦点（下载 / Java / 音乐页） |
| `Esc` `q` | 返回上一屏；主界面上是退出 |
| `Ctrl-C` | 退出 |

音乐页：`←→` 切换列表、`↑↓`/`jk` 选择、`t` 唱片 ⇄ 背景音乐、`Space` 播放/暂停、
`Enter` 从头播放、`o` 播放顺序、`r` 重扫、`Esc` 返回（并停止播放）。

侧栏切换页面不会重置任何页面：下载页保留搜索词、筛选、展开的快照族和正在进行的安装，
音乐页保留选中的版本、曲目和播放位置（切到别的页面时音乐继续播放），Java 页保留列表
和选择。只有 `Esc`/`q` 离开页面才算“关闭”，音乐页会因此停掉声音。

## 行为要点

**启动**：跟随 `inheritsFrom` 合并版本链（最多 8 层），按 `rules` 过滤 classpath、
写 `-Djava.library.path`，替换 `${...}` 占位符（`natives_directory` 一律绝对路径）。
工作目录是版本目录本身，所以 mods 要放进 `versions/<id>/mods/`，不是 `data/游戏/mods/`。
游戏进程由启动器后台 `wait`，退出码写进状态栏和日志。

1.13–1.16.5 的 LWJGL 3.1/3.2 在 `dlopen` 前会把 native 路径转成 ASCII，数据目录含中文时
会加载 `libglfw` 失败。启动器会为这些版本在临时目录建一个纯 ASCII 的 natives 镜像：
Unix 用 symlink，Windows 优先用 NTFS junction（不需要管理员权限），失败才退回 symlink
或复制。1.12.2 及更早（LWJGL 2）和 1.17+（LWJGL 3.3+）不受影响。

原版 1.8.x 是最后一批带 netty 4.0.23 的版本，而那个 netty 在现代 JDK 上用不了 epoll：
它探测 `Unsafe` 失败后一律分配堆缓冲，epoll 的原生读用 `GetDirectBufferAddress` 拿不到
地址，进服务器就报 `Unable to access address of buffer`（单机走 LocalChannel 所以正常）。
netty 4.0 没有 `noNative` 开关，启动器改为把 `io.netty.native.workdir` 指向 Tessera
可执行文件本身——那个路径是个文件，netty 建不出解压目录，`Epoll.isAvailable()` 变回
false，游戏自动退回 NIO（只用普通 ByteBuffer，不碰地址）。只在原版 1.8.x 上加这个参数：
Mod Loader profile 按元数据固定用 Java 8，那里的 epoll 是好的。classpath 不动，AOT 缓存
不受影响。

**Java**：只用 `data/运行时/` 里启动器自己下载的运行时，不查 `JAVA_HOME`/`PATH`。
元数据里的 `javaVersion.majorVersion` 只用来判断“这个版本要不要更高的 Java”：
1.20.5+ → 21，1.18–1.20.4 → 17，1.17 → 16（实际用 17），1.16.5 及更老 → 8；
缺元数据时按版本号推。但原版（含 1.0–1.5.2 的 LaunchWrapper 时代）一律锁在
**Java 28 EA**——启动补丁只在那一个运行时上验证过，磁盘上装了别的 Java 也不会用它，
没装就提示去下载。要跑 AOT 缓存也用同一个 Java（缓存只在写它的 JVM 上重放），
两条路径因此一致。Mod Loader profile 仍按元数据 pin，要求 8 的就只接受 8。

**下载**：`d` 打开选版本界面，`/` 搜索、`f` 循环类型、`Space` 展开快照族（搜索/筛选时
一律平铺）、`Tab` 切焦点。右下角切换下载源（默认 BMCLAPI，`s` 键），只影响本次运行，
不写盘。Loader 版本取该 MC 版本的最新构建，Forge/NeoForge 安装器同样跑在
`data/运行时/` 的 Java 上。Loader 只装自己的 profile（`versions/<mc>-<loader>-<版本>/`
里只有一个 json，`inheritsFrom` 指向原版）：客户端 jar、natives、assets 都由那一份原版
提供，不会在每个 Loader 目录里再复制一遍。进度页可 `Esc` 退出，下载在后台继续，完成后
自动重扫版本。

**Java 下载**：`v` 打开，Adoptium 的 8 / 17 / 21 / 25 / 28 EA 最新 JRE，元数据与
SHA-256 全部来自 API。默认清华源（无 EA，28 走 GitHub），`s` 换源，`p` 设 GitHub Proxy，
`u` 批量更新可更新的大版本。装到 `data/运行时/<版本号>/`，启动时优先使用。

**AOT**：训练由主页的「开始 AOT 训练」手动触发，安装不会背着玩家启动游戏。任何原版
都可以训练（Loader profile 除外，自定义类加载器加载的类进不了缓存）：强制 Java 28 EA、
去签名客户端副本、unseal 密封库、`-javaagent` 全量预加载，缓存写
`versions/<id>/<id>.aot`。只有同一 Java build 与 agent 都在时才在启动时启用；Java 升级
后需要重训。26.x（含 Loader profile）启动还带 `fastmenu-agent.jar` 跳过菜单淡入。
训练失败不影响安装本身。

**音乐**：曲目名来自 `assets/indexes/<id>.json`，字节来自 `assets/objects/<sha1>`，
不信任 `sounds.json`；时长由 Ogg granule 算出，播放位置来自解码器时钟。`Esc`/`q`
关闭音乐页会停掉声音；用侧栏切到别的页面则继续播放，切回来看到的是同一首、同一个位置。

**卸载**：删除版本后按引用计数清理 `libraries/`、`assets/{objects,indexes,virtual}`、
`resources/` 和下载缓存；引用了它的 Loader 版本会一并删除。引用集故意取超集、不做
`rules` 过滤；任何版本 JSON 读不出来就整个跳过清理。硬链接按 (dev, inode) 归组统计
释放量，不虚报。

**账户与登录**：`a` 管理 `data/凭据/账户/*.json`（`0600`），同一玩家可登录多次
（`Steve (1).json`），当前账户和刷新都按文件路径认人。`l` 打开登录，身份只有三项：

- **官方启动器**：直接进 device code 页（可打开/复制链接、复制设备码）。
- **自定义 app id**：一个输入框加一张 `azure_app_ids.toml` 的凭据表。填了 id 就用填的，
  留空就登录光标所在的那条现有凭据；表读不出来时只说原因，不会拿别的 id 顶上。
- **离线登录**：输入用户名即可，不发任何请求，联机时会被服务器拒绝。旁边「允许任意
  用户名」不勾时按原版规则校验（3–16 个字母、数字、下划线），勾上后中文、空格等都能用。
  它和正版会话一样写成 `凭据/账户/<名字>.json`（`0600`，`offline: true`，没有 `session`），
  所以会出现在账户管理里、重启后还在，右侧灰字是 `Offline`；`d` 连按两次删掉这个文件。
  启动时按 `offline` 标记跳过令牌刷新，直接用它算出的离线 UUID 开局。

启动前会在后台刷新令牌，网络失败时沿用旧令牌，需要重登则退回离线身份（有名字才退回，
没有就直接打开登录界面）。**没有任何账户时不会拿 `Player` 顶替**：按启动会跳到身份
列表，先选一种登录方式。

**配置**：`data/配置.toml`（手改，缺失即默认）：

```toml
memory_mb = 2048
window_width = 854
window_height = 480
github_proxy = ""
```

下载源故意不持久化。每次启动在 `data/日志/tessera.log` 追加日志，窗口里只留 500 行。

## 仓库结构

```
crates/tessera-core/  纯逻辑 + 屏幕状态机：发现、启动参数、下载、Java、账户、清理、音乐
crates/tessera-core/src/app/   状态机（App / Screen / Effect）+ 每个流程的状态
crates/tessera-core/src/launch/  启动组装：build（参数/类路径）、manifest（版本链）、
                              java（运行时选择）、natives（native 路径与 ASCII 镜像）、
                              aot_cache（选择已训练缓存）、agents（内嵌 Java agent）
crates/tessera-core/assets/    AOT 预加载、fastmenu、legacy 三个 Java agent 的源码和 jar
crates/tessera-gui/   桌面前端：gpui-kit 窗口，复用同一个状态机（每屏一个文件）
vendor/mdt-minecraft  下载层（upstream + 本地修复，见其 VENDORED.md）
vendor/minecraft-auth 登录层（device code → 启动凭据）
```

`tessera-core` 不含 `gpui`；`vendor/` 下两个 crate 各自是独立 workspace 根，在本仓库
`[workspace] exclude` 里排除，作为 path 依赖引入。分工是：下载 → `mdt-minecraft`，
登录 → `minecraft-auth`，Java 运行时 → Adoptium，启动与清理 → 本仓库。

## 许可

GPL-3.0-or-later。`vendor/mdt-minecraft` 为 MIT，`vendor/minecraft-auth` 为
LGPL-3.0-or-later，均可并入 GPLv3。
