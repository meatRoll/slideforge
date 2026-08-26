# SlideForge

用 Rust 编写的 **PPTD → PPTX** 编译引擎。

PPTD（PPT-DSL）是一种基于 YAML、面向 AI/编辑器友好的幻灯片中间语言，是对 OOXML 的抽象：每页自包含、所见即所得、可在 PPTX 与 PPTD 之间双向转换。SlideForge 的目标是：**解析并校验 PPTD 项目，然后把它编译成可编辑的 PPTX 文件**，让「用标准化的 PPTD 语言来编辑 PPTX」变得可编程、可测试。

## 命名与 Rust 风格约定

| 项 | 约定 |
|---|---|
| 项目名 | `SlideForge`（幻灯片锻造厂） |
| crate | `slideforge`（snake_case） |
| 二进制 | `slideforge` |
| 模块 | `pptd` / `pptx` / `error` / `cli`（小写 snake_case） |
| 类型 | `Presentation` / `Page` / `Element` / `Text`（PascalCase） |
| 字段/函数 | `element_id` / `font_size` / `load_project`（snake_case） |
| 枚举变体 | `Single` / `Cover` / `FadeIn`（PascalCase） |
| 常量 | `SUPPORTED_VERSION`（SCREAMING_SNAKE_CASE） |

PPTD 里的 camelCase 字段（`elementId`、`fontSize`…）由 serde 的 `rename_all` 负责互转，Rust 内部始终遵循官方命名风格。

## 架构

```text
slideforge/
├── src/
│   ├── main.rs            # 二进制入口（exit code 驱动）
│   ├── cli.rs             # clap 子命令：check / dump / build
│   ├── error.rs           # thiserror 统一错误
│   ├── pptd/              # PPTD 语言
│   │   ├── ast.rs         # Presentation / Page
│   │   ├── shared.rs      # Color / Bounds / Fill / Border / Shadow ...
│   │   ├── theme.rs       # Theme / TextStyleConfig / TableStyleConfig
│   │   ├── elements.rs    # Element 枚举（text/shape/line/image/icon/table/chart）+ Cell
│   │   ├── chart.rs       # ChartData + 系列配置（bar/line/area/pie/scatter 已建型）
│   │   ├── animation.rs   # Page.animations
│   │   ├── parser.rs      # YAML → AST（主入口 + 页面加载，校验 version=v2）
│   │   └── validate.rs    # 语义校验：id 唯一、bounds 合法、主题引用、表格/图表结构
│   └── pptx/              # OOXML / OPC 输出管线
│       ├── package.rs     # content-type 与包条目模型
│       ├── opc.rs         # 命名空间、关系类型、[Content_Types].xml / .rels
│       ├── xml.rs         # 迷你缩进 XML 写入器
│       ├── theme.rs       # Theme → theme1.xml 映射与颜色解析
│       ├── render.rs      # 元素 → drawingml（文本/形状/线条，含富文本）
│       ├── import.rs      # PPTX → PPTD 反向编译（convert）
│       └── writer.rs      # 包组装 + ZIP 输出（PptxWriter）
└── tests/
    ├── fixtures/demo/     # 样例 PPTD 项目（封面 + 内容页）
    ├── parse_project.rs   # 解析测试
    └── validate.rs        # 校验测试

docs/pptx-layout-synthesis.md   # PPTX 版式合成设计（master/layout 骨架策略）
```

设计要点：

- **AST 与 PPTD 规范一一对应**，并保留 `Option` 语义来表达「未设置 → 沿继承链回退」。
- `Element` / `ChartSeries` 使用 serde 的内部标签枚举（`#[serde(tag = ...)]` + newtype 变体）：判别字段（`elementType` / `type`）与元素内容合并进同一张 map，纯 derive 即得到规范的 YAML 形状。公共字段由 `ElementCommon` 承载，经 `#[serde(flatten)]` 摊平进各元素。
- 校验器产出 `Diagnostic` 列表而非直接报错，`check` 一次遍历列出全部问题。

## 快速开始

```bash
cargo run -- check tests/fixtures/demo/demo.pptd
cargo run -- dump tests/fixtures/demo/demo.pptd
cargo run -- build tests/fixtures/buildable/buildable.pptd --output out.pptx
# 反向解析：把任何 PPTX 转回 PPTD 项目目录（含 media/ 与 pages/）
cargo run -- convert 公司模板.pptx ./converted && cargo run -- build ./converted/deck.pptd --output roundtrip.pptx
```

`convert` 提取 layer（母版→布局→页面自底向上烘焙）、文本框/形状/连线/图片/图标、主题色与尺寸，并把自定义几何（custGeom）的 guide 公式求值后写成 SVG path。**不支持的元素不会静默丢弃**：图表、表格等会在报告里逐项列出（页号 + 元素名 + 原因），转换照常成功。

## 已实现能力（writer）

- text / shape / line / icon / image + 背景、主题色、渐变填充、弹入过渡
- `elementType: shape: custom`（SVG path + viewBox ↔ custGeom）
- 富文本 `content.text`：`<p style="...">` / `<span style="...">` / `<strong>` / `<em>` → 逐段落/逐 run 样式；段落级 `text-align`、`line-height`（倍率或 px）、`margin-top`
- 文本框级 `wrap` / `align` / `autofit`（FitShape/FitText）/ 四边 margin
- table / chart 仍报明确的 not-supported 错误（见路线图）

## 路线图

- [x] PPTD 类型化 AST（主题、7 类元素、动画、表格、图表核心）
- [x] 项目解析器（主入口 + 页面、version 校验）
- [x] 语义校验器
- [x] OOXML writer：theme.xml / slides / drawingml（详见 `src/pptx/writer.rs`）
    - [x] OPC 骨架：Content_Types / rels / core 属性 / presentation.xml
    - [x] theme1.xml（clrScheme 槽位映射，见 `docs/pptx-layout-synthesis.md`）
    - [x] 合成 slideMaster + layout（结构合规，样式显式落盘）
    - [x] text / shape / line / icon / image 渲染 + 页背景 + fade 过渡
    - [x] custom 几何（custGeom ↔ SVG path）
    - [x] 富文本标签 → run 拆分（`<p>`/`<span>`/`<strong>`/`<em>`）
    - [ ] table / chart 元素、notes / animations / shadow / Fill::Image
- [x] OPC 打包（`zip` crate），产出可编辑 .pptx
- [x] pptx → pptd 反向解析（`slideforge convert`，含 40 页模板往返像素级回归）
- [ ] 图表 → 原生 OOXML chart parts 、动画 → `p:timing`、备注 → notesSlide

## 许可证

MIT OR Apache-2.0