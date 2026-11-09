# Tessera

终端里的 Minecraft: Java Edition 启动器。TUI（`ratatui` + `crossterm`），
无 WebView、无 GTK、无 GPU 依赖，可在 SSH 里跑。

## 当前状态

可用的启动器：扫描并启动本地版本、微软 device code 登录（官方 / 自定义 app id /
`azure_app_ids.toml` 清单）、版本与 5 种 Mod Loader 下载、Adoptium JRE 下载、
令牌刷新、卸载与共享库/资源回收、日志落盘。主界面扫描 `<root>/versions` 并显示
基础版本 / Java / 主类 / 目录 / 大小；游戏退出后状态栏会报告退出码。

未做：mods/资源包管理、服务器列表、实例设置页、自动更新。

### 便携布局

二进制所在目录 + `data/` 就是全部，**启动器不写用户主目录**：

```
<exe>/data/
├── 游戏/          versions / libraries / assets（--dir 可覆盖）
├── 运行时/        下载的 JRE，一个版本一个文件夹（21.0.13+9、8u512-b09）
├── 凭据/账户/     一个会话一个 json（玩家名.json / 玩家名 (1).json …）
├── 凭据/启动器/   azure_app_ids.toml：可选的启动器 app id 清单
├── 缓存/          下载缓存（见下）
├── 日志/          tessera.log：每次启动追加（内存里的面板只留 500 行）
└── 配置.toml      堆大小 / 窗口尺寸 / GitHub Proxy（首次改动时才写）
```

`缓存/` 是 vendor 里的 MDT 的内容寻址注册表：它默认写 `$HOME/.mdt/registry`，
启动时被 `MDT_HOME` 指到 `data/缓存`，实际路径是 `data/缓存/.mdt/registry`。
`assets/objects/` 里的文件是那个缓存的**硬链接**，所以同一份数据在磁盘上只占一次
空间（也是卸载后资产清理释放不出空间的原因，见卸载一节）。环境里已有 `MDT_HOME`
时不覆盖，方便临时改用别处。

`TESSERA_DATA` 可覆盖 `data` 根目录（测试和源码运行时用）。

```sh
cargo run                                   # 默认 $HOME/.minecraft
cargo run -- --dir /tmp/mdt-1899            # 指定目录
cargo run -- --once --dir /tmp/mdt-1899     # 渲染一帧后退出，不需要 TTY
cargo run -- --once --login                 # 一帧里直接展示登录面板
cargo run -- --once --jre                   # 一帧里直接展示 Java 下载界面
```

按键：`↑/k ↓/j` 选择，`g/G` 首尾，`PgUp/PgDn` 滚动日志，`l` 登录，`d` 下载版本，
`v` 下载 Java（`j` 已经用来往下选了，`v` 取"虚拟机"），`a` 账户，`r` 重新扫描，
`x` 卸载，`q/Esc` 退出，`Enter` 启动。

**从文件管理器双击**：文件管理器启动程序时不给终端，`enable_raw_mode` 会失败，
错误写进没人看的 stderr，窗口一闪都没有。`terminal.rs` 因此在 `stdin` 不是 TTY 时
把自己重新丢进一个终端模拟器再退出；终端里启动（shell、tmux、SSH）`stdin` 是 TTY，
这条路径不会触发，`tessera < 文件` 也照旧读文件。

三个系统的情况不一样，所以只有 Linux/BSD 需要猜：

- **Windows**：控制台子系统（Rust 默认）的程序被资源管理器双击时由系统分配控制台
  窗口，本来就能跑；只有从 GUI 进程里启动时才用 `cmd /C start` 补一个。
- **macOS**：Finder 双击 Unix 可执行文件本来就开 Terminal；从别的 App 启动时用
  `open -a Terminal` 补上（这条不转交参数，有参数时不走）。
- **Linux/BSD**：文件管理器一律直接 `exec`，没有终端。猜测顺序是
  `$TERMINAL` → 桌面自己的终端（`$XDG_CURRENT_DESKTOP`：KDE→`konsole -e`、
  GNOME/Unity/Cinnamon→`gnome-terminal --`、Xfce→`xfce4-terminal -x`、
  MATE→`mate-terminal -x`、LXQt/LXDE→`qterminal -e`）→ 装了就用的 freedesktop 助手
  `xdg-terminal-exec`（读 `~/.config/xdg-terminal-exec/xdg-terminals.list`）→
  固定名单（`alacritty`、`ghostty`、`foot`、`kitty`）→ `x-terminal-emulator`。
  每个终端的执行参数不同（`-x` / `--` / `-e`），表里的值取自 `xdg-terminal-exec`
  自带的 `execarg_default` 名单。

