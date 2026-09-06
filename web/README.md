# `web/` — the dark-factory console

SvelteKit 2 · Svelte 5 (runes) · Tailwind v4 · TypeScript, strict.

Everything a human touches: signing up, enrolling an authenticator, members, teams, repos
and their live leases, a read-only queue, the usage meter, and the page that tells any MCP
client how to connect.

```bash
npm install
npm run dev      # Vite on :5173, proxying /api /oauth /.well-known to DF_API_ORIGIN
npm run check    # compile messages, check they are complete, then svelte-check + tsc
npm run lint     # prettier --check
npm test         # vitest — the Worker's routing rule, locale resolution, the error map
npm run build    # static bundle in build/
npm run deploy   # build, then deploy the production Worker — docs/deploy/cloudflare.md
```

`npm run dev` needs a server behind it. Set `DF_API_ORIGIN` if it is not on
`http://127.0.0.1:8080`. **Until task 13 binds a port there is nothing to proxy to**, so
the dev server renders the shell and every request 502s.

## Why it is a single-page app

Not a performance choice. The console's session is an `HttpOnly`, `__Host-`-prefixed
cookie, and `__Host-` means the browser refuses to store it unless it is `Secure`, has
`Path=/`, and carries **no `Domain`** — so the cookie is bound to one origin and cannot be
sent anywhere else.

A SvelteKit server rendering these pages would therefore have to hold that credential to
fetch on the user's behalf: a second process with the keys to every console session, for
pages that are behind a login and cannot be cached anyway. Building to static files that
`df-server` serves beside `/api` keeps the cookie in exactly one place — the browser — and
makes CORS a non-question, because there is no second origin.

The same fact drives `vite.config.ts`. Dev proxies `/api`, `/oauth`, and `/.well-known`
rather than pointing `fetch` at another port, because a cross-port request would not carry
the session and no CORS header could rescue it.

## What holds across the app

**No credential is spent on a `GET`.** Invitation and claim links point
at pages here — `/verify`, `/recover`, `/invite/{org}` — which render a button that
`POST`s the token. Mail scanners and link-preview fetchers follow every URL in every
message, and a single-use `GET` is burned before the human clicks it. `df-web`'s
`every_single_use_redemption_is_a_post` and `redeemable_urls_are_pages_not_endpoints`
assert the server's half of the same bargain.

**An org you are not in renders as "no such organization".** The API answers `404` for both
a nonexistent org and one the caller is not in, precisely so the two cannot be told apart.
A console that helpfully said "you don't have access to acme" would undo that from the
client side.

**Roles decide what is _shown_, never what is _allowed_.** `OrgContext.isAdmin` hides
buttons that would fail. Every one of them is still refused by the server, on every
request, by `OrgCtx`.

**The queue is read-only.** There is no button here that changes a job. A job is created
and finished by the agent doing the work, over MCP; a human pressing "mark complete" would
be telling the queue something they cannot observe, and the audit trail would record it as
fact.

**Nothing about the deployment is baked into the bundle.** The MCP endpoint and the
grantable scopes come from `/.well-known/oauth-protected-resource` at runtime. A hard-coded
MCP URL is how a staging or self-hosted deployment ends up printing a connect command that
points at production.

## Messages, and what a new string costs

