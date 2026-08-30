# 三模式分步剧本

> 按用户输入确定模式（见 SKILL.md §2）后，读对应小节。命令里 `$SF` 指
> `bin/slideforge` 多平台派发器（技能目录相对，见 SKILL.md §1 bootstrap；后文 `$SF` 指代它）。**技能内文件**（references/）相对技能目录；**用户文件**（.pptx、work_dir、mydeck/）相对当前 cwd。

---

## 模式 A：生成（从零创建）

**触发**：用户给主题/大纲/需求，无现成 .pptx。

1. **规划**：先和用户对齐——页数、每页页型（cover/content/final…）、主题色、
   是否要图。**只规划能用 build 的元素**（text/shape/line/image/icon；不要 table/chart，
   见 SKILL.md §4）。要表格/图表就用 shape+text+line 手拼。

2. **建目录**：
   ```bash
   mkdir -p mydeck/pages mydeck/media
   ```

3. **写主入口 `mydeck/deck.pptd`**：version/size/theme/pages 列表。
   - 固定 `version: v2`；16:9 用 `size: [960, 540]`。
   - 主题色/文本样式集中放 `theme`，元素里用 `$key` 引用，少写魔法色值。
   - 样板见 `docs/samples/min/deck.pptd` 与 `tests/fixtures/demo/`。

4. **并行写 `mydeck/pages/*.page`**：每页一文件。一次写多页减少往返。
   - 写前必读 `references/pptd/pptd-writing-guide.md` 的 §7 元素速查 + §11 do/don't。
   - 含 `style="..."` 的富文本一律用块标量 `|`，避免 YAML 误解析。
   - `elementId` 页内唯一；`bounds` 之和不超过画布。

5. **进标准循环**（SKILL.md §3）：`check` → `build` → （可用预览工具看一眼） → 反馈 → 改 → 重 build。

6. **交付**：给 .pptx 绝对路径 + 结构摘要；保留 `mydeck/` 供迭代。

### 快速覆盖页骨架（改 fill/color 即用）

```yaml
pageType: cover
background: {type: solid, color: "$primary"}
elements:
  - elementId: title
    elementType: text
    bounds: [100, 200, 760, 80]
    content:
      style: "$title"
      align: [center, middle]
      text: 在此填标题
```

---

## 模式 B：改（编辑已有 .pptx）

**触发**：用户给了 .pptx + 改动需求（先按 SKILL.md §2.1 判 B vs C；本文假设 B）。
B 内部分 **B1 纯AI改 / B2 用户也改PPTX** 两个子模式，识别见 SKILL.md §2.2。
**默认 B1**；用户一交回改过的 .pptx 就自动转 B2 协议。

共用步骤（convert / dump / 编辑模式 / 交付标注）如下；B2 在此基础上加 resync 循环。

### 共用：convert + 摣结构 + 定向编辑

1. **convert（先比 hash，同则跳过，见 SKILL.md §2.4）**：
   ```bash
   sha256of() { { command -v sha256sum >/dev/null 2>&1 && sha256sum "$1" || shasum -a 256 "$1"; } | awk '{print $1}'; }
   cur=$(sha256of <用户给的>.pptx); old=$(cat <work_dir>/.src.hash 2>/dev/null)
   if [ "$cur" = "$old" ]; then echo "unchanged → skip convert";
   else "$SF" convert <用户给的>.pptx <work_dir> && sha256of <用户给的>.pptx > <work_dir>/.src.hash; fi
   ```
   产出 `<work_dir>/deck.pptd` + `pages/` + `media/`。读 stderr 里的 skip 报告；
   若 skip 了用户在意的内容（table/chart），转模式 C 重生成该页。

2. **摸结构**：
   ```bash
   "$SF" dump <work_dir>/deck.pptd                # 看每页元素数/类型
   ```
   读 `pages/*.page` 定位要改的元素（按 `elementId` 或 bounds）。

3. **定向编辑 PPTD**（只改目标字段）——
   - 改文字：改 `content.text`（富文本注意块标量 `|`）。
   - 改配色：改 `theme.colors` 的 key 值（一处改全局生效）或元素 `fill`/`color`。
   - 换图：把新图丢进 `media/`，改 `image.src` 指向新文件。
   - 挪框/缩框：改 `bounds`。
   - 加元素：在 `elements` 数组末尾追加（越后层级越高）。
   - 删元素：从数组移除（注意被动画 `elementId` 引用的别删，否则 check 报）。

### B1 纯AI改（默认）

用户只描述改动、不碰 PPTX。走标准循环：

4. **check → build（输出新文件，不碰原稿）**：
   ```bash
   "$SF" check <work_dir>/deck.pptd
   "$SF" build <work_dir>/deck.pptd --output <交付件>.pptx      # ≠ 原稿，见下
   ```
   - **B1 不覆盖原稿**：交付件默认 `<原稿 stem>-edited.pptx`（与原稿同目录）。
     原稿始终保留作安全网；小迭代在同一 `<交付件>.pptx` 上覆盖（AI 自己的衍生品，可覆盖）。
   - **与 B2 的根本区别**：B2 因 ping-pong 必须"一个文件覆盖"（§2.3 铁律 1）；
     B1 无此约束，故留原稿。B1 的 `.src.hash` 只在初始 convert 后写一次（原稿静态，不更新）。
   - **skip 非破坏**：convert 若 skip 了 table/chart，交付件里没有，但原稿还在——
     交付时如实标注（§4），用户可接受或转 C 重做该页，数据不丢。

