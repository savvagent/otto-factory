# Queue poller — the queue page keeps itself current, keyed on filters too

## Goal

Migrate `web/src/routes/o/[org]/queue/+page.svelte`'s job-list fetch from a one-shot `$effect` (with
a hand-rolled `latest`/`seq` staleness counter) to `Poller<Job[]>` (`web/src/lib/poll.svelte.ts`),
closing `savvagent/otto-factory#104`. Per `savvagent/otto-factory#92`, the poll's restart condition
must key on the org **and** all four filter values (`status`, `repo`, `team`, `mine`) — not the org
alone — or a filter change would leave a stale poll running against the filter set the reader just
left.

**Spec:** `docs/specs/2026-09-10-queue-poller-design.md` — read it first. This plan implements it
exactly.

## Status — 2026-09-10

🚧 In progress.

## Global Constraints

- `web/` conventions: Svelte 5 runes only (`$state`/`$derived`/`$props`/`$effect`), no Svelte 4
  stores, no `export let`. `adapter-static` — no SvelteKit server-side code.
- A page that shows live state polls through `Poller`, never a bare `setInterval` — the six rules
  in `CLAUDE.md` and `poll.svelte.ts`'s own doc comment: refresh ≠ load; a failed refresh keeps the
  last good data with an age indicator; a fatal failure (401/404/403) stops the poll; a hidden tab
  doesn't poll; refreshes never overlap and back off on failure; a long-idle tab parks itself.
- The console ships in six languages (`en`, `es`, `de`, `fr`, `it`, `hi`); every new message key
  needs an entry in all six `web/messages/*.json` catalogs, or `npm run check`'s
  `scripts/check-messages.mjs` step fails.
- No AI self-attribution anywhere (commits, comments, docs, PR body).
- Run `cd web && npm run lint -- --write` (prettier) before committing, then re-run `npm run lint`
  to confirm clean.
- This is a `web/`-only change: no SQL, no MCP tool, no console route, no migration, no `OF_*`
  config key. Tenant isolation and metering rules do not apply — no cross-org test and no
  `of-billing::classify` step are owed. State this explicitly in the PR body rather than silently
  omitting the checklist items.
- Gates: `cd web && npm run check` (svelte-check + tsc + the message-catalog check), `npm run lint`
  (prettier), `npm test` (vitest), `npm run build`.
- No out-of-band artifact beyond the console bundle itself is touched (no `Dockerfile`/`fly.toml`,
  no `web/worker/`, no migration) — state that explicitly in the PR body.

## File Structure

| File                                                          | Responsibility                                                                                                                                                 |
| --------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `web/src/routes/o/[org]/queue/+page.svelte`                     | **Modify.** Job-list `$effect` rewired through `Poller<Job[]>`, keyed on `org.slug` plus a `$derived` over the four filters; render tree gains `stale`/`parked`/`stopped` branches. |
| `web/messages/en.json`, `es.json`, `de.json`, `fr.json`, `it.json`, `hi.json` | **Modify.** Add `queue_refresh_failed`, `queue_paused`, `queue_retrying` to each, mirroring the existing `overview_*` triad.                                    |
| `web/src/routes/o/[org]/queue/page.render.test.ts`              | **Modify.** Add poller-behavior cases alongside the existing filter-navigation tests (neither set is removed).                                                 |
| `web/README.md`                                                 | **Modify.** One Layout-table row: `poll.svelte.ts` now used by the overview *and* `/queue`.                                                                     |

## Task Order & Rationale

Single task. The page rewrite, the six catalog edits, and the test additions are one coherent unit —
there is no useful checkpoint between "the page still compiles with the old fetch" and "the page
compiles with the new one," since the render tree reads from the same `jobs`/`loading`/`error`
names either way. The README edit is a one-line tail on the same commit sequence.

## Task 1 — Migrate the queue's job-list fetch to `Poller`, keyed on org + filters ⬜

