# SlideForge

用 Rust 编写的 **PPTD ↔ PPTX 双向编译引擎**。

PPTD（PPT-DSL）是一种基于 YAML、面向 AI/编辑器友好的幻灯片中间语言，是对 OOXML 的抽象：每页自包含、所见即所得。SlideForge 解析并校验 PPTD 项目，编译成可编辑的 PPTX 文件（`build`）；也能把任意 PPTX 反向编译回 PPTD 项目（`convert`），让「用标准化的 PPTD 语言来编辑 PPTX」变得可编程、可测试。

## 命名与 Rust 风格约定

| 项 | 约定 |
|---|---|
| 项目名 | `SlideForge`（幻灯片锻造厂） |
| crate | `slideforge`（snake_case） |
| 二进制 | `slideforge` |
| 模块 | `pptd` / `pptx` / `error` / `cli` / `hash`（小写 snake_case） |
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
│   ├── lib.rs             # crate 入口（pptd / pptx / cli / error / hash）
│   ├── cli.rs             # clap 子命令：check / dump / build / convert
│   ├── error.rs           # thiserror 统一错误
│   ├── hash.rs            # .sync.hash 同步记录（convert/build 写入即记账 / build 覆写守卫）
│   ├── pptd/              # PPTD 语言
│   │   ├── mod.rs         # 模块导出 + Project（入口 + 页面序列）
│   │   ├── ast.rs         # Presentation / Page
│   │   ├── shared.rs      # Color / Bounds / Fill / Border / Shadow ...
│   │   ├── theme.rs       # Theme / TextStyleConfig / TableStyleConfig
│   │   ├── elements.rs    # Element 枚举（text/shape/line/image/icon/table/chart/video/audio）+ Cell + GroupDef
│   │   ├── chart.rs       # ChartData + 系列配置（bar/line/area/pie/scatter 已建型）
│   │   ├── animation.rs   # Page.animations
│   │   ├── layout.rs      # 布局扩展：LayoutDef / PlaceholderDef / placeholder 继承
│   │   ├── parser.rs      # YAML → AST（主入口 + 页面加载，校验 version=v2）
│   │   └── validate.rs    # 语义校验：id 唯一、bounds 合法、主题引用、表格/图表结构
│   └── pptx/              # OOXML / OPC 双向管线
│       ├── mod.rs         # 模块注释与导出
│       ├── package.rs     # content-type 与包条目模型
│       ├── opc.rs         # 命名空间、关系类型、[Content_Types].xml / .rels
│       ├── xml.rs         # 迷你缩进 XML 写入器
│       ├── theme.rs       # Theme → theme1.xml 映射与颜色解析
│       ├── media.rs       # PNG/JPEG 尺寸嗅探 + 包 media 注册表
│       ├── svg_path.rs    # SVG path d → custGeom 路径指令
│       ├── fa.rs          # Font Awesome solid 图形数据集（编译期内嵌）
│       ├── render.rs      # 元素 → drawingml（文本/形状/线条/图标/媒体，含富文本与 grpSp 重建）
│       ├── import.rs      # PPTX → PPTD 反向编译（convert）
│       └── writer.rs      # 包组装 + ZIP 输出（PptxWriter）
├── assets/
│   └── fa-solid-icons.json  # Font Awesome 6 solid path 数据（CC BY 4.0，编译期内嵌）
├── tests/
│   ├── fixtures/demo/       # 最小样例项目（封面 + 内容页，check/dump 用）
│   ├── fixtures/buildable/  # 可 build 的样例项目（build/往返测试用）
│   ├── parse_project.rs     # 解析测试
│   ├── validate.rs          # 校验测试
│   ├── build_pptx.rs        # 端到端 OPC 包测试
│   ├── reverse.rs           # PPTX → PPTD → PPTX 往返回归
│   ├── schema_guard.rs      # OOXML 结构守卫（对应曾触发 WPS 修复的回归规则）
│   └── guard.rs             # .sync.hash 同步与覆写守卫测试
├── docs/                    # 规范与设计文档（索引见下）
├── skill/slideforge-ppt/    # 开箱即用的 agent 技能（含五平台预编译二进制）
└── scripts/                 # install-hooks.sh / cross-build.sh
```

设计要点：

- **AST 与 PPTD 规范一一对应**，并保留 `Option` 语义来表达「未设置 → 沿继承链回退」。
- `Element` / `ChartSeries` 使用 serde 的内部标签枚举（`#[serde(tag = ...)]` + newtype 变体）：判别字段（`elementType` / `type`）与元素内容合并进同一张 map，纯 derive 即得到规范的 YAML 形状。公共字段由 `ElementCommon` 承载，经 `#[serde(flatten)]` 摊平进各元素。
- **规范之上的可选扩展**保持向后兼容：布局层（`Presentation.layouts` / `Page.layout`）、组合重建（`groupId` / `Page.groups`）等只影响往返保真，纯 PPTD v2 项目不受影响（见 `docs/` 各扩展文档）。
- 校验器产出 `Diagnostic` 列表而非直接报错，`check` 一次遍历列出全部问题。