`xdg-terminal-exec` 排在本桌面终端之后是有意的：它是配置的正确来源，但发行版默认
会把任何声明了 `X-TerminalArgExec` 的终端选进来——Debian 13 上 `xfce4-terminal.desktop`
还没这个键，于是会给你一个 `uxterm`。想强制用它就把 `$TERMINAL` 设成自己的终端。

桌面快捷方式当然也还有效：`.desktop` 里 `Terminal=true`、`Exec` 指向二进制即可。

### Java 运行时

`v` 打开（点页脚的 `v Java` 也可以），从 Adoptium 取 **8 / 17 / 21 / 25 / 28 EA** 各个大版本最新的 **JRE**：

```
┌ 下载 Java（清华源） ────────────────────────────────────────────────────┐
│┌ Java 运行时 · linux/x64 → /data/运行时 ──────────────────────────────┐│
││  版本        最新 JRE              大小        状态                  ││
││▶ Java 8   8u512-b09           39.9 MiB   未安装                     ││
││  Java 17  17.0.21+9           44.6 MiB   已安装                     ││
│└──────────────────────────────────────────────────────────────────────┘│
│ 下载源  清华源  官方源（GitHub）                                        │
│ GitHub Proxy 如 gh-proxy.com                                            │
│  ↑↓ 选择 · Enter 下载 · s 换源 · p 改代理 · Esc 返回                    │
└─────────────────────────────────────────────────────────────────────────┘
```

- 元数据（文件名、大小、**SHA-256**）一律来自 Adoptium 的公开 API
  `api.adoptium.net/v3/assets/latest/{feature}/hotspot?...&image_type=jre`；
  28 还是 EA，走 `api.adoptium.net/v3/assets/feature_releases/28/ea?...`，只提供 **JRE**
- 下载源两份，`s` 或点标签切换，**默认清华源**：
  - **清华源**：`mirrors.tuna.tsinghua.edu.cn/Adoptium/<大版本>/jre/<arch>/<os>/<文件名>`
    （与官方同一份文件，实测 sha256 一致；拉不到时自动回落 GitHub）。
    清华源没有 EA：Java 28 会直接走 GitHub（Proxy 仍可用）
  - **官方源（GitHub）**：Adoptium API 指向的 GitHub release；因为国内常常连不上，
    可以填一个 **GitHub Proxy**（`p` 进入输入，或点那行）：填 `gh-proxy.com` 就先试
    `https://gh-proxy.com/github.com/<owner>/…`，再试 `https://gh-proxy.com/https://github.com/…`，
    最后才直连
- **下载完必校验 SHA-256**，不匹配就换下一个候选地址重试，全失败则在那一行显示原因
- 打开界面时会把**已安装的版本和 Adoptium 最新小版本比对**（例如 `8u502-b07` → `8u512-b09`），
  状态列写“可更新”；`Enter` 更新当前行，`u` 依次更新所有可更新的大版本。
  只比较启动器自己命名的目录（`8u…`、`21.0.…+…`、`28+…`），手改的 `运行时/21` 不会被当成旧版
- 装到 **`data/运行时/<版本号>/`**（如 `data/运行时/21.0.13+9/`，Java 8 是 `8u512-b09`），
  不在游戏目录里；解包时去掉压缩包的顶层目录，所以 `bin/java` 就在那个文件夹下。装新版本时
  同一大版本的旧文件夹会被删掉（日志里写明），不同的 `8 / 17 / 21 / 25 / 28` 各自并存
- 下载/校验/解包都在后台线程，Esc 可以退出界面继续下
- 启动时**只在 `data/运行时/` 里挑**（按版本号，够用就行；8 的档位仍然钉死 8）。
  `JAVA_HOME` / `PATH` 一律不看，`--java` 已移除（传了会明确报错）
- **Forge / NeoForge 的安装器也用同一个运行时**：这两个 Loader 靠官方的 installer jar
  安装，要跑一个 JVM。挑哪个 Java 和启动时是同一条规则（版本 json 的 `javaVersion`，
  没有就按版本号推），装不了就在日志里说"还没有下载 Java N：按 v 打开 Java 下载界面"。
  Fabric / Quilt / LiteLoader 不跑 JVM，不需要 Java
- 这个选择**不持久化**：下次打开回到清华源（和 `--mirror` 决定的是两件不同的事）

### 启动

`Enter` 用选中版本启动游戏。命令由 `launch.rs` 纯函数拼装，`--print-launch` 可以只看不跑：

```sh
tessera --dir /path/to/游戏 --print-launch 1.26.3
tessera --dir /path/to/游戏 --print-launch 1.26.3 --memory 4096
```

做的事：

- **版本链合并**：跟随 `inheritsFrom`（最多 8 层），子 profile 的 mainClass / libraries
  优先，父的 `minecraftArguments`、`assetIndex`、`javaVersion` 兜底