**Files:** `web/src/routes/o/[org]/queue/+page.svelte` (modify), `web/messages/*.json` (modify, all
six), `web/src/routes/o/[org]/queue/page.render.test.ts` (modify), `web/README.md` (modify)
**Interfaces:** Consumes `Poller` from `$lib/poll.svelte`, `ApiError` from `$lib/api`, `session`
from `$lib/session.svelte` (new imports in `+page.svelte`, mirroring `o/[org]/+page.svelte`'s
existing imports). Produces no new public interface — this is page-internal state only.

- [ ] **Add the three message keys to all six catalogs first**, so the page can reference them
      before the catalog check runs. In `web/messages/en.json`, add near the existing `queue_*`
      keys:
      ```json
      "queue_refresh_failed": "Refresh failed. {reason} Showing the result from {age}.",
      "queue_paused": "Updates paused while you were away. Interact with the page to resume.",
      "queue_retrying": "Still retrying in the background."
      ```
      (Identical wording to `overview_refresh_failed`/`overview_paused`/`overview_retrying` — the
      failure semantics are the same, just for a table instead of tiles.) Add the matching
      translated triad to `es.json`, `de.json`, `fr.json`, `it.json`, `hi.json`, copying the
      existing `overview_refresh_failed`/`overview_paused`/`overview_retrying` translations in each
      file verbatim (same reason, same wording, different key name) — do not re-translate from
      scratch. Confirm `{reason}`/`{age}` placeholders are present in every locale's
      `queue_refresh_failed` entry.
- [ ] Run `cd web && npm run check` now, before touching the page — confirm it still passes (the
      new keys are unused so far, which `check-messages.mjs` does not flag) and that no locale was
      missed.
