# PPTD 写作参考指南（面向 AI）

> 给 AI agent 用的 PPTD 写作规范与工作流指南。读完它，你应该能用
> [SlideForge](../README.md) 这个本地编译器独立产出一个**可校验、可编译成
> 可编辑 PPTX** 的 PPTD 项目。
>
> 本指南以本仓库（`slideforge`，Rust 实现的 PPTD→PPTX 编译引擎）为操作骨干。
> **规范语义**见 [`docs/pptd-spec.md`](./pptd-spec.md)（Moonshot PPTD v2 的本仓库只读镜像）；
> **SlideForge 的 layouts 扩展**见 [`docs/pptd-layout-extension.md`](./pptd-layout-extension.md)
> （`Presentation.layouts` / `Page.layout` / `Text.placeholder`，可选、向后兼容）。
> **SlideForge 的 group 扩展**见 [`docs/pptd-group-extension.md`](./pptd-group-extension.md)
> （`ElementCommon.groupId`/`groupBounds` + `Page.groups`，扁平形式不变、writer 重建 `<p:grpSp>`，保留 WPS 组合选择框）。
> **SlideForge 的 round-trip 扩展字段**见 [`docs/pptd-roundtrip-extension.md`](./pptd-roundtrip-extension.md)
> （`Text.fill/border`、`TextContent` 的 autofit/margin/bullet/bodyPrExtras、`Border.gradient`、`Shadow.scale+inner`、`GradientFill.scaled`、`Image.softEdge`、`video`/`audio` 嵌入媒体元素等，convert 写出、build 尊重、手写可忽略）。
> 这些文档是 AI 编辑 PPTD 时的权威参考。Moonshot 的参考资料在「形状库 / 字体库 /
> 设计系统 / 场景分类」上更完整，写作时互补。

---

## 0. 一句话定位

- **PPTD**（PPT-DSL）是一种基于 YAML 1.2、面向 AI/编辑器友好的幻灯片中间语言。
  它是 OOXML 的**扁平抽象**：没有 Slide Master / Layout，每页自包含，所见即所得。
- **SlideForge** 是用 Rust 写的 PPTD 编译引擎：解析 → 校验 → 编译成可编辑 PPTX，
  并且能把任意 PPTX 反向编译回 PPTD 项目。**不依赖浏览器、不依赖网络、无需登录**。

本仓库 PPTD 的特点：

- **编译方式**：纯 Rust 本地编译，无需浏览器/网络/登录。
- **可编程性**：强类型 AST + 语义校验器 + 反向解析（`convert`）。
- **反向解析**：`convert` 把任意 PPTX → PPTD（含 custGeom↔SVG path），故"改已有 PPTX"是一等公民。
- **校验**：`check` 一次列出全部 `Diagnostic`；`build` 校验不过则拒绝写文件。
- **形状库/字体库/设计系统**：随仓库/技能自带 `shapes.md`（177 个 shapeName）/`fonts.md`/`design_system/`，无需外查。

> 写 PPTD 时用本指南 + 本仓库的 `check`/`build` 闭环；需要查形状 `shapeName`/`adjustments`
> 或可用字体时，查本仓库自带的 `shapes.md`/`fonts.md`（技能里在 `references/pptd/`）。

---

## 1. 工作流与命令（必读）

SlideForge 是 `check → build` 闭环。**永远先 `check` 再 `build`**：`build` 内部
会先跑一遍校验，有任何 issue 就拒绝写文件（退出码 2）；校验通过后才开始渲染，
渲染阶段遇到 writer 还不支持的元素会报 `not supported yet` 并中止（退出码 1）。

```bash
# 1) 校验：解析 + 语义校验，一次列出全部问题（不报错式中止）
cargo run -- check path/to/deck.pptd
#   退出码：0=通过  2=有 issue（stderr 逐条列出）  1=加载/解析失败

# 2) 摘要：把解析后的 AST 概要打印出来（调试用，不写文件）
cargo run -- dump path/to/deck.pptd

# 3) 编译：校验通过后写成可编辑 .pptx（OPC 打包，含 fade 页过渡）
cargo run -- build path/to/deck.pptd --output out.pptx
#   --output 省略时默认 <entry basename>.pptx

# 4) 反向解析：把任意 .pptx 转成 PPTD 项目目录（含 media/ 与 pages/）
cargo run -- convert 某模板.pptx ./converted
#   产出 converted/deck.pptd；不支持的元素会逐条列入 skipped 报告，不静默丢弃
```

