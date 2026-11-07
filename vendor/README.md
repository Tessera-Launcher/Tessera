# vendor

两个 crate 的源码直接放在这里（含本地修改），不走 crates.io。

- `mdt-minecraft/` — 上游 MIT，本地改动见 `mdt-minecraft/VENDORED.md`
- `minecraft-auth/` — 上游 LGPL-3.0-or-later，本地改动见其 `README.md`

它们的 `.git` 目录被导出成 bundle 后移除，因为嵌套 git 仓库在主仓库里只会变成
没有内容的 gitlink。历史仍可取回：

```sh
git clone .upstream/mdt-minecraft.bundle /tmp/mdt-history
git clone .upstream/minecraft-auth.bundle /tmp/auth-history
# 上游 remote 需手动补：
#   mdt:  https://github.com/Lavaver/Minecraft-Download-Toolkit
#   auth: https://github.com/CCBlueX/minecraft-auth-rs
```

两个 crate 的 `Cargo.toml` 各自带 `[workspace]`，所以根 `Cargo.toml` 用
`exclude = ["vendor"]` 把它们排除，再按 path 依赖引入。
