# Console version footer design

> **Status:** IMPLEMENTED — shipped in `savvagent/otto-factory#142`, merged as `4d5cedf`, closing
> `savvagent/otto-factory#83` — show the running otto-factory version in the console footer

## Scope

**In:**

- Surface the product's SemVer version (see CLAUDE.md's "Releases & versioning": one version for
  the whole product, `[workspace.package] version` and `web/package.json`'s version move together
  via release-please) somewhere in the console UI.
- Add it to the root footer (`web/src/routes/+layout.svelte`), next to the existing
  `/docs/api` link — the footer already renders unconditionally on every page (gated and
  ungated alike), which is exactly the "somewhere" the issue asks for and needs no new page.
- Source the value from `web/package.json`'s own `version` field at build time.

**Out:**

- No server-side change of any kind — no new endpoint, no new field on an existing response, no
  new config. See "Existing mechanism check" below for why.
- No settings page or "about" panel — the footer is sufficient and is already the one place that
  renders on every route (`web/src/routes/+layout.svelte`'s `<footer>` sits outside every
  conditional in the template, so it shows on `/login` and `/signup` too, which is useful: someone
  reporting a bug from the sign-in screen can still read off the version).
- No link from the version text to a changelog, release, or commit — the issue only asks to
  *display* it; wiring a URL to a specific GitHub release/tag is a separate, unrequested feature
  and one more thing that could point at the wrong place if release-please's tagging shape changes.
- No i18n catalog entry. A SemVer string prefixed with `v` (`v0.3.0`) is a wire value, not natural
  language — CLAUDE.md's own i18n section carves out exactly this category ("Commands, config
  paths, product names and every wire value stay verbatim too"). Every other console string goes
  through Paraglide; this one is intentionally the same kind of exception the MCP surface already
  is, and adding six identical catalog entries for a value that is never actually translated would
  be catalog noise the `check-messages.mjs` gate provides no benefit for.
- This is a `web/`-only presentational change — no MCP tool, no console API route, no SQL, no
  migration, no `OF_*` config key. Consistent with constraint 2 (substrate, not workflow): what
  version string to print is UI presentation, not something a customer's own skill would ever call
  the server about.

## Existing mechanism check

The job description asked to check for an existing version-exposing mechanism before adding a new
one. Three already exist, none of them fit for a console footer:

- `crates/of-mcp/src/server.rs:250` — `Implementation::new("otto-factory", env!("CARGO_PKG_VERSION"))`
  sets the MCP protocol handshake's server version. Visible only to an MCP client during
  initialization, never to a browser.
- `crates/of-web/src/openapi.rs:81` — the OpenAPI document's `info.version` field, also
  `env!("CARGO_PKG_VERSION")`, served at `GET /api/openapi.json` (`crates/of-web/src/catalog.rs:261`,
  `Auth::Public`). Reachable from the browser, but fetching the entire API document — which the
  `/docs/api` page already does once, for a different reason — just to read one field out of it for
  a footer that renders on every page is an odd, heavier dependency than the value is worth, and it
  is the *server's* crate version rather than something that needs a network round trip at all.
- `crates/of-server/src/health.rs` — `/healthz` and `/readyz` carry no version field today. Adding
  one would be an additive, in-bounds change (Non-Negotiable Rule 6 treats a new optional response
  field as normal), but it exists for liveness/readiness probes, not product display, and would
  still cost the footer an extra fetch on every page load for a value that never changes without a
  fresh deploy.

**Chosen mechanism: none of the above.** `web/package.json`'s `version` is already the documented
source of truth for "the web half" of the one product version (CLAUDE.md, "Releases & versioning").
Reading it at build time, via TypeScript's `resolveJsonModule` (already enabled in
`web/tsconfig.json`), needs no server change and no network call. This mirrors the precedent in
`docs/specs/2026-09-05-header-logo-design.md`: prefer a presentational, build-time answer over a
new or repurposed server surface when one is available.

