# Token 文件结构

## 顶层布局

```text
design/tokens/
├── tessera.resolver.json
├── foundation/
│   ├── color.tokens.json       # 生成
│   ├── dimension.tokens.json   # 生成
│   ├── number.tokens.json      # 生成
│   ├── duration.tokens.json    # 生成
│   ├── fontFamily.tokens.json
│   ├── fontWeight.tokens.json
│   └── cubicBezier.tokens.json
└── semantic/
    ├── number.tokens.json
    └── color/
        ├── dark.tokens.json
        └── light.tokens.json
```

## 分层

- `foundation/` 保存不随外观变化的角色：颜色色阶、间距、圆角、布局尺寸、笔画、字体、字重、时长与缓动。
- `foundation/color.tokens.json` 保存关键色与色阶步进；`foundation/{dimension,number,duration}.tokens.json` 由尺度配方生成。
- `semantic/` 保存角色：`color/*` 按 `background`、`text`、`fill`、`border`、`status` 分组，`number.tokens.json` 保存透明度别名；颜色文件里只有别名。
- 组件读取 `background.*`、`text.*`、`fill.*`、`border.*`、`status.*`、`space.*` 等语义路径；`primitive.color.*` 供生成与解析使用。
- 尺寸与字体以角色命名并直接使用（`space.md`、`layout.sidebar`）。单品牌桌面应用不需要额外一层尺寸别名。

## 文件按 DTCG 类型拆分

- 一个文件承载一种需要解析的 DTCG 类型，组级 `$type` 在合并时保持唯一。
- `font` 是概念分组，跨三个文件：字号在 `dimension`，行高在 `number`，字体栈在 `fontFamily`。每个叶子显式声明 `$type`。
- `motion` 同理：时长在 `duration`，缓动在 `cubicBezier`。
- token 路径与文件位置解耦，拆分不改变 Rust 字段或 CSS 变量名。

## Resolver

`tessera.resolver.json` 按以下顺序组合，后者覆盖前者：

1. `sets.foundation`：颜色、尺寸、数字、字体、字重、时长与缓动。
2. `sets.semantic-static`：不随外观变化的语义角色（当前是透明度）。
3. `modifiers.colorScheme`：`light`（默认）与 `dark` 的颜色角色。

新增基础文件时，同步加入 `sets.foundation.sources`。新增随外观变化的值时，写入两个上下文文件；构建会检查两套角色的键完全一致。

## 生成文件

`foundation/{color,dimension,number,duration}.tokens.json`、`semantic/color/*.tokens.json` 与 `semantic/number.tokens.json` 由 `npm run generate` 整体写入，并通过 `$extensions["com.tessera.generation"]` 记录来源；颜色文件额外记录模式。生成以整文件为单位，手工调整并入 recipe、策略或生成器后重新生成。角色与使用规则见 [颜色角色](color-usage.md)，过期检查见 [生成流程](generation.md)。