## 快速开始

```bash
cargo run -- check tests/fixtures/demo/demo.pptd
cargo run -- dump tests/fixtures/demo/demo.pptd
cargo run -- build tests/fixtures/buildable/buildable.pptd --output out.pptx
# 反向解析：把任何 PPTX 转回 PPTD 项目目录（含 media/ 与 pages/）
cargo run -- convert 公司模板.pptx ./converted && cargo run -- build ./converted/deck.pptd --output roundtrip.pptx
```

`convert` 提取 layer（母版→布局→页面自底向上烘焙）、文本框/形状/连线/图片/图标、主题色与尺寸，并把自定义几何（custGeom）的 guide 公式求值后写成 SVG path；母版/版式写入 `Presentation.layouts`（布局扩展），`<p:grpSp>` 组合以组合扩展重建而非拍平。**不支持的元素不会静默丢弃**：图表、表格等会在报告里逐项列出（页号 + 元素名 + 原因），转换照常成功。

**`.sync.hash` 同步记录与覆写守卫**：项目目录只有一个旁车文件 `.sync.hash`，每两行一条记录（hash + 路径），记的是“本项目上次合法写入某文件时它的 hash”——`convert` 记它转换的源，`build` 记它写出的产物，**写入即记账**，无需旗标。之后对同一文件再 `convert`，若实时哈希未变则跳过（“unchanged → skipped convert”）。`build` 对已存在的输出文件强制验明正身：实时计算其哈希与记录比对，一致才允许覆盖；被外部改过、或无记录无法担保，一律拒绝构建（退出码 2）并提示先 `convert` 吸收——**build 绝不静默覆盖它无法担保的文件**，这是「就地编辑已有 PPTX」流程的安全网。

## 已实现能力（writer）

- text / shape / line / icon / image + video / audio 嵌入媒体，页背景、主题色、渐变填充、默认 fade 切换
- 布局扩展：每个 `Presentation.layouts` key 产出真实 `slideLayoutN.xml`（背景、装饰元素、placeholder 几何与样式继承）
- 组合扩展：`groupId` / `groupBounds` / `Page.groups` → 真实 `<p:grpSp>` 重建（含嵌套）
- `elementType: shape: custom`（SVG path + viewBox ↔ custGeom）
- 富文本 `content.text`：`<p style="...">` / `<span style="...">` / `<strong>` / `<em>` → 逐段落/逐 run 样式；段落级 `text-align`、`line-height`（倍率或 px）、`margin-top`
- 文本框级 `wrap` / `align` / `autofit`（FitShape/FitText）/ 四边 margin
- shape 阴影（`a:outerShdw` / `a:innerShdw`，含 `scale` 与 `inner` 扩展）
- 页/版式背景的图片填充（`Fill::Image` → `a:blipFill`）；形状级图片填充仍报 not-supported
- 图片 `contain` / `cover` 裁剪：按嗅探的 PNG/JPEG 尺寸计算 `a:srcRect`
- table / chart 仍报明确的 not-supported 错误（见路线图）