- [ ] **In `web/src/routes/o/[org]/queue/+page.svelte`, replace the job-list `$effect` and its
      supporting state.** Remove the `let jobs = $state<Job[]>([])`, `let loading = $state(true)`,
      `let error = $state<string | undefined>(undefined)` triad and the `let latest = 0` counter
      plus the `$effect` that uses them (the whole block from the `latest` declaration through the
      end of the second `$effect`, per the spec's §1). Add:
      ```ts
      import { Poller } from '$lib/poll.svelte';
      import { ApiError } from '$lib/api';
      import { session } from '$lib/session.svelte';

      const jobsPoll = new Poller<Job[]>();

      const filters = $derived({ status, repo, team, mine, limit: 200 });

      $effect(() => {
        const slug = org.slug;
        if (!slug) return;
        const active = filters;
        return jobsPoll.start(() => api.jobs(slug, active), { fatal });
      });

      function fatal(failure: unknown): boolean {
        if (!(failure instanceof ApiError)) return false;
        if (failure.isUnauthenticated) {
          session.clear();
          return true;
        }
        return failure.isNotFound || failure.status === 403;
      }

      const jobs = $derived(jobsPoll.value ?? []);
      const loading = $derived(!jobsPoll.value);
      const error = $derived(
        jobsPoll.failed ? messageFor(jobsPoll.error, m.queue_load_failed()) : undefined
      );
      ```
      Keep the existing `error` variable name and the `loading`/`jobs` names unchanged so the render
      tree below needs no further edits for its existing branches — only the new `stale`/`parked`
      branches are additions. Import `relative` is already imported (used elsewhere on the page for
      `job.createdAt`); reuse it for `jobsPoll.updatedAt`.
- [ ] **Add the `stale`/`parked` status line to the render tree**, inside the `{:else}` branch that
      currently renders the table directly (i.e. once `error` is falsy and `loading` is false),
      immediately before the existing `{#if error}...{:else if loading}...{:else}` table markup —
      per the spec's §2, `role="status"` on both lines so a screen reader is told:
      ```svelte
      {#if jobsPoll.parked}
        <p role="status" class="text-xs text-faint">{m.queue_paused()}</p>
      {:else if jobsPoll.stale}
        <p role="status" class="text-xs text-warn">
          {m.queue_refresh_failed({
            reason: messageFor(jobsPoll.error, m.error_network()),
            age: relative(
              jobsPoll.updatedAt === undefined
                ? undefined
                : new Date(jobsPoll.updatedAt).toISOString()
            )
          })}
        </p>
      {/if}
      ```
      And append the retrying note to the existing `{#if error}<Alert>...` branch:
      ```svelte
      {#if error}
        <Alert>
          {error}
          {#if !jobsPoll.stopped}{m.queue_retrying()}{/if}
        </Alert>
      {:else if loading}
      ```
      Confirm `m.error_network` already exists (it is used by the overview page) rather than adding
      a new key for it.
- [ ] **Type-check and build the page in isolation**: `cd web && npm run check`. Fix any TS error
      before moving on (in particular: `ApiError`'s exported shape, `session.clear()`'s signature —
      both already used identically in `o/[org]/+page.svelte`, so mirror it exactly rather than
      re-deriving the types).
- [ ] **Write the new test cases first (failing), then confirm the implementation above makes them
      pass** — add to `web/src/routes/o/[org]/queue/page.render.test.ts`, alongside (not replacing)
      the existing `describe('the queue filters', ...)` block. Follow
      `o/[org]/page.render.test.ts`'s pattern exactly (a `serve(healthy: () => boolean)` fetch stub,
      `vi.useFakeTimers()`, `REFRESH_INTERVAL` imported from `$lib/poll.svelte`, `vi.waitFor` for the
      real-`Response`-body race described in that file's comments):
      - `describe('the queue poller', ...)`:
        - **"keeps the table and shows the stale note when a refresh fails, then clears it on
          recovery"** — mount with a healthy stub returning one job, `settle()`, assert the job's
          title renders and no `[role="status"]` exists; flip to unhealthy (502), advance
          `REFRESH_INTERVAL * 1.4`, `vi.waitFor` a `[role="status"]` containing "Refresh failed"
          while the job's title still renders; flip back healthy, advance `REFRESH_INTERVAL * 4`,
          `vi.waitFor` the status node gone and the title still present.
        - **"stops polling and shows the error in place of the table on a 404"** — mount with a
          stub that always 404s, `settle()`, assert the table is absent, an `Alert`-rendered error
          is present, and advancing the clock by several intervals produces no further `fetch`
          calls beyond the first (`fetchMock.mock.calls.length` stays at 1) — this is the
          `jobsPoll.stopped` behavior; the existing job-render assertions from the healthy case
          must NOT appear.
        - **"restarts the poll when a filter changes, and drops a late response for the old
          filter"** — mount with `page.url` returning the unfiltered queue path; capture the first
          `fetch` call's URL. Using the same `Object.defineProperty(page, 'url', ...)` override the
          existing "Clear filters" test already uses, change `page.url` to
          `?status=pending`, trigger a re-render (re-`mount` a second `Harness` instance, or force
          the effect to re-run the way the existing filter tests do — match whichever technique
          those tests use rather than inventing a third), and assert the newest `fetch` call's URL
          contains `status=pending` while a response for the pre-change URL that resolves after the
          switch does not overwrite the rendered table (assert on the *last* rendered content, not
          call count, since `Poller`'s own generation guard is what's under test here transitively).
      Run `cd web && npm test -- queue` and confirm the new cases fail against the still-unmodified
      page (if Task order was followed and the page is already migrated by this point, run the new
      tests against a `git stash` of the page changes first to confirm they fail for the right
      reason, then restore).
- [ ] Run `cd web && npm test` (full suite, not just `queue`) and confirm everything passes,
      including the untouched existing two filter-navigation tests in the same file.
- [ ] **Update `web/README.md`'s Layout table.** Change the `poll.svelte.ts` row from "Polling a
      page hands to an `$effect`. The overview uses it; `/queue` and `/repos` should." to "Polling
      a page hands to an `$effect`. The overview and `/queue` use it; `/repos` should."
- [ ] **Format and commit.**
      ```bash
      cd web && npm run lint -- --write && npm run lint
      cd .. && git add web/src/routes/o/\[org\]/queue/+page.svelte web/messages/*.json \
        web/src/routes/o/\[org\]/queue/page.render.test.ts web/README.md
      git commit -m "web: migrate the queue page's job list to Poller, keyed on filters too"
      ```
- [ ] **Full gate, once more, from a clean state**: `cd web && npm run check && npm run lint && npm test && npm run build`.
