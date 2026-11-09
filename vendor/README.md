# vendor

两个 crate 的源码直接放在这里（含本地修改），不走 crates.io。

- `mdt-minecraft/` — 上游 MIT，本地改动见 `mdt-minecraft/VENDORED.md`
- `minecraft-auth/` — 上游 LGPL-3.0-or-later，本地改动见其 `README.md`

它们的 `.git` 目录曾被导出成 bundle 放在 `.upstream/`；那个目录已经删掉（嵌套 git
仓库在主仓库里只会变成没有内容的 gitlink）。bundle 本身还在 git 历史里，需要时取回：

```sh
rev=$(git log --diff-filter=D --format=%H -1 -- vendor/.upstream/mdt-minecraft.bundle)
git show "$rev^:vendor/.upstream/mdt-minecraft.bundle" > /tmp/mdt-history.bundle
git clone /tmp/mdt-history.bundle /tmp/mdt-history

rev=$(git log --diff-filter=D --format=%H -1 -- vendor/.upstream/minecraft-auth.bundle)
git show "$rev^:vendor/.upstream/minecraft-auth.bundle" > /tmp/auth-history.bundle
git clone /tmp/auth-history.bundle /tmp/auth-history
```

上游 remote 需手动补：

- mdt: <https://github.com/Lavaver/Minecraft-Download-Toolkit>
- auth: <https://github.com/CCBlueX/minecraft-auth-rs>

两个 crate 的 `Cargo.toml` 各自带 `[workspace]`，所以根 `Cargo.toml` 用
`exclude = ["vendor"]` 把它们排除，再按 path 依赖引入。