## 路线图

- [x] PPTD 类型化 AST（主题、9 类元素、动画、表格、图表核心）
- [x] 项目解析器（主入口 + 页面、version 校验）
- [x] 语义校验器
- [x] OOXML writer：theme.xml / slides / drawingml（详见 `src/pptx/writer.rs`）
    - [x] OPC 骨架：Content_Types / rels / core 属性 / presentation.xml
    - [x] theme1.xml（clrScheme 槽位映射，见 `docs/pptx-layout-synthesis.md`）
    - [x] 合成 slideMaster + layout（结构合规，样式显式落盘）
    - [x] text / shape / line / icon / image 渲染 + 页背景 + fade 过渡
    - [x] video / audio 嵌入媒体（poster + `a:videoFile` / `a:audioFile`）
    - [x] 布局扩展（真实 slideLayout + placeholder 继承）
    - [x] 组合扩展（`p:grpSp` 重建，含嵌套）
    - [x] custom 几何（custGeom ↔ SVG path）
    - [x] 富文本标签 → run 拆分（`<p>`/`<span>`/`<strong>`/`<em>`）
    - [x] shape 阴影（`a:outerShdw`/`a:innerShdw`）
    - [x] 页/版式背景图片填充（`a:blipFill`）
    - [ ] table / chart 元素、notes / animations
- [x] OPC 打包（`zip` crate），产出可编辑 .pptx
- [x] pptx → pptd 反向解析（`slideforge convert`）：母版/版式烘焙进 layouts、组合重建、custGeom → SVG path、`.sync.hash` 同步记录与覆写守卫
- [x] agent 技能 `skill/slideforge-ppt/`（五平台预编译二进制 + 设计系统参考）与 `v*` tag 触发的多平台 release workflow
- [ ] 图表 → 原生 OOXML chart parts、动画 → `p:timing`、备注 → notesSlide

## 文档

| 文档 | 内容 |
|---|---|
| `docs/pptd-spec.md` | PPTD v2 规范（语言本身） |
| `docs/pptd-writing-guide.md` | PPTD 手写指南 |
| `docs/pptd-layout-extension.md` | 布局扩展（master/layout 层与 placeholder 继承） |
| `docs/pptd-group-extension.md` | 组合扩展（`p:grpSp` 重建） |
| `docs/pptd-roundtrip-extension.md` | 往返保真扩展字段（`sysDot`、阴影 `scale`/`inner` 等） |
| `docs/pptx-layout-synthesis.md` | PPTX 版式合成设计（master/layout 骨架策略） |
| `skill/slideforge-ppt/SKILL.md` | agent 技能：从零生成 / 就地改已有 PPTX / 参考模板换装 |

## 开发约定

CI（`.github/workflows/ci.yml`）会跑 `cargo fmt --all -- --check` / clippy / test。为了在本地提前发现格式漂移，仓库随附了一个 tracked 的 git hook：

```bash
./scripts/install-hooks.sh   # 一次性：把 core.hooksPath 指向 .githooks/
```

装好后 `git commit` 会先跑 `cargo fmt --all -- --check`，失败时拒绝提交并提示运行 `cargo fmt --all` 后重新 `git add`。CI 与本地钩子使用同一命令，避免「CI 红 / 本地绿」。

推送 `v*` tag 时，`.github/workflows/release.yml` 会交叉构建五平台二进制（macOS/Linux 各 x86_64+arm64、Windows x86_64，见 `scripts/cross-build.sh`）并发布到 GitHub Release。

## 许可证

MIT
