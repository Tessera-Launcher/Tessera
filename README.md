# Tessera Desktop

**胆大心细、敢想敢做的 Minecraft: Java Edition 启动器。**

## 许可

GPL-3.0-or-later。其中 `vendor/mdt-minecraft` 为 MIT，`vendor/minecraft-auth` 为 LGPL-3.0-or-later。

## 构建

字体资源不进入 Git。首次构建前先生成两个内嵌字体（拉丁字母用像素字体，
其余字形落到 Unifont）：

```sh
python3 -m pip install fonttools
python3 tools/fetch_pixel_font.py
python3 tools/fetch_unifont.py
cargo build -p tessera-gui
```

`tools/fetch_unifont.py` 需要系统里有 `fonttosfnt`（Debian 包名相同）。
两个字体各自的来源、许可与改动见 `assets/fonts/NOTICE.md`。