`build` 写出的 PPTX：结构合规的合成 slideMaster/slideLayout 骨架（`type="blank"`、
无占位符），样式**全部显式落盘**到每个元素的 `a:rPr`/`a:tcPr`，不依赖运行时继承。
默认每页带一个根级 `fade` 页过渡。

---

## 2. 支持矩阵（决定你能写什么）

这是本指南最重要的一节。PPTD 规范定义的能力，**远大于** 本仓库 writer 当前能编译
的能力。下表把「解析/校验」与「编译成 PPTX」分开，**写 PPTD 前先对照此表**：

| 能力 | `check`/`dump`（解析+校验） | `build`（写 PPTX） | `convert`（PPTX→PPTD） |
|---|---|---|---|
| `text` 文本框 | ✅ | ✅ | ✅ |
| `shape` 内置形状 | ✅ | ✅ | ✅ |
| `shape` custom（SVG path） | ✅ | ✅ | ✅（custGeom 公式求值→path） |
| `line` 连线 | ✅ | ✅ | ✅ |
| `image` 图片 | ✅ | ✅ | ✅ |
| `video` / `audio` 嵌入媒体 | ✅ | ✅（需 poster 海报帧） | ✅（p:pic+videoFile/audioFile） |
| `icon`（Font Awesome 7.x） | ✅ | ✅ | ✅ |
| `table` 表格 | ✅ | ❌ `not supported yet` | ⚠️ 视元素而定，可能 skip |
| `chart` 图表 | ⚠️ 仅 5 类 series | ❌ `not supported yet` | skip |
| 主题 `colors` / `textStyles` | ✅ | ✅（显式落盘） | ✅ |
| 主题 `tableStyles` | ✅ | ❌（依赖 table） | — |
| 富文本 `<p>/<span>/<strong>/<em>` | ✅ | ✅ | ✅ |
| 页背景 `solid` / `gradient` | ✅ | ✅ | ✅ |
| 页背景 `Fill::Image`（图片填充） | ✅ | ✅（`a:blipFill`） | ✅ |
| `animations` 动画 | ✅ | ❌ 路线图（`p:timing`） | — |
| `notes` 演讲者备注 | ✅ | ❌ 路线图 | — |
| `shadow` 阴影 | ✅ | ✅（shape `a:outerShdw`/`a:innerShdw`；`image.shadow` 仍静默丢弃） | — |
| 渐变填充 `gradient` | ✅ | ✅ | ✅ |

> **给 AI 的硬规则**：
> 1. 目标是「能 `build` 出 PPTX」时，**只使用** `text` / `shape` / `line` /
>    `image` / `icon` + solid/gradient/image 背景 + 主题 colors/textStyles + 富文本。
>    **不要** 在页面里放 `table` 或 `chart` 元素，否则 `build` 会中止。
> 2. 若用户需要表格/图表，**优先用 `shape` + `text` 手工拼装**（网格用 `shape`
>    画单元格、`text` 写内容、`line` 画分隔线），效果可控且能 build。
> 3. 若只要「可校验的 PPTD 数据」（后续交给别的工具或未来 writer 补全），可写全要素，
>    但需在交付说明里标注「table/chart 当前无法 build 成 PPTX」。
> 4. 图表 series 目前只建型 5 类：`bar` / `line` / `area` / `pie` / `scatter`。
>    写 `bubble` / `candlestick` / `radar` / `waterfall` / `heatmap` / `treemap` /
>    `sunburst` / `sankey` **会解析失败**（未知 series tag）。

---

## 3. 项目结构

PPTD 是**多文件**结构：一个主入口 `.pptd` + 若干 `.page` + `media/`。

```text
my_deck/
├── deck.pptd            # 主入口：version/size/theme + pages 列表
├── pages/
│   ├── 1_cover.page     # 每页一个文件
│   └── 2_content.page
└── media/               # 图片等资源（可选）
    └── logo.png
```

**路径规则（强制）**：

