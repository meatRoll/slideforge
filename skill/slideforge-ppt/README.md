# slideforge-ppt 技能目录说明

本文件只说**目录结构与定位**；技能定义、作者入口剧本、协议规则全在 [`SKILL.md`](./SKILL.md)。

## 目录树

```
slideforge-ppt/
├── SKILL.md            pi 技能定义 + 作者剧本（§0–§10）。AI 的主入口。
├── README.md           本文件：目录结构与定位说明。
└── references/         全部参考材料。按需加载（progressive disclosure），
                         AI 只在用到某主题时 `read` 对应文件，不全塞上下文。
    ├── modes.md        ◄ 工作流剧本：A 生成 / B 改（B1 纯 AI、B2 ping-pong
    │                     resync）/ C 借风格。进入某模式时读对应小节。
    ├── pptd/           ◄ PPTD 格式规范集（7 份，相互 `./` 互链，整集同目录移动以保链接）
    │   ├── pptd-spec.md              PPTD 规范语义（权威，有出入以此为准）
    │   ├── pptd-writing-guide.md     写作指南：字段/样式优先级/富文本/陷阱。
    │   │                              写 `.page` 前必读（§2 支持矩阵 + §7 速查 + §11 do/don't）
    │   ├── pptd-layout-extension.md  layouts 扩展（`Presentation.layouts`/`Page.layout`/
    │   │                              `Text.placeholder`，本仓库对 Kimi PPTD 的扩展）
    │   ├── pptd-group-extension.md   group 扩展（`groupId`/`groupBounds`/`Page.groups`，本仓库扩展）
    │   ├── pptd-roundtrip-extension.md round-trip 扩展字段（`Text.fill`/`Text.border`、
    │   │                              `bulletChar`/`listMargin`/listIndent`、各 margin、
    │   │                              `autofit`/`softEdge`/`Shadow` 等；保 OOXML 往返保真）
    │   ├── shapes.md                形状库（177 个 shapeName）。pptd-spec.md 的 `./shapes.md` 指向此。
    │   └── fonts.md                 字体选择原则。pptd-spec.md 的 `./fonts.md` 指向此。
    └── design/         ◄ 设计方法论 + 主题预设（源自 Kimi open-kimi-ppt，后端无关）
        ├── slides_categories.md      PPT 设计方法论 + 内容纪律（四轴分析、反套话、
        │                              禁卡片堆层级/等分网格/彩虹配色/编造数据）
        ├── slides_categories/        7 个场景文档：
        │   ├── academic-research.md      学术研究
        │   ├── analysis-decision.md     分析决策
        │   ├── brand-creative.md         品牌创意
        │   ├── business-plan.md         商业提案
        │   ├── education-training.md    教育培训
        │   ├── management-report.md      管理汇报
        │   └── tech-engineering.md       技术工程
        ├── general-poster.md          海报/信息图设计指南（仅海报/单页视觉任务读）
        └── design_system/            主题预设库（60 份；用户显式指定才用，见下方双结构说明）
            ├── <类别>/<主题>/design.md    ← 按名结构（30，canonical，推荐）
            └── 0N_<类别>/NN/en/<主题>.md  ← 编号结构（30，Kimi 源遗留，备选）
```

## 三桶职责

| 桶 | 职责 | 何时读 |
|---|---|---|
| `references/pptd/` | PPTD **格式规范**（怎么写 `.page` 才合法） | 写/改任何 PPTD 前 |
| `references/design/` | PPT **设计方法论 + 主题预设**（写得好不好看、配不配色） | A 生成 / C 借风格前；选主题时 |
| `references/modes.md` | **工作流剧本**（这一步该干嘛） | 进入某模式（A/B/C）时 |

## `design_system/` 双结构说明

Kimi 源里就**两套并行**组织，本技能原样保留（`都要`）。两套都是「5 类 × 6 主题 = 30」的设计方向参考，**选其一即可**：

- **按名结构** `<类别>/<主题>/design.md`（canonical，推荐）——路径语义化，如 `design_system/consulting/indigo-due-diligence/design.md`。5 类：`academic` `consulting` `finance` `promotion` `work`。
- **编号结构** `0N_<类别>/NN/en/<主题>.md`（Kimi 源遗留，备选）——如 `design_system/01_strategy/02/en/indigo-due-diligence.md`。5 类：`01_strategy` `02_business` `03_work` `04_promotion` `05_academic`。`en/` 子目录为本地化层。

两类别的主题名**部分重叠、部分不同**（如 `indigo-due-diligence` 两套都有，`apricot-white-brief` 仅按名结构有）。**定位某主题的 design.md 时优先用按名结构**；编号结构作为兜底。两套读法相同：取所选主题的 `design.md` 作为该 deck 的设计方向（配色/字体/排版基调）。

## 路径约定

- **仓库根相对**：SKILL.md / modes.md / slides_categories.md 里引用 references 文件一律用 `skill/slideforge-ppt/references/...` 全路径，因为 AI 的 `read` 从 CWD（=仓库根）解析。**不要**改成技能目录相对（`references/...`），那会从 AI 的 CWD 解析失败。
- **pptd/ 内互链**用 `./`（同目录），因这 7 份是封闭互链集，整集同目录移动即保链接。
- **外链回仓库根**从 `references/pptd/` 起 4 层：`../../../../README.md`、`../../../../docs/pptx-layout-synthesis.md`。

## 与仓库 `docs/` 的同步

`references/pptd/` 下的 `pptd-*.md` 与仓库 `docs/pptd-*.md` **逐份对应**（顶部有同步注释标明来源）。`docs/` 改动后需重同步 references/ 副本，否则技能里的规范会落后于实现。`shapes.md` / `fonts.md` / `design/` 下文件源自 Kimi，与仓库 `docs/` 无对应关系，不参与同步。

## 技能发现（本地，不入库）

`.pi/settings.json`（被 `.gitignore` 忽略，本地态）登记 `{"skills":["skill"]}` 即可被 pi 自动发现；或在信任项目后 `pi --skill skill/slideforge-ppt`。新克隆仓库需自行创建 `.pi/settings.json`，技能不会自动发现。
