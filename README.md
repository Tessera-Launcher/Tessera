# yaml

终端里的 Minecraft: Java Edition 启动器。TUI（`ratatui` + `crossterm`），
无 WebView、无 GTK、无 GPU 依赖，可在 SSH 里跑。

## 当前状态

主界面 + 登录。主界面扫描 `<root>/versions`、列出本地已装版本、显示启动层将来需要的
信息（基础版本 / Java / 主类 / 目录 / 大小）。登录走微软 device code，支持两种身份：
官方启动器（title client id）和自定义 Azure 应用 id（**必须填 id 才能开始**）。
下载与启动还没接，界面会明说"尚未实现"而不是假装成功。

### 便携布局

二进制所在目录 + `data/` 就是全部，**启动器不写用户主目录**：

```
<exe>/data/
├── 游戏/          versions / libraries / assets（--dir 可覆盖）
├── 凭据/账户/     一个账户一个 json（玩家名.json）
├── 缓存/          下载缓存（见下）
└── 日志/
```

`缓存/` 是 vendor 里的 MDT 的内容寻址注册表：它默认写 `$HOME/.mdt/registry`，
启动时被 `MDT_HOME` 指到 `data/缓存`，实际路径是 `data/缓存/.mdt/registry`。
`assets/objects/` 里的文件是那个缓存的**硬链接**，所以同一份数据在磁盘上只占一次
空间（也是卸载后资产清理释放不出空间的原因，见卸载一节）。环境里已有 `MDT_HOME`
时不覆盖，方便临时改用别处。

`YAML_DATA` 可覆盖 `data` 根目录（测试和源码运行时用）。

```sh
cargo run                                   # 默认 $HOME/.minecraft
cargo run -- --dir /tmp/mdt-1899            # 指定目录
cargo run -- --once --dir /tmp/mdt-1899     # 渲染一帧后退出，不需要 TTY
cargo run -- --once --login                 # 一帧里直接展示登录面板
```

按键：`↑/k ↓/j` 选择，`g/G` 首尾，`PgUp/PgDn` 滚动日志，`l` 登录，
`r` 重新扫描，`x` 卸载，`q/Esc` 退出，`Enter` 启动。

### 启动

`Enter` 用选中版本启动游戏。命令由 `launch.rs` 纯函数拼装，`--print-launch` 可以只看不跑：

```sh
yaml --dir /path/to/游戏 --print-launch 1.26.3
yaml --dir /path/to/游戏 --java /path/to/java8/bin/java --memory 4096
```

做的事：

- **版本链合并**：跟随 `inheritsFrom`（最多 8 层），子 profile 的 mainClass / libraries
  优先，父的 `minecraftArguments`、`assetIndex`、`javaVersion` 兜底
- **classpath**：按 `rules`（OS / arch / features）过滤，按 `downloads.artifact.path`
  取路径，没有就按 Maven 坐标推导；子 profile 在前（Loader 才能覆盖）
- 客户端 jar 找 `<id>.jar` → `<父id>.jar` → 目录里任意 jar（Loader 是复制父目录来的）
- **natives**：找 `<id>-natives` 或 `<父id>-natives`，写进 `-Djava.library.path`；
  1.19+ 没有 natives 目录也不影响（现代版本的 natives 直接在 classpath 上）
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

**Java 选择**：扫 `JAVA_HOME` 再扫 `PATH`，逐个跑 `java -version` 解析主版本号。

- 要求 Java 8 的版本（1.7–1.16）**只接受 8**——17/21 上会以 `NoSuchMethodError`
  的形式崩在几十秒之后，给个明确的"需要 Java 8"比让它崩更好
- 要求 17/21/25 的版本接受**同版本或更高**
- 同版本优先；否则取满足条件的最小值；都不满足就把找到的版本列出来
- `--java <路径>` 可以强制指定，跳过探测

**凭据与令牌刷新**：按 `Enter` 时，如果当前账户有保存的凭据，会先在后台刷新：

- 令牌没过期 → 不联网，直接启动
- 令牌过期 → 用 refresh token 换新的；**微软会轮换 refresh token**，所以刷新后整份
  session 会原子写回 `data/凭据/账户/<玩家名>.json`（临时文件 + rename）
- 刷新被判定为"需要重新登录"（`invalid_grant` 等）→ 日志提示按 `l` 重新登录，
  本次仍以离线身份启动
- 刷新因网络失败 → **继续用磁盘上的旧令牌**，不因为一次网络抖动就拦下启动

刷新在独立线程里跑（同登录/下载），失败原因都会写进日志。未登录或没有凭据文件时
直接用离线身份（`user_type=legacy`、令牌 `0`、确定性离线 UUID）。

### 下载

`d` 打开选版本界面：

```
┌ 下载 BMCLAPI · 3 / 5 ──────────────────────────────────────────────────────┐
│┌ 搜索 ──────────────────────────────────────┐┌ 类型 ────────────────────┐│
││1.2▏  3/5                                   ││ 全部  正式版  快照  旧版  ││
│└────────────────────────────────────────────┘└──────────────────────────┘│
│┌ 版本 ─────────────────────────────────┐┌ 安装内容 ──────────────────────┐│
││  版本               类型      发布    ││▶ 无                            ││
││▶ 1.21.11         release  2025-12-09 ││  Fabric                        ││
││  1.21            release  2024-06-13 ││  Quilt                         ││
││  1.20.6          release  2024-04-29 ││  Forge                         ││
│└──────────────────────────────────────┘│ 仅安装原版                     ││
└────────────────────────────────────────────────────────────────────────────┘
```

