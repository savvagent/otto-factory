# Console internationalization design

> **Status:** DRAFT — localize the console and the two server-rendered browser pages into
> English, Spanish, German, French, Italian and Hindi.

Implements [`#42`](https://github.com/savvagent/dark-factory/issues/42).

## Brief

Quoting the issue:

> The console ships English only. Every string in `web/src` is a literal in a component,
> `<html lang="en">` is hard-coded in `web/src/app.html`, and the two server-rendered browser
> pages in `df-web` (`consent_html` and the OAuth error page in `crates/df-web/src/oauth.rs`)
> are English HTML with no content negotiation. Localize the web interface for **English,
> Spanish, German, French, Italian and Hindi**.

The issue's "Definition of done" is the acceptance criterion verbatim:

- All six locales complete, with a check that every locale has every key — wired into
  `npm run check` so a partial translation cannot merge.
- `npm run check`, `npm run lint`, `npm test`, `cargo test` and
  `cargo clippy --all-targets -- -D warnings` all pass.
- The consent page negotiates `Accept-Language`, with a test per locale asserting the
  negotiated `lang` attribute and one translated string.
- `web/README.md` and the `web/` section of `CLAUDE.md` document where messages live and what
  adding a string now costs.

## Decisions taken before drafting

The issue left three questions open under "Design questions to settle in the PR". They were
put to the developer and answered; the answers are inputs to this spec, not assumptions.

1. **Library: Paraglide JS (inlang).** Compile-time message functions, tree-shaken, no runtime
   catalog lookup, and no SvelteKit server requirement — which is what makes it compatible with
   `adapter-static` and the SPA-for-a-security-reason rule in `web/README.md`.
2. **Persistence: server-side, on the user record.** *This overrides the issue's own
   `localStorage`-only proposal.* The choice follows the account across devices, which costs a
   `users` column, a migration and an API field. See "Where the locale lives" for how the
   round-trip cost the issue warned about is paid without a flash of the wrong language.
3. **Scope: the whole issue in one PR** — console, formatting, error-code map, the server-rendered
   pages, and all six translations together.

## Assumptions

Every choice made without asking, with its rationale.

- **`en` is the base locale and the fallback.** A locale we cannot resolve, and a message key
  missing at runtime, both render English rather than a key name. English is the language the
  product is already written in and the one the server's untranslated `message` strings are in,
  so it is the only coherent fallback.
- **The six locales are named by bare language subtags** — `en`, `es`, `de`, `fr`, `it`, `hi` —
  not region-qualified (`es-419`, `pt-BR`). The issue names languages, not regions, and adding a
  region later is a new locale file, not a rework.
- **`users.locale` is nullable, and `NULL` means "never chose".** A `NULL` is not "English"; it
  is the state where browser detection is still in charge. Defaulting the column to `'en'` would
  make a German-speaking new account permanently English-by-default and hide the difference
  between "chose English" and "has not chosen".
- **The set of valid locales is enforced in Rust, not by a `CHECK` constraint.** A `CHECK`
  listing six values makes the seventh locale a migration. `df-core::i18n` validates against one
  `SUPPORTED_LOCALES` constant and returns `Error::Invalid` naming the valid options, which is
  the house error style ("say what went wrong, what the valid options were"). See "One list, and
  who owns it" for why that constant is the only hand-written copy in the workspace.
- **Locale changes reload the document.** Paraglide's message functions are plain calls, not
  reactive reads; Svelte 5 has no dependency to invalidate when the locale changes underneath
  them. Rather than wrap the app in a `{#key}` block and hope every subtree re-renders, an
  explicit change reloads. The picker is used approximately once per account, so the cost is
  paid almost never. See "Boot and switch" for why this cannot loop.
- **`localStorage` is a paint-time cache, never the source of truth.** The server record is
  authoritative. The cache exists only so the first paint of every subsequent load is already in
  the right language instead of flashing English until `/api/me` resolves.
- **Both server-rendered pages prefer the signed-in user's stored locale over `Accept-Language`.**
  The consent page requires the console session, so the account's choice is available and is a
  better signal than the browser's header — a user who set Spanish on an English-configured work
  laptop means it. `Accept-Language` is the fallback when the account has made no choice.

  **The error page follows the identical rule, and an earlier draft of this spec got that wrong.**
  It claimed the error page "can be reached without a session" and should therefore negotiate on
  the header alone. It cannot: all four `error_page`/`error_page_html` call sites
  (`oauth.rs:196`, `:213`, `:315`, `:367`) sit downstream of `authorize_page`'s
  `CurrentUser::from_request_parts_public`, which redirects a signed-out visitor to `/login`, or
  inside `authorize_decision`, which takes `CurrentUser` as an extractor. Had the draft shipped, a
  user with `locale = 'de'` on an `Accept-Language: en-US` laptop would have got a German consent
  page and an English error page **in the same flow** — exactly the inconsistency the rule above
  exists to prevent. The locale is therefore resolved **once**, at the top of the handler, and
  passed to whichever page is rendered.
- **The `df-web` message table is hand-written Rust, not a second Paraglide project.** It is
  roughly twenty strings across two pages plus seven scope descriptions. A compiler toolchain for
  that is more moving parts than a `match` on a locale enum, and the two surfaces do not share
  keys anyway.
- **`clients.ts`: prose is translated, everything a machine reads is verbatim.** The `note` fields
  and the one prose `name` — `'Any other MCP client'`, which is a description where its siblings
  (`Claude Code`, `Cursor`) are product names — are keyed. Commands, JSON and TOML snippets, and
  `location` paths like `~/.copilot/mcp-config.json` stay exactly as they are. A translated
  `--transport http` is a broken command, and a translated config path is a file nobody has.
- **Error-code coverage is best-effort by construction.** The console maps the codes it knows and
  renders the English `message` the thrower supplied for anything else. A new server error is
  untranslated, never blank — asserted by test.
- **`WebauthnError` gains a `code`, because it has to go through the same door as `ApiError`.**
  `web/src/lib/webauthn.ts` throws six errors whose English prose is rendered verbatim by the
  login, signup and settings pages — they are the entire error surface of the first page anyone
  sees, and today they carry no code at all, so a `code`-keyed lookup could not reach them. Rather
  than key them at the throw site (which would put message functions in a module that has no other
  reason to know about locales), `WebauthnError` gets the same `code`-before-`message` contract
  `ApiError` already has. That is the existing house rule applied to the one error type that was
  exempt from it, not a new mechanism.

  **The field alone changes nothing, and the other half is the load-bearing one.** All three call
  sites read `.message` off the error directly — `e instanceof WebauthnError || e instanceof
  ApiError ? e.message : '<English literal>'` at `login/+page.svelte:44`, `signup/+page.svelte:52`
  and `settings/+page.svelte:66`. Each of those catch blocks has to be rewritten to call the
  `errors.ts` resolver, **and each `else` literal** — `'Could not load your passkeys.'`,
  `'That did not work.'`, `'Could not save that.'` — keyed like any other string. Adding the field
  and stopping there leaves the login page's whole error surface in English.

## Goal & success criteria

Make every string a human reads in a browser — the SPA and the two server-rendered pages —
available in six languages, without weakening the SPA/static constraint, the tenant isolation
rules, or the "an error has a code before it has a message" contract.

Measurable:

1. `messages/{en,es,de,fr,it,hi}.json` have identical key sets and identical placeholder sets per
   key, enforced by a check wired into `npm run check`.
2. No user-visible English literal remains anywhere a human reads it — `web/src/routes/**`,
   `web/src/lib/components/**`, **and the message-bearing modules under `web/src/lib/`**:
   `format.ts`, `clients.ts`, `webauthn.ts` and the two client-authored strings in `api.ts`.
3. `relative()`, `absolute()` and `day()` take the selected locale, never `undefined`.
4. `GET /api/me` returns `user.locale`; `PATCH /api/me` accepts and validates it; a cross-locale
   round trip is tested.
5. `GET /oauth/authorize` emits `<html lang="…">` matching the negotiated locale, with one
   asserted translated string, for each of the six.
6. All five gate commands pass.

## Scope

**In**

- All of `web/src` — routes, components, `format.ts`, `clients.ts` prose, `StatusPill` labels,
  document `<title>`s, `aria-label`s and `title` attributes.
- `web/src/lib/webauthn.ts` — its six thrown errors gain codes and keyed messages.
- `web/src/lib/api.ts` — the two messages the client itself authors (`network`, and the
  `The server answered {status}.` used when the server sent no `message`). Every other message
  rendered from an `ApiError` comes from the server and is keyed by `code`, not translated here.
- `web/src/app.html` — the shell's `lang`.
- A locale picker in `/settings`.
- A `users.locale` column, its migration, its `df-core` accessor and validation, the
  `PATCH /api/me` field, and that endpoint's entry in `crates/df-web/src/catalog.rs` — the
  catalog is the reachability gate *and* the source of the OpenAPI description, so a new field
  that is not described there is undocumented by construction.
- `crates/df-web/src/oauth.rs` — `consent_html` and `error_page_html`, plus a new
  `crates/df-web/src/i18n.rs` for negotiation and the message table.
- `web/README.md` and the `web/` section of `CLAUDE.md`.

**Out**

- **The MCP surface.** `df-mcp` tool descriptions and `df-core` error messages are written for an
  LLM caller. Translating them fragments the one audience they have, and `tests/tools.rs` asserts
  their shape. They stay English. This is the issue's own boundary and CLAUDE.md's
  "Descriptions are the documentation" convention.
- Audit event payloads, `docs/`, README content outside `web/README.md`.
- RTL and bidi. None of the six are RTL. No hard-coded `left`/`right` is *added* where a logical
  property would do; existing ones are not audited as part of this.
- Locale-aware number/currency beyond what `Intl` already gives `toLocaleString`.
- Translating the server's `message` strings themselves. The console maps `code`s; `df-web`
  keeps writing English messages, which remain the documented fallback.

## Architecture

### Where the locale lives

Three tiers, in resolution order, each with one job:

| Tier | Holds | Authority |
|---|---|---|
| `users.locale` (Postgres) | the account's explicit choice, or `NULL` | **source of truth** |
| `localStorage['df.locale']` | a copy of the above, for first paint | cache only |
| `navigator.languages` | the browser's preference | fallback when no choice was made |

`users` is a global identity table with no `org_id` — it is not a tenant table, so the two-guard
rule in CLAUDE.md ("give it a `NOT NULL org_id`, add it to `tenant_tables`…") does not apply and
no `<table>_tenant_isolation` policy is added. The column is per-user, reachable only through the
caller's own `CurrentUser`, and no handler accepts a user id from the request.

### Boot and switch

```
boot ──► locale = localStorage['df.locale'] ?? match(navigator.languages) ?? 'en'
         overwriteGetLocale(() => locale)         # synchronous, before first paint
              │
              ▼
         render  ─────────────────────────────►  GET /api/me
                                                      │
                        ┌─────────────────────────────┘
                        ▼
         me.user.locale is null      →  nothing to do
         == localStorage value       →  nothing to do
         != localStorage value       →  write cache, location.reload()   (once)
```

**The reload cannot loop.** The cache is written *before* the reload, so the next boot resolves
to exactly the value that triggered it, and the comparison that fired is false. The only way to
reload twice is for the server value to change between two loads, which requires a second device
writing a different choice — a real change, not a loop.

The picker in `/settings` runs the same path deliberately: `PATCH /api/me` → on success write the
cache → reload. The write is not applied optimistically, so a rejected locale never leaves the UI
in a language the account does not actually have.

### `PATCH /api/me` — three states, not two

`ProfileRequest` today is `Option<String>` per field with `COALESCE` semantics: absent leaves the
value alone, and there is no way to clear one. A locale needs a third state, because "match my
browser" is a real choice a user makes after having chosen Spanish once:

| Body | Meaning |
|---|---|
| `{}` — field absent | leave the stored locale alone |
| `{"locale": "de"}` | set it to German |
| `{"locale": null}` | clear it — go back to following the browser |

That is `Option<Option<String>>` behind the existing `double_option` deserializer already used by
`UpdateRepoRequest::team_id` in `crates/df-web/src/routes/repos.rs`, and for the same documented
reason: serde collapses "absent" and "explicit null" into one `None` unless the whole
deserialization is wrapped. The helper moves to a shared location rather than being duplicated.

`email` and `name` keep their current two-state behaviour — widening them is not this change.

### One list, and who owns it

Four places could plausibly hold "the six locales", which is three too many. The owner is
**`df-core::i18n`**:

| Place | How it gets the list |
|---|---|
| `df-core::i18n::{Locale, SUPPORTED_LOCALES}` | **the hand-written original** — the domain list, next to the validation that uses it |
| `df-web` | `pub use df_core::i18n::Locale` — `df-core` cannot depend on `df-web`, so `i18n.rs` in `df-web` holds only the HTTP concern (`negotiate`) and the two pages' message table |
| `web/project.inlang/settings.json` | hand-written, and the one copy that cannot be a `use` statement — it is JSON read by a compiler in another language |
| `web/src/lib/locale.svelte.ts` | `import { locales } from '$lib/paraglide/runtime'` — **generated from the settings file**, never re-typed |

That leaves exactly two hand-written lists, in two languages, and a drift between them is a real
failure: a locale offered in the `/settings` picker but missing from `SUPPORTED_LOCALES` is a
`400` on click, and the reverse is a translation nobody can reach. So a `df-core` test reads
`web/project.inlang/settings.json` off disk and asserts the two agree.

This is a **stronger** guard than the one the repo already uses for the `API_PREFIXES` /
`worker/index.ts` pair, not the same one — worth being precise about, because the weaker pattern
would not work here. That pair has a test on each half, but neither reads the other's file;
`worker/index.test.ts` re-types the list, and both tests assert *behaviour*. Mirrored constants
catch a prefix drift because there are two behavioural surfaces to disagree. Here there is only
one, so nothing would fail: a locale added to `settings.json` and not to `SUPPORTED_LOCALES` is
simply a `400` nobody wrote a test for. Hence the file read.

**The path has to be `concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/project.inlang/settings.json")`.**
`cargo test` runs the binary with its working directory at the *package* root, so the obvious
`fs::read_to_string("web/project.inlang/settings.json")` is the implementation that fails. CI runs
`cargo test --workspace` from a full checkout and the Dockerfile only ever runs `cargo build`, so
nothing compiles `df-core`'s tests without `web/` on disk.

### `web/` module layout

| Path | Responsibility |
|---|---|
| `project.inlang/settings.json` | base locale `en`, the six locales, the message-file plugin |
| `messages/{en,es,de,fr,it,hi}.json` | the catalogs, Inlang Message Format |
| `src/lib/paraglide/**` | **generated**, git-ignored, emitted with `--emit-ts-declarations` by the Vite plugin and by an explicit compile step in `check`/`build` |
| `src/lib/locale.svelte.ts` | resolution, the cache, `SUPPORTED`, `applyLocale`, the `<html lang>` effect's source |
| `src/lib/errors.ts` | `ApiError.code` **and `WebauthnError.code`** → message function, with the thrower's English `message` as the fallback |
| `scripts/check-messages.mjs` | key-set and placeholder-set equality across catalogs |

Messages are addressed as `m.some_key()` from `$lib/paraglide/messages`. Paraglide's compiler
fails on an unknown variable reference inside a pattern, and — **only when the compiler is run
with `--emit-ts-declarations`** — the generated `.d.ts` files make a renamed or deleted key a
type error. Verified: without the flag no declarations are emitted at all and a bogus key type-checks
clean; with it, `m.this_key_does_not_exist()` fails as `TS2339`. The flag is therefore not a
nicety, it is what satisfies the issue's requirement that "message keys must be type-checked, so a
missing or renamed key fails `npm run check` rather than rendering a key name to a customer".

What the compiler does *not* catch, with or without the flag, is a key present in `en` and absent
in `hi`: it silently falls back to the base locale. Verified — a key deleted from `messages/de.json`
still returns the English string from `m.key({}, { locale: 'de' })`, with no warning at compile or
run time. `scripts/check-messages.mjs` is the guard for exactly that, and it is the reason the
issue's "a partial translation cannot merge" is satisfiable at all.

Plurals are expressed in the message, not at the call site. **Verified empirically against
`@inlang/paraglide-js@2.25.0` before this spec was finalized** — the ICU one-liner
(`"{count, plural, one {# job} other {# jobs}}"`) that the library's comparison docs show belongs
to a different plugin and compiles to the literal string `undefined other }` here. The Inlang
Message Format's variant form is what this plugin accepts:

```json
{
  "queue_jobs_count": [{
    "declarations": ["input count", "local countPlural = count: plural"],
    "selectors": ["countPlural"],
    "match": { "countPlural=one": "# job", "countPlural=other": "# jobs" }
  }]
}
```

This is what retires `plural(count, one, many)`. Its two-form signature cannot express Hindi's or
the Romance languages' categories, and pushing the choice into the catalog lets each locale
declare the categories `Intl.PluralRules` actually gives it.

**The required category set differs per locale, and that is the subtlety the completeness check
has to survive.** Measured: `en`, `de` and `hi` need `one`/`other`; `es`, `fr` and `it` also need
`many` (Spanish `1 000 000` is "un millón **de** trabajos", not "1000000 trabajos"). A naive
key-set-equality check would either reject the correct Spanish catalog or accept a French one
missing `many`. `check-messages.mjs` therefore compares **message keys** for exact equality and,
for a variant message, requires each locale to cover exactly the categories
`Intl.PluralRules(locale).resolvedOptions().pluralCategories` reports for it.

### `<html lang>`

`app.html` keeps `lang="en"` and **is not templated per locale**. `adapter-static` emits one
`index.html` shell for every route; there is no build-time locale to bake in and no server to pick
one. The attribute is set from script at boot — `document.documentElement.lang = locale` — in the
same synchronous step that resolves the locale, before the first paint.

The consequence is worth stating because issue #42's design question 3 is about screen readers:
the *served* markup says `lang="en"` for the moment between parse and hydration. A screen reader
announcing content before script runs would use English phonetics for that instant. This is
unavoidable under `adapter-static` without reintroducing a server, and it is the same trade the
SPA decision already makes for every other piece of content on the page — the shell is empty until
JS runs, so there is nothing to mispronounce yet.

### `format.ts`

`relative()` and `day()` currently pass `undefined` as the locale and `absolute()` passes no
argument at all — behaviourally the same thing, the browser's locale, but the edit differs (one
replaces an argument, the other adds one). They take the resolved app locale instead, read from `locale.svelte.ts`. The
functions stay pure — the locale is read through an accessor at call time rather than captured —
so a `.ts` module does not need to become a `.svelte.ts` one.