- **classpath**：按 `rules`（OS / arch / features）过滤，按 `downloads.artifact.path`
  取路径，没有就按 Maven 坐标推导；子 profile 在前（Loader 才能覆盖）
- 客户端 jar 找 `<id>.jar` → `<父id>.jar` → 目录里任意 jar（Loader 是复制父目录来的）
- **natives**：找 `<id>-natives` 或 `<父id>-natives`，写进 `-Djava.library.path`；
  1.19+ 没有 natives 目录也不影响（现代版本的 natives 直接在 classpath 上）
- **工作目录是版本目录本身**（`versions/<id>/`）：`mods/`、`saves/`、`options.txt`、
  `logs/` 都按实例各一份。这是有意的实例隔离，但和原版启动器共享 `.minecraft/mods`
  不同——把 mod 放进 `data/游戏/mods/` 不会生效，要放进
  `data/游戏/versions/<id>/mods/`（崩溃日志也在这个目录下）
- **占位符**：`${auth_player_name}` `${auth_uuid}` `${auth_access_token}` `${assets_root}`
  `${assets_index_name}` `${game_directory}` `${user_type}` `${classpath}`
  `${library_directory}` `${resolution_width}` `${natives_directory}` 等；未识别的
  占位符原样保留，便于排查。`natives_directory` 一律替换成**绝对路径**——游戏的工作
  目录是版本目录，相对路径会被解析到错的地方（曾经因此在每个版本目录里生成一个
  叫 `${natives_directory}` 的目录，25 MB）
- 老版本用 `minecraftArguments`（按空白切分），新版本用 `arguments.game/jvm` +
  rules 求值；缺 `--width/--height` 时补上
- 内存 `-Xmx`、`-Dminecraft.launcher.brand/version`，macOS 自动加 `-XstartOnFirstThread`
- 启动失败不会静默：缺库、缺 jar、Java 版本不对都会在日志里说明原因
- 游戏进程由启动器后台 `wait`：退出后状态栏和日志会写明退出码，非 0 时给出
  `versions/<id>/logs/latest.log` 的位置（没 wait 的话既会留僵尸进程，也看不出
  游戏是不是两秒就崩了）

**Java 选择**：只用启动器自己下载的运行时（`data/运行时/`），**不再查 `JAVA_HOME` 或
`PATH`，也没有 `--java`**。理由很直接：游戏跑在哪个 Java 上，启动器必须知道并能解释——
环境里的 Java 是什么版本、什么时候变的，都不在启动器的控制之内。

- **需要哪个版本**：版本 json 里的 `javaVersion.majorVersion` 说了算；没有这个字段
  （老元数据、手写的 profile）就按版本号推：
  | Minecraft | Java | 备注 |
  |---|---|---|
  | 1.20.5 及更新 | 21 | |
  | 1.18 – 1.20.4 | 17 | |
  | 1.17 | 16 | 下载界面不提供 16，实际用 17 |
  | 1.16.5 及更老 | 8 | 含 alpha/beta 等解析不出数字的 id |
- 要求 Java 8 的版本（1.7–1.16）**只接受 8**——17/21 上会以 `NoSuchMethodError`
  的形式崩在几十秒之后，给个明确的"需要 Java 8"比让它崩更好
- 要求 17/21/25 的版本接受**同版本或更高**；同版本优先，否则取满足条件的最小值
- 选中的运行时会跑一次 `java -version` 复核，目录名和实际版本对不上就直接报错
- 缺运行时不会崩：日志里写"还没有下载 Java N：按 v 打开 Java 下载界面"，并列出已有的版本

**凭据与令牌刷新**：按 `Enter` 时，如果当前账户有保存的凭据，会先在后台刷新：

- 令牌没过期 → 不联网，直接启动
- 令牌过期 → 用 refresh token 换新的；**微软会轮换 refresh token**，所以刷新后整份
  session 会原子写回 `data/凭据/账户/<玩家名>[ (N)].json`（临时文件 + rename）
- 刷新对象按**文件路径**定位：同名玩家的多个会话各刷各的
- 刷新被判定为"需要重新登录"（`invalid_grant` 等）→ 日志提示按 `l` 重新登录，
  本次仍以离线身份启动
- 刷新因网络失败 → **继续用磁盘上的旧令牌**，不因为一次网络抖动就拦下启动

刷新在独立线程里跑（同登录/下载），失败原因都会写进日志。未登录或没有凭据文件时
直接用离线身份（`user_type=legacy`、令牌 `0`、确定性离线 UUID）。

### 设置与日志

启动参数里只有两样会被记住，写在 `data/配置.toml`（手改也行，缺失就是默认值）：