The console ships in **English, Spanish, German, French, Italian and Hindi**. Messages are
compiled, not looked up at runtime: [Paraglide JS](https://inlang.com/m/gerre34r) turns
`messages/{locale}.json` into tree-shaken functions under `src/lib/paraglide/`, which is what
lets an i18n layer exist here at all without a SvelteKit server.

```
project.inlang/settings.json   the six locales and the base locale
messages/{en,es,de,fr,it,hi}.json   the catalogs
scripts/check-messages.mjs     the completeness gate, wired into `npm run check`
src/lib/locale.ts              which language this document is in, and how it got there
src/lib/errors.ts              ApiError.code / WebauthnError.code → a sentence
src/lib/status.ts              JobStatus → a word (the wire value never changes)
src/lib/paraglide/**           generated. git-ignored, prettier-ignored, never edited.
```

**Adding a user-visible string costs six catalog entries, not one.** `npm run check` fails if
any locale is missing a key, has a key the base locale does not, drops a `{placeholder}`, or
gets its plural categories wrong. That is deliberate: Paraglide *silently* falls back to the
base locale for a missing key, so without the check a half-translated release looks fine in
development and reaches a customer as half a page in the wrong language.

```svelte
<script lang="ts">
  import { m } from '$lib/paraglide/messages';
</script>

<h1>{m.queue_title()}</h1>
<p>{m.queue_jobs_shown({ count: jobs.length })}</p>
```

Three things about the toolchain that the obvious reading gets wrong, all verified against
`@inlang/paraglide-js` rather than assumed:

- **Plurals use the variant form, not the ICU one-liner.** `"{count, plural, one {…} other {…}}"`
  belongs to a different plugin and compiles here to the literal string `undefined other }`.
  Write `[{ "declarations": ["input count", "local countPlural = count: plural"], "selectors":
  ["countPlural"], "match": { "countPlural=one": "# job", "countPlural=other": "# jobs" } }]`.
- **The required plural categories differ per locale, and the check enforces both directions.**
  `en`, `de` and `hi` need exactly `one` and `other`; `es`, `fr` and `it` also need `many`
  (Spanish 1 000 000 is "un millón *de* trabajos"). Declaring `many` where the locale never
  selects it fails too.
- **`--emit-ts-declarations` is what makes a key a type error.** Without it Paraglide emits no
  `.d.ts` at all and `m.no_such_key()` type-checks clean. It is in the `paraglide:compile`
  script for that reason, not for tidiness.

`src/lib/paraglide/` is generated and git-ignored, so `check` and `build` compile it first. A
bare `npx svelte-check` in a fresh clone reports missing modules until `npm run paraglide:compile`
has run once.

**What is not translated, on purpose.** The MCP surface — `df-mcp` tool descriptions and
`df-core` error messages — stays English: its reader is an LLM that has never seen these docs,
and translating it would fragment the one audience it has. Commands, JSON/TOML snippets, config
paths, product names, and every wire value stay verbatim; a translated `--transport http` is a
broken command. The two server-rendered pages (`/oauth/authorize`'s consent screen and its error
page) are localized, but by a hand-written table in `crates/df-web/src/i18n.rs`, because they
have no client-side JS to swap strings and share no keys with these catalogs.

## Which language, and how it is remembered

Three tiers, each with one job:

| Tier | Holds | Authority |
| --- | --- | --- |
| `users.locale` | the account's explicit choice, or `null` | **source of truth** |
| `localStorage['df.locale']` | a copy of it, for first paint | cache only |
| `navigator.languages` | the browser's preference | fallback when nothing was chosen |

The choice lives on the account so it follows the person to their next device; the cache exists
so every load after the first paints in the right language instead of flashing English until
`/api/me` returns. `null` means **"never chose"** — it is not "chose English", and collapsing
the two would pin every account that never opened the picker to the base locale.

A change of language **reloads the document**. Paraglide's `m.*()` are plain calls, not reactive
reads, so Svelte has no dependency to invalidate when the locale changes underneath them.
`locale.ts`'s `needsReload` is split out so the termination argument is a test rather than a
comment: the cache is written *before* the reload, so the next boot resolves to exactly the
value that triggered it.

`<html lang>` is set from script at boot. `app.html` ships `lang="en"` and is not templated per
locale — `adapter-static` emits one shell for every route and there is no server to pick a
language for it.

## Layout

| Path                        | What it is                                                                   |
| --------------------------- | ---------------------------------------------------------------------------- |
| `src/lib/api.ts`            | The only place that talks to `df-web`. `ApiError` carries the stable `code`. |
| `src/lib/types.ts`          | The wire types, transcribed from `df-web`'s OpenAPI document.                |
| `src/lib/session.svelte.ts` | Who is signed in. A rune module, not a store.                                |
| `src/lib/org.svelte.ts`     | The org the current route is about, via context.                             |
| `src/lib/clients.ts`        | One recipe per coding agent, all the same shape.                             |
| `src/routes/`               | Public pages at the root; org pages under `/o/[org]`.                        |
| `worker/index.ts`           | The Cloudflare Worker: serves this bundle, proxies the API to `df-server`.   |
| `wrangler.jsonc`            | That Worker's config. `worker/tsconfig.json` type-checks it separately.      |

Org pages live under `/o/[org]` rather than `/[org]` so that no org slug can ever collide
with a page name. The routes the _server_ names — `/login`, `/verify`, `/recover`,
`/invite/{org}`, `/settings/billing` — are fixed by what the server puts in an invitation link and in
`df-billing`'s upgrade prompt, and must not be renamed here alone.

## Deploying to Cloudflare

`npm run deploy` uploads `build/` and `worker/index.ts` as one Worker: Cloudflare serves the
SPA from its own network and the Worker forwards `/api`, `/oauth`, `/.well-known`, `/mcp`,
`/healthz` and `/readyz` to `df-server`.

**It deploys `--env production`, and that is not a formality.** The default configuration
names a _different_ Worker (`dark-factory-console-dev`) pointing at `http://127.0.0.1:8080`,
so a bare `wrangler deploy` cannot land on the Worker the console actually runs on, and a
deploy that forgets which environment it is in proxies to something that is not listening
rather than quietly serving production traffic.

It proxies rather than redirecting for the same reason the app is a SPA at all — the
`__Host-` cookie is bound to one origin, so the API cannot live on a second hostname. The
prefix list in `worker/index.ts` mirrors `API_PREFIXES` in `crates/df-server/src/lib.rs` and
must not drift from it — `worker/index.test.ts` is the half of that check which lives here,
and `api_prefixes_do_not_match_by_string_prefix_alone` is the other half.

Two things on the `df-server` side are not optional and are easy to miss:
`DF_ALLOWED_HOSTS` must name the origin's own hostname, or every authenticated MCP call
fails while everything else looks healthy; and the origin must refuse traffic that did not
come through Cloudflare, because `DF_CLIENT_IP_HEADER=cf-connecting-ip` is only trustworthy
while Cloudflare is the one writing it. [`docs/deploy/cloudflare.md`](../docs/deploy/cloudflare.md)
has both, and what a local `wrangler dev` run proved about each.

None of this is required to run the console. `df-server` still serves `build/` itself, which
is what a self-hosted deployment does.

## Adding a client to the connect page

Add an entry to `CLIENTS` in `src/lib/clients.ts`. Every client gets the same two forms —
OAuth and access token — because dark-factory is coding-agent agnostic by constraint, and a
console that gave one agent a bespoke wizard and the rest a footnote would be the first
place that promise quietly broke.