1. **完全自包含**：所有被引用的文件必须在 `.pptd` 所在目录之内，**禁止引用目录外文件**。
2. **只支持相对路径**（相对于 `.pptd` 所在目录）：
   - `.pptd` 里的 `pages` 列表：`pages/1_cover.page`
   - `.page` 里的图片：`media/logo.png`
3. **媒体支持 URL**：`Image.src` 与图片填充 `Fill.type: image` 的 `src` 可以是
   `https://...`（仅 jpg/jpeg/png/gif）。`build` 已支持页/版式背景的 `Fill::Image`
   （`a:blipFill`），形状级图片填充仍不支持；`Image` 元素的本地路径与 URL 均可 build。
4. **主入口必需**：不能单独把一个 `.page` 丢给 `check`/`build`，必须经由 `.pptd` 主入口
   加载（页顺序由 `.pptd` 的 `pages` 列表决定）。

---

## 4. 全局约定

### 坐标与单位

- 所有几何/尺寸单位都是 **px**；页面原点 `(0, 0)` 在**左上角**。
- 默认画布尺寸：16:9 → `[960, 540]`；4:3 → `[720, 540]`。海报尺寸另有规定（见
  Moonshot `general-poster.md`），本指南聚焦 PPT。
- **本规范定义 1px = 1pt**：`fontSize: 18` 在 PPTX 里就是 18pt。
- **堆叠顺序**：由 `Page.elements` 数组顺序决定，**越靠后层级越高**（后画的盖在前面）。

### 样式优先级链（核心，写文本/表格必懂）

属性冲突时，按下列来源**从上到下**找第一个有值的来源；都没有才回退到默认值：

**① 文本框内文本**（`Text.content`）：

1. 富文本语义标签（`<strong>`/`<u>`/`<sup>` …）
2. `<span style="...">` 内联属性
3. `<p style="...">` 段落属性
4. `Text.content` 上直接写的样式字段（`color`/`fontSize`/`fontFamily`/`bold`/
   `italic`/`backgroundColor`/`lineHeight`/`lineHeightPx`/`letterSpacing`/`marginTop`）
5. `Text.content.style` 引用的 `theme.textStyles["$key"]`
6. 默认值：`color=#000000`、`fontSize=18`、`fontFamily="MiSans"`、`bold=false`、
   `italic=false`、`lineHeight=1`、`letterSpacing=0`、`marginTop=0`

**② 表格单元格**（`Cell`）：富文本 → span → p → `Cell` 内联字段 → `Cell.textStyle`
引用的 `TextStyleConfig`（**仅作用于文本字段，不含 fill/border/align**）→
`TableStyleConfig` 的位置分类样式（行分类 vs 列分类，由 `rowOverColumn` 决定胜负，
默认 `true`=行胜）→ `bodyStyles`（首末行之间的数据行按索引循环）→ `cellStyle`（全表
基线）→ 默认值。

> **重要**：`lineHeight`（倍率）与 `lineHeightPx`（固定 px）互斥，同时设时
> `lineHeightPx` 优先。

### 主题引用机制

用 `$<key>` 在相关字段引用主题，三处各自查表：

| 主题类型 | 引用字段 | 例 |
|---|---|---|
| `theme.colors` | 任何 `Color` 字段 | `$primary` |
| `theme.textStyles` | `Text.content.style` / `Cell.textStyle` | `$title` |
| `theme.tableStyles` | `Table.style` | `$default` |

`$key` 不在对应表里 → `check` 报 Diagnostic：`style \`$key\` is not defined in theme.*`。

---

## 5. 主入口 `.pptd`

### Presentation

```yaml
version: v2          # 必填，固定 "v2"；loader 强校验，写别的会被拒
title: 我的演示      # 可选
customFonts:          # 可选；Google Fonts CSS URL，仅默认字重
  - family: Noto Serif SC
    src: "https://fonts.googleapis.com/css2?family=Noto+Serif+SC"
size: [960, 540]      # 必填；[宽, 高]
theme:                # 可选；见下
  colors: { ... }
  textStyles: { ... }
  tableStyles: { ... }
pages:                # 必填；相对路径列表，顺序=幻灯片顺序
  - pages/1_cover.page
  - pages/2_content.page
```

### Theme