```toml
memory_mb = 2048      # --memory 4096 会改成这里并保存
window_width = 854    # --resolution 1280x720 同理
window_height = 480
github_proxy = ""     # Java 下载界面里填的 GitHub Proxy，关掉界面也还在
```

下载源**故意不写进去**（见下载一节：一次切换不该变成长期设置）。游戏窗口默认
854×480，改分辨率用 `--resolution 宽x高` 或直接编辑这个文件；`--width/--height` 仍然
只作用于 `--once` 的渲染尺寸。

每次启动都会在 `data/日志/tessera.log` 追加一段日志（带一行 session 头），面板里的
500 行是内存副本，退出就没了。日志文件写不进去（比如磁盘满）时只提示一次，不影响
启动器的其余部分。

### 下载

`d` 打开选版本界面：

```
┌ 下载 BMCLAPI · 3 / 5 ──────────────────────────────────────────────────────┐
│┌ 搜索 ──────────────────────────────────────┐┌ 类型 ────────────────────┐│
││1.2▏  3/5                                   ││ 全部  正式版  快照  旧版  ││
│└────────────────────────────────────────────┘└──────────────────────────┘│
│┌ 版本 · 3 组折叠 · Space 展开 ─────────┐┌ 安装内容 ──────────────────────┐│
││  版本               类型      发布    ││▶ 无                            ││
││▶ ▸ 1.21.11-rc3 ~ 1.21.11-rc1  快照 3 个││  Fabric                      ││
││  1.21.11         release  2025-12-09 ││  Quilt                         ││
││  ▸ 25w46a ~ 25w41a        快照 6 个  ││  Forge                         ││
│└──────────────────────────────────────┘│ 仅安装原版                     ││
│                                        │          下载源 官方源 BMCLAPI ││
└────────────────────────────────────────────────────────────────────────────┘
```

- `/` 进搜索框，输入即过滤（子串、不分大小写）；`Esc` 先清空搜索，再按一次离开输入框
- `f` 循环类型筛选：全部 → 正式版 → 快照 → 旧版；也可直接点上面的标签
- **全部视图是树状的**：正式版各占一行，同一族快照（`1.21.9-pre9 … 1.21.9-pre1`、
  `25w37a … 25w31a`、`26.4-snapshot-7 … 26.4-snapshot-1`，含镜像那种
  `26.3-pre-3` 的写法）和同一时代的旧版（`b1.8.1 … b1.0`、`a1.2.6 … a1.0.4`、
  `c0.30_01c … c0.0.11a`、`rd-…`，按 id 前缀分时代，alpha 与 classic 不会混进
  同一行）折成一行 `新 ~ 旧`，类型列写"快照"/"旧版 Beta"/"旧版 Alpha"、发布列写条目数；
  点一下展开成 `▾ 标题` + 缩进的成员，再点标题折回去（`→`/`Space` 展开、`←` 收起，
  键盘在折起的组上只停一次）。折叠只发生在"全部"且搜索框为空时：其他筛选、以及
  搜索时一律平铺，**绝不会把搜到的版本藏进一个折起的组里**
- 折起的一行代表该族**最新**的版本：`Enter` 直接装它，Loader 列表也按它算
- 标题显示 `匹配 / 总数`，没有匹配时版本区写"没有匹配的版本"而不是留白
- `Tab` 循环 版本栏 → Loader 栏 → 搜索框，`↑↓` 选择（在搜索框里也照样移动版本），`Enter` 开始
- 滚轮按**绘制行**滚（一次 3 行，折起的一组只算一行），在版本面板内任意位置都有效，
  两端不循环；`↑↓` 仍是一次一行、到两端会绕回
- Loader 列表按版本过滤（Fabric/Quilt ≥1.14、NeoForge ≥1.21、LiteLoader ≤1.12.2）
- Forge / NeoForge 的官方安装器跑在 `data/运行时/` 里的 Java 上（见“Java 运行时”），
  系统里有没有 `java` 都不影响
- Loader 版本自动取该 MC 版本的最新构建，并在日志里写明选中的版本
- 换版本时若原 Loader 仍可用就保留选择，否则回落到"无"
- **下载源可以在面板里切换**：右下角（"安装内容"面板最后一行）是 `下载源 官方源 BMCLAPI`
  两个标签，点一下即切换，`s` 键在两者之间来回切；默认 `BMCLAPI`。切换只影响**这一次**
  运行的后续下载，**不写盘**，重开回到默认（或 `--mirror` 指定的那个）。面板标题里的
  `下载 BMCLAPI` 会立刻跟着变
- `--mirror` 决定启动时的初始源（`bmclapi` 默认 / `mojang`）

开始后进入安装界面：

