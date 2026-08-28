---
name: slideforge-ppt
description: Create or edit PowerPoint (.pptx) presentations fully locally and offline via the SlideForge CLI. PPTD (YAML) is the editable layer; PPTX<->PPTD convert bidirectionally, so you can also edit an existing .pptx in place. Use when the user wants to make, modify, restyle, or iterate on a deck without any browser/cloud/login. Three flows - generate from a topic, edit an existing .pptx in place, or restyle using a reference .pptx.
license: MIT
compatibility: Requires the `slideforge` Rust binary (built via `cargo build --release` in this project) or `slideforge` on PATH. Works offline. macOS/Windows/Linux.
metadata:
  project: slideforge
---

# slideforge-ppt — 本地 PPT 技能

完全本地、离线地创建或修改 PowerPoint(.pptx)。以 SlideForge CLI 为引擎：
**PPTD（YAML）是可编辑中间层，PPTX↔PPTD 双向可转**。不依赖浏览器、网络、登录。

> 与 Kimi `open-kimi-ppt` 的根本差异：Kimi 只能"从零生成 + 浏览器导出"；
> 本技能因 `convert` 能把任意 PPTX 反编译成 PPTD，**"改已有 PPTX"是一等公民**。

---

## 0. 心智模型（先建立这个）

- **PPTD 项目目录 = 源码**：`deck.pptd`（主入口）+ `pages/*.page`（每页一文件）+ `media/`。
- **PPTX = build 产物**（交付物）。不要手编 PPTX。
- **全程在 PPTD 层编辑**；PPTX 只在两处出现：①最终 `build` 交付；②用户给了现成
  .pptx 时 `convert` 进来当编辑起点。
- **会话内 PPTD 目录持久化**：改一页 → `build` 一次 → 用户反馈 → 再改 → 再 build。
  小迭代**不重新 convert**。这是本地闭环最快的点。

---

## 1. Bootstrap：定位 `slideforge` 二进制（每个会话跑一次）

技能假定在本项目（slideforge）内使用，cwd = 项目根。优先用已编译的 release 二进制：

```bash
# 若 target/release/slideforge 不存在就先构建（一次性）
[ -x target/release/slideforge ] || cargo build --release
SF=target/release/slideforge            # 后文命令以 $SF 指代
"$SF" --version                          # 确认可用
```

> 若用户把 `slideforge` 装到了 PATH，直接 `SF=slideforge`。命令示例统一写 `"$SF"`。

---

## 2. 入口分流 —— 与 Kimi 的关键差异

### 2.0 一级分流：A / B / C

| 模式 | 触发输入 | 起点 | 终点 |
|---|---|---|---|
| **A 生成** | 只给主题/大纲/需求 | 空白，写全新 PPTD | build 出新 PPTX |
| **B 改** | 给了 .pptx + 改动需求 | `convert` 该 PPTX → 改 PPTD | build 出新 PPTX |
| **C 借风格** | 给 .pptx 当风格参考 + 新主题 | `convert` 取主题/版式作参考 → 写新页 | build 出新 PPTX |

### 2.1 B vs C：判断改动幅度

- **小改**（改文字 / 调配色 / 换图 / 挪框 / 加减个别元素，**版式与大部分内容不变**）
  → 走 **B**。`convert` 往返对文本/形状/图片/图标/主题色的小改保真度高。
- **大改 / 重设计**（>50% 内容重写、换版式、换风格）→ 走 **C 借风格**（或干脆 A 生成）。
  因为 `convert` 有保真缺口（见 §4），大改会被拖累；不如只取它的主题色/字号/版式作参考，
  重新生成干净 PPTD。
- 模糊就问一句："在这份原稿上小改，还是借它的风格重做？"

### 2.2 B 子模式识别：B1 纯AI改 / B2 用户也改PPTX ⭐

B 内部还分两种，**区别只在"谁动 PPTX"**：

| 子模式 | 谁改 PPTX | PPTD 的角色 | 协议 |
|---|---|---|---|
| **B1 纯AI改** | 没人手改；PPTX 只是 build 产物 | AI 唯一编辑层 | 改PPTD→check→build→交付 |
| **B2 用户也改PPTX** | 用户在 PowerPoint 里手改 | AI 与用户轮替，PPTX 为同步点 | ping-pong resync（§2.3） |

**识别信号**（按下表判，拿不准就问一句）：

