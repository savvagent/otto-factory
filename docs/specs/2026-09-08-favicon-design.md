# Favicon design

> **Status:** DRAFT — replace the placeholder favicon with a small-scale mark derived from the logo

> **Depends on:** `docs/specs/2026-09-05-header-logo-design.md` (`savvagent/otto-factory#44`) — that
> task put the logo mark in the header; this one gives the same brand a matching tab icon.

## Goal & Success Criteria

Replace the placeholder `favicon.svg` with a small-scale composition derived from the brand mark in
`web/static/logo.svg`, shipped in every format modern browsers and platforms need, so the tab icon
reads as "otto-factory" rather than a generic abstract shape.

- A designed, legible favicon exists in `web/static/` as SVG, ICO, and PNG (32/192/512), plus an
  Apple touch icon — not a placeholder.
- The mark stays a crisp, high-contrast silhouette at 16×16 and 32×32 (verified by rendering).
- `web/src/app.html` references the full set with correct `rel`/`sizes`/`type` attributes and a
  working `.ico` fallback.
- `favicon.ico` is a genuine multi-resolution ICO container, not a renamed PNG.
- `npm run check`, `npm run lint`, `npm test`, `npm run build` all pass.

## Scope

**In:**

- A new `web/static/favicon.svg` derived from the brand mark in `web/static/logo.svg`, redrawn as
  its own small-scale composition (not a scaled-down copy of the 390×409 illustration).
- A multi-resolution `web/static/favicon.ico` (16×16, 32×32, 48×48) for browsers/platforms that
  ignore SVG favicons.
- `web/static/apple-touch-icon.png` (180×180) for iOS/iPadOS home-screen shortcuts.
- PNG icons at 32×32, 192×192, and 512×512 for Android home-screen / PWA-style install prompts,
  referenced from `<link rel="icon">` tags (no manifest file is added — see Out).
- `web/src/app.html`'s `<link rel="icon" ...>` updated to reference the full set with correct
  `rel`/`sizes`/`type` attributes, SVG first and `.ico` as the universal fallback.

**Out:**

- No change to `web/static/logo.svg` or the header `Logo` component
  (`web/src/lib/components/Logo.svelte`) — the favicon is a separate, simplified composition drawn
  for small sizes, per `savvagent/otto-factory#45`'s brief.
- No change to `web/src/routes/+layout.svelte` or any other header/layout markup.
- No PWA manifest (`manifest.json` / `manifest.webmanifest`) — none exists today, and adding one is
  a separate concern per the issue's own scope note. The 192×192/512×512 PNGs are wired up as plain
  `<link rel="icon">` entries only, not manifest icons.
- No server-side change of any kind — this is a `web/static/` + `app.html` change only. No MCP tool,
  no console route, no SQL, no config surface, no migration. Consistent with constraint 2
  (substrate, not workflow).

## Assumptions

- **The existing placeholder's color identity is worth keeping, its content is not.** The current
  `favicon.svg` (a rounded-square badge, `#0f172a` background, light mark) already matches the
  app's single dark theme (`--color-canvas: oklch(0.19 0.02 260)` in `web/src/app.css`, i.e.
  `#0f172a`-ish slate). The badge shape and dark background are kept; only the abstract
  lines-and-dot glyph is replaced with a simplified derivative of the actual logo mark.
- **The logo's full detail (gear-tooth crown + three separately-detailed smokestacks with window
  rows) does not survive to 16×16** — rendering the existing `logo.svg` path scaled into a 32×32
  favicon and downsampling to 16×16 produces an illegible grey smear (verified by rendering; see
  Testing). The favicon is redrawn as a new, bolder composition: a stepped three-chimney factory
  silhouette with one smoke puff over the tallest stack, sitting on a solid base block. This keeps
  the same subject (a factory) that the full mark depicts, simplified to shapes that stay crisp at
  16×16: each stroke/gap is at least 2 SVG units wide against the 32-unit viewBox, i.e. survives a
  4× downsample to a 1-device-pixel minimum feature width.
- **A `prefers-color-scheme` media query is included, per the issue's "ideally" ask**, using
  `<style>` with two classes (`.bg`/`.mark`) rather than duplicating the whole glyph, since only
  the two fill colors change between schemes. The rest of the app has no light theme (single dark
  palette in `app.css`), but the favicon renders against arbitrary browser chrome/OS taskbars
  outside the app's own theme control, so adapting there is still worthwhile and matches what the
  issue asks for.
- **Raster exports (ICO/PNG) cannot carry a media query**, so they are rendered once, from the dark
  variant — the app's own (only) theme, and the more common default assumption for a favicon with
  no adaptation.
