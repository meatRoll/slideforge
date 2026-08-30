---
name: slideforge-ppt
description: Create or edit PowerPoint (.pptx) presentations fully locally and offline via the SlideForge CLI. PPTD (YAML) is the editable layer; PPTX<->PPTD convert bidirectionally, so you can also edit an existing .pptx in place. Use when the user wants to make, modify, restyle, or iterate on a deck without any browser/cloud/login. Three flows - generate from a topic, edit an existing .pptx in place, or restyle using a reference .pptx.
license: MIT
compatibility: Bundles prebuilt `slideforge` binaries (macOS/Linux x86_64+arm64, Windows x86_64) in `bin/` with a per-host dispatcher; falls back to a local `cargo build --release` or PATH on other hosts. Works offline. macOS/Windows/Linux.
metadata:
  project: slideforge
---

# slideforge-ppt — 本地 PPT 技能

完全本地、离线地创建或修改 PowerPoint(.pptx)。以 SlideForge CLI 为引擎：
**PPTD（YAML）是可编辑中间层，PPTX↔PPTD 双向可转**。不依赖浏览器、网络、登录。

> 因 `convert` 能反编译任意 PPTX 成 PPTD，**"改已有 PPTX"是一等公民**——不止"从零生成"，也能就地改已有文件。

---

## 0. 心智模型（先建立这个）

- **PPTD 项目目录 = 源码**：`deck.pptd`（主入口）+ `pages/*.page`（每页一文件）+ `media/`。
- **PPTX = build 产物**（交付物）。不要手编 PPTX。
- **全程在 PPTD 层编辑**；PPTX 只在两处出现：①最终 `build` 交付；②用户给了现成
  .pptx 时 `convert` 进来当编辑起点。
- **会话内 PPTD 目录持久化**：改一页 → `build` 一次 → 用户反馈 → 再改 → 再 build。
  小迭代**不重新 convert**。这是本地闭环最快的点。

---

## 1. Bootstrap：定位 `slideforge` 派发器（每个会话跑一次）

技能自带五平台预编译二进制（`bin/` 下，按三元组命名：macOS/Linux 各 x86_64+arm64、Windows x86_64），由多平台派发器按宿主自动选；无预编译时回退到 PATH。重建全部平台用 `scripts/cross-build.sh --install`（Linux/Windows 走 cargo-zigbuild + zig，macOS 原生 `cargo build --target`）。

**本技能所有相对路径（`bin/`、`references/`）都相对技能目录**（即本 SKILL.md 所在目录；pi 在系统提示的 `<location>` 给了它的绝对路径）。据此把 `bin/slideforge` 解析成绝对路径再赋给 `$SF`：

```bash
SF=<技能目录>/bin/slideforge     # <技能目录> = 本 SKILL.md 的 dirname（见系统提示 <location>）
"$SF" --version                   # 确认可用（应输出 slideforge 0.1.0）
```

> 派发器找不到合适二进制会退出码 127 并打印修复提示：把对应平台二进制命名为 `slideforge-<triple>` 放进 `bin/`，或把 `slideforge` 装到 PATH（派发器会兜到）。命令示例统一写 `"$SF"`。

---

## 2. 入口分流

### 2.0 一级分流：A / B / C

| 模式 | 触发输入 | 起点 | 终点 |
|---|---|---|---|
| **A 生成** | 只给主题/大纲/需求 | 空白，写全新 PPTD | build 出新 PPTX |
| **B 改** | 给了 .pptx + 改动需求 | `convert` 该 PPTX → 改 PPTD | 就地覆盖该 PPTX |
| **C 借风格** | 给 .pptx 当风格参考 + 新主题 | `convert` 取主题/版式作参考 → 写新页 | build 出新 PPTX |

### 2.1 B vs C：判断改动幅度

- **小改**（改文字 / 调配色 / 换图 / 挪框 / 加减个别元素，**版式与大部分内容不变**）
  → 走 **B**。`convert` 往返对文本/形状/图片/图标/主题色的小改保真度高。