| 信号 | 判定 |
|---|---|
| 用户交回一份**改过的 .pptx**（路径或 mtime 与上次 build 不同） | B2 → 先 `convert` 那份做新 baseline |
| 用户说"我打开改 / 我自己调 / 在PowerPoint里加 / 手动改" | B2 → 走版本化文件名协议 |
| 用户只描述改动（"把…改成 / 加上 / 换掉"）且没碰 PPTX | B1 |
| 模糊（"帮我改下，我也会看看"） | 问一句："这改动你自己用 PowerPoint 改（改完把文件给我），还是我改 PPTD？" |

> B1/B2 可中途切换：用户本来让 AI 改，中途说"这个我自己来"即切 B2；反之亦然。
> **B2 不是预先选的，是被"用户交回改过的 PPTX"这个事件触发的**——默认走 B1，用户一交回文件就自动切 B2 协议。

### 2.3 B2 协议：ping-pong resync（用户也改 PPTX 时必读）

核心：**全程就一个 .pptx 文件（用户那份），AI 与用户轮替编辑，绝不并发。**
AI 的 build 直接覆盖该文件；靠轮替 + 用户存盘交回来保证不互踩；**不生成副本**。
每次“用户改完交回”，AI 先用 hash 判断要不要 re-convert（见 §2.4），再叠 AI 这轮改动，再 build 覆盖。

```
初始：convert <用户的>.pptx → pptd/（baseline）；写 .src.hash = sha256(该pptx)
回合：AI 改 pptd → check → build → 覆盖同一 .pptx → 更新 .src.hash → 交用户“改好了”
      用户在 PowerPoint 改同一文件 → 存盘 → 告诉AI“改完了”
      AI: cur=sha256(该pptx)；与 .src.hash 比（§2.4）
          相同 → 用户没动，跳过 convert，pptd 即当前
          不同 → convert 该pptx → pptd/（吸收用户改动）→ dump 看结构
          → 在新 baseline 上做 AI 这轮改动 → check → build → 覆盖同一 .pptx → 更新 .src.hash
      …
```

**铁律**：
1. **就一个文件，AI 直接覆盖**：build 输出 = 用户编辑的那份。不生成副本、不留 `_user`/`_v{n}`。
2. **轮替不并发**：用户存盘并说“改完了”之后 AI 才动；AI build 完才交回用户。绝不同时编辑。
3. **交给用户前先 build（flush）**：把 PPTD 改动落进 PPTX 再交，确保用户编辑基线含 AI 全部改动 → re-convert 时 AI 改动不丢。
4. **re-convert 用 hash 判断**（§2.4）：用户没改（hash 同）就跳过，省一次 convert + 省一次保真缺口。
5. **同元素冲突默认用户赢**：AI 这轮要改的元素若用户上轮已动过（re-convert 后值与预期不符），先问用户以谁为准，别盲改。
6. **有 skip 时覆盖前提醒**：若上次 convert 报告了 skip（table/chart），覆盖会从用户文件里丢掉这些 → 覆盖前明确告诉用户“这会丢失 X，自行决定是否先备份”。

> re-convert 的 convert 税：仅在用户真改了时付一次（hash 不同才 convert），文本/形状/图片高保真；
> table/chart/space-run 的已知缺口见 §4。用户在 PPTX 里加了表格/图表 → re-convert 会 skip → AI 在 PPTD 用 shape+text 补，或该页转 C 重生成。

### 2.4 hash 跳过冗余 convert（B1/B2 通用）

每次要 `convert` 一个 .pptx 前，先比 hash：若与上次转换该 pptx 时记的 hash 一致，
说明 PPTD 已与该 pptx 同步，**跳过 convert**，直接改 PPTD。hash 存工作目录的
`.src.hash` 旁车文件。

```bash
# 跨平台 sha256（macOS=shasum，Linux=sha256sum）
sha256of() { { command -v sha256sum >/dev/null 2>&1 && sha256sum "$1" || shasum -a 256 "$1"; } | awk '{print $1}'; }

# convert 之后（建立 sync point）写：
sha256of <用户的>.pptx > <work_dir>/.src.hash

# 下次要 convert 前先比：
cur=$(sha256of <用户的>.pptx); old=$(cat <work_dir>/.src.hash 2>/dev/null)
[ "$cur" = "$old" ] && echo "unchanged → skip convert" || "$SF" convert <用户的>.pptx <work_dir>
```

- `.src.hash` 记的是“上次 pptx↔pptd 同步点的 pptx 哈希”。**sync point = 每次 convert 后、或每次 build（覆盖 pptx）后**——两者都让 pptx 与 pptd 一致，都要更新 .src.hash。
- 所以 AI 回合开头只需比一次 hash：相同 → pptd 已最新，跳 convert；不同 → 用户改过，re-convert 吸收。
- `.src.hash` 持久化在 work_dir，跨会话也有效（用户隔天回来编辑同一 pptx，hash 同 → 直接用旧 pptd）。

