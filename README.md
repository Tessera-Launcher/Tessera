# yaml

终端里的 Minecraft: Java Edition 启动器。TUI（`ratatui` + `crossterm`），
无 WebView、无 GTK、无 GPU 依赖，可在 SSH 里跑。

## 当前状态

主界面 + 登录。主界面扫描 `<root>/versions`、列出本地已装版本、显示启动层将来需要的
信息（基础版本 / Java / 主类 / 目录 / 大小）。登录走微软 device code，支持两种身份：
官方启动器（title client id）和自定义 Azure 应用 id（**必须填 id 才能开始**）。
下载与启动还没接，界面会明说"尚未实现"而不是假装成功。

### 便携布局

二进制所在目录 + `data/` 就是全部，不写用户主目录：

```
<exe>/data/
├── 游戏/          versions / libraries / assets（--dir 可覆盖）
├── 凭据/账户/     一个账户一个 json（玩家名.json）
└── 日志/
```

`YAML_DATA` 可覆盖 `data` 根目录（测试和源码运行时用）。

```sh
cargo run                                   # 默认 $HOME/.minecraft
cargo run -- --dir /tmp/mdt-1899            # 指定目录
cargo run -- --once --dir /tmp/mdt-1899     # 渲染一帧后退出，不需要 TTY
cargo run -- --once --login                 # 一帧里直接展示登录面板
```

按键：`↑/k ↓/j` 选择，`g/G` 首尾，`PgUp/PgDn` 滚动日志，`l` 登录，
`r` 重新扫描，`q/Esc` 退出，`Enter` 安装/启动（未实现）。

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
| 版本区 滚轮 | 上/下切换选择 |
| 版本行 点击 | 选中该行（按当前滚动偏移换算下标） |
| 右上角账户 点击 | 打开账户管理 |
| 登录面板 点击选项 | 选中并确认（官方直接开始；自定义进入 app id 输入） |
| 登录面板 滚轮 | 上下切换身份 |
| 登录面板外 点击 | 取消 |

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
3. `launch.rs`：版本链合并、classpath、natives、`${}` 占位符替换、Java 选择
4. 接下载：版本列表 + 安装进度
5. 账户切换（读回 `data/凭据/账户/*.json` 并刷新）、设置页、日志落盘

## 许可

GPL-3.0-or-later。`vendor/mdt-minecraft` 为 MIT，`vendor/minecraft-auth` 为
LGPL-3.0-or-later（含 SCL 风格例外不适用），均可并入 GPLv3。