```yaml
theme:
  colors:                       # 命名色，任意 key
    primary: "#2563EB"
    accent: "#F59E0B"
    text: "#1F2937"
  textStyles:                   # 命名文本样式，被 $key 引用
    title:
      fontSize: 40
      color: "$primary"
    body:
      fontSize: 18
      color: "$text"
      lineHeight: 1.6
  tableStyles:                  # 命名表格样式（build 暂未支持 table，但 check/dump 可用）
    default:
      firstRowStyle:
        fill: {type: solid, color: "$primary"}
        color: "#ffffff"
        bold: true
      bodyStyles:
        - {fill: {type: solid, color: "#f8fafc"}}
        - {fill: {type: solid, color: "#ffffff"}}
```

`TextStyleConfig` 字段（全部可选，未设则沿继承链回退）：`color` / `fontSize` /
`fontFamily` / `bold` / `italic` / `backgroundColor` / `lineHeight` / `lineHeightPx` /
`letterSpacing` / `marginTop`。

---

## 6. 页面 `.page`

```yaml
pageType: cover          # 可选；语义标签，不影响渲染（cover/content/final/... 或自定义）
background:              # 可选；默认 {type: solid, color: "#FFFFFF"}
  type: solid
  color: "$primary"
notes: 演讲者备注         # 可选；纯文本；build 暂未写 notesSlide（路线图）
elements: [ ... ]        # 必填；见下；越靠后层级越高
animations: [ ... ]      # 可选；见 §10；build 暂未写 p:timing（路线图）
```

---

## 7. 元素速查（ElementBase + 7 类）

所有元素共享 `ElementBase`：

```yaml
elementId: title1        # 必填；页内唯一（重复→check 报 duplicate）
elementType: text       # 必填；text|shape|line|image|icon|table|chart
bounds: [x, y, w, h]     # 必填；[x, y, width, height] px；左上为原点
# 以下三个对所有元素可选（table/chart 的整体旋转/翻转受 PPT 限制，见规范）：
rotation: 15            # 顺时针度数
opacity: 0.8            # [0,1]；超范围→check 报错
flip: [false, true]     # [水平翻转, 垂直翻转]
```

> `check` 会校验：`elementId` 唯一、`bounds` 正数有限（`line` 允许单轴退化为 0）、
> `opacity ∈ [0,1]`、动画 `elementId` 引用存在、文本/表格的 `$style` 引用可解析、
> 表格行列比例和为 1、图表 `encode` 列存在等。

### 7.1 text 文本框

```yaml
- elementId: title
  elementType: text
  bounds: [100, 200, 760, 80]
  content:
    style: "$title"            # 引用 theme.textStyles
    align: [center, middle]    # [水平, 垂直]；默认 [left, top]
    text: 年度工作总结           # 纯文本或富文本（见 §8）
```

`TextContent` 全字段：`text`（必填，富文本）/ `style` / `color` / `fontSize` /
`fontFamily` / `bold` / `italic` / `backgroundColor` / `lineHeight` / `lineHeightPx` /
`letterSpacing` / `marginTop` / `textDirection`(horizontal|vertical) / `wrap`(默认true) /
`align` / `autofit`(fit_shape/fit_text/fixed) / `gradient` / `shadow`。

> 上述含 SlideForge 扩展字段（`autofit`/四边 margin 等）；完整 round-trip 扩展字段见
> [`pptd-roundtrip-extension.md`](./pptd-roundtrip-extension.md)。
> 单行文本建议显式 `wrap: false`；多行用块标量 `|`（见 §8）。

### 7.2 shape 形状

```yaml
- elementId: badge
  elementType: shape
  bounds: [420, 120, 120, 60]
  shapeName: roundRect          # 见 shapes.md；custom 见下
  adjustments: [16667]          # 几何参数，复用 OOXML 顺序与数量
  fill: {type: solid, color: "$accent"}
  border: {style: solid, width: 2, color: "$primary"}
```

`shape: custom` 用 `viewBox` + SVG `path`（支持 `M/L/H/V/C/S/Q/A/Z`）：

```yaml
- elementId: ring
  elementType: shape
  bounds: [400, 200, 150, 150]
  shapeName: custom
  viewBox: [1000, 1000]
  path: "M500,0 A500,500 0 1 1 499,0 Z M500,200 A300,300 0 1 0 499,200 Z"
  fill: {type: solid, color: "$accent"}
```