**Revised during PR review (`savvagent/otto-factory#142`):** the original draft of this section
claimed the console bundle and the `of-server` binary "cannot drift, because they are artifacts of
the same build" — true only under the single-image Docker/Fly deploy shape (`Dockerfile` builds
`web/` and the Rust binary together into one image, per `docs/deploy/fly.md`). It is not true
under the Cloudflare Worker deploy shape documented in `docs/deploy/cloudflare.md`, where the
console (`npm run deploy` → `wrangler deploy`) and `of-server` (`flyctl deploy`) ship independently
on their own cadences — `docs/deploy/cloudflare.md`'s own "What is still open" section names this
as a real limitation of that shape. The shipped code in `web/src/lib/version.ts` states the
guarantee accordingly: the displayed version is always the version *the console bundle itself* was
built from, and is additionally the version the accompanying server was built from only under the
Docker/Fly shape. See Risks & Open Questions.

## Assumptions

- "The deployed otto-factory version" (job description) means the one SemVer version this product
  ships as a whole, not `of-mcp`'s or `of-web`'s individual crate version read separately — they are
  the same number today (workspace-inherited), so this is moot in practice, but `web/package.json`'s
  version is the one explicitly named as the web half's source of truth, so it is the one read here.
- A plain `v{version}` string (e.g. `v0.3.0`) is sufficient "display" per the issue's one-line body;
  no build metadata (git SHA, build timestamp) is requested and none is added — CLAUDE.md's
  versioning section describes exactly one number moving with every release, not a build
  fingerprint, and the issue does not ask for one.
- The footer is the right "somewhere": it is the console's one element that already exists on every
  route regardless of auth state (see Scope, Out), so no new page or gating logic is needed, and it
  already carries one other terse, low-emphasis link (`/docs/api`) in the same register.