`day()`'s existing comment about `new Date('2026-09-01')` parsing as UTC midnight stays true and
the split-the-parts implementation is unchanged; only the locale argument moves.

`person(name, email)`'s `'Unnamed account'` becomes a message key.

### `StatusPill`

`JobStatus` values (`pending`, `in-progress`, `active`, `completed`, `failed`) are wire values and
do not change. The component gains a label lookup alongside its existing `tones` map. Colour *and*
the word remains the rule — the word is now a translated word.

### Server-rendered pages

New `crates/df-web/src/i18n.rs`:

- `pub use df_core::i18n::Locale;` — the enum is **defined in `df-core`** (see "One list, and who
  owns it"); `df-web` only re-exports it. `df-core` cannot depend on `df-web`, so this direction is
  the only one available, and it is also the right one: the list is a domain fact, not an HTTP one.
- `negotiate(accept_language: Option<&str>) -> Locale` — RFC 9110 `Accept-Language` parsing:
  split on `,`, take `;q=` weights (default `1.0`), ignore malformed entries rather than failing,
  match on the primary subtag so `es-419` selects `es`, sort by descending weight, first
  supported wins, else `en`.
- A message table: `fn msg(locale: Locale, key: Key) -> &'static str`, exhaustive over both, so a
  new key without a translation is a non-compiling `match` rather than a blank page.

`consent_html` and `error_page_html` take a `Locale` and emit `<html lang="…">` — which neither
does today; both currently start `<!doctype html><meta charset=utf-8>` with no `<html>` element
at all. `scope_description` becomes locale-aware and keeps its existing property that an
unlisted scope renders as its bare name and fails a test.

The locale is resolved **once per request**, at the top of `authorize_page` and
`authorize_decision` — signed-in user's `locale` if set, else `negotiate(Accept-Language)` — and
passed to whichever page renders. **Both pages, one rule.** See the assumption above for why the
draft's "the error page negotiates on the header alone" was wrong and what it would have produced.

**The error page is only partly translatable, and that is deliberate.** `error_page(&e)` renders
`escape(&detail)` straight off an `AuthError`, and translating the server's own message strings is
explicitly out of scope. So three of the four call sites emit a localized `<html lang>`, title and
closing note around an English `AuthError` body; the fourth (`oauth.rs:213`, "No organization yet")
is client-authored and fully translated. Stated here so a reviewer reads it as the scope boundary
it is, rather than as a half-finished job.

## Error handling & edge cases

- **Unknown locale from the server.** `users.locale` is validated on write, but a value that
  predates a locale being removed would fail to parse. The console falls back to detection; the
  server falls back to `en`. Neither throws.
- **Missing message key at runtime.** Paraglide falls back to the base locale. This is the
  designed-for path for a race between a deploy and a cached bundle; `check-messages.mjs` is what
  keeps it from being the *normal* path.
- **Unknown `ApiError.code`.** Render the server's English `message`. Asserted by a test that
  feeds a fabricated code and checks something non-empty renders — the issue calls this out
  explicitly and it is the reason the code/message split exists.
- **`Accept-Language: *`**, absent, empty, or entirely malformed → `en`.
- **`q=0`** on a locale means "not acceptable" and is excluded, not treated as weight zero and
  ranked last.
- **`PATCH /api/me` with an invalid locale** → `400 invalid_argument`, message naming the six.
  The console does not apply it locally, so nothing to roll back.
- **`localStorage` unavailable** (private mode, blocked): reads and writes are wrapped; the app
  falls back to detection on every load and re-reloads at most once per load. Degraded, not broken.
- **German text expansion.** ~30% longer than English. Nav, buttons, table headers and the
  `StatusPill` are audited at `de` and fixed where they break; that is part of this change, per
  the issue, not a follow-up.

## Testing approach

| Gate | What it proves |
|---|---|
| `node scripts/check-messages.mjs` (in `npm run check`) | every locale has every key, with matching placeholders |
| `npm run check` | svelte-check + tsc; a renamed or missing key is a type error |
| `npm run lint` | prettier over `web/`, including the catalogs |
| `npm test` | existing vitest (Worker routing) plus new unit tests for locale resolution, the reload-cannot-loop guard, and the error-code fallback |
| `npm run build` | the static bundle still builds with the Paraglide plugin |
| `cargo test` | `negotiate()` table tests; a consent-page test **per locale** asserting `lang="xx"` and one translated string; a second consent test proving the *stored* locale outranks the header; `users.locale` round trip through `PATCH /api/me`; an invalid-locale rejection; the `SUPPORTED_LOCALES` ↔ `settings.json` drift guard |
| `cargo clippy --all-targets -- -D warnings` | — |

New Rust tests live beside the existing ones in `crates/df-web/tests/oauth_http.rs` and
`crates/df-web/tests/console.rs`. `users` is not a tenant table, so no cross-org negative test is
required for the new column — stated explicitly rather than skipped silently.

**The per-locale consent test has to use a fixture user whose `locale` is `NULL`, or it tests the
wrong thing.** Resolution is *stored locale, then the header*; a fixture that happens to have a
locale set would satisfy the assertion without `negotiate()` ever running, and the issue's headline
requirement — "the consent page negotiates `Accept-Language`" — would pass while measuring nothing.
The stored-locale-wins test is the deliberate mirror of it and needs a user who *has* one.

`src/lib/paraglide/**` is generated and git-ignored, so it goes in `web/.prettierignore` alongside
`.svelte-kit` and `build`. Without that, `npm run lint` fails after every fresh compile — on code
no human wrote.

## Out-of-band artifacts

- **One migration**, `0018_user_locale.sql`, additive and forward-only: a nullable column with no
  default and no backfill. It is safe to apply before the code that reads it and safe to leave in
  place if the code is rolled back, which is what makes it deployable independently.
- No config surface, no secret, no grant, no feature flag, no generated code committed.

## Risks & open questions

- **Translation quality is not machine-verifiable.** The gates prove every key exists in every
  locale with the right placeholders; they cannot prove the Hindi reads well. The catalogs are
  written to be reviewable — one file per locale, keys in the same order — so a speaker can
  diff-read one file rather than hunt through components. Flagged for review, not solved.
- **Paraglide's generated output is git-ignored**, so `npm run check` and `npm run build` both
  have to compile first. If a contributor runs `svelte-check` directly they will see missing-module
  errors. Documented in `web/README.md`; the alternative — committing generated code — drifts.
- **`setLocale`/reload versus in-place reactivity.** The reload is a deliberate simplification. If
  a future surface needs live switching without a reload, this decision is the thing to revisit.
- **`users.locale` is readable by any handler holding a `CurrentUser`.** It is a display
  preference, not a secret, and it is already implied by the rendered page. Noted so the choice is
  on the record.
