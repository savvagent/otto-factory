# Overview polling — the console overview keeps itself current

## Goal

Make `/o/{org}` refresh itself every 30 seconds instead of rendering whatever was true when the tab
was opened, without becoming worse than the static page it replaces: no skeleton flash on a refresh,
no working dashboard thrown away by one failed request, no polling behind a hidden tab, and no
overlapping requests.

## Status — 2026-09-09

✅ Shipped in `savvagent/otto-factory#58` (squashed to `6fdc7c7`), closing
`savvagent/otto-factory#57`. All four tasks are done and the console gates were green at merge.

One finding was deliberately not fixed here and is tracked instead: the tick repeats two unbounded
per-org queries (`Jobs::stats`, `list_repos`), which is server-side work with its own design —
`savvagent/otto-factory#61`.

**This spec and plan were written alongside the pull request rather than ahead of it** — the change
was implemented before the plan-by-plan discipline was applied to it, and the documents were
back-filled to bring it under the same record as every other change in `docs/plans/`. The task steps
below describe the work as it was actually done, in the order it was actually done; nothing here is
a reconstruction of a path not taken.

**Spec:** `docs/specs/2026-09-09-overview-polling-design.md` — read it first. This plan implements it
exactly.

## Global Constraints

- Client-side only. No SQL, no MCP tool, no console route, no migration, no `OF_*` config key — so
  the tenant-isolation, metering, and public-interface rules have nothing to attach to: no cross-org
  negative test, no `of-billing::classify` entry, no `docs/clients/matrix.md` change.
- `web/` conventions: Svelte 5 runes only (`$state` / `$derived` / `$props` / `$effect`), no Svelte 4
  stores, no `export let`; shared reactive state lives in a `.svelte.ts` module; the org slug is read
  from the route on every access, never copied into state.
- A new user-visible string costs six catalog entries. `npm run check` runs
  `scripts/check-messages.mjs` first and fails on a key missing from any locale.
- No AI self-attribution anywhere — commits, comments, docs, PR body.
- Gates, all four, before the PR: `npm run check`, `npm run lint`, `npm test`, `npm run build`.
  They need `cd web && npm install` first — a fresh worktree has no `node_modules`, and every gate
  fails as `command not found` without it.
- Out-of-band artifacts: this touches the **console bundle** (`web/`), so `npm run build` is a
  required gate rather than an optional one. It does not touch the container image, the Cloudflare
  Worker (`web/worker/`), or `crates/of-core/migrations/`.

## File Structure

| File                                    | Responsibility                                                          |
| --------------------------------------- | ----------------------------------------------------------------------- |
| `web/src/lib/poll.svelte.ts`            | **Create.** `Poller<T>`, the `Visibility` seam, `REFRESH_INTERVAL`.     |
| `web/src/lib/poll.svelte.test.ts`       | **Create.** Vitest coverage of the four rules and the generation guard. |
| `web/src/routes/o/[org]/+page.svelte`   | **Modify.** Fetch through the poller; render the stale marker.          |
| `web/messages/{en,es,de,fr,it,hi}.json` | **Modify.** Add `overview_refresh_failed`.                              |

## Task Order & Rationale

Two tasks. The helper is written and proven first, against its own tests, because it is the part
with a state machine in it — generations, the in-flight guard, the visibility subscription — and
proving that through the page would mean asserting a timing rule through four mocked endpoints and a
context provider. The page then becomes a mechanical rewiring with nothing left to discover.

## Task 1 — `Poller<T>` and its tests ✅

**Files:** `web/src/lib/poll.svelte.ts` (new), `web/src/lib/poll.svelte.test.ts` (new)
**Interfaces:** produces `Poller<T>`, `Visibility`, `PollOptions`, `REFRESH_INTERVAL`; consumes
nothing from the app — the loader is a callback the caller supplies.

- [x] Write `web/src/lib/poll.svelte.test.ts` first, with `vi.useFakeTimers()` and a fake
      `Visibility` (`hidden()` plus a listener set the test drives), covering: first load sets
      `loading` and a refresh does not; a first-load failure sets `error` and later recovers; a
      failed refresh keeps `value` and sets `stale`, cleared by the next success; a hidden tab does
      not poll and refreshes on becoming visible, resuming its interval; going hidden mid-flight
      still parks the poll; concurrent `refresh()` issues one request; a slow refresh delays rather
      than stacks the next tick; teardown clears the timer and unsubscribes; a superseded
      subscription's response is dropped and its replacement still runs.
- [x] Run `cd web && npx vitest run src/lib/poll.svelte.test.ts` — expect failure (no module yet).
- [x] Implement `web/src/lib/poll.svelte.ts` per spec §§1–4: `$state` fields `value` / `loading` /
      `error` / `stale`; `start(load, options)` returning its teardown; `refresh()`; a private
      `#generation` counter bumped on start and teardown and checked before every state write; a
      chained `setTimeout` (never `setInterval`) scheduled only when the tab is visible; the
      `Visibility` seam defaulting to `documentVisibility`.
- [x] Run `cd web && npx vitest run src/lib/poll.svelte.test.ts` — expect all cases green.
- [x] Format (`npm run format`) and commit with Task 2 — one logical change; the module has no
      caller until the page has one.

## Task 2 — Wire the overview and add the stale marker ✅