- **大改 / 重设计**（>50% 内容重写、换版式、换风格）→ 走 **C 借风格**（或干脆 A 生成）。
  因为 `convert` 有保真缺口（见 §4），大改会被拖累；不如只取它的主题色/字号/版式作参考，
  重新生成干净 PPTD。
- 模糊就问一句："在这份原稿上小改，还是借它的风格重做？"

### 2.2 B 协议：动 PPTD 前先验 hash，hash 由代码判 ⭐

hash 同步已内建在 CLI（§2.3），B 不再区分“AI 独改 / 用户也改”，也不需要预先问谁改：
**唯一红线 = 动 PPTD 之前先验 hash（铁律 2）；一致直接改，不一致先 convert。**

```
初始：convert <用户的>.pptx → pptd/（baseline；工具自动写 .src.hash）
每轮：
  ① 验 hash（红线，见铁律 2）：比对源 pptx 与 <work_dir>/.src.hash
      IN_SYNC → 没人动过文件，pptd 即当前，直接进 ③（不必跑 convert）
      STALE / 无 .src.hash → convert <用户的>.pptx pptd/ 吸收外部改动、刷新 baseline
  ② dump 看结构（STALE 转换后才需要；留意 stderr skip）
  ③ 叠 AI 这轮的 PPTD 改动
  ④ check → build --output <用户的>.pptx   # 就地覆盖；hash 自动同步（auto-sync）
      # 若这步被拒（"changed after the last sync point"）：说明漏了 ① 或文件在同步点后
      # 又被改过 → 先 convert 吸收，再重做 PPTD 改动、重 build。
  ⑤ 交用户
```

**顺序就是安全**：convert 永远在 AI 改动之前（①先于③），用户改动先吸收、AI 改动后叠加，
谁改过、改了什么都不需要 LLM 关心，代码保证不丢。

**铁律**：
1. **就一个文件**：build 输出 = 用户给的那份。不生成副本、不留 `_user`/`_v{n}`。
   例外：用户明确要求"别动我的原稿"时，`build --output` 到别处（此时产物不是
   同步点，不影响原文件的同步状态）。
