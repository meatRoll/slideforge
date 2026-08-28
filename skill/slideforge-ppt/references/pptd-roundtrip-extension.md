<!-- Synced from docs/pptd-roundtrip-extension.md · self-contained skill copy · re-sync when source changes. Cross-links between pptd-*.md resolve within this folder. -->

# PPTD Round-trip Extension (SlideForge)

> **Status:** SlideForge extension layered on PPTD v2. **Not** part of the
> canonical spec ([`pptd-spec.md`](./pptd-spec.md)); other PPTD v2 consumers
> ignore the extra fields — a deck using only the canonical fields remains
> 100 % spec-compliant PPTD v2. All fields below are optional and backward
> compatible (serde ignores unknown keys, no `deny_unknown_fields`).
>
> These are small fields SlideForge adds to preserve OOXML round-trip fidelity
> (`convert` emits them, `build` respects them). They are **not** required for
> hand-authoring; an AI writing PPTD from scratch can ignore them. The two
> feature-level extensions — [`pptd-layout-extension.md`](./pptd-layout-extension.md)
> (layouts) and [`pptd-group-extension.md`](./pptd-group-extension.md)
> (groups) — are documented separately; this doc covers the remaining
> per-type fields that did not fit in either.

## 1. Shared types

### `Border`

Canonical `Border` is `{style, width, color}`.

| field | type | meaning |
|---|---|---|
| `gradient` | `GradientFill?` | Gradient outline (overrides `color` when present). |

### `Shadow`

Canonical `Shadow` is `{blur, color, offset?}`.

| field | type | meaning |
|---|---|---|
| `scale` | `number?` | Scale factor (1.0 = 100 %); mirrors OOXML `outerShdw` `sx`/`sy`. `> 1.0` lets the shadow peek beyond the shape edge (a centered halo). |
| `inner` | `boolean?` | Render an inner shadow (`a:innerShdw`, inset) instead of the default outer shadow (`a:outerShdw`). |

### `GradientFill`

Canonical `GradientFill` is `{type: "gradient", gradientType, stops, angle?}`.

| field | type | meaning |
|---|---|---|
| `scaled` | `boolean` (default `false`) | `a:lin scaled` — the gradient is scaled to the shape's bounding box rather than using absolute coordinates. Present on both the standalone `GradientFill` (used by `content.gradient` / `border.gradient`) and `Fill::Gradient`. |

> **Note:** SlideForge's standalone `GradientFill` (used by `content.gradient`
> and `border.gradient`) omits the canonical `type: "gradient"` discriminator.
> The `type` key is accepted but ignored when present; write
> `{gradientType: linear, stops: [...], angle: 90}` (no `type`).

## 2. `Text` element

Canonical `Text` carries only `rotation`/`opacity`/`flip`/`content`. SlideForge
adds a box background and outline (so a text box can be a coloured card
without a separate `shape` behind it):

| field | type | meaning |
|---|---|---|
| `fill` | `Fill?` | Box background (solid/gradient). Absent → `a:noFill` (a plain text box must not inherit the automatic shape fill). |
| `border` | `Border?` | Box outline. |
| `placeholder` | `string?` | Layout extension field — the layout placeholder `type` to inherit geometry/run-style defaults from (e.g. `"title"`). See [`pptd-layout-extension.md`](./pptd-layout-extension.md) §6. |

## 3. `TextContent`

Canonical `TextContent` lists `text`/`style`/`color`/`fontSize`/`fontFamily`/
`bold`/`italic`/`backgroundColor`/`lineHeight`/`lineHeightPx`/`letterSpacing`/
`marginTop`/`textDirection`/`wrap`/`align`/`gradient`/`shadow`. SlideForge
adds:

| field | type | meaning |
|---|---|---|
| `marginLeft` / `marginRight` / `marginBottom` | `number?` | Box insets (px); canonical only has `marginTop`. Written to `a:bodyPr lIns`/`rIns`/`bIns`. |
| `autofit` | `"fit_shape"` / `"fit_text"` / `"fixed"?` | Auto-fit mode: `fit_shape` → `a:spAutoFit` (resize shape to text), `fit_text` → `a:normAutofit` (shrink font to box), `fixed` → `a:noAutofit`. |
| `bulletChar` | `string?` | List bullet glyph (e.g. `•`); absent → no bullet. |
| `bulletFont` | `string?` | Bullet typeface (e.g. `Arial`). |
| `listMargin` | `number?` | Paragraph left margin `marL` (px) — the bullet's text offset. |
| `listIndent` | `number?` | Hanging indent (px; negative pulls the bullet left of `marL`). |
| `bodyPrExtras` | `Record<string, string>?` | Raw `a:bodyPr` attributes not covered above (e.g. `vertOverflow`, `horzOverflow`, `numCol`, `compatLnSpc`, `anchorCtr`, `forceAA`) — preserved verbatim so PowerPoint/WPS layout quirks round-trip instead of falling back to engine defaults. |

## 4. `Image`

Canonical `Image` has `rotation`/`opacity`/`flip`/`src`/`cropShape`/`fit`/`crop`/
`border`/`shadow`. SlideForge adds:

| field | type | meaning |
|---|---|---|
| `softEdge` | `number?` | Soft-edge radius (px) written as `a:softEdge rad` in the picture's effect list (e.g. a feathered circular avatar). |

## 5. Layout extension additions

[`pptd-layout-extension.md`](./pptd-layout-extension.md) defines
`LayoutDef{background, elements, placeholders}`. SlideForge adds:

| field | type | meaning |
|---|---|---|
| `LayoutDef.groups` | `Record<string, GroupDef>?` | Reconstructed `<p:grpSp>` metadata for the layout's *decorative* elements (same shape as `Page.groups`; see [`pptd-group-extension.md`](./pptd-group-extension.md)). |

## 6. Group extension additions

[`pptd-group-extension.md`](./pptd-group-extension.md) defines
`GroupDef{xfrm, name, parent}`. SlideForge adds:

| field | type | meaning |
|---|---|---|
| `GroupDef.fill` | `Fill?` | The group's own fill (`<p:grpSpPr>` solidFill/gradFill). Emitted so `<a:grpFill>` children inherit it and renderers that paint the group's fill (QuickLook) show the banner. |

## 7. Build-side support

All fields above are respected by `build` (rendered to the corresponding
OOXML) except where the README roadmap marks a feature as not yet emitted
(`notes` → notesSlide, `animations` → `p:timing` are still outstanding).
`convert` emits them whenever the source PPTX carries the corresponding
OOXML construct.
