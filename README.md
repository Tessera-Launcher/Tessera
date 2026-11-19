# Tessera

**胆大心细、敢想敢做的 Minecraft: Java Edition 启动器。**

## 许可

GPL-3.0-or-later。其中 `vendor/mdt-minecraft` 为 MIT，`vendor/minecraft-auth` 为 LGPL-3.0-or-later。

## 构建

字体资源不进入 Git。首次构建前先生成内嵌字体：

```sh
python3 -m pip install fonttools
python3 tools/fetch_unifont.py
cargo build -p tessera-gui
```

`tools/fetch_unifont.py` 需要系统里有 `fonttosfnt`（Debian 包名相同）。