- **`favicon.ico` must be built as an actual multi-resolution ICO container**, not a renamed PNG —
  the acceptance criteria call this out explicitly. `Pillow`'s `Image.save(..., format="ICO",
  sizes=[(16,16),(32,32),(48,48)])` produces a real multi-image ICO from one high-resolution source
  render; this is checked in Testing.
- **File names follow the plain, discoverable convention already used by browsers/tooling**:
  `favicon.svg`, `favicon.ico`, `favicon-32x32.png`, `favicon-192x192.png`, `favicon-512x512.png`,
  `apple-touch-icon.png` — no build-tool-specific hashing, since `web/static/` is served verbatim by
  `adapter-static`.

## Composition

`web/static/favicon.svg`, viewBox `0 0 32 32` (unchanged canvas size from the placeholder it
replaces):

- A rounded-square backing shape (`rx="7"`), consistent with the placeholder's badge treatment and
  distinct from the full mark's gear-tooth crown (too fine at this scale).
- A symmetric three-chimney factory silhouette centered on the badge: a solid base block, three
  vertical chimney blocks of ascending-then-descending height (short, tall, short), and one circle
  for smoke above the tallest (center) chimney.
- Two color pairs via `prefers-color-scheme`:
  - Default (dark scheme / no support): background `#0f172a`, mark `#f8fafc`.
  - `prefers-color-scheme: light`: background `#eaf6ff`, mark `#0f172a`.

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
  <style>
    .bg { fill: #0f172a; }
    .mark { fill: #f8fafc; }
    @media (prefers-color-scheme: light) {
      .bg { fill: #eaf6ff; }
      .mark { fill: #0f172a; }
    }
  </style>
  <rect class="bg" width="32" height="32" rx="7"/>
  <rect class="mark" x="7" y="19" width="18" height="7" rx="0.5"/>
  <rect class="mark" x="10.5" y="11" width="3.4" height="8" rx="0.5"/>
  <rect class="mark" x="14.3" y="7.5" width="3.6" height="11.5" rx="0.5"/>
  <rect class="mark" x="18.2" y="11" width="3.4" height="8" rx="0.5"/>
  <circle class="mark" cx="16.1" cy="5" r="2"/>
</svg>
```

## Raster generation

Generated once (not a build step — the outputs are committed as static assets, matching every
other file in `web/static/`), from the dark-scheme colors of the SVG above, using `cairosvg` to
rasterize and `Pillow` to pack the ICO container:

| Output                                 | Size(s)                | Purpose                                             |
| --------------------------------------- | ----------------------- | ---------------------------------------------------- |
| `web/static/favicon.ico`               | 16×16, 32×32, 48×48 (one file) | universal fallback; older Safari/Windows shortcuts |
| `web/static/apple-touch-icon.png`      | 180×180                 | iOS/iPadOS home-screen icon                          |
| `web/static/favicon-32x32.png`         | 32×32                   | `<link rel="icon">` PNG fallback                    |
| `web/static/favicon-192x192.png`       | 192×192                 | Android home-screen / PWA-style install prompt       |
| `web/static/favicon-512x512.png`       | 512×512                 | Android home-screen / PWA-style install prompt       |

The apple-touch-icon and the two largest PNGs are rendered without corner rounding lost to
supersampling artifacts — rendered directly at target size from the vector source, not scaled from
a smaller raster, so edges stay crisp.

## `app.html` change

Replace the single placeholder `<link>`:

```html
<link rel="icon" href="%sveltekit.assets%/favicon.svg" />
```

with the full set, SVG first (modern browsers that support it use it and ignore the rest), `.ico`
next (the universal fallback for everything else, including browsers that parse `<link rel="icon">`
but don't support SVG), then sized PNGs, then the Apple-specific tag:

```html
<link rel="icon" href="%sveltekit.assets%/favicon.svg" type="image/svg+xml" />
<link rel="icon" href="%sveltekit.assets%/favicon.ico" sizes="any" type="image/x-icon" />
<link rel="icon" type="image/png" sizes="32x32" href="%sveltekit.assets%/favicon-32x32.png" />
<link
  rel="icon"
  type="image/png"
  sizes="192x192"
  href="%sveltekit.assets%/favicon-192x192.png"
/>
<link
  rel="icon"
  type="image/png"
  sizes="512x512"
  href="%sveltekit.assets%/favicon-512x512.png"
/>
<link rel="apple-touch-icon" sizes="180x180" href="%sveltekit.assets%/apple-touch-icon.png" />
```

`sizes="any"` on the `.ico` link is the documented way to tell browsers that already picked the SVG
to skip the ICO, while still serving as the fallback for a browser with no SVG favicon support at
all (per the MDN/web.dev guidance the issue references).

## Testing

- Render the *unmodified* `logo.svg` scaled into a 32×32/16×16 favicon (throwaway script, not
  committed) to confirm the premise that the full-detail mark doesn't survive downsampling — this
  is what motivates the redraw rather than a direct scale-down.
- Render the new `favicon.svg` at 16×16 and 32×32 (dark scheme) and 32×32 (light scheme, colors
  swapped) and visually confirm the three-chimney silhouette stays legible and high-contrast at
  both sizes.
- Verify `favicon.ico` is a real multi-resolution container: `python3 -c "from PIL import Image;
im = Image.open('web/static/favicon.ico'); print(im.info.get('sizes'))"` must report all three
  sizes, not a single-image file with a renamed extension.
- `npm run check`, `npm run lint`, `npm run build` all pass (`app.html` is the only source file
  touched; the rest are static binary/vector assets `svelte-check`/`prettier` don't parse).
- `npm test` (vitest) is also run for completeness against the repo's standard `web/` gate set, even
  though it exercises the Cloudflare Worker (`web/worker/`) and has no code path that touches
  `app.html` or `web/static/` — a vacuous pass, not a skipped gate.
- Manual: serve the built SPA (or `npm run dev`) and load it in a Chromium-based browser and in
  Firefox; confirm the tab icon renders (not a broken-image glyph) in both.

## Error Handling & Edge Cases

- None — static assets and a document-head change, no runtime branches, no user input, no network
  call beyond the browser's own favicon fetch.

## Risks & Open Questions

- None outstanding.
