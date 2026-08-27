# PPTD Group Extension (SlideForge)

Canonical PPTD v2 is a **flat** model: `Page.elements` is a one-dimensional
array, `Element` has no `Group` variant, and `bounds` is slide-space (see
[`pptd-spec.md`](./pptd-spec.md) §ElementBase). When a source PPTX carries a
`<p:grpSp>` (组合/group), the standard approach is to **flatten** it — decompose
the group into individual top-level elements with slide-space bounds.

Flattening is visually lossless, but it changes WPS/PowerPoint **edit-time
behavior**: a real group's selection bounding box encompasses the union of its
children's visual bounds **including effects** (drop shadows, glow…), so the
dashed selection frame sits *outside* the shadow. A flattened single shape's
selection box is just the geometry, so the shadow extends *beyond* the box — a
visible discrepancy when editing.

To preserve the group selection box while keeping the PPTD **form flat**, SlideForge
adds an **optional** group extension: the `elements` array stays flat (slide-space
`bounds`), and the group is reconstructed as a real `<p:grpSp>` on output by the
writer.

## 1. Data model

All fields are optional and backward compatible. A standard PPTD consumer
ignores them and reads the flat `elements` array + slide-space `bounds`
unchanged.

### `ElementCommon` (every element)

| field | type | meaning |
|---|---|---|
| `groupId` | `string?` | id of the group this element belongs to. |
| `groupBounds` | `[x, y, w, h]?` | the element's `a:xfrm` in the **group's child space** (verbatim), emitted inside the reconstructed `<p:grpSp>`. Only meaningful with `groupId`. |

`bounds` remains **slide-space** (where the element actually appears) so a
direct PPTD renderer that ignores the extension still draws the slide
correctly. `groupBounds` is the raw child-space coordinate the writer swaps in
when emitting the element inside its group — this avoids transform composition
for degenerate (WPS-artifact) groups whose `ext`/`chExt` ratio would otherwise
fabricate phantom children.

### `Page.groups`

```ts
interface Page {
  // …standard fields…
  groups?: Record<string, GroupDef>;
}
interface GroupDef {
  xfrm: GroupXfrm;     // the group's raw <a:xfrm> (off/ext/chOff/chExt), verbatim
  name?: string;       // the original p:cNvPr name (e.g. "组合 22")
  parent?: string;     // parent groupId for nested groups (null = top-level)
}
interface GroupXfrm {
  off:     [x, y];     // slide-space offset (px)
  ext:     [cx, cy];   // slide-space extent (px)
  chOff:   [x, y];     // child-space offset (px)
  chExt:   [cx, cy];   // child-space extent (px)
  rot?:    number;     // clockwise degrees
  flip?:   [boolean, boolean];
}
```

## 2. Convert (PPTX → PPTD)

`flatten_group` walks a `<p:grpSp>` once and, for each **non-degenerate** group:

1. allocates a group id (`grp1`, `grp2`, …) and stores `GroupDef { xfrm: raw,
   name, parent }` in `Page.groups`;
2. recursively flattens children into the **flat** `elements` array (slide-space
   `bounds`, as before) and tags each leaf with `groupId` + `groupBounds` (the
   child's raw child-space xfrm);
3. nested groups recurse with `parent = <this group's id>`.

**Degenerate groups are skipped**, not preserved. WPS emits artifact groups
whose own `ext` is ~0 (or whose `ext`/`chExt` ratio collapses children to ~0)
while a parent group enlarges them back up. Composing those scales per the
OOXML spec fabricates full-size children that renderers (WPS/QuickLook) draw
**nothing** for — so a verbatim copy would be correct, but the standard's
flatten would fabricate a phantom (e.g. a header banner covering a card). A
group is degenerate when its own `ext` is sub-pixel in both axes, or its
`ext`/`chExt` ratio is `< 0.01` in both axes (shrinking-to-zero). Enlarging
groups (`ext` >> `chExt`, e.g. 635×) are **not** skipped — their children
render, matching the source.

Rotated groups are preserved (the rotation rides on `GroupXfrm.rot`).

## 3. Build (PPTD → PPTX)

`render_sp_tree` iterates the flat `elements` array. When an element carries a
`groupId`:

- it finds the run of consecutive members of that group (plus members of
  descendant groups, for nesting) and emits a `<p:grpSp>` with the group's
  verbatim `<a:xfrm>` (`off`/`ext`/`chOff`/`chExt`);
- each direct leaf is emitted with its **child-space** `groupBounds` as the
  `<a:xfrm>` (verbatim — no transform composition);
- nested groups are emitted recursively inside their parent's `<p:grpSp>`.

Elements without a `groupId` render inline (slide-space `bounds`), as in
canonical PPTD v2.

The reconstructed group's `<a:xfrm>` is byte-identical to the source's (modulo
px↔EMU rounding of `< 5 EMU`), so WPS renders the group — and draws the
selection box — exactly as the source.

## 4. Limitations

- **z-order contiguity**: a group's members must be consecutive in the flat
  `elements` array (the OOXML source is contiguous, so convert preserves this).
  Non-contiguous membership is not reconstructed as a single group.
- **Unrepresentable children**: a group containing charts/tables (not yet
  supported) keeps the group but drops those children (still skipped).
- **Orphan nested groups**: a nested group whose parent was skipped
  (degenerate) has no parent span to nest into; its members fall back to flat
  rendering (rare — typically the nested group is skipped alongside its parent).
- **Editability**: grouped children carry child-space `groupBounds`; manually
  moving a grouped child in the PPTD requires understanding the group xfrm.
  Round-trip fidelity is prioritized over hand-editing of grouped content.