```
┌ 日志 ───────────────────────────────────────────────────────┐
│ Downloaded lwjgl-2.9.4-nightly.jar by Minecraft 1.8.9       │
│ 本批完成：34 个文件，21.8 MiB，3.8 MiB/s，最大 8.1 MiB…     │
│                                                             │
└─────────────────────────────────────────────────────────────┘
┌ 安装进度 ───────────────────────────────────────────────────┐
│ ████████████████░░░░░░░░░░░░░░  50%  1.2 MiB/s              │
│ 10 / 40 个文件 · 剩余 5.0 MiB · 本批 10.0 MiB               │
└─────────────────────────────────────────────────────────────┘
```

- 中间日志区滚轮可滚（和其他日志区一致）
- 进度每 100 ms 最多报一次（而不是每批只有 10 次），几千个小文件也不会看起来卡死；
  界面在下载时以 50 ms 的节奏轮询，所以进度条是连续走的
- 底部：进度条 + 百分比 + 平滑后的实时速度 + 文件计数 + 剩余字节
- 进度条上的文字**跨过绿条时自动换色**：被条盖住的那部分变深色（黑字 + 条的颜色），
  没盖住的部分保持浅色，避免"白字压在绿条上"看不清
- 有字节总量时按字节算百分比，否则按文件数
- `Esc` 关界面但**下载在后台继续**，完成后自动重扫本地版本
- 完成/失败会停在结果页，`Enter`/`Esc` 返回列表

### 26.1+ 原版的 AOT 训练

安装**原版**（不选任何 Mod Loader）且版本是 **26.1 及之后**（含快照，如
`26.4-snapshot-7`）时，下载完成后会自动接一次 AOT 训练；1.x 和更早版本、所有
Mod Loader 安装都不做这一步。

- **强制 Java 28 EA**：训练固定使用 Java 28（`data/运行时/28+…`），没有就先用 Adoptium
  的 EA JRE 自动安装；不退回 Java 25。缓存和之后启动用的是**同一个 JVM build**
- **强制去签名**：训练前从已安装的原始 jar 生成 `versions/<id>/<id>.unsigned.jar`
  （删 `META-INF/*.SF`、`*.RSA`、`*.DSA`、`*.EC`，保留 MANIFEST 和 `META-INF/services`）。
  训练和之后的启动都用这个副本，原 jar 不动，JVM 不再把 client 类当 signed 类跳过
- **全量类加载**：训练时带 `versions/<id>/<id>.aot-agent.jar`（`-javaagent`），
  agent 在游戏 main 之前遍历整个 classpath，用 `Class.forName(name, false, …)`
  尝试加载每一个 `.class`；失败的通常是有可选依赖/确实不可加载的类，会跳过。
  实测 26.3 的 43,246 个候选类里加载了 43,040 个（99.5%）
- 训练启动时带 `-XX:AOTCacheOutput` 和 `-Xlog:class+load=info`，进度条显示
  `已记录 / 候选类数`
- 检测到 `Sound engine started` 后多停 10 秒，让字体/GUI 的懒加载类也进缓存，然后向
  JVM 发正常终止信号；JVM 在退出时组装缓存，这期间进度条显示“组装 AOT 缓存…”
- 缓存写在 `versions/<id>/<id>.aot`，旁边 `<id>.aot.json` 记录 `java_major`、
  `java_version`、`unsigned_jar`、`agent`。下次启动只有**同一 Java build**、去签名副本
  和 agent 都在时才加 `-XX:AOTCache`/`-javaagent` 并强制用同一个 Java；缺一样就忽略缓存。
  26.x 的 fastmenu agent 也同时出现在训练和启动命令里，避免参数不一致导致缓存失效
- 实测 26.3 全量缓存约 **454 MB**，训练+组装约 **4 分钟**；缓存大小随版本/库变化
- 训练失败、Java 下载失败、没有图形显示（SSH/headless），或系统不让进程正常退出，都
  **不影响安装本身**；结果页会写明“安装完成，AOT 训练失败/跳过”，缓存不完整时不会写元数据
- 只对原版有意义：Fabric 的 Knot、Forge/NeoForge 的 ModLauncher 是自定义
  ClassLoader，JEP 483 不缓存这些类，所以 Loader 安装直接跳过

### 26.x 菜单淡入跳过

26.x 的类名没有混淆，所以启动器给所有 **26.x**（`26.0` 起，含快照和 Loader profile）
启动都带一个小 Java agent：`data/缓存/fastmenu-agent.jar`。

- `LoadingOverlay` 的 1 秒淡出 + 1 秒 logo 淡出、`TitleScreen` 的 2 秒控件淡入，
  在资源加载完成后只是表现层等待；agent 等 overlay 的自然 `tick()` 调用完 `onFinish`
  后直接 `Gui.setOverlay(null)`，不再改写 `fadeOutStart` 时间戳，同时把
  `fadeInStart` 拨到过去、把 `TitleScreen.fading` 置 false