> **镂空规则**：外轮廓顺时针（`sweep=1`）、内轮廓逆时针（`sweep=0`）= 镂空。
> **比例**：改 `bounds` 缩放形状，无需改 path；但 viewBox 被独立拉伸到 bounds，比例不同
> 会变形——保持 `viewBoxW : viewBoxH = bounds.w : bounds.h` 才不变形。
> **shape 不支持内嵌文字**，要叠文字就再加一个 `text` 元素。

常用 `shapeName`：`rect` / `roundRect`(`[16667]`) / `ellipse` / `triangle`(`[50000]`) /
`diamond` / `star5` / `rightArrow`(`[50000,50000]`) / `chevron` / `donut`(`[25000]`)。
完整 177 个见 Moonshot `reference/shapes.md`。

### 7.3 line 连线

```yaml
- elementId: divider
  elementType: line
  bounds: [200, 360, 560, 40]
  viewBox: [560, 40]            # path 坐标系 [w,h]；points 住在里面
  points: "0,20 560,20"         # "x1,y1 x2,y2 ..."；首末=曲线经过点，中间=控制点
  curve: sharp                  # sharp|round|smooth（默认 round）
  arrow: [null, arrow]         # [起始, 结束]；arrow|stealth|diamond|oval
  border: {style: solid, width: 2, color: "#ffffff"}
```

> `points` 至少 2 个，所有坐标必须在 `viewBox` 内。改 `bounds` 不用改 `points`；
> 保持 `viewBoxW:viewBoxH = bounds.w:bounds.h` 才不变形。直线可退化 `bounds` 的单轴为 0。

### 7.4 image 图片

```yaml
- elementId: logo
  elementType: image
  bounds: [300, 140, 80, 40]
  src: media/parts/logo.png     # 相对路径或 https://
  fit: {mode: contain}          # fill|contain|cover（默认 cover）
  crop: {top: 0.1, bottom: 0.1} # 比例裁剪；正值内裁、负值外扩补透明
  cropShape: {shapeName: roundRect, adjustments: [15000]}  # 形状裁剪
  border: {style: solid, width: 1, color: "#000000"}
  shadow: {blur: 10, color: "#00000033", offset: [0, 4]}
```

渲染顺序固定：`crop`（调源矩形）→ `fit`（适配 bounds）→ `cropShape`（形状裁剪）。
注意 `image.shadow` 当前 `build` 仍静默丢弃（shape 的 shadow 已支持）。

### 7.5 video / audio 嵌入媒体

```yaml
- elementId: demo-clip
  elementType: video            # 或 audio
  bounds: [400, 200, 480, 270]
  src: media/clip.mp4           # mp4/m4v/mov · mp3/m4a/wav/wma
  poster: media/poster.png      # 播放前显示的海报帧（png/jpg，必填）
```

渲染为 OOXML 媒体图片（`p:pic` + `a:videoFile`/`a:audioFile` + `p14:media`
扩展）；`convert` 遇到源 PPTX 里的嵌入媒体会还原成此元素，媒体文件字节级
保真。播放控制（自动播放/循环/音量）暂不支持，PowerPoint 默认点击播放。

### 7.6 icon 图标（Font Awesome 7.x free）

```yaml
- elementId: bulb
  elementType: icon
  bounds: [100, 100, 48, 48]
  iconName: "fas:lightbulb"    # "style:name"；fas=实心/far=regular/fab=brands
  fill: {type: solid, color: "$primary"}
```

检索：https://fontawesome.com/search?ic=free-collection

### 7.7 table 表格（check/dump 可用，build 暂不支持）

```yaml
- elementId: tbl
  elementType: table
  bounds: [60, 120, 840, 260]
  columnWidths: [0.4, 0.3, 0.3]   # 列宽比例，和为 1
  rowHeights: [0.2, 0.4, 0.4]     # 行高比例，和为 1
  style: "$default"               # $key 或 inline TableStyleConfig
  rows:
    - - {text: 指标}
      - {text: "2023"}
      - {text: "2024"}
    - - {text: 营收}
      - {text: "82.5"}
      - {text: "96.3"}
```