---

---

## 3. 标准循环（三模式汇合于此）

```
编辑 PPTD (写/改 .page / deck.pptd)
   ↓
"$SF" check <deck.pptd>          # 必跑：一次列出全部 Diagnostic
   ↓ 有 issue → 修 PPTD → 重 check
"$SF" build <deck.pptd> --output <out>.pptx
   ↓ build 内部先校验，不过则拒绝写文件（退出码 2）
(可选) qlmanage -t -s 1024 <out>.pptx -o /tmp  # Quick Look 看第 1 页
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
- 空文本框的空段落会被清掉（正常，形状本身保留）。
- custGeom→SVG path、主题色、文本/形状/图片/图标保真度高。

→ 结论重申：**B 模式只对小改高保真；大改走 C/A。**

---

## 5. 视觉 QA 的真相（诚实设预期）

- **Quick Look 只渲染第 1 页缩略图**：`qlmanage -t -s 1024 x.pptx -o /tmp` → 只能看封面。
  只适合单页肉眼核对。
- **多页视觉检查**：让用户在 PowerPoint / Keynote / WPS 打开产物。本技能无 LibreOffice，
  不能批量渲染所有页。
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

本技能**自带 PPTD 文档副本**（都在 `skill/slideforge-ppt/references/`，与仓库 `docs/` 同步；仓库根相对，可直接 `read`）。写作时再读（progressive disclosure）：

- **完整 PPTD 写作指南（字段/样式优先级/富文本/陷阱）**：`skill/slideforge-ppt/references/pptd-writing-guide.md`
  ← 最重要，写 .page 前必读其 §2 支持矩阵 + §7 元素速查 + §11 do/don't。
- **三模式分步剧本（含 B1/B2 子模式）**：`skill/slideforge-ppt/references/modes.md`
  ← 进入某模式时加载对应小节；B 模式按 B1/B2 分支。
- **layouts 扩展**（`Presentation.layouts`/`Page.layout`/`Text.placeholder`，本仓库对 Kimi PPTD 的扩展）：`skill/slideforge-ppt/references/pptd-layout-extension.md`
- **group 扩展**（`groupId`/`groupBounds`/`Page.groups`，本仓库扩展）：`skill/slideforge-ppt/references/pptd-group-extension.md`
- **round-trip 扩展字段**（`Text.fill`/`Text.border`、`bulletChar`/`listMargin`/`listIndent`、`marginLeft`/`marginRight`/`marginBottom`、`autofit`、`softEdge`、`Shadow.inner`/`scale` 等；保 OOXML 往返保真）：`skill/slideforge-ppt/references/pptd-roundtrip-extension.md`
- **PPTD 规范语义**（权威，有出入以此为准）：`skill/slideforge-ppt/references/pptd-spec.md`
- **可运行样例**：`docs/samples/min/`（仓库内；AI 的 CWD 即仓库根，可直接 `check`/`build` 验证）
- 形状库(177 个 shapeName)/字体库：Moonshot `reference/`（若用户已集成；spec 内 `shapes.md`/`fonts.md` 为悬空链接，查 Moonshot reference 为主）。

> **为何嵌入**：本仓库的 PPTD 是在 Kimi PPTD v2 之上做了扩展（layouts/group/round-trip），AI 写 PPTD 必须照**本仓库的扩展版**规范来写，所以把文档随技能一起带上，脱离 `docs/` 也能正确写作。`docs/` 改动后需重同步 `skill/slideforge-ppt/references/pptd-*.md`（见各文件顶部同步注释）。

---

## 9. 交付

最终 `build` 成功 → 把 **.pptx 绝对路径**给用户，并附一旬结构摘要（N 页、每页主要元素）。
**保留 PPTD 目录**，告诉用户"想再改就继续说，我在原 PPTD 上改完重新 build"。
明确点出已知限制（如某页用了 table 但已手拼替代；某处 inter-run 空格可能丢失）。

---

## 10. 失败兜底

- `check` 报 issue → 逐条读 stderr，按 §8 写作指南修 PPTD，重 check。
- `build` 报 `not supported yet` → 该页有 table/chart，按 §4 用 shape+text 替换。
- `convert` 列出 skip → 若是用户在意的内容，转 C 模式重生成该页；否则忽略并在交付说明里标注。
- 用户抱怨"改完某个词粘在一起了" → §4 的纯空白 run 空格丢失；该处手动在 PPTD 里补空格再 build。
