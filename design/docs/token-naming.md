# Token 命名

## 路径契约

token 路径决定 CSS 变量名与 Rust 调色板字段：

| 层 | 规则 | 示例 |
| --- | --- | --- |
| DTCG 路径 | 小写组名 + camelCase 叶子 | `background.canvas`、`fill.primaryHover`、`layout.rowHeight` |
| CSS 变量 | 各组段转 kebab，统一 `--color-` 前缀 | `--color-background-canvas`、`--color-fill-primary-hover` |
| Rust 字段 | 语义颜色合并到 `color::Palette`，组名作为前缀 | `colors.canvas`、`colors.fill_primary_hover` |

尺寸、字体等 token 仍按第一段生成模块（`space::MD`、`font::BODY`）。颜色不再生成模块，而是集中到一个调色板结构，避免组件绕过角色取色。

## 分层

- `primitive.color.<name>` 是 `$root` 关键色，`primitive.color.<name>.<step>` 是该色的色阶步进（0–1000）。
- `background.*`、`text.*`、`fill.*`、`border.*`、`status.*` 是语义角色；组件只读这些路径。
- `space.*`、`radius.*`、`font.*` 等尺寸与字体角色直接使用，没有额外 primitive 层。
- `layout.*` 使用具体部件名（`sidebar`、`dialog`、`rowHeight`），与界面结构一一对应。

## 颜色角色

| 组 | 角色 | 用途 |
| --- | --- | --- |
| background | `canvas` / `surface` / `raised` | 页面、容器、行 |
| | `hover` / `selected` | 交互与选中背景 |
| | `backdrop` | 模态遮罩，半透明 |
| text | `primary` / `secondary` | 正文与辅助文字 |
| | `accent` / `accentHover` | 链接、选中文字、强调图标 |
| | `onFill` | 主填充上的文字与图标 |
| fill | `primary` / `primaryHover` | 主按钮、进度、滑块、开关 |
| | `neutral` / `warning` / `danger` | 指示点与填充指示器 |
| border | `default` / `strong` / `focus` | 描边、控件边界、焦点环 |
| status | `success` / `warning` / `danger` | 状态文字与图标 |

## 命名规则

- 一个名字对应一种用途；用途不同时分别命名，即使当前色值相同。
- 填充用 `fill.*`，文字与图标用 `text.*` 或 `status.*`；这是使用校验会检查的边界。
- 状态色只表达状态；版本类型、装饰和品牌强调用填充或文字角色。
- 组件通过既有角色取色；新增用途时先扩展角色表，再生成。
- 别名使用 `{text.accent}` 语法，由 Terrazzo 解析。
- 文件名跟随 DTCG 类型，不跟随界面部件。`font` 是跨类型的概念组，见 [文件结构](token-structure.md)。
