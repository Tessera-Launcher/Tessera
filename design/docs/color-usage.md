# 颜色角色的使用规则

颜色分三层：primitive 色阶、semantic 角色、组件。组件只引用角色；生成器与解析器使用 primitive。

## 角色

| 组 | 角色 | 用途 |
| --- | --- | --- |
| background | `canvas` | 页面底色 |
| | `surface` | 卡片、面板、区块 |
| | `raised` | 列表行、次级控件 |
| | `hover` | 行与次级控件的悬停态 |
| | `selected` | 选中的行或导航项 |
| | `backdrop` | 模态遮罩，带透明度 |
| text | `primary` | 正文与标签 |
| | `secondary` | 辅助文字、元数据、弱化图标 |
| | `accent` | 链接、选中文字、强调图标 |
| | `accentHover` | 链接与强调文字的悬停态 |
| | `onFill` | 放在主填充上的文字与图标 |
| fill | `primary` | 主按钮、进度条、滑块、开关 |
| | `primaryHover` | 主填充的悬停态 |
| | `neutral` | 中性指示点与弱填充 |
| | `warning` | 警告指示器与进度条 |
| | `danger` | 危险指示器 |
| border | `default` | 分隔线、输入框、容器描边 |
| | `strong` | 控件边界、滚动条滑块 |
| | `focus` | 键盘焦点环 |
| status | `success` | 成功状态文字与图标 |
| | `warning` | 警告状态文字与图标 |
| | `danger` | 错误与危险状态文字与图标 |

## 规则

- 填充用 `fill.*`，文字与图标用 `text.*` 或 `status.*`；两者不互换。
- 状态色只表达状态。版本类型、装饰和品牌强调使用填充或文字角色。
- 指示点、进度条、开关与按钮使用 `fill.*`；`status.*` 用在文字与图标上。
- 悬停与选中使用同一角色的相邻步进；不为悬停新增色相。
- 遮罩使用 `background.backdrop`，透明度来自 `lightness.<mode>.scrim`。
- 浅色与深色必须暴露相同的角色键。
- 颜色不单独传达信息：类型、状态同时用文字或图标表达。

## 对比度

构建时强制以下配对：

| 前景 | 背景 | 阈值 |
| --- | --- | --- |
| `text.primary`、`text.secondary`、`text.accent`、`text.accentHover` | 五级表面 | 4.5:1 |
| `status.success`、`status.warning`、`status.danger` | 五级表面 | 4.5:1 |
| `text.onFill` | `fill.primary`、`fill.primaryHover` | 4.5:1 |
| `fill.*` | canvas / surface / raised | 3:1 |
| `border.strong`、`border.focus` | canvas / surface / raised | 3:1 |

## 强制校验

`npm run check:contrast`、`npm run check:descriptions` 与 `npm run check:usage` 和构建、测试一起运行：

- 角色必须存在、浅深键一致。
- 每个角色都有描述。
- 上述对比度配对全部达标。
- Rust 与预览样式不得出现字面量颜色（生成文件除外）。
- 预览样式中，`background` 只能用 background/fill 角色，`color` 只能用 text/status 角色，边框只能用 border/fill 角色。
- Rust 中，间距只能用于 padding/margin/gap，尺寸只能用于宽高，圆角只能用于 rounded，描边只能用于 border，字号只能用于 text_size。
- Rust 引用的 `Palette` 字段必须与生成文件一致，且每个字段都有组件使用。

审美判断（色相是否和谐、品牌气质是否匹配）不进入强制校验，由截图评审与用户测试决定。