- 不写 `fadeOutStart` 是为了兼容 AOT cache：26.2 的缓存训练时带过旧 agent，
  改写时间戳会被缓存代码看成未来时间而永久停在 logo；直接移除 overlay 与缓存无关
- 26.1 的 `screen`/`overlay` 在 `Minecraft` 上，26.2+ 才移到 `Gui`；agent 两种
  布局都读取，并分别调用 `Minecraft.setOverlay` 或 `Gui.setOverlay`
- 只读反射/公开 `setOverlay`，不修改 client jar；字段或方法不存在就静默 no-op
- 对 26.x 原版、Fabric/Forge/NeoForge 等 profile 都生效（只要 26.x 类名不变）
- Agent 同时参与 AOT 训练和生产启动，保证 AOT cache 的 JVM 参数一致

### 卸载

主界面 `x`（或 `Delete`）卸载选中版本，**先问一次**，再按一次才真正删除。删完
自动清理 `libraries/`、`assets/`、`resources/` 和下载缓存里再没有版本引用的文件。

- 先删 `versions/<id>`，有 Loader 依赖它时（`inheritsFrom` 指向它，或目录名是
  `<mc>+<Loader>`）会点名并**一并删除**：基础版本没了，它们反正起不来
- 反向不成立：卸载 Loader 版只删它自己的目录，vanilla 基础版本留着，并且会立刻从
  “已隐藏的基础版本”里露出来（不用再按 `h`）
- 然后按**引用计数**清共享目录：解析剩下每个版本的版本链（含 `inheritsFrom`），收集
  它们可能用到的全部库（含所有 classifier、含没有 `downloads` 块时按 Maven 坐标推导的
  路径）和 asset 索引里的全部对象哈希，其余删掉，空目录一并剪掉
- 引用集是**故意取的超集**（不做 `rules` 过滤），宁可少删不可错删
- 有任何版本的 JSON 读不出来（比如装了一半）就**整个跳过清理**并在状态栏说明：那种
  情况下它的引用无从得知，不能赌
- 清的四个地方：`libraries/`、`assets/{objects,indexes,virtual}`、`resources/`（legacy
  `map_to_resources` 镜像，只在没有任何已装版本需要它时才整个删除）、以及下载缓存
  `data/缓存/.mdt/registry`（资产对象按哈希、加载器库按 Maven 路径映射、索引缓存按
  索引 JSON 的 sha1）
- 同一份数据常有四个名字（registry / `assets/objects` / `assets/virtual` /
  `resources`），所以释放量按 **(dev, inode) 归组**统计：只有一组链接全被删掉才算
  释放，否则如实报“N 个文件是硬链接，数据还有别的名字”，不虚报
- 移动选择、按任意其他键、`r` 刷新、`h` 切换都会取消待确认——"再按一次"不能跨过
  别的操作生效
- 库/资源/缓存的清理**在后台线程**跑（要遍历并逐文件 stat 上万项，放 UI 线程会
  卡住整屏），完成后日志和状态栏再补上释放量；卸载本身立刻返回
- 失败会说明原因并保留列表行；删到一半失败时，状态栏会写清楚哪些已经删掉了
- 版本 id 必须是一个普通路径分量（`..`、`/` 之类直接拒绝），避免误删 `versions/` 外面

### 账户管理

`a`（或点击右上角账户）打开，列出 `data/凭据/账户/*.json`：

| 键 / 鼠标 | 动作 |
|---|---|
| `↑↓` / 滚轮 / 点击行 | 选择（点击同时切换为当前账户） |
| `Enter` | 切换为当前账户 |
| `d` | 移除：**先问一次**，再按一次才删文件（移动选择会取消） |
| `l` | 登录新账户（关闭后回到账户列表，不是主界面） |
| `Esc` / 点击面板外 | 关闭 |

删掉当前账户会自动变成"未登录"——只有被删的那一个会话，同名的另一个还在。

凭据文件按 `0600` 写（目录 `0700`），文件名就是玩家名，refresh token 只给属主看。

**同一个玩家可以登录任意多次**，不管用的是哪个 app id：第一次写 `Steve.json`，
第二次 `Steve (1).json`，第三次 `Steve (2).json`…… 文件永不被覆盖，各自的
refresh token 互不影响。列表和标题栏里那个编号是**灰色**的（名字本身不变色），
表明它是"同名会话的第几个"，不是玩家名的一部分：

```
 Steve (1) · HMCL                 ← (1) 灰色
```

