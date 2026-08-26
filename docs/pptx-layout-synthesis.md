# PPTX 版式合成设计（Slide Master / Layout Synthesis）

> 状态：设计文档 · 对应实现已部分落地（OPC 骨架 + theme + text/shape/line 渲染，
> 见 `src/pptx/writer.rs`）· 槽位映射仍为草案

## 1. 问题

PPTD v2 是**扁平模型**：没有 Slide Master / Slide Layout，页面完全自包含，样式靠
`theme.$key` 与元素内继承链。而 PPTX（OOXML/OPC）是**层级模型**：

```text
slideMaster → slideLayout → slide
```

PowerPoint 要求每个 slide 必须通过关系链挂到某个 slideLayout，slideLayout 再挂到
slideMaster，`[Content_Types].xml` 必须覆盖所有 part——否则文件被视为损坏、拒绝打开。

因此 SlideForge 写 PPTX 时必须**合成**一个最小的 master/layout 骨架：
结构上满足 OOXML 要求，语义上不引入 PPTD 没有的概念。

## 2. 设计原则

1. **结构合规，语义透明**。合成的 master/layout 只是 PPTX 的"合法骨架"；版式上
   不承载任何内容与样式，页面上有什么就画什么（与 PPTD 的 WYSIWYG 一致）。
2. **样式全部显式落盘**。PPTD 的全部样式继承（`content.style` → theme.textStyles
   → 默认值、表格分类样式、图表 seriesDefaults 浅合并）由 writer 在写 slide 时
   **解析完并直接写进元素属性**（`a:rPr` / `a:tcPr` 等），绝不依赖合成的
   master/layout 里的 `txStyles` 做运行时继承——合成骨架的 txStyles 不会随 deck
   变化，依赖它等于让样式"碰运气"。
3. **占位符（placeholder）不引入**。PPTD 没有占位符概念，页面元素都是独立对象
   （文本框/形状/表格/图表可单独编辑），合成 layout 一律 `type="blank"`、无
   `p:ph`。
4. **pageType 暂不参与**。`pageType`（cover/final/...）是语义标签，本阶段不改变
   生成结果；未来落地"页面级预设"时再由它挑选真实 layout。

## 3. 生成的 OPC 结构（每个 deck 一份）

```text
[Content_Types].xml
_rels/.rels                        rId1 → ppt/presentation.xml (officeDocument)
                                   rId2 → docProps/core.xml (core-properties)
docProps/core.xml
ppt/presentation.xml               sldMasterIdLst · sldIdLst · sldSz · notesSz
ppt/_rels/presentation.xml.rels    rId1 → slideMaster (slideMaster)
                                   rId2 → theme (theme)
                                   rId3.. → slideN (slide)
ppt/theme/theme1.xml               ← Presentation.theme 映射而来
ppt/slideMasters/slideMaster1.xml  clrMap · txStyles(骨架) · sldLayoutIdLst ×2
ppt/_rels/... 幻灯片与 master/layout 各有一份 .rels
ppt/slideLayouts/slideLayout1.xml  type="blank"  （默认，全体 slide 使用）
ppt/slideLayouts/slideLayout2.xml  type="title"  （预留，见 §6）
ppt/slides/slideN.xml              ← 一页一个，全部内容在 cSld/spTree
ppt/slides/_rels/slideN.xml.rels   rId1 → slideLayout1
```

要点：

- **一个 master + 一个 blank layout** 即可满足最小合规；`title` layout 为未来
  预设/占位符扩展预留（本阶段也可不生成）。
- **rId 分配固定方案**：`presentation → master(rId1) → theme(rId2) → slides(依次)`；
  `master → theme(rId1), layouts(rId2..)`；`slide → layout(rId1)，media 从 rId2 起`。
  固定顺序让打包器与验证器可确定性检查。
- **slide 数量 = pages 数量**，顺序与 `presentation.pages` 一致；`p:sldIdLst` 的
  id 值按 256 起递增。

## 4. theme → theme1.xml 映射（草案）

`theme.colors` 的键是任意的（`$primary` `/ $text` ...），OOXML `clrScheme` 只有
固定槽位（dk1/lt1/dk2/lt2/accent1-6/hlink/folHlink）。映射规则草案：