**合并单元格规则**：用 `rowSpan`/`colSpan` 声明合并范围，**被覆盖的位置从 `rows`
数组里省略，不需要 `null` 占位**。例如左上 2×2 合并后：第 0 行只剩 2 项（合并格 +
(0,2)），第 1 行只剩 1 项（(1,2)）。`check` 会校验：比例和为 1、行数=`rowHeights`
长度、每行 cell 数 ≤ 列数（合并格已省略）。

> ⚠️ build 阶段遇到 `table` 会报 `not supported yet` 并中止。要出 PPTX 请用
> `shape`+`text` 手工拼装表格。

### 7.8 chart 图表（check/dump 仅 5 类 series，build 不支持）

```yaml
- elementId: trend
  elementType: chart
  bounds: [60, 400, 840, 120]
  data:
    cols: [quarter, revenue]      # 列名，唯一且非空
    rows:                         # 每行长度=cols 长度；缺失用 null
      - [Q1, 120]
      - [Q2, 132]
  series:
    - type: bar                   # bar|line|area|pie|scatter（仅此 5 类可解析）
      encode: {x: quarter, y: revenue}
      fill: "$primary"
```

> ⚠️ 写 `bubble`/`candlestick`/`radar`/`waterfall`/`heatmap`/`treemap`/`sunburst`/
> `sankey` **会解析失败**（AST 未建型）。`build` 阶段任何 chart 都会报 not supported。
> 需要图表时，用 `shape` 画坐标系 + `shape`/`line` 画数据图形 + `text` 标注。

---

## 8. 富文本规则（`Text.content.text` 与 `Cell.text`）

`text` 支持 HTML 子集。**含特殊字符（`:`/`#`/`{`/`}`/`style="..."`）时务必用块
标量 `|`**，否则 YAML 会误解析。

### 支持的标签

| 标签 | 用途 |
|---|---|
| `<p>` | 段落，可带段落样式 |
| `<span style="...">` | 内联样式（仅 color/font-size/font-family/background-color） |
| `<strong>` / `<em>` | 加粗 / 斜体 |
| `<u>` / `<s>` | 下划线 / 删除线 |
| `<sup>` / `<sub>` | 上标 / 下标 |
| `<a href="...">` | 超链接（https/http/mailto，自动蓝下划线） |
| `<ul>`/`<ol>`/`<li>` | 列表 |

### style 属性映射

- **段落样式（仅 `<p>`）**：`text-align`(left|center|right|justify|distributed) /
  `line-height`(无单位=倍率 / `24px`=固定) / `margin-top` / `margin-left` /
  `margin-right`（都是 px 字符串）。**不要在 `<p>` 上设 letter-spacing**。
- **列表项样式（仅 `<li>`）**：上述对齐/行高/间距 + `list-style*` 系列。
- **内联样式（仅 `<span>`）**：`color` / `font-size` / `font-family` /
  `background-color`（色值均支持 `$theme` 引用）。

### 纯文本速记

- 单行 `text: Hello` ≡ `text: "<p>Hello</p>"`
- 多行块标量每行 ≡ 一个 `<p>`；段落内换行用 `<br/>`（但编辑后往返不保证保留，
  稳定换行用多个 `<p>`）。
- LaTeX：`\(a^2+b^2=c^2\)`，公式内**不允许**富文本标签，只继承 color/font-size。

### 样例

```yaml
content:
  align: [left, top]
  lineHeight: 1.6
  text: |
    <p><strong>关键成就</strong>：完成 <span style="color:$primary;">3</span> 个重点项目</p>
    <p style="text-align:right"><span style="font-size:14px; color:#6b7280;">—— FY2024</span></p>
```

> `build` 已实现：`<p>`/`<span>`/`<strong>`/`<em>` → 逐段落/逐 run 拆分；段落级
> `text-align`/`line-height`/`margin-top`；文本框级 `wrap`/`align`/`autofit`/四边 margin。
> `<u>`/`<s>`/`<sup>`/`<sub>`/`<a>`/`<ul>`/`<ol>`/`<li>`/LaTeX 在规范里定义，writer
> 落地情况以源码为准。

---

## 9. 动画（`Page.animations`，build 暂不支持）

