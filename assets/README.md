# Font Awesome dataset

`fa-solid-icons.json` — SVG path data (`d`) and viewBox for the Font Awesome
**solid** (free) icons referenced by SlideForge's fixture decks / tests.

- **Source**: Font Awesome 6.7.2 metadata (`metadata/icons.json` from
  `@fortawesome/fontawesome-free`)
- **License**: Font Awesome Free — CC BY 4.0
  (https://fontawesome.com/license/free) — icons are used to reproduce the
  glyphs in the generated PPTX; attribution belongs in the deck, not the
  binary.
- **Regenerate**: extract the subset your decks need:

  ```python
  import json
  meta = json.load(open('icons.json'))                # FA metadata
  names = [...]                                       # e.g. ["mobile-screen"]
  out = {n: {"w": meta[n]["svg"]["solid"]["viewBox"][2],
             "h": meta[n]["svg"]["solid"]["viewBox"][3],
             "d": meta[n]["svg"]["solid"]["path"]} for n in names}
  json.dump(out, open('assets/fa-solid-icons.json', 'w'), sort_keys=True,
            separators=(',', ':'))
  ```

  Icons not in the dataset fail the build with an explicit error telling you
  to regenerate it.
