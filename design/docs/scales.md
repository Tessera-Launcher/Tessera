# 尺寸、字号与时长

所有尺寸以整像素表达，序列来自 8px 基准与 1px 细粒度；角色名保持稳定，值按阶梯取用。

## 间距

| 角色 | 值 | 相当于 8px 阶梯的 1px 步进 |
| --- | --- | --- |
| `space.xxs` | 2px | 50 |
| `space.xs` | 4px | 100 |
| `space.sm` | 8px | 200 |
| `space.md` | 12px | 300 |
| `space.lg` | 16px | 400 |
| `space.xl` | 24px | 600 |
| `space.xxl` | 32px | 800 |
| `space.xxxl` | 48px | 1200 |

`layout.*` 是具体部件的整数尺寸（侧栏、对话框、行高、控件高度）。新增间距时优先取 8px 的倍数；需要 1–4px 的细粒度时，用 2px 与 4px。

## 圆角

| 角色 | 值 |
| --- | --- |
| `radius.sm` | 3px |
| `radius.md` | 6px |
| `radius.lg` | 12px |
| `radius.pill` | 1600px（完全圆角） |

数值从 3px 起逐级翻倍，`pill` 用足够大的半径，使任意矩形都能变成胶囊。

## 字号与行高

字号来自 12px 基准、比例 2、每 5 音一级的音阶；行高按 4px 网格吸附。

| 角色 | 字号 | 行高 | 行高比 |
| --- | --- | --- | --- |
| `font.caption` | 12px | 16px | 1.333333 |
| `font.body` | 14px | 20px | 1.428571 |
| `font.title` | 21px | 28px | 1.333333 |
| `font.display` | 32px | 40px | 1.25 |

`font.lineHeight` 是正文比例，作为继承默认值；标题、展示字与各自的 `lineHeight*` 一起使用，说明文字沿用正文比例。字重来自 `weight.*`（400/500/600/700）。

## 时长与描边

| 角色 | 值 |
| --- | --- |
| `motion.fast` | 150ms |
| `motion.standard` | 200ms |
| `motion.ease` | `cubic-bezier(0.2, 0, 0, 1)` |
| `stroke.thin` | 1px |
| `stroke.focus` | 2px |

## 公式

- 间距：`px = step / 25`，所有间距落在 2px 网格上。
- 圆角：`px = 3 × 2^((step − 100) / 100)`，角色映射到 step 100/200/300；`pill` 固定 1600px。
- 字号：`px = round(12 × 2^((step − 200) / 500))`，说明文字、正文、标题、展示字分别对应 step 200/300/600/900。
- 行高：在 4px 网格（<48px）或 8px 网格上取最接近的倍数，且不小于字号；锚点比率为 10px→1.2、16px→1.45、84px→1.1，按对数字号插值。
- 描边：`px = 2^((step − 100) / 100)`，step 100/200 对应 1px/2px。
- 时长：取 50ms 的倍数。
- 透明度：`value = step / 1000`，禁用态对应 step 400，即 0.4。

上述公式由 `generation/src/scales.mjs` 实现，配方生成器直接调用；`generate:check` 保证提交的 token 与配方一致。公式本身有单元测试，不再增加重复的尺度检查脚本。

## 模型对照

| 模型 | 我们的实现 | 说明 |
| --- | --- | --- |
| dimension | 采用 | 8px 基准，`px = step / 25`；配方选取实际使用的间距与部件尺寸 |
| radius | 采用 | 3px 起逐级翻倍，`full` 固定 1600px |
| typography | 采用 | 12px 基准、比例 2、每 5 音一级；行高按 4/8px 网格 |
| duration | 采用 | 取 50ms 倍数；当前配方只选 150/200ms |
| borderWidth | 采用 | 1px 起逐级翻倍；当前配方选 1/2px |
| opacity | 采用 | `step / 1000`；primitive 生成完整档位，语义只暴露禁用态 |
| color | 采用 | OKHSL 钟形色阶；关键色与角色选取见 [配色](color-palette.md) |
| breakpoint | 未采用 | GPUI 没有媒体查询，界面也没有响应式断点消费者 |
| layer | 未采用 | GPUI 没有 z-index，叠放由元素顺序与覆盖层控制 |
| scale | 未采用 | 当前没有缩放动画或弹窗缩放 |
| shadow / transition | 未采用 | 没有阴影或组合过渡的消费者；时长与缓动已直接供 GPUI 主题使用 |

配方只列出实际使用的档位，这一点与上游一致：未选中的档位不会生成 token。透明度是唯一保留完整 primitive 档位的数字尺度，因为它的语义只暴露了禁用态。

## 扩展

- 新增间距先看现有 8 个角色能否表达；不能时再加角色，值取 8px 的倍数或 1px 进位的整数。
- 新增字号从音阶取值：`12 × 2^(n/5)`，取整到像素后补一条 4px 网格行高。
- 新增时长取 50ms 的倍数，并在 `theme.rs` 的动画配置里使用语义名。
