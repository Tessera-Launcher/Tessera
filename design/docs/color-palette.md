# 配色与派生规则

## 关键色

4 个关键色定义在 `generation/recipes/color.json`，每个色带一个饱和度区间：

| 名称 | 关键色 | 饱和度 | 用途 |
| --- | --- | --- | --- |
| brand | `#2F7FC1` | 0.35 – 0.95 | 主操作、链接、选中背景、成功状态 |
| neutral | `#6E6E6E` | 0 | 表面、文字、边框、中性指示器 |
| warning | `#E0A63C` | 0.35 – 0.95 | 警告状态与警告填充 |
| danger | `#D9645B` | 0.35 – 0.95 | 错误状态与危险填充 |

关键色以 `$root` 写入 `tokens/foundation/color.tokens.json`，色阶步进挂在同一组下。品牌色是单色阶：同一蓝色相，饱和度保持在 0.35–0.95，只靠明度分层；中性色为纯石墨，警告用金，危险用珊瑚红。深色主题采用更深的黑位与更亮的文字，浅色主题为默认外观。更换关键色会同时更新两套外观的强调色、填充、选中背景、焦点与成功状态；中性表面仍由 neutral 决定。

## 色阶

每个关键色生成一条 OKHSL 色阶：

```text
p = step / 1000
L = 1 - p
S = min + (max - min) * bell(p)
```

`bell` 是以 0.5 为中心、σ = 0.25 的高斯曲线，两端归零、中段最强。色阶转成 sRGB 后量化到 8 位通道，只输出角色实际引用的步进。

## 角色选取

`generation/config.json` 为每类角色给出目标明度。生成器按顺序选步进：

1. 五级表面：canvas、surface、raised、hover 用 neutral；selected 用 brand，并与正文保持 4.5:1。
2. 文字与强调：`text.*` 在满足 4.5:1 的步进中选目标明度最近的一个。
3. 填充：`fill.primary` 与 `fill.primaryHover` 在满足「与 `text.onFill` 4.5:1、与页面 3:1」的步进中选取。
4. 状态：warning/danger 在满足 4.5:1 的步进中选取；填充指示器只需 3:1。
5. 边框：`border.strong` 与 `border.focus` 满足 3:1；`border.default` 直接取目标明度。

语义文件只保存别名，例如 `"canvas": { "$value": "{primitive.color.neutral.940}" }`。构建时由 Terrazzo 解析成实际颜色，CSS 与 Rust 拿到同一组 8 位值。

## 固定角色

- `border.focus` 与 `status.success` 别名到 `text.accent`。
- `background.backdrop` 取 neutral 第 920 步（约 8% 亮度），透明度来自 `lightness.<mode>.scrim`。

## 约束

- 两套外观拥有完全相同的角色键，构建与使用校验都会检查。
- 状态色相来自 brand、warning、danger，测试覆盖全部生成色。
- 对比度阈值覆盖设计系统定义的配对，其余组合单独评估。