页级 `animations` 数组，按数组顺序编排，通过 `elementId` 引用本页元素。`trigger` 决定
何时开始，`effect` 决定效果。

```yaml
animations:
  - elementId: title
    effect: fade-in            # 见下；22 种
    trigger: onClick           # onClick|withPrevious|afterPrevious（默认 onClick）
    direction: up              # up|down|left|right；仅 fly/wipe/peek/float 有效
    durationMs: 500            # >0
    delayMs: 0                 # >=0
    easing: linear             # linear|ease-in|ease-out|ease-in-out
    repeat: 1                  # 正整数
```

- 入场：`appear`/`fade-in`/`fly-in`/`zoom-in`/`wipe-in`/`float-in`/`peek-in`/`rise-in`
- 强调：`pulse`/`grow-shrink`/`spin`/`teeter`/`fill-color`(需 `color`)/
  `transparency`(需 `amount ∈ [0,1]`)/`color-pulse`(需 `color`)
- 退场：`disappear`/`fade-out`/`fly-out`/`zoom-out`/`wipe-out`/`float-out`
- 路径：`motion-path`（需 `path`，SVG path，相对元素当前位置的偏移，`M 0 0` 起始）

> 每页 1–3 组、优先 fade/fly/zoom 等简单效果。`check` 会校验动画引用的 `elementId`
> 在本页存在。`build` 写 `p:timing` 在路线图中。

---

## 10. 最小可运行样例

把以下三个文件按目录放好，即可 `cargo run -- check` 与 `cargo run -- build`：

`docs/samples/min/deck.pptd`：
```yaml
version: v2
title: SlideForge 最小样例
size: [960, 540]
theme:
  colors:
    primary: "#2563EB"
    accent: "#F59E0B"
    text: "#1F2937"
  textStyles:
    title:
      fontSize: 40
      color: "$primary"
    body:
      fontSize: 18
      color: "$text"
      lineHeight: 1.6
pages:
  - pages/1_cover.page
  - pages/2_content.page
```

`docs/samples/min/pages/1_cover.page`：
```yaml
pageType: cover
background:
  type: solid
  color: "$primary"
elements:
  - elementId: title
    elementType: text
    bounds: [100, 200, 760, 80]
    content:
      style: "$title"
      align: [center, middle]
      text: 你好 SlideForge
  - elementId: subtitle
    elementType: text
    bounds: [100, 290, 760, 40]
    content:
      align: [center, middle]
      color: "#ffffff"
      text: 用 PPTD 写幻灯片
  - elementId: badge
    elementType: shape
    bounds: [420, 120, 120, 60]
    shapeName: roundRect
    adjustments: [16667]
    fill: {type: solid, color: "$accent"}
  - elementId: line1
    elementType: line
    bounds: [200, 360, 560, 0]
    viewBox: [560, 1]
    points: "0,0 560,0"
    curve: sharp
    border: {style: solid, width: 2, color: "#ffffff"}
```

`docs/samples/min/pages/2_content.page`：
```yaml
pageType: content
elements:
  - elementId: heading
    elementType: text
    bounds: [60, 40, 840, 50]
    content:
      style: "$title"
      text: 关键指标
  - elementId: body
    elementType: text
    bounds: [60, 120, 840, 360]
    content:
      style: "$body"
      text: |
        <p><strong>说明</strong>：本页只用了 <span style="color:$primary;">text</span> 与
        <span style="color:$accent;">shape</span> 元素，可以安全 <em>build</em> 成 PPTX。</p>
        <p style="text-align:right"><span style="font-size:14px; color:#6b7280;">—— 示例页</span></p>
```

跑：
```bash
cargo run -- check docs/samples/min/deck.pptd   # 期望：no issues
cargo run -- build docs/samples/min/deck.pptd --output min.pptx
```

---

## 11. 常见陷阱与最佳实践（给 AI 的 do/don't）

**✅ 该做的**