| PPTD 键（不区分大小写） | clrScheme 槽 |
|---|---|
| `text` / `text2` | dk1 / dk2 |
| `background` / `bg` / `back` | lt1 |
| `primary` | accent1 |
| `secondary` | accent2 |
| `accent` | accent3 |
| `success` / `ok` | accent4 |
| `warning` / `warn` | accent5 |
| `danger` / `error` | accent6 |
| `link` / `hlink` | hlink |

- 未出现在表中的键按声明顺序填入尚未占用的槽位（先 accent1-6，再 hlink/folHlink）。
- 未提供的槽位使用 Office 默认色（如 accent1 `#5B9BD5` 系列）兜底。
- `fontScheme`：major/minor 字体取 `theme.textStyles` 里首个字体或默认 `MiSans`；
  中英文字体分别写入 `a:latin` / `a:ea`（对应 PPTD `FontFamily: {latin, ea}`）。
- `fmtScheme`：先用 OOXML 默认三分法占位。

> 该映射属于**草案**，落地时需在 PowerPoint 里逐槽验证渲染；对文档可编辑性无影响
> （§2-2 保证样式显式化后不依赖 clrScheme 的运行时解析）。

## 5. 样式解析→显式化对照（writer 内部）

| PPTD 配置 | 写进 slide 的 OOXML |
|---|---|
| `content.fontSize` / 继承值 | `a:rPr sz`；1px = 1pt，EMU 换算 ×12700 |
| `content.fontFamily` | `a:rPr > a:latin/a:ea typeface`（`{latin, ea}` 或统一单值） |
| `content.color` | `a:rPr > a:solidFill > a:srgbClr`；`$key` 在此刻解析为 hex |
| `bold/italic/underline` | `a:rPr b/i/u`；富文本 `<strong>/<em>/<u>` 按段内 run 拆分 |
| `lineHeight` / `lineHeightPx` | `a:pPr > a:lnSpc`（spcPct / spcPts） |
| `align` | `a:pPr algn` + 垂直锚点 `a:bodyPr anchor`，多余方向裁剪说明 |
| `bounds/rotation/flip` | `a:xfrm off/ext/rot/flipH/flipV` |
| 富文本标签 | `<p>`→`a:p`，`<span style=...>`→`a:rPr` 内联覆盖（按优先级链合并） |
| 表格样式链 | 按 firstRow/lastRow/firstColumn/lastColumn/body/cell 顺序解析后写进 `a:tcPr` + `a:trPr`（网格 `a:gridCol`） |
| page.background | slide 背景：`p:bg > a:bgPr`（solid/grad/image），不放进 master |
| 动画 | §6 之后按 `Page.animations` → `p:timing` |

## 6. 扩展路径

- **页面级预设（design system / 模板）**：预设定义为「背景 + 一组元素模板 +
  可选占位符」，writer 展开为真实 `slideLayoutN.xml`；`pageType=cover` 等映射到
  对应 layout，slide 通过 `sldLayoutId → 版式` 关联。此时 §2-3 放宽为「layout 可
  携带占位符，但 slide 内容仍是独立对象」。
- **共用背景优化（可选）**：若多页 `background` 相同，可合并到一个 layout 的
  `p:bg`，slide 留空背景位——减少冗余，但与"样式显式化"原则冲突，属可关闭的优化。
- **PPTX → PPTD 反向**：解析时反方向"拍平"——把 master/layout 的继承结果合并进
  每页（背景、占位符文本、默认字体），产出纯自包含的 `.page`。

## 7. 验收清单（对应 skill 的导出校验）

1. ZIP 内所有 part 有 content-type；`_rels/.rels` 指向 officeDocument + core-props。
2. 每个 slide 恰好一条 `slide → slideLayout` 关系；layout → master → theme 链条完整。
3. `p:transition`（fade）位于合法 CT_Slide 顺序（cSld 之后）。
4. 每个 slide 的元素计数非零：独立文本框/形状/表格/图表（即"可编辑"判定），
   不得依赖 layout 占位符提供内容。
5. PowerPoint/WPS 打开冒烟测试；后续可选 LibreOffice `--convert-to pdf` 无头渲染校验。