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

**触发**：用户给了 .pptx + 改动需求（先按 SKILL.md §2.1 判 B vs C）。
B 只有一条循环（SKILL.md §2.2）：**动 PPTD 之前先验 hash（红线），不一致先 convert**；
不需要判断/询问“AI 独改还是用户也改”。

1. **验 hash（每轮编辑前的红线动作，见 SKILL.md §2.2 铁律 2）**：
   ```bash
   d=<work_dir>; if [ ! -f "$d/.src.hash" ]; then echo STALE; else
     s=$(sed -n 2p "$d/.src.hash"); [ -f "$s" ] || s="$d/$s"
     [ "$(shasum -a 256 "$s" | cut -d' ' -f1)" = "$(sed -n 1p "$d/.src.hash")" ] \
       && echo IN_SYNC || echo STALE; fi
   ```
   - IN_SYNC → 没人动过文件，pptd 即当前，直接去第 3 步；
   - STALE（含 .src.hash 缺失）→ 先 convert 再动 PPTD。

2. **STALE 转换后才摸结构**：
   ```bash
   "$SF" dump <work_dir>/deck.pptd                # 看每页元素数/类型
   ```
   读 stderr skip 报告：用户在 PPTX 里加了 table/chart 会 skip → AI 需在 PPTD 用
   shape+text 补，或该页转模式 C 重生成。读 `pages/*.page` 定位要改的元素
   （按 `elementId` 或 bounds）。re-convert 后值与 AI 预期不符（用户动过）→
   先问用户以谁为准，别盲改。

3. **定向编辑 PPTD**（只改目标字段）——
   - 改文字：改 `content.text`（富文本注意块标量 `|`）。
   - 改配色：改 `theme.colors` 的 key 值（一处改全局生效）或元素 `fill`/`color`。
   - 换图：把新图丢进 `media/`，改 `image.src` 指向新文件。
   - 挪框/缩框：改 `bounds`。
   - 加元素：在 `elements` 数组末尾追加（越后层级越高）。
   - 删元素：从数组移除（注意被动画 `elementId` 引用的别删，否则 check 报）。

4. **check → build（就地覆盖；hash 自动同步）**：
   ```bash
   "$SF" check <work_dir>/deck.pptd
   "$SF" build <work_dir>/deck.pptd --output <用户给的>.pptx
   ```
   - **就一个文件**：build 输出 = 用户给的那份，不生成副本、不留 `_user`/`_v{n}`。
   - 覆盖 convert 源文件时 hash **自动记**（输出 `auto-sync: ...`），下一轮 convert
     才能正确判 "unchanged"；无需任何旗标。
   - **若被拒绝**（`refusing to build: ... changed after the last sync point`）：
     说明文件在同步点后被外部改过而 pptd 未吸收。按提示先 convert 吸收，
     再重做 PPTD 改动、重 build。**不要**试图绕过——拒绝意味着用户改动会丢。
   - 例外：用户明确要求"别动我的原稿" → `--output` 到别处（如 `<stem>-edited.pptx`）；
     此时产物不是同步点，不记 hash（下次 convert 会重转，对隔离副本来说正确）；
     小迭代可在同一衍生品上覆盖。
   - 首次覆盖前若 convert 报过 skip（table/chart）：覆盖会把丢这些写进用户文件 →
     先告诉用户"这会丢失 X，自行决定是否备份"。

5. **交付**：给 .pptx 绝对路径 + 结构摘要。**诚实标注 convert 缺口**（见 §4）：
   若原稿有 table/chart 被 skip，交付件里没有；若出现"某处两词粘一起"→
   纯空白 run 空格丢失（xmltree 限制），手动在 PPTD 补空格再 build。
   用户之后手改了文件再交回 → 回到第 1 步（验 hash 判 STALE，convert 吸收）。

### B 模式保真度自检（可选）
```bash
"$SF" build <work_dir>/deck.pptd --output out.pptx
"$SF" convert out.pptx /tmp/back             # 把产物再转回来（fresh 目录无 .src.hash，自然全量转）
"$SF" dump /tmp/back/deck.pptd               # 对比元素数有无掉失
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
