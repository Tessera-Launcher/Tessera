# 生成流程

```text
generation/recipes/{color,dimension,radius,typography,duration,borderWidth,opacity}.json
+ generation/config.json
        ↓  generate.mjs
tokens/foundation/{color,dimension,number,duration}.tokens.json
tokens/semantic/{color/*,number}.tokens.json
        ↓  Terrazzo resolver（别名解析）
        ↓  build.mjs
crates/tessera-gui/src/design_tokens.rs + preview/tokens.css
```

## 修改入口

| 目标 | 位置 |
| --- | --- |
| 关键色、饱和度区间 | `generation/recipes/color.json` |
| 明暗层级、填充与对比度阈值 | `generation/config.json` |
| 色阶与角色选取算法 | `generation/src/ramp.mjs`、`color.mjs` |
| 间距、布局、圆角、描边 | `generation/recipes/{dimension,radius,borderWidth}.json` |
| 字号与行高 | `generation/recipes/typography.json` |
| 时长 | `generation/recipes/duration.json` |
| 透明度 | `generation/recipes/opacity.json` |
| 主题组合方式 | `tokens/tessera.resolver.json` |
| Rust / CSS 序列化 | `scripts/lib/token-lib.mjs` |

`tokens/foundation/{color,dimension,number,duration}.tokens.json`、`tokens/semantic/color/*.tokens.json` 与 `tokens/semantic/number.tokens.json` 由 `generate` 整体写入，调整从 recipe、策略或生成器入手。

## 命令与过期检查

```sh
npm run generate          # 写入颜色与尺度 token
npm run generate:check    # 只检查，不写；过期即失败
npm run build             # generate + 生成 Rust / CSS
npm run build:check       # 只检查 Rust / CSS 是否过期
npm run check:contrast    # 文字、填充与边界的对比度
npm run check:descriptions # 语义角色描述
npm run check:usage       # 字面量与属性兼容
```

`npm run check` 依次执行：官方 schema 校验 → 生成物过期检查 → 平台产物过期检查 → 对比度 → 描述 → 使用规则 → 测试。`generate:check`、`check:schema`、`check:contrast`、`check:descriptions` 沿用他们的命名；其余为本项目新增。CI 在离线状态完成上述校验。

## 所有权

颜色文件整体由生成器拥有，`primitive` 与语义组带 `$extensions["com.tessera.generation"]`，记录来源与模式。生成以整文件为单位，无需叶级清理。将生成 token 复制到其他文件时，同时移除该标记，使其成为手工 token。

## 新增颜色角色

1. 在 `generation/config.json` 的两套 `lightness` 里加目标值，并同步 `config.schema.json` 的必填列表。
2. 在 `color.mjs` 里选择它属于背景、文字、填充、边框还是状态，并决定是否需要对比度约束。
3. 生成并构建；使用校验会检查浅深角色一致、描述、对比度与组件引用。

配方中的重复名称、饱和度上下限倒置、关键色超出 sRGB 会在生成前报错。
