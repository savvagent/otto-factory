# Back-to-platform link — a persistent way from the console back to the otto platform

**Goal:** add a "Back to otto" link to the console header, built from the platform address the
server reports at runtime, visible signed in and signed out. Closes savvagent/otto-factory#212.

**Spec:** `docs/specs/2026-10-09-back-to-platform-link-design.md` — read it first. This plan
implements it exactly.

## Status — 2026-10-09

⬜ Task 1 not started.

## Global constraints

- `web/` only. No server change, no new route, no new config.
- Never hard-code the platform URL; use `session.platformUrl` or `/.well-known`.
- Runes only. Every new string in all six `web/messages/*.json`, translated; "otto" verbatim.
- No AI self-attribution anywhere.
- Gates: `cd web && npm run check && npm run lint && npm test && npm run build`.

## File structure

| File | Responsibility |
|---|---|
| **Modify.** `web/src/lib/platform.ts` | `platformHome`, `discoverPlatformUrl` |
| **Modify.** `web/src/lib/platform.test.ts` | their unit tests |
| **Modify.** `web/src/routes/+layout.svelte` | the header link and the discovery effect |
| **Modify.** `web/messages/{en,es,de,fr,it,hi}.json` | `nav_back_to_platform`, `nav_back_to_platform_title` |

## Task 1 — Back-to-platform link ⬜

**Files:** as above. **Interfaces:** produces `platformHome(url?: string): string | undefined`
and `discoverPlatformUrl(fetcher?: typeof fetch): Promise<string | undefined>`.

- [ ] Write failing tests in `platform.test.ts` for `platformHome` (http/https accepted, trailing
      slash stripped, path kept; `javascript:`, relative, empty, undefined refused) and
      `discoverPlatformUrl` (first `authorization_servers` entry; `undefined` on non-2xx, bad
      JSON, missing/empty array, non-http URL, thrown fetch).
- [ ] `cd web && npx vitest run src/lib/platform.test.ts` — expect failures (missing exports).
- [ ] Implement both in `platform.ts`. Re-run — expect pass.
- [ ] Add the catalog keys to all six locales.
- [ ] In `+layout.svelte`: `discovered = $state<string | undefined>()`; an `$effect` that, once
      `session.ready && !session.signedIn` (or `fatal`), calls `discoverPlatformUrl()` once;
      `home = $derived(platformHome(session.platformUrl) ?? discovered)`; render the link first
      in the right-hand group when `home` is set.
- [ ] Out-of-band: console bundle — `npm run check && npm run lint && npm test && npm run build`;
      confirm no platform host appears in `web/build` (`grep -r savvagent web/build` is empty).
- [ ] Visual check signed out against `npm run dev` (Playwright screenshot).
- [ ] Format (`npm run format`) and commit: `web: add a back-to-otto link to the console header`.
