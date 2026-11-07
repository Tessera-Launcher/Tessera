# yaml

终端里的 Minecraft: Java Edition 启动器。TUI（`ratatui` + `crossterm`），
无 WebView、无 GTK、无 GPU 依赖，可在 SSH 里跑。

## 当前状态

**只有主界面。** 它扫描 `<root>/versions`、列出本地已装版本、显示启动层将来需要的
信息（基础版本 / Java / 主类 / 目录 / 大小）。下载、登录、启动都还没接，
界面会明说"尚未实现"而不是假装成功。

```sh
cargo run                                   # 默认 $HOME/.minecraft
cargo run -- --dir /tmp/mdt-1899            # 指定目录
cargo run -- --once --dir /tmp/mdt-1899     # 渲染一帧后退出，不需要 TTY
```

按键：`↑/k ↓/j` 选择，`g/G` 首尾，`r` 重新扫描，`q/Esc` 退出，`Enter` 安装/启动（未实现）。

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
2. `launch.rs`：版本链合并、classpath、natives、`${}` 占位符替换、Java 选择
3. 接下载：版本列表 + 安装进度
4. 接登录：device code（token 只在 Rust 侧，**不进 UI**）
5. 设置页、实例隔离、日志落盘

## 许可

GPL-3.0-or-later。`vendor/mdt-minecraft` 为 MIT，`vendor/minecraft-auth` 为
LGPL-3.0-or-later（含 SCL 风格例外不适用），均可并入 GPLv3。
