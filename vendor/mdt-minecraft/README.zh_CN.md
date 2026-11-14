# MDT — Minecraft Download Toolkit

把 Minecraft 版本和 Fabric 解析成一份 **aria2 下载清单**。

[![Crates.io](https://img.shields.io/crates/v/mdt-minecraft.svg?style=flat-square)](https://crates.io/crates/mdt-minecraft)
[![docs.rs](https://docs.rs/mdt-minecraft/badge.svg?style=flat-square)](https://docs.rs/mdt-minecraft)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg?style=flat-square)](https://opensource.org/licenses/MIT)

**语言 / Language:** [English](README.md) · [简体中文](README.zh_CN.md)

MDT 只做三件事：取元数据、写 profile、出清单。它把版本清单、版本 JSON、资源索引、
Fabric profile 解析出来，把 profile 写回 `versions/<id>/<id>.json`，再为每个版本写一份清单
（放在版本目录外面，它只是下载器的输入）：

```
列表/26.3-fabric-0.20.1.txt
```

这份文件就是 `aria2c --input-file` 的格式：一行 URL，后面跟缩进的选项。

```text
https://piston-data.mojang.com/v1/objects/e877…/client.jar
  dir=/game/versions/26.3-fabric-0.20.1
  out=26.3-fabric-0.20.1.jar
```

清单默认写到 `<root>/列表/`，可用 `.list_dir(...)` 换位置。写清单的同时会把每个条目的
目录建好——aria2 不会创建 `dir=`，缺一个目录整条就失败。

## 特性

- 全版本原版 Minecraft（manifest 里的任意版本）+ Fabric 加载器。
- 每个版本一份清单，按目标路径去重；清单放在调用方指定的目录（默认
  `<root>/列表/`），不进 versions。
- 同时写版本 JSON：启动器要读它拼命令行；Fabric profile 会与 vanilla 合并，目录自包含。
- 镜像：`Mirror::BMCLAPI`、`Mirror::Mojang`、`Mirror::Custom("<name>")`，端点由
  `providers.compile.toml` 编译进静态表。
- 一个文件一个地址，清单里不带 `checksum=`；校验只做一次，由
  `plan.verify()` 在下载后负责。
- `minimal` 特性：只保留版本清单解析与清单核心。

MDT **不**下载游戏文件、不启动游戏、不处理登录，也没有 CLI / TUI，唯一的二进制是示例。

## 依赖

```toml
[dependencies]
mdt-minecraft = "0.7.0"
```

## 快速开始

```rust
use mdt::{MdtClient, Mirror, ModLoader};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = MdtClient::builder()
        .mirror(Mirror::Mojang)
        .root_dir("./.minecraft")
        .build()?;

    // 原版：versions/1.20.1/1.20.1.{json,txt}
    let plan = client.plan_vanilla("1.20.1").await?;

    // Fabric：整栈一份清单，包含 loader 自己的 maven 库
    let plan = client.plan_loader("1.20.1", ModLoader::Fabric, "0.15.11").await?;

    println!(
        "{} 个文件，{} 个带哈希，{:.1} MiB -> {}",
        plan.entries.len(),
        plan.hashed(),
        plan.total_bytes() as f64 / (1024.0 * 1024.0),
        plan.path(client.root_dir()).display()
    );
    Ok(())
}
```

Fabric 的 loader 版本由调用方传入；用 `manifest::fetch_fabric_manifest(...)` 取列表的
**第一条** 就是最新构建（meta 列表按最新在前排序，beta 也在里面）。

## 交给 aria2

```sh
aria2c -i versions/<id>/<id>.txt \
  -j64 -x1 -s1 \
  --no-conf=true --enable-color=false \
  --show-console-readout=false --summary-interval=0 \
  --console-log-level=notice \
  --auto-file-renaming=false --allow-overwrite=true \
  --stop-with-process=<启动器 pid>
```

实测要点：

- 几千个小文件时 `-j` 才是关键；`-x`/`-s` 只对超过 `--min-split-size`（默认 20 MiB）
  的文件有意义，保持 1 即可。
- 哈希不写进清单：aria2 自己校验等于校验两遍，而且失败时文件照样留在盘上。校验一次就够：

  ```rust
  let failed = plan.verify(client.root_dir()).await;   // 缺失 / 空 / 大小不符 / 哈希不符
  ```

- 目标已存在时 aria2 退出码 13，传输失败是 32；非 0 退出就当“这批路径要复检”。
- `--stop-with-process` 防止 aria2 子进程比启动器活得久。
- `--save-session=<file>` 把失败的条目写回去，第二轮只补缺的。

## 特性开关

| 特性 | 默认 | 作用 |
|---|---|---|
| `tokio` | ✅ | async API，reqwest 传输 |
| `http2` | ❌ | 启用 HTTP/2（对 Minecraft 资源 CDN 实测更慢） |
| `minimal` | ❌ | 去掉加载器与资源模块，只留版本清单 + 清单核心 |

## 错误

一切返回 `Result<T, MdtError>`；枚举是 `#[non_exhaustive]`，记得留通配分支。出清单时可能
遇到的是 `NetworkError`、`ChecksumMismatch`、`InvalidManifest`、`VersionNotFound`、
`LoaderInstallFailed`、`Io`。

## 许可

MIT，与上游一致。本副本被 vendored 进 Tessera，本地改动见 `VENDORED.md`。