"当前账户"和启动前刷新都按**文件路径**认人，不按玩家名——同名会话只有文件不同，
按名字找会刷错人的令牌。

### 鼠标

启动时开启鼠标捕获（`EnableMouseCapture`，SGR 1006），退出/panic 时必定关闭。

| 位置 | 动作 |
|---|---|
| 日志区 滚轮 | 滚动日志，每次 3 行；标题显示 `日志 (N 行之前)` 提示已回滚 |
| 版本区 滚轮 | 上/下移动选择，每次 3 行；到头就停（键盘 `j/k` 才循环） |
| 版本行 单击 | 选中该行（按当前滚动偏移换算下标） |
| 版本行 **双击** | 启动该实例（400 ms 内同一行两次点击；`request_launch` 全部逻辑，含令牌刷新与 Java 选择） |
| 右上角账户 点击 | 打开账户管理 |
| 底部按键标注 点击 | 等价于按那个键（`d 下载` / `x 卸载` / `a 账户` / `h 基础版本` / `r 刷新` / `q 退出`；其他界面同理） |
| 面板内提示行 点击 | 账户面板与登录面板底部的提示按**整串**可点：`Enter 切换`、`d 移除`、`l 登录新账户`、`Esc 关闭` 各是一块下划线按钮，点在卡在中文上也算（和底部标注走同一条回放路径，不会和绑定脱节） |
| **右键（任意位置）** | 返回 / 取消，等价于 `Esc`。主界面除外——那里 `Esc` 是退出，右键不做事，避免误触关掉启动器 |
| 下载页 搜索框 点击 | 光标进入搜索框 |
| 下载页 类型标签 点击 | 直接切到该类型 |
| 下载页 版本列表 滚轮 | 每次滚 3 **行**（折起的一组算一行，不会跳过看不见的版本），到头就停；指针在整块面板内即可（表头、行、下方空白都行），只有点击才必须落在行上 |
| 下载页 版本列表 点击 | 选中版本；点在折起的一行上则展开/收起（选中后加载器列表按该版本重建） |
| 下载页 版本行 **双击** | 开始安装（400 ms 内同一行两次点击，和主界面的双击启动一致）；单击只选中，不会误下 100+ MB |
| Java 界面 行 点击 / **双击** | 单击只选中，双击开始下载（一个 JRE 约 50 MiB，值得多按一次） |
| 下载页 加载器列表 滚轮 / 点击 | 移动或选中加载器 |
| 登录面板 点击选项 | 选中并确认（官方直接开始；自定义进入 app id 输入；现有凭据进入清单） |
| 登录面板 滚轮 | 上下切换身份 / 清单条目 |
| 登录面板外 点击 | 取消 |

页脚布局：状态文字（绿色）固定靠左，按键标注固定靠右——状态长度变化不会推动按钮，
按钮位置永远不变；状态过长时按可用宽度截断并加 `…`。

底部每个按键标注本身就是按钮（带下划线），点击等价于按那个键——实现上不是复制一份
逻辑，而是把对应的 `KeyCode` 回放到 `App::handle_key`，所以标注和绑定不可能对不上。
账户面板和登录面板内部的提示行用的是同一套机制（`ui::draw_hint`），键位后面那串说明
文字和键位本身是**同一个**按钮：弹窗里的 `Enter 切换`、`d 移除`、`l 登录新账户`、
`Esc 关闭` 整串都能点，而不是只有字母能点。

代价：开启捕获后终端里的鼠标拖选复制交给应用，需要按住 Shift 才能用终端自身的选区（多数终端如此）。

### 登录

`l` 打开面板：

1. **官方启动器** — 直接用 `00000000402b5328`（title id + `MBI_SSL` + SISU 签名链）
2. **自定义 app id** — 输入某个启动器的 Azure client id（如 Prism 的
   `c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb`），走 `user/authenticate` + `xsts`
3. **现有凭据……** — 读 `data/凭据/启动器/azure_app_ids.toml`，把里面的 app id 列出来，
   选中哪个就用哪个登录；账户列表里显示的是文件里写的 `name`

选自定义时输入框为空就按 Enter 不会启动流程（不会偷偷回退到任何内置 id）。
第三种身份每次打开都会重新读文件（改完不用重启），文件缺失或写错就在面板里直接
显示原因，不会拿一个来路不明的 id 去登录。

```toml
# data/凭据/启动器/azure_app_ids.toml
[[api_keys]]
name = "HMCL"
key = "6a3728d6-27a3-4180-99bb-479895b8f88e"
```

首次启动会写入一份**全部注释掉**的模板，只教格式，不会替任何人登录。
这里的 `key` 是 Azure 的公开客户端 id（public client），不是密钥：它只能发起
device code 流程，仍然需要账户本人在浏览器里确认。
面板显示 `user_code` 与链接，浏览器确认后凭据写入 `data/凭据/账户/<玩家名>.json`。
token 只存在于登录线程和那个文件里，**UI 永远拿不到 token**。