- `/` 进搜索框，输入即过滤（子串、不分大小写）；`Esc` 先清空搜索，再按一次离开输入框
- `f` 循环类型筛选：全部 → 正式版 → 快照 → 旧版；也可直接点上面的标签
- 标题显示 `匹配 / 总数`，没有匹配时版本区写"没有匹配的版本"而不是留白
- `Tab` 循环 版本栏 → Loader 栏 → 搜索框，`↑↓` 选择（在搜索框里也照样移动版本），`Enter` 开始
- Loader 列表按版本过滤（Fabric/Quilt ≥1.14、NeoForge ≥1.21、LiteLoader ≤1.12.2）
- Loader 版本自动取该 MC 版本的最新构建，并在日志里写明选中的版本
- 换版本时若原 Loader 仍可用就保留选择，否则回落到"无"
- 源由 `--mirror` 决定（`bmclapi` 默认 / `mojang`），asset 并发随之调整
  （BMCLAPI 前门限流约 10 次/窗口，所以默认 8；官方源用 32）

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
- 有字节总量时按字节算百分比，否则按文件数
- `Esc` 关界面但**下载在后台继续**，完成后自动重扫本地版本
- 完成/失败会停在结果页，`Enter`/`Esc` 返回列表

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

删掉当前账户会自动变成"未登录"。

### 鼠标

启动时开启鼠标捕获（`EnableMouseCapture`，SGR 1006），退出/panic 时必定关闭。

| 位置 | 动作 |
|---|---|
| 日志区 滚轮 | 滚动日志，每次 3 行；标题显示 `日志 (N 行之前)` 提示已回滚 |
| 版本区 滚轮 | 上/下移动选择，每次 3 行；到头就停（键盘 `j/k` 才循环） |
| 版本行 单击 | 选中该行（按当前滚动偏移换算下标） |
| 版本行 **双击** | 启动该实例（400 ms 内同一行两次点击；`request_launch` 全部逻辑，含令牌刷新与 Java 探测） |
| 右上角账户 点击 | 打开账户管理 |
| 底部按键标注 点击 | 等价于按那个键（`d 下载` / `x 卸载` / `a 账户` / `h 基础版本` / `r 刷新` / `q 退出`；其他界面同理） |
| **右键（任意位置）** | 返回 / 取消，等价于 `Esc`。主界面除外——那里 `Esc` 是退出，右键不做事，避免误触关掉启动器 |
| 下载页 搜索框 点击 | 光标进入搜索框 |
| 下载页 类型标签 点击 | 直接切到该类型 |
| 下载页 版本列表 滚轮 / 点击 | 移动或选中版本（选中后加载器列表按该版本重建） |
| 下载页 加载器列表 滚轮 / 点击 | 移动或选中加载器 |
| 登录面板 点击选项 | 选中并确认（官方直接开始；自定义进入 app id 输入） |
| 登录面板 滚轮 | 上下切换身份 |
| 登录面板外 点击 | 取消 |

页脚布局：状态文字（绿色）固定靠左，按键标注固定靠右——状态长度变化不会推动按钮，
按钮位置永远不变；状态过长时按可用宽度截断并加 `…`。

底部每个按键标注本身就是按钮（带下划线），点击等价于按那个键——实现上不是复制一份
逻辑，而是把对应的 `KeyCode` 回放到 `App::handle_key`，所以标注和绑定不可能对不上。

代价：开启捕获后终端里的鼠标拖选复制交给应用，需要按住 Shift 才能用终端自身的选区（多数终端如此）。

### 登录

`l` 打开面板：

1. **官方启动器** — 直接用 `00000000402b5328`（title id + `MBI_SSL` + SISU 签名链）
2. **自定义 app id** — 输入某个启动器的 Azure client id（如 Prism 的
   `c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb`），走 `user/authenticate` + `xsts`

选自定义时输入框为空就按 Enter 不会启动流程（不会偷偷回退到任何内置 id）。
面板显示 `user_code` 与链接，浏览器确认后凭据写入 `data/凭据/账户/<玩家名>.json`。
token 只存在于登录线程和那个文件里，**UI 永远拿不到 token**。

## 仓库结构

```
src/                 启动器本体（纯逻辑 + 渲染，可无终端测试）
vendor/mdt-minecraft 下载层（upstream + 本地修复，见其 VENDORED.md）
vendor/minecraft-auth 登录层（device code → 启动凭据）
```

`vendor/` 下的两个 crate 各自是独立的 workspace 根，因此在本仓库的
`[workspace] exclude` 里排除，作为 path 依赖引入。

## 分工（已定）

| 层 | 来源 | 产出 |
|---|---|---|
| 下载 | `vendor/mdt-minecraft` | `versions/<id>/` 全部文件 + 完整 version.json |
| 登录 | `vendor/minecraft-auth` | `player_name/uuid/access_token/xuid` |
| 启动 | 本仓库（待写） | 读 JSON 链 → 合并 → 拼 argv → `Command` |

## 路线

1. **主界面** ✅
2. **登录** ✅（官方 / 自定义 app id，一账户一 json；恢复会话待接）
3. **下载** ✅（版本 + Loader 选择，进度条/速度/文件日志；镜像与并发可调）
4. **启动** ✅（版本链合并、classpath、natives、占位符、Java 探测；`--print-launch` 可预览）
5. **令牌自动刷新** ✅（过期才联网，轮换后的 refresh token 写回凭据）
6. 设置页、日志落盘、Java 运行时目录扫描

## 许可

GPL-3.0-or-later。`vendor/mdt-minecraft` 为 MIT，`vendor/minecraft-auth` 为
LGPL-3.0-or-later（含 SCL 风格例外不适用），均可并入 GPLv3。
