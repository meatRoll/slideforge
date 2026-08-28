# PPTD Layout Extension (SlideForge)

> **Status:** SlideForge extension layered on PPTD v2. **Not** part of the
> canonical spec ([`pptd-spec.md`](./pptd-spec.md)); other PPTD v2 consumers
> (e.g. Moonshot's browser writer) ignore the extra fields — content is
> preserved but layout grouping is lost (flattened to self-contained pages).
> A deck using only the canonical fields remains 100 % spec-compliant PPTD v2;
> the extension is opt-in.
>
> **Fields added by this extension:** `Presentation.layouts`,
> `Page.layout`, `Text.placeholder`, and the `LayoutDef` / `PlaceholderDef`
> types.

## 1. Why

PPTD v2 is flat (no Slide Master / Layout; see `pptd-spec.md` §0). To round-trip
a real PPTX without flattening master/layout decorations onto every slide —
which makes full-bleed backgrounds **draggable** slide shapes and leaves the
WPS layout panel **blank** — SlideForge adds an optional layout layer that
mirrors the OOXML `slideLayout` concept while keeping pages self-sufficient when
the field is unset.

## 2. Manifest: `layouts`

```ts
interface Presentation {
  // ...all canonical fields unchanged...
  layouts?: { [key: string]: LayoutDef };   // SlideForge extension; default none
}
```

A deck may declare any number of named layouts. Keys are author-chosen
(`cover`, `section`, `content` …). A page references one by key via
`Page.layout`.

```yaml
layouts:
  cover:
    background: { type: image, src: media/cover_bg.jpg }
    elements:
      - { elementId: deco_bar, elementType: shape, bounds: [0, 520, 960, 20],
          shapeName: rect, fill: { type: solid, color: "#0070C0" } }
    placeholders:
      title: { bounds: [60, 40, 600, 60], style: "$title" }
  content:
    placeholders:
      title: { bounds: [60, 40, 840, 50], style: "$title" }
      body:  { bounds: [60, 120, 840, 360], style: "$body" }
```

## 3. Page: `layout`

```ts
interface Page {
  // ...all canonical fields unchanged...
  layout?: string;   // SlideForge extension; references layouts[key]; default none
}
```

```yaml
layout: cover
background: { type: solid, color: "#FFFFFF" }   # optional; overrides layout.background
elements: [...]                                  # page's own; painted above layout.elements
```

A page **without** `layout` is fully self-contained — identical to canonical
PPTD v2. The extension is opt-in and backward compatible.

## 4. `LayoutDef`

```ts
interface LayoutDef {
  background?:   Fill;                            // page background when the page doesn't set one
  elements?:     Element[];                       // decorative elements, painted UNDER page.elements
  placeholders?: { [type: string]: PlaceholderDef };  // geometry + style for slide placeholders
  groups?:       { [key: string]: GroupDef };     // SlideForge ext; reconstructed <p:grpSp> metadata for decorative elements (see pptd-roundtrip-extension.md)
}
```

- `background` and `elements` reuse the canonical `Fill` / `Element` types
  (no new element kinds).
- `placeholders` is keyed by OOXML placeholder type (`title`, `body`,
  `subTitle`, `dt`, …) — see §5.
- `groups` is a SlideForge round-trip field (same shape as `Page.groups`);
  see [`pptd-roundtrip-extension.md`](./pptd-roundtrip-extension.md) §5.

## 5. `PlaceholderDef`

A layout placeholder captures the geometry + default run-style that a slide
placeholder of the same `type` inherits when it omits its own geometry /
run properties (the OOXML layout→slide placeholder inheritance, now expressed
in PPTD).

```ts
interface PlaceholderDef {
  bounds:      [number, number, number, number];  // xfrm geometry, px (required)
  style?:      string;                            // references theme.textStyles, e.g. "$title"
  // run-style defaults (each filled in only when the slide placeholder omits it)
  color?:      Color;
  fontSize?:   number;
  fontFamily?: FontFamily;
  bold?:       boolean;
  italic?:     boolean;
  align?:      Alignment;
}
```

## 6. Slide placeholder reference

A slide `Text` element opts into placeholder inheritance via an extension field:

```ts
interface Text extends ElementBase {
  // ...canonical fields...
  placeholder?: string;   // SlideForge extension; the layout placeholder type to inherit from (e.g. "title")
}
```

When `placeholder: "title"` is set **and** the page has a `layout` with
`placeholders.title`, the merge rule §7.3 applies. `bounds` (and any run-style)
on the element still win where set — a slide may reposition or restyle a
placeholder without losing inheritance of the fields it omits.

## 7. Merge / inheritance (bounded: page → layout, one level)

1. **background** — `page.background` if set, else `layout.background`, else the
   canonical default (white solid). Same priority posture as the canonical
   per-element style chain.
2. **elements paint order** — `layout.elements` (bottom) → `page.elements`
   (top). Both are full `Element[]`; individual element fields are not merged.
3. **placeholders** — a slide `Text` with `placeholder: <type>` inherits from
   `layout.placeholders[type]`. Only the fields the slide placeholder **omits**
   are filled in (`bounds`, `color`, `fontSize`, `fontFamily`, `bold`,
   `italic`, `align`, `style`). The slide placeholder wins wherever it sets a
   value.
4. **no deeper chain** — there is no master-of-layout. `theme` is still
   referenced separately by `style: "$key"` and resolved through the canonical
   style priority chain (`pptd-spec.md` §1).

> The current `convert`-side placeholder inheritance logic (layout xfrm +
> prstGeom + lstStyle defRPr lookup, Fix 1/5) is exactly what §5–§7 formalise:
> the runtime lookup is replaced by storing the resolved defaults in
> `layout.placeholders[type]` at convert time.

## 8. OOXML mapping (SlideForge `build`)

- Each `layouts[key]` → one real `ppt/slideLayouts/slideLayoutN.xml`:
  - `<p:bg>` from `layout.background` (incl. `<a:blipFill>` for image fills —
    requires `fill_xml` `Fill::Image` support, see §11 P3);
  - `spTree` = `layout.elements` as `<p:sp>`/`<p:pic>` **plus** one `<p:sp>` per
    `placeholders[type]` carrying `<p:ph type="…">`, its `xfrm`/`prstGeom` and a
    `<a:lstStyle>`/defRPr built from `PlaceholderDef`.
- Each `slideN.xml` → `spTree` = `page.elements` only; `slideN.xml.rels` → the
  referenced `slideLayoutN.xml`.
- `slideMaster1.xml` — still synthesised (one, minimal) unless a future
  extension adds master-level data.
- Net effect: WPS/PowerPoint layout panel shows real layout thumbnails;
  layout-level decorations are non-selectable on slides; backgrounds render via
  `<p:bg>` (non-draggable).

## 9. PPTX → PPTD (`convert`)

- Group slides by their source `slideLayoutN.xml` → one `layouts[key]` per
  distinct layout (key auto-derived from the layout part name, e.g.
  `slideLayout13` → `layout_13`, or a `pageType` hint when available).
- `layout.background` — the layout's `<p:bg>` (or the master's, or a promoted
  full-bleed decorative picture, see §10).