1. **先 check 再 build**。`check` 一次列出全部 Diagnostic，修完再 build。
2. **严格用相对路径 + 自包含目录**。绝不引用项目目录外的文件。
3. **画布与比例**：16:9 用 `[960, 540]`；`bounds` 之和不超过画布。
4. **主题优先**：颜色/文本样式集中放 `theme`，元素里用 `$key` 引用，少写魔法色值。
5. **shape 比例守恒**：custom 几何保持 `viewBoxW:viewBoxH = bounds.w:bounds.h`。
6. **富文本用块标量 `|`**：含 `style=` 的 `text` 一律块标量，避免 YAML 误解析。
7. **要 build 就别用 table/chart**：用 `shape`+`text`+`line` 手工拼表格/图表。
8. **并行写文件**：生成多页时一次性并行写多个 `.page`，减少往返。
9. **line 的退化**：水平直线 `bounds` 高度可为 0、`viewBox:[w,1]`、`points:"0,0 w,0"`。

**❌ 别踩的坑**

1. `version` 写成 `v1`/`2`/`"2"` → loader 直接拒（必须精确 `"v2"`）。
2. `bounds` 出现 0 宽 0 高（非 line）→ check 报 degenerate。
3. `$primary` 没在 `theme.colors` 定义 → check 报 style 未定义。
4. `columnWidths`/`rowHeights` 和不为 1（浮点误差也要 ≤ 1e-3）→ check 报错。
5. 表格合并格没从 `rows` 省略，还塞 `null` → check 报 cell 数超列数。
6. chart 写了 `bubble`/`sankey` 等 → **解析失败**（未知 series tag）。
7. 页面里有 `table` 或 `chart` 还去 `build` → 中止报 `not supported yet`。
8. `content.text` 用普通字符串却含 `style="..."` → YAML 解析错乱。
9. `Image.src` 引用目录外文件 → 违反自包含规则。
10. 给 `shape` 想加文字 → shape 不支持内嵌文字，要另加 `text` 元素。

---

## 12. 反向工作流：从已有 PPTX 学/改

```bash
# 把任意 .pptx 转成 PPTD 项目
cargo run -- convert 某模板.pptx ./converted
# 产出 ./converted/deck.pptd + pages/ + media/，并在 stderr 列出 skip 项
```

`convert` 提取母版→布局→页面三层烘焙、文本框/形状/连线/图片/图标、主题色与尺寸，
并把自定义几何（custGeom）的 guide 公式求值后写成 SVG path。**不支持的元素不静默
丢弃**：图表、表格等会逐项报告（页号 + 元素名 + 原因），转换照常成功。改完后再
`build` 回 PPTX 做往返验证。

版式（layouts）行为：每个 slideMaster 下的**全部** slideLayout 都会转换——包括没
有页面引用的版式（在 PowerPoint 里把页面改指到别的版式后 re-convert，原版式定义
不会丢）；版式键名用模板自带的语义名 `<p:cSld name>`（如 `cover` / `标题幻灯片`），
无名时才回退 `layout_N`，重名自动加 `_2`/`_3` 后缀。页面用 `layout: <键名>` 引用。

> 注意：PPTX→PPTD 不是完美无损的。用户反馈格式错乱时，对照原 PPTX 修 PPTD。

---

## 13. 参考资料

- 本仓库
  - [`README.md`](../README.md)：架构、命名约定、能力清单与路线图
  - [`docs/pptx-layout-synthesis.md`](./pptx-layout-synthesis.md)：master/layout 合成骨架策略
  - [`docs/pptd-roundtrip-extension.md`](./pptd-roundtrip-extension.md)：SlideForge round-trip 扩展字段（Text/TextContent/Border/Shadow/Image 等的额外字段）
  - `tests/fixtures/demo/`、`tests/fixtures/buildable/`：可运行样例
  - 源码：`src/pptd/`（AST+parser+validate）、`src/pptx/`（OPC+render+writer+import）
- Moonshot PPTD 规范与配套（语义权威，写作时查）
  - `reference/pptd.md`：完整格式定义（本指南的语义来源）
  - `reference/shapes.md`：177 个 `shapeName` 与 `adjustments` 取值
  - `reference/fonts.md`：可用字体清单
  - `reference/slides_categories.md` + `slides_categories/`：场景设计指南
  - `reference/design_system/`：预设设计系统
  - `reference/general-poster.md`：海报/信息图单页设计

> 本指南与 Moonshot `reference/pptd.md` 的字段语义一致；若有出入，**以
> `pptd.md` 为准**。本仓库独有的「能不能 build」判断，以本指南 §2 支持矩阵为准。