- **Fast-path judgment call:** this change is not run through the otto-factory-development skill's
  trivial-task fast-path even though it is small. The fast-path's own criteria list explicitly names
  `web/` among the surfaces that disqualify a change ("No change to deploy/distribution shape
  (`Dockerfile`, `fly.toml`, `.github/workflows/`, `web/`, `web/worker/`,
  `crates/of-core/migrations/`)"), and this change edits files under `web/`. The
  `docs/specs/2026-09-05-header-logo-design.md` precedent — a comparably small, two-file, `web/`-only
  presentational change — was likewise carried through the full spec-and-plan path rather than
  fast-pathed, which is the pattern this spec follows.

## Implementation

**New file** `web/src/lib/version.ts` (shown as shipped, after the PR review revision described
above):

```ts
import { version } from '../../package.json';

/**
 * The console bundle's own release version.
 *
 * `web/package.json`'s `version` and the workspace crate version move together
 * via release-please (see CLAUDE.md, "Releases & versioning"), so this value
 * with no network call is always the version the console was built from.
 *
 * Whether it is also the version the *accompanying server* was built from
 * depends on the deploy shape. Under the single-image Docker/Fly shape
 * (`Dockerfile`, `docs/deploy/fly.md`) the console bundle and the `of-server`
 * binary are built in the same `Dockerfile` from the same source tree, so the
 * two cannot drift. Under the Cloudflare Worker shape (`docs/deploy/cloudflare.md`)
 * the console is deployed independently (`npm run deploy`) from `of-server`
 * (`flyctl deploy`), on its own cadence — `docs/deploy/cloudflare.md`'s own
 * "What is still open" section notes the Worker's bundle and the image's
 * bundle can drift because nothing deploys both from one commit. So this is
 * the version the console was built from, not a guarantee about the origin
 * it happens to be talking to.
 */
export const APP_VERSION = version;
```

A dedicated module rather than importing `package.json` directly in the layout: it gives the value
one clear name at its point of use, and — more importantly — a unit test target that needs no
component mount (see Testing).

**Modify** `web/src/routes/+layout.svelte` — add the import alongside the existing ones:

```ts
import { APP_VERSION } from '$lib/version';
```

and extend the footer:

```svelte
<footer class="border-t border-edge/40 px-4 py-4 text-center text-xs text-faint">
  <a class="hover:text-muted" href="/docs/api">{m.nav_api_reference()}</a>
  <span aria-hidden="true"> · </span>
  <span>v{APP_VERSION}</span>
</footer>
```

The middle `<span aria-hidden="true">` is a plain visual separator between the two footer items. It
carries no text a screen reader needs to announce twice, so it is hidden from the accessibility
tree rather than read as "middle dot". (A "·" separator appears elsewhere in the console —
`settings/+page.svelte`, `o/[org]/+page.svelte`, `o/[org]/repos/+page.svelte` — but always as
inline punctuation inside one translated message string, not as a standalone `aria-hidden` span
between two locale-neutral fragments; this footer has no existing pattern to match, so this is a
new, self-contained choice rather than a precedent being followed.)

## Testing

- **New** `web/src/lib/version.test.ts` — a pure-value unit test in the same style as
  `web/src/lib/labels.test.ts` / `locale.test.ts` (no component mount needed): asserts
  `APP_VERSION` equals `web/package.json`'s own `version` field (re-imported in the test) and
  matches a SemVer-shaped pattern, so a malformed `package.json` version fails loudly here rather
  than silently rendering garbage in the footer.
- `npm run check` — svelte-check + tsc must accept the JSON import and the new module.
- `npm run lint` — prettier.
- `npm run build` — confirms the static bundle still builds with the JSON import resolved.
- Manual: `npm run dev`, confirm the footer shows `v<version>` matching `web/package.json` on both
  a gated page (e.g. `/o/<org>`) and an ungated one (`/login`).
- No `+layout.svelte`-level render test is added. Mounting the root layout in `jsdom` pulls in
  `$app/navigation`, `$app/state`, and `session.svelte.ts`'s network-backed session resolution —
  substantially more scaffolding than this two-line footer addition warrants, and no existing test
  in this repo mounts `+layout.svelte` today (the page-level render tests that do exist —
  `docs/api/page.render.test.ts`, `o/[org]/page.render.test.ts` — mount individual routed pages, not
  the root layout). The unit test on `APP_VERSION` plus the manual check above are the right-sized
  coverage for a static string sourced from a JSON import.
- **Added during PR review:** `the_console_and_the_server_agree_on_the_version` in
  `crates/of-web/tests/console.rs`, a plain `#[test]` (no database pool) that `include_str!`s
  `web/package.json` and asserts its `version` equals `env!("CARGO_PKG_VERSION")` — mirroring
  `the_console_and_the_server_agree_on_the_locales` in `crates/of-core/src/i18n.rs`. This enforces,
  under the Docker/Fly build, the one invariant `version.test.ts` alone cannot: not just that
  `APP_VERSION` matches `web/package.json` (which it always will, since it's read from the same
  file), but that `web/package.json`'s version actually agrees with the workspace crate version
  the server was compiled from.

## Error Handling & Edge Cases

- None at runtime — this is a build-time constant with no branches, no user input, and no network
  call. The one failure mode (a missing or malformed `version` field in `web/package.json`) is a
  build-time TypeScript/test failure, not a runtime one: `resolveJsonModule` typing plus
  `version.test.ts`'s SemVer-pattern assertion catch it before the bundle ships.

## Risks & Open Questions

- **Version skew under the Cloudflare Worker deploy shape.** `docs/deploy/cloudflare.md` documents
  a shape where the console (deployed via `wrangler deploy`) and `of-server` (deployed via
  `flyctl deploy`) ship independently, on independent cadences, from one commit each but not the
  same commit as each other. Under that shape the footer's `v<version>` is guaranteed to be the
  console bundle's own version, but not guaranteed to match the `of-server` instance it happens to
  be talking to at that moment — a stale Worker deploy would show a version number the origin has
  since moved past. This is an accepted limitation, not a defect: the alternative (fetching the
  version from the server at runtime) reintroduces a network round trip on every page load for a
  value that is fully correct under this deployment's actual primary shape (Fly/Docker, per
  `docs/deploy/fly.md`) and only ever wrong-in-a-specific-way (stale, not incorrect-about-itself)
  under the secondary one. `the_console_and_the_server_agree_on_the_version` (see Testing) at least
  guarantees the two are never wrong *at build time* for whichever shape is being built.