- `layout.elements` — the **master** spTree decorative shapes **+** the
  **layout** spTree decorative shapes (the shapes currently flattened onto
  slides; now kept at layout level). Slide placeholders (those carrying
  `<p:ph>`) are **not** decorative — they become `layout.placeholders`.
- `layout.placeholders[type]` — each layout placeholder's `xfrm` + `prstGeom` +
  `lstStyle` lvl1pPr defRPr (color/font/size/bold/italic/align), keyed by
  `ph.type` (`title`/`body`/…). This **replaces** the current
  `layout_placeholders` runtime lookup (Fix 1/5 logic moves here at convert
  time).
- `slide.elements` — the slide's own spTree shapes only (content). Each
  placeholder-bearing one carries `placeholder: "<type>"` plus its own geometry
  / run-style where the slide set them.
- Placeholder geometry inheritance (the old Fix 1/5 path) folds entirely into
  `layout.placeholders` — no separate runtime inheritance.

## 10. Full-bleed background promotion (heuristic)

When the master/layout spTree contains a `<p:pic>` or `<p:sp>` whose
`off = (0, 0)` and `ext = (slideW, slideH)` (the whole canvas), it is treated
as the **layout background** (`layout.background = { type: image, src: … }` for
a picture, or the shape's solid/gradient fill for a `<p:sp>`) rather than a
decorative element, so it renders as `<p:bg>` (non-draggable) instead of a
layout spTree shape. Near-full-bleed (within a small EMU tolerance) is also
accepted.

## 11. Phasing

- **P1 — data model.** `ast.rs` gains `Presentation.layouts`, `Page.layout`,
  `Text.placeholder`; a new module (or `shared.rs`) gains `LayoutDef` /
  `PlaceholderDef`. Loader/writer schema is made tolerant (optional fields,
  back-compat). All existing tests pass unchanged; `build` still bakes `layout`
  into each page (current flat behaviour) so output is unaffected.
- **P2 — `convert` grouping.** Group by source layout; move decorative shapes +
  background + placeholders into `layouts[key]`; stop flattening decorations
  onto slides; slide placeholders carry `placeholder: type`. `build` still
  bakes until P3.
- **P3 — `build` real layouts.** `writer.rs` emits one `slideLayoutN.xml` per
  `layouts[key]`; `fill_xml` gains `Fill::Image` → `<a:blipFill>` (for `<p:bg>`
  image backgrounds); slides reference their layout via `.rels`. This is the
  phase that makes WPS show real layout thumbnails and locks backgrounds to the
  layout (non-draggable).

## 12. Interop & divergence

- Extension fields: `Presentation.layouts`, `Page.layout`, `Text.placeholder`,
  `LayoutDef`, `PlaceholderDef`.
- Canonical PPTD v2 consumers that ignore unknown YAML fields (Kimi browser
  writer; serde with `deny_unknown_fields` off) drop layout grouping but keep
  every page's content (flattened). No data loss.
- This doc marks which fields are SlideForge-only so AI editors know the
  boundary.