等待授权那一屏把两样东西都变成可点的（下划线按钮，和别处一样走按键回放）：

| 位置 | 按钮 | 键 | 作用 |
|---|---|---|---|
| 链接下面一行 | `o 浏览器打开` / `y 复制链接` | `o` / `y` | 用系统默认浏览器打开 / 复制链接 |
| 设备码下面一行 | `c 复制设备码` | `c` | 复制设备码 |

复制同时走两条路，因为哪一条都不够用：

1. **剪贴板工具**：依次尝试 `wl-copy` / `xclip` / `xsel` / `pbcopy` / WSL 的 `clip.exe` /
   `termux-clipboard-set`，谁先成功算谁的（每个最多等 500 ms，卡住的会被杀掉）。
   这是本地桌面最可靠的一条。
2. **OSC 52**（`ESC ] 52 ; c ; <base64>` + `BEL` 和 `ST` 两种结尾各发一次）：请**终端**去设
   剪贴板，是 SSH 下唯一能落到"你粘贴的那台机器"的办法；但它要终端支持（xterm 默认关、
   tmux 要 `set-clipboard on`、有的模拟器干脆没有），不支持时静默无效。

日志里会写清楚这次用的是哪条（`剪贴板：xsel` / `剪贴板：OSC 52`）。只走了 OSC 52 时还会
多一行提示——**如果粘贴出来是空的**，装一个 `wl-clipboard` 或 `xclip` 基本就能解决。

链接的打开用 `xdg-open`（macOS `open`，Windows `cmd /C start`），失败会写进日志并提示按
`y` 复制。

## 仓库结构

```
crates/tessera-core/  纯逻辑：发现、启动参数、下载、Java、账户、清理（不依赖终端）
crates/tessera-tui/   终端前端：屏幕、输入状态机、剪贴板（依赖 tessera-core）
src/main.rs           组装：解析参数，把 core 与 tui 接起来
crates/tessera-core/assets/  AOT 预加载与 fastmenu 两个 Java agent 的源码和 jar
vendor/mdt-minecraft  下载层（upstream + 本地修复，见其 VENDORED.md）
vendor/minecraft-auth 登录层（device code → 启动凭据）
```

`tessera-core` 不含 `ratatui`/`crossterm`，所以未来的 GUI 只是另一个依赖 core 的前端；
`vendor/` 下的两个 crate 各自是独立的 workspace 根，因此在本仓库的
`[workspace] exclude` 里排除，作为 path 依赖引入。

## 分工（已定）

| 层 | 来源 | 产出 |
|---|---|---|
| 下载 | `vendor/mdt-minecraft` | `versions/<id>/` 全部文件 + 完整 version.json |
| Java | `api.adoptium.net` + 清华/GitHub | `运行时/<版本号>/` 一个解包好的 JRE |
| 登录 | `vendor/minecraft-auth` | `player_name/uuid/access_token/xuid` |
| 启动 | 本仓库 | 读 JSON 链 → 合并 → 拼 argv → `Command` |
| 清理 | 本仓库 | 引用计数后回收 `libraries/`、`assets/`、下载缓存 |

## 路线

1. **主界面** ✅
2. **登录** ✅（官方 / 自定义 app id / `azure_app_ids.toml` 清单，一账户一 json；恢复会话待接）
3. **下载** ✅（版本 + Loader 选择，进度条/速度/文件日志；镜像与并发可调；26.1+ 原版安装后自动训练 AOT 缓存）
4. **启动** ✅（版本链合并、classpath、natives、占位符；Java 只用 `data/运行时` 里下载的，
   按版本 json / 版本号自动挑；`--print-launch` 可预览）
5. **令牌自动刷新** ✅（过期才联网，轮换后的 refresh token 写回凭据）
6. **Java 运行时** ✅（Adoptium 最新 JRE × 8/17/21/25/28 EA，清华源/官方源+GitHub Proxy，sha256 校验，
   装到 `data/运行时/<版本号>/`，启动时优先使用；打开界面时检查各大版本的小版本更新，`u` 批量更新）
7. 日志落盘 ✅（`data/日志/tessera.log`，面板仍只留 500 行）；设置持久化 ✅
   （`data/配置.toml`：堆、窗口、GitHub Proxy）
8. 设置页（目前只有命令行开关和手改 toml）

## 许可

GPL-3.0-or-later。`vendor/mdt-minecraft` 为 MIT，`vendor/minecraft-auth` 为
LGPL-3.0-or-later（含 SCL 风格例外不适用），均可并入 GPLv3。
