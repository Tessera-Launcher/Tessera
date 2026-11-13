# Tessera 设计系统

界面颜色由基础配色与生成规则统一派生。浅色与深色共用一套语义角色名，组件引用角色，不持有色值。

## 结构

```text
design/
├── generation/
│   ├── config.json          # 明度层级与对比度阈值
│   ├── config.schema.json
│   ├── recipes/             # color + dimension/radius/typography/duration/borderWidth/opacity
│   ├── schemas/             # 每类配方的 JSON Schema
│   └── src/
│       ├── color.mjs        # 颜色角色选取
│       ├── ramp.mjs         # OKHSL 钟形色阶
│       ├── scales.mjs       # 字号、圆角、行高等公式
│       ├── scale-recipes.mjs
│       ├── recipe.mjs       # 颜色配方加载
│       ├── config.mjs       # 颜色策略加载
│       └── generators/      # 六类尺度生成器
├── schemas/
│   └── dtcg/2025.10/        # 官方 DTCG schema、manifest、NOTICE
├── tokens/
│   ├── tessera.resolver.json
│   ├── foundation/
│   │   ├── color.tokens.json      # 生成的关键色与色阶步进
│   │   ├── dimension.tokens.json
│   │   ├── number.tokens.json
│   │   ├── fontFamily.tokens.json
│   │   ├── fontWeight.tokens.json
│   │   ├── duration.tokens.json
│   │   └── cubicBezier.tokens.json
│   └── semantic/
│       └── color/{dark,light}.tokens.json   # 语义别名
├── scripts/
│   ├── generate.mjs
│   ├── build.mjs
│   ├── validate-tokens.mjs
│   ├── tokens.test.mjs
│   └── lib/token-lib.mjs
└── docs/
```

Color.js 负责 OKHSL 色阶、sRGB 量化与 WCAG 对比度；Terrazzo 负责 DTCG 解析、类型继承、别名与 resolver 组合；Ajv 负责官方 schema 与生成策略的校验。三者都是构建时依赖。间距、圆角、字号与时长遵循同一套阶梯，见 [尺度](docs/scales.md)。

## 命令

```sh
npm ci
npm run generate          # recipes + config.json → 颜色与尺度 token
npm run generate:check    # 只检查生成物是否过期
npm run build             # generate + resolver → Rust / CSS
npm run build:check       # 只检查 Rust / CSS 是否过期
npm run check:schema      # 官方 DTCG schema 校验
npm run check:contrast    # 文字、填充与边界的对比度配对
npm run check:descriptions # 语义角色描述
npm run check:usage       # 字面量、属性兼容与 Palette 字段
npm run check             # 以上全部 + 测试
npm run preview           # 构建后启动浏览器预览
```

无需 Rust 查看界面：双击根目录 `preview.cmd`，或直接打开 `preview/index.html`。预览使用已生成的静态文件，运行时不需要 Node、Rust 或网络。

## 校验

1. `check:schema` 使用 `design/schemas/dtcg/2025.10/` 的官方 DTCG 2025.10 schema 校验每个 token 与 resolver 文件。schema 原件随仓库保存，`manifest.json` 记录来源 URL 与 SHA-256；校验和不匹配时校验中止，并提示重新获取官方文件。校验离线执行。
2. Terrazzo 在解析时检查取值语义、继承、别名与 resolver 组合。
3. `check:contrast` 检查角色存在、浅深一致与全部对比度配对。
4. `check:descriptions` 检查每个语义角色都有描述。
5. `check:usage` 检查字面量颜色、CSS/Rust 属性兼容与 Palette 字段覆盖。
6. 测试覆盖量化后的对比度、关键色传播、色相区间、配方/策略范围与尺度公式。

`generate:check`、`check:schema`、`check:contrast`、`check:descriptions` 沿用他们的命名；`check:usage` 与 `build:check` 是本项目为 Rust/GPUI、无 Stylelint 的组件层新增的检查。

角色的完整定义与使用规则见 [颜色角色](docs/color-usage.md)。

## 原生与预览

生成的 Rust 常量随源码保存，普通 `cargo build` 不需要 Node。`theme.rs` 把当前语义调色板映射到 GPUI，并在切换时统一重绘；侧栏提供浅/深切换，选择在本次运行内有效。网页预览通过 `data-theme` 切换同一组语义变量，并用本地存储记住选择。

浏览器预览使用示例数据，用于检查布局、状态与主题切换；它读取静态文件，不连接账户、不下载、不启动游戏。

## 云端构建 Windows 程序

1. 推送分支到自己的 GitHub 仓库或 fork。
2. Actions → **CI** → **Run workflow**。
3. 下载 `Tessera-Windows-x64` artifact，解压运行 `tessera.exe`。

CI 由 `workflow_dispatch` 手动触发；Windows job 在 GitHub 的 runner 上完成 Rust 编译，本地不需要安装 Rust。