5. **交付**：给 `<交付件>.pptx` 绝对路径 + 结构摘要。**诚实标注 convert 缺口**（见 §4）：
   若原稿有 table/chart 被 skip，交付件里没有——原稿仍完整，用户可转 C 模式重做该页。
   若出现"某处两词粘一起"→ 纯空白 run 空格丢失（xmltree 限制），手动在 PPTD 补空格再 build。

### B2 用户也改PPTX（ping-pong resync）

**触发**：用户在 PowerPoint 里手改了那份 .pptx（与上轮 build 同一文件），存盘后说"改完了"。
协议（一个文件、轮替不并发、6 铁律）见 SKILL.md §2.3；hash 跳过见 §2.4。以下是分步命令。

4. **resync（比 hash 决定是否 convert）**：
   ```bash
   cur=$(sha256of <用户的>.pptx); old=$(cat <work_dir>/.src.hash 2>/dev/null)
   if [ "$cur" != "$old" ]; then "$SF" convert <用户的>.pptx <work_dir> \
       && sha256of <用户的>.pptx > <work_dir>/.src.hash; fi   # 用户改了才 convert 吸收
   "$SF" dump <work_dir>/deck.pptd                            # 看结构有无 skip/异常
   ```
   - hash 相同 → 用户没改（或只看），跳过 convert，pptd 即当前。
   - hash 不同 → convert 吸收用户改动做新 baseline；读 stderr skip：用户在 PPTX 里加了 table/chart 会 skip → AI 需在 PPTD 用 shape+text 补，或该页转 C 重生成。

5. **在新 baseline 上做 AI 这轮改动**：按上方"定向编辑 PPTD"。同元素冲突默认用户赢（铁律 5）：re-convert 后值与预期不符，先问用户以谁为准，别盲改。

6. **check → build（覆盖同一文件，不生成副本）**：
   ```bash
   "$SF" check <work_dir>/deck.pptd
   "$SF" build <work_dir>/deck.pptd --output <用户的>.pptx       # 就地覆盖
   sha256of <用户的>.pptx > <work_dir>/.src.hash               # build 后也是 sync point
   ```
   - 交给用户前先 build（flush，铁律 3），确保用户下一轮编辑基线含 AI 全部改动。
   - 有 skip 时覆盖前提醒用户"会丢失 skip 内容，自行决定是否备份"（铁律 6）。

7. **交回用户**：说“已更新 <那一份>.pptx”，等用户在 PowerPoint 改完存盘后说“改完了” → 回第 4 步。
   仅在用户真改了时付一次 convert（秒级），文本/形状/图片高保真。

### B 模式保真度自检（可选）
```bash
"$SF" build <work_dir>/deck.pptd --output out.pptx
"$SF" convert out.pptx /tmp/back              # 把产物再转回来
"$SF" dump /tmp/back/deck.pptd                 # 对比元素数有无掉失
```

---

## 模式 C：借风格（参考 .pptx 重做）

**触发**：用户给了 .pptx 当风格参考，但要新主题/重排/>50% 重写。或 B 模式因 skip/缺口
放弃在原 PPTD 上改、转而重生成。

1. **convert 参考稿**：
   ```bash
   "$SF" convert <参考>.pptx <ref_dir>          # 如 ./refsrc/
   ```
   把它当"参考"读，不当编辑起点。

2. **摘取风格信息**：从 `<ref_dir>/deck.pptd` 的 `theme`（colors/textStyles）、
   各页 `background`、典型元素的 `fontSize`/`fontFamily`/`fill`/几何，提炼出
   要复用的配色与版式骨架。

3. **新建干净 PPTD**：开 `mydeck/`，把摘取的 theme 填进 `deck.pptd`，
   按用户新主题**重新写 `pages/*.page`**（走模式 A 的步骤 3-5）。
   - 可以把参考稿里某些满意的页"搬"过来：复制该 `.page` 内容，改文字/数据。
   - 但别整稿照搬——带进 convert 缺口（round-trip 扩展字段、被 skip 的元素）。
     主动用 build 支持的元素重写有问题的页。

4. **进标准循环**：`check` → `build` → 反馈 → 改 → 重 build。

5. **交付**：给新 .pptx 路径 + 结构摘要。注明"风格参考自 <参考>.pptx，内容为新生成"。

---

## 通用：迭代与收尾

- **迭代**：用户要改就回到当前 PPTD 目录，编辑对应 `.page` → `check` → `build`。
  不要每轮 convert / 新建目录。
- **收尾**：最后 `build` 出终版 .pptx，给绝对路径。问一句要不要保留 PPTD 源目录
  （便于后续再改）；用户要清理再说。
- **大改切换**：迭代中发现改动越来越大（从 B 滑向 C），果断切到 C 模式重生成，
  别在原 convert 产物上硬补。
