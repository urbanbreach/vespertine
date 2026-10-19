# @vespertine/examples

Reference example surfaces for the Vespertine React adapter (APP-011, APP-012). They validate renderer
capabilities and impose no product look: every color is a role of a neutral palette (`src/theme.js`).

| Example | Module | What it shows |
| --- | --- | --- |
| Task list | `tasks` | sidebar (236 px, animated collapse), header (52 px), virtual list (48 px rows), composer (46 px) |
| Area mosaic | `mosaic` | external store, squarified layout, hit testing, keyboard navigation |
| Direct image | `image` | pixels uploaded through an image reference, reference-driven scrolling |
| Editing surface | `editing` | shared viewport, captured drags, snapping, trimming, marquee, zoom, culling |

Every component takes optional `width` / `height` (`viewportWidth` / `viewportHeight` for the editing
surface) so automated input coordinates do not depend on live window geometry.