**Files:** `web/src/routes/o/[org]/+page.svelte` (modify), `web/messages/{en,es,de,fr,it,hi}.json`
(modify)
**Interfaces:** consumes `Poller<T>` from Task 1 and the four existing API calls (`api.queueStats`,
`api.jobs`, `api.repos`, `api.usage`); produces no new interface.

- [x] Add `overview_refresh_failed` to all six catalogs, in the `overview_*` block after
      `overview_load_failed`, preserving each catalog's key order.
- [x] Run `cd web && npm run check:messages` — must report all six locales complete.
- [x] Replace the page's four `$state` fields and its fetch-once `$effect` with a single
      `Poller<Overview>`, the effect returning `overview.start(...)` as its teardown, and
      `$derived` accessors for `stats` / `recent` / `repos` / `usage` / `error`.
- [x] Keep all four requests in one `Promise.all` inside the loader (spec Assumptions: staggering
      them would let the tiles and the job list describe two different instants).
- [x] Change the loading branch from `loading && !stats` to `overview.loading` — `Poller.loading` is
      already first-load-only, and the companion condition would now hide the skeleton it needs.
- [x] Render `m.overview_refresh_failed()` in the page header, in warn tone, only while
      `overview.stale`.
- [x] Run `cd web && npm run check` — svelte-check, tsc over `worker/`, and the catalog check.
- [x] Run `cd web && npm test` — the whole vitest suite, not just the new file.
- [x] Run `cd web && npm run lint` and `cd web && npm run build`.
- [x] Commit: `git commit -m "web: poll the overview every 30 seconds"` — no attribution trailer of
      any kind.

## Task 3 — Documents and review ✅

**Files:** `docs/specs/2026-09-09-overview-polling-design.md` (new), this plan (new)
**Interfaces:** none.

- [x] Write the design spec and this plan; commit them onto the PR branch.
- [x] Dispatch the mandatory review trio — a Rust expert, an architect, and an independent security
      reviewer that receives only the diff — plus the conditional reviewers the diff triggers
      (code review, silent-failure, test-coverage, comment, and type-design analysis) and the spec
      and plan critiques.
- [x] Aggregate the reports into one PR comment grouped Critical / Important / Suggestions /
      Strengths, then fix or explicitly dismiss every Critical and Important finding.

## Task 4 — Address the review ✅

**Files:** `web/src/lib/poll.svelte.ts`, `web/src/lib/poll.svelte.test.ts` (rewritten),
`web/src/lib/poll.dom.test.ts` (new), `web/src/routes/o/[org]/+page.svelte`,
`web/src/routes/o/[org]/OrgPageHarness.svelte` (new),
`web/src/routes/o/[org]/page.render.test.ts` (new), `web/messages/*.json`, `web/README.md`,
`CLAUDE.md`, and the spec
**Interfaces:** `PollOptions` gains `timeout`, `idleAfter`, `fatal`, `activity`, and `random`;
`Poller` gains `failed`, `failures`, `updatedAt`, `parked`, `stopped`; `refresh()` returns
`Promise<boolean>`; the four reactive fields become getters over private `$state`.

- [x] **Terminal vs transient failures** (security, architect, silent-failure, all agreeing): add
      the `fatal` predicate, stop the poll on one, and have the page classify `401` (clearing the
      session so the root layout routes to `/login`), `403`, and `404`. Spec §5.
- [x] **`refresh()` after teardown could resurrect the previous org's poll** and arm a timer nothing
      could stop: clear `#load` in `#stop()`, and return whether a refresh actually ran.
- [x] **A hung `load` left a dead poll wearing a healthy page's face**: fail it with `PollTimeout`
      after `LOAD_TIMEOUT`.
- [x] **An outage was met at full rate by every open tab**: exponential backoff to an 8× cap with
      ±15% jitter, reset on success.
- [x] **An open tab defeated the server's 14-day idle session deadline** (architect, Critical): park
      the poll after `IDLE_AFTER` with no user input, resume on any. Spec §6.
- [x] **A failure that rejects with `undefined` rendered as a confident empty dashboard**: `failed`
      becomes a flag rather than the `error !== undefined` sentinel.
- [x] **The stale line said nothing useful**: it now carries the server's own sentence and the age
      of the data, and it is a `role="status"` live region.
- [x] **`documentVisibility` answered "visible" where it could not tell**: the no-`document` answer
      is now "hidden", and every unit test injects both seams.
- [x] Comment corrections from the comment review: the `LocaleStore` comparison (same seam, not the
      same shape), the test file's rule count, the staggering rationale (what makes the page
      coherent is one `Promise.all` landing as one value, not simultaneity), and the billing note,
      which is a fact about the caller rather than the module.
- [x] Test the mutations that survived the first suite: the guarded teardown, the superseded
      rejection, `start` clearing value/error/stale, `#clearTimer` on teardown (via
      `vi.getTimerCount()`), the `interval` option, and `refresh()` itself.
- [x] Add `web/src/lib/poll.dom.test.ts` (jsdom) for the shipped seams and
      `web/src/routes/o/[org]/page.render.test.ts` for the stale banner and the error branch.
- [x] Record the convention: a Layout row in `web/README.md` and a sixth bullet in the root
      `CLAUDE.md`'s `web/` section.
- [x] Re-run all four gates, then commit as `web: distinguish fatal from transient poll failures`.