2. **红线：动 PPTD 前先验 hash**：对 deck.pptd / pages/*.page 做任何 write/edit
   之前，先校验源 pptx 与 `work_dir/.src.hash`（第 1 行 hash、第 2 行源路径）：
   ```bash
   d=<work_dir>; if [ ! -f "$d/.src.hash" ]; then echo STALE; else
     s=$(sed -n 2p "$d/.src.hash"); [ -f "$s" ] || s="$d/$s"
     [ "$(shasum -a 256 "$s" | cut -d' ' -f1)" = "$(sed -n 1p "$d/.src.hash")" ] \
       && echo IN_SYNC || echo STALE; fi
   ```
   - IN_SYNC → pptd 即当前，直接改；STALE（含 .src.hash 缺失或旧版无路径行）
     → 先 convert 再改（convert 会顺带把旧格式 sidecar 刷新成带路径的新格式）。
   - 这是每轮编辑的前置动作，不是可选步骤：忘了它，build 覆盖源文件时会被
     "changed after the last sync point" 拒绝（退出码 2），整轮编辑白做。
   - 这个检查很便宜（一条命令），比“每轮先 convert”更容易被遵守；检查结果
     自然指向下一步要不要 convert。
3. **真转换了才 dump**：unchanged 时结构没变，不浪费；真转换了则读 stderr skip：
   用户在 PPTX 里加了 table/chart 会 skip → AI 用 shape+text 补，或该页转 C 重生成。
4. **同元素冲突默认用户赢**：re-convert 后值与 AI 预期不符（说明用户动过），先问用户以谁为准。
5. **首次覆盖前看 skip**：上次 convert 若报告过 skip（table/chart），覆盖会把丢这些写进
   用户文件 → 覆盖前明确告诉用户"这会丢失 X，自行决定是否先备份"。

### 2.3 hash 同步：convert/build 内置，LLM 无需手管

**hash 校验已内建在 CLI 里**，不再是 shell 手工活：`convert` 自己比 hash、自己记
hash，LLM 想忘也忘不掉。同步点哈希存工作目录的 `.src.hash` 旁车文件。

```bash
# ① 转换前无需任何比对——工具自己判：
"$SF" convert <用户给的>.pptx <work_dir>
#   → 源文件没变时自动跳过：
#     "unchanged → skipped convert (...)"
#     "  hint: the existing PPTD reflects this exact .pptx; edit it directly. ..."
#   → 转换成功时自动记新同步点：
#     "  sync point recorded: .src.hash = <sha256>"

# ② 就地覆盖时 hash 自动记——无需任何旗标。build 发现输出覆盖了 convert 的源文件，
#    自动写新同步点：
#     "  auto-sync: output overwrites the convert source; sync point recorded: ..."
#    若源文件在同步点后被外部改过（hash 不符），build 会**拒绝**（退出码 2）并提示
#    先 convert——这防止外部改动被静默覆盖。convert 无重转旗标；要重置就删掉
#    work_dir（连带 .src.hash）重新 convert。
```

- **读到 `unchanged → skipped convert` 就直接改 PPTD**：说明 pptd 已与该 pptx 完全同步，
  不要覆盖 work_dir（跳过的 convert 没有写入任何文件）。
- `.src.hash` 记的是"上次 pptx↔pptd 同步点的 pptx 哈希 + 该文件路径"。sync point =
  每次 convert 成功后（自动写）；build 输出覆盖该源文件时也自动写（auto-sync）。
  同步点按**文件**计，不按内容计：另一个文件哪怕内容相同也不会被误判 unchanged。
- `.src.hash` 持久化在 work_dir，跨会话有效（用户隔天回来编辑同一 pptx，
  hash 同 → 自动跳过冗余 convert）。
- 文件被外部改过（哈希变）→ convert 正常执行吸收改动；skip（table/chart）处理方式
  见 §2.2 铁律 3/5 与 §4。

---

### 2.4 需求四轴分析 + 内容纪律（A/C 生成前必做）

业界通用 PPT 方法论，后端无关。动笔前锁四轴，写作守纪律。

**四轴**（任一不明就问用户，用 ask/clarification）：
1. **目的**：A 生成 / B 改 / C 借风格（见 §2.0）。
2. **设计方向**：① 自由设计（AI 自定，**不得**自选 `design_system/` 预设）② 设计系统（用户显式指定 `references/design/design_system/` 某主题，读其 `design.md`）③ 用模板（用户给 pptx → 先 `convert` 看风格再沿用）④ 风格迁移（参考图/网页，提取配色字体排版复用）。
3. **输入类型**：仅话题 / 完整文档 / 逐页大纲。后两者默认可搜素材扩写（除非用户明说不扩）。
4. **页数**：用户指定优先；大纲→匹配其页数；完整文档→问"一页覆盖多少"+估总页；仅话题→建议并确认。

**内容纪律**（详见 `references/design/slides_categories.md` 的 General rules / Strictly forbidden）：每页有明确读者任务、分页有节奏；禁卡片堆层级/等分网格/彩虹 AI 配色/编造数据（缺则标占位/待补）；外部数据须标来源+超链接。反套话：除非用户明要，避免"不是X而是Y""N大路径/战场""闭环/抓手/赋能"等比喻口号与抽象黑话。

**图片工作流**：批量搜/生成/下载完再排版（存 `media/`，不拉伸）；优先级 用户给的 > 官方源 > 相关搜索图 > 概念生成图；产品/人物/地点/界面/案例页优先实图；不为凑数加无关图。

---

## 3. 标准循环（A/B/C 汇合于此；B 的 ④ 之后从这里接）

```
编辑 PPTD (写/改 .page / deck.pptd)
   ↓
"$SF" check <deck.pptd>          # 必跑：一次列出全部 Diagnostic
   ↓ 有 issue → 修 PPTD → 重 check
"$SF" build <deck.pptd> --output <out>.pptx
   ↓ build 内部先校验，不过则拒绝写文件（退出码 2）
(可选) 本机可用的预览工具看一眼首屏
   ↓
给用户 .pptx 路径 + 结构摘要 → 收集反馈 → 回到"编辑 PPTD"
```

**铁律**：永远先 `check` 再 `build`。`build` 校验不过会拒绝写文件；渲染阶段遇
不支持的元素会 `not supported yet` 中止（退出码 1）。

---

## 4. 能力边界（决定可行性，动笔前必读）

**`build` 能出 PPTX 的**：`text` / `shape`(含 custom SVG path) / `line` / `image` /
`icon`(Font Awesome 7.x) + 页背景(solid/gradient/image) + 富文本 + 主题 colors/textStyles。

**`build` 不支持（写了就中止）**：`table` / `chart` / `animations` / `notes`。
→ 要表格/图表：用 `shape`+`text`+`line` 手工拼。要动画/备注：先说明"当前 build 不写"，
仍可把数据留在 PPTD 里（check 能过）但产物里没有。

**`convert` 的已知保真缺口**（影响 B 模式保真度，要诚实告知）：
- `table` / `chart` 会逐项 `skipped`（不静默，stderr 报告页号+名+原因）。
- **纯空白 run 的空格会丢**（xmltree 解析器丢弃纯空白 `<a:t>` 文本节点）：
  不同样式的相邻 run 之间若靠空格 run 分隔，可能粘在一起。这是已定位的解析器层限制。
- 空文本框的空段落会被清掉（正常，形状本身保留）；**带显式行距的空段落保留**
  （模板常用小行距空段做节间距，行距丢失会把后续文字顶出文本框——已在 build 侧修复）。
- custGeom→SVG path、主题色、文本/形状/图片/图标保真度高。

**PowerPoint ≠ 本地预览器渲染**（本地验证的盲区，务必注意）：
- 本机可用的渲染预览（自行探测有什么可用）能渲染≠PowerPoint 能渲染。
  实战案例：line 的 custGeom path 坐标超 int32（`w=91249817500`）时预览器归一化后
  渲染正常，PowerPoint 却把直线画出幻灯片边界。若用户报“PowerPoint 里不对但预览正常”，
  先查产物 XML 的数值边界（所有 `x/y/cx/cy/path w/h` 应 < 2^31）。
- 预览正常但用户仍说不对时，不要只靠截图对比，**直接 unzip 产物 diff 原稿 XML**（元素级
  token diff 最快定位：prstGeom↔custGeom、prstDash 值、坐标数值、schemeClr↔srgbClr 转写）。

→ 结论重申：**B 模式只对小改高保真；大改走 C/A。**

---

## 5. 视觉 QA 的真相（诚实设预期）

- **预览工具取决于环境**：自行探测本机可用的 PPTX 渲染手段并使用（注意各工具的局限，
  如有的只能出首页缩略图）；都没有就直接让用户在 PowerPoint / Keynote / WPS 打开。
  预览结果仅供参考（见上节盲区）。
- **多页视觉检查**：让用户在 PowerPoint / Keynote / WPS 打开产物。
- **结构检查**：`"$SF" dump <deck.pptd>` 看每页元素数与类型分布，与预期对照。
- **（可选）往返保真自检**：B 模式 build 后，可再 `convert` 产物回 PPTD，对比元素数
  有无掉失。不作为默认步骤。

---

## 6. 命令速查

```bash
"$SF" convert <in.pptx>  <out_dir>           # PPTX→PPTD；写 out_dir/deck.pptd+pages/+media/
"$SF" check    <deck.pptd>                    # 解析+语义校验；退出码 0/2/1
"$SF" build    <deck.pptd> --output <out>.pptx # 校验通过后打包 PPTX（OPC+ZIP）
"$SF" dump     <deck.pptd>                    # 打印 AST 摘要（页数/每页元素统计）
```

退出码：`check` 0=过 / 2=有 issue(stderr 逐条) / 1=加载失败；`build` 同，外加渲染失败=1。

---

## 7. 工作目录约定

- 新建 deck：在 cwd 下建 `<name>/`（如 `mydeck/`），内含 `deck.pptd` + `pages/` + `media/`。
  **全程自包含**：所有被引文件（图等）必须在该目录内；媒体支持本地相对路径或 https URL。
- B 模式：`convert` 会自己建好 `<out_dir>/` 结构，直接在其内编辑。
- 一个会话内反复迭代就复用这个目录，别每轮新建。

---

## 8. 作者细节（按需加载，别全塞进上下文）

本技能自带 **PPTD 文档副本 + 设计辅助**（都在 `references/` 下：`pptd/` = 格式规范与速查、`design/` = 设计方法论与主题、`modes.md` = 工作流；与仓库 `docs/` 同步；技能目录相对，pi 按本技能 `<location>` 解析）。写作时再读（progressive disclosure）：

- **完整 PPTD 写作指南（字段/样式优先级/富文本/陷阱）**：`references/pptd/pptd-writing-guide.md`
  ← 最重要，写 .page 前必读其 §2 支持矩阵 + §7 元素速查 + §11 do/don't。
- **三模式分步剧本**：`references/modes.md`
  ← 进入某模式时加载对应小节。
- **layouts 扩展**（`Presentation.layouts`/`Page.layout`/`Text.placeholder`，本仓库对 PPTD 的扩展）：`references/pptd/pptd-layout-extension.md`
- **group 扩展**（`groupId`/`groupBounds`/`Page.groups`，本仓库扩展）：`references/pptd/pptd-group-extension.md`
- **round-trip 扩展字段**（`Text.fill`/`Text.border`、`bulletChar`/`listMargin`/`listIndent`、`marginLeft`/`marginRight`/`marginBottom`、`autofit`、`softEdge`、`Shadow.inner`/`scale` 等；保 OOXML 往返保真）：`references/pptd/pptd-roundtrip-extension.md`
- **PPTD 规范语义**（权威，有出入以此为准）：`references/pptd/pptd-spec.md`
- **PPT 设计方法论（后端无关，A/C 生成前读）**：`references/design/slides_categories.md`（+ `design/slides_categories/` 下 7 个场景文档：分析决策/商业提案/管理汇报/学术/教育/技术工程/品牌创意）。每页明确读者任务、分页节奏、禁卡片堆层级/等分网格/彩虹配色/编造数据、场景→对应风格文档。
- **海报/信息图设计**（仅海报/单页视觉任务读）：`references/design/general-poster.md`
- **主题预设库**（无主题参考时，from-scratch 生成可挑一个；用户显式指定才用）：`references/design/design_system/`（按 `design/design_system/<类别>/<主题>/design.md` 组织，读所选主题的 design.md 作为该 deck 设计方向；来源见该目录 README）
- **可运行样例**：若本机有 slideforge 仓库，可 `check`/`build` 其 `docs/samples/min/` 验证；否则用任意 `.page` 自测（`build` 出 pptx 看能否打开）。
- 形状库(177 个 shapeName)/字体库（自包含副本）：`references/pptd/shapes.md`、`references/pptd/fonts.md`（pptd-spec.md 的 `./shapes.md`/`./fonts.md` 链接在 `pptd/` 内可解析；来源见各文件头部注释）。

> **为何嵌入**：本仓库的 PPTD 在标准 PPTD 之上做了扩展（layouts/group/round-trip），AI 写 PPTD 必须照**本仓库的扩展版**规范来写，所以把文档随技能一起带上，脱离 `docs/` 也能正确写作。`docs/` 改动后需重同步 `references/pptd/pptd-*.md`（见各文件顶部同步注释）。

---

## 9. 交付

最终 `build` 成功 → 把 **.pptx 绝对路径**给用户，并附一句结构摘要（N 页、每页主要元素）。
**保留 PPTD 目录**，告诉用户"想再改就继续说，我在原 PPTD 上改完重新 build"。
明确点出已知限制（如某页用了 table 但已手拼替代；某处 inter-run 空格可能丢失）。

---

## 10. 失败兜底

- `check` 报 issue → 逐条读 stderr，按 §8 写作指南修 PPTD，重 check。
- `build` 报 `not supported yet` → 该页有 table/chart，按 §4 用 shape+text 替换。
- `convert` 列出 skip → 若是用户在意的内容，转 C 模式重生成该页；否则忽略并在交付说明里标注。
- 用户抱怨"改完某个词粘在一起了" → §4 的纯空白 run 空格丢失；该处手动在 PPTD 里补空格再 build。
