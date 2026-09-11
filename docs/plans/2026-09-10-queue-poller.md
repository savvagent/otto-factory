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

✅ Shipped in `savvagent/otto-factory#146`, closing `savvagent/otto-factory#104` and
`savvagent/otto-factory#92`. Merged as `867c10b`; the merge commit's own CI run
(`34554517396`) is green on `rust` and `web`. Task 1 went through three PR-review fix rounds
(the mandatory Rust/architect/security trio plus Copilot) beyond what this plan's steps
describe verbatim — most notably splitting `navError`/`pollError` into two independent
notices rather than merging them, hoisting the stale/parked note above the `Empty`/table
split so it is reachable on an empty result, and extracting the shared `fatalApiFailure`
classifier into `web/src/lib/poll-fatal.ts` (also adopted by the overview page). See the
design spec's §2a for the corrected account of what shipped.

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
| `web/src/routes/o/[org]/queue/+page.svelte`                     | **Modify.** Job-list `$effect` rewired through `Poller<Job[]>`, keyed on `org.slug` plus a `$derived` over the four filters; render tree gains `stale`/`parked`/`stopped` branches, keeping its existing `error` / `loading` / empty / table four-way branch intact. |
| `web/src/lib/poll-fatal.ts`                                     | **New**, not in the original plan's scope. Added during implementation, in response to review feedback, to extract the `fatal()` classifier (401 clears the session, 403/404 stop the poll) out of page-local closures so this page and the overview page share one definition instead of two copies that could drift. Exports `fatalApiFailure`. |
| `web/src/routes/o/[org]/+page.svelte`                           | **Modify**, not in the original plan's scope. Touched as part of the same extraction: its own inline `fatal()` closure was replaced with the shared `fatalApiFailure` import from `poll-fatal.ts`. |
| `web/messages/en.json`, `es.json`, `de.json`, `fr.json`, `it.json`, `hi.json` | **Modify.** Add `queue_refresh_failed`, `queue_paused`, `queue_retrying` to each, mirroring the existing `overview_*` triad.                                    |
| `web/src/routes/o/[org]/queue/QueueHarness.svelte`              | **Modify.** Accept an optional reactive `url` prop so a test can drive a live filter change without a second mount — needed for the new "restarts on filter change" test. Existing callers that omit `url` are unaffected. |
| `web/src/routes/o/[org]/queue/page.render.test.ts`              | **Modify.** Add poller-behavior cases alongside the existing filter-navigation tests (neither set is removed).                                                 |
| `web/README.md`                                                 | **Modify.** One Layout-table row: `poll.svelte.ts` now used by the overview *and* `/queue`.                                                                     |

## Task Order & Rationale

Single task. The page rewrite, the six catalog edits, and the test additions are one coherent unit —
there is no useful checkpoint between "the page still compiles with the old fetch" and "the page
compiles with the new one," since the render tree reads from the same `jobs`/`loading`/`error`
names either way. The README edit is a one-line tail on the same commit sequence.

## Task 1 — Migrate the queue's job-list fetch to `Poller`, keyed on org + filters ✅

**Files:** `web/src/routes/o/[org]/queue/+page.svelte` (modify), `web/messages/*.json` (modify, all
six), `web/src/routes/o/[org]/queue/page.render.test.ts` (modify), `web/README.md` (modify). During
implementation, a review round extracted the shared `fatal()` classifier out of both this page and
the overview page into a new `web/src/lib/poll-fatal.ts` — see the note at the end of this task.
**Interfaces:** Consumes `Poller` from `$lib/poll.svelte` and `fatalApiFailure` from
`$lib/poll-fatal` (not `ApiError`/`session` directly — those live behind `fatalApiFailure` now,
shared with `o/[org]/+page.svelte`). Produces no new public interface — this is page-internal state
only.

- [x] **Add the three message keys to all six catalogs first**, so the page can reference them
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
- [x] Run `cd web && npm run check` now, before touching the page — confirm it still passes (the
      new keys are unused so far, which `check-messages.mjs` does not flag) and that no locale was
      missed.
- [x] **In `web/src/routes/o/[org]/queue/+page.svelte`, replace the job-list `$effect` and its
      supporting state.** Remove the `let jobs = $state<Job[]>([])`, `let loading = $state(true)`,
      `let error = $state<string | undefined>(undefined)` triad and the `let latest = 0` counter
      plus the `$effect` that uses them (the whole block from the `latest` declaration through the
      end of the second `$effect`, per the spec's §1). Add:
      ```ts
      import { Poller } from '$lib/poll.svelte';
      import { fatalApiFailure } from '$lib/poll-fatal';

      const jobsPoll = new Poller<Job[]>();

      // A rejected `goto` (see `applyFilters` below) is not something `Poller`
      // knows about, so it renders as its own independent notice rather than being
      // folded into the poll's own error state — see the render tree below for why
      // merging the two was wrong.
      let navError = $state<string | undefined>(undefined);

      const filters = $derived({ status, repo, team, mine, limit: 200 });

      $effect(() => {
        const slug = org.slug;
        if (!slug) return;
        // Switching orgs or filters runs this effect's cleanup, which stops the
        // poll before the next subscription starts — see the generation counter
        // in `poll.svelte.ts`. That is what keeps a late response for a superseded
        // filter from repainting the table with rows that do not match the
        // controls the reader is looking at.
        const active = filters;
        // A genuine org/filter change is the only event that supersedes an earlier
        // navigation failure — clearing `navError` on an unrelated poll tick
        // succeeding would prove nothing about whether the navigation itself ever
        // actually applied, and would silently erase a report about a control that
        // never worked.
        navError = undefined;
        return jobsPoll.start(() => api.jobs(slug, active), { fatal: fatalApiFailure });
      });

      const jobs = $derived(jobsPoll.value ?? []);
      const loading = $derived(!jobsPoll.value);
      const pollError = $derived(
        jobsPoll.failed ? messageFor(jobsPoll.error, m.queue_load_failed()) : undefined
      );
      ```
      `navError` and `pollError` are two independent pieces of state, not one combined `error` — a
      rejected navigation and a failed poll are different failures with different lifetimes (see the
      render tree step below), and folding them into one variable is exactly what an earlier review
      round undid. The shared `fatalApiFailure` classifier (401 clears the session and is fatal; a
      404 or a plain 403 is fatal; everything else is retried) lives in `$lib/poll-fatal.ts`, not as a
      page-local closure — this page and `o/[org]/+page.svelte` both import it, so there is one
      definition instead of two copies that could drift. Keep the `loading`/`jobs` names unchanged so
      the render tree below needs no further edits for its existing branches. `relative` is already
      imported (used elsewhere on the page for `job.createdAt`); reuse it for `jobsPoll.updatedAt`.
- [x] **Add the `navError`/`stale`/`parked` notices and the retrying note, without collapsing the
      existing four-way content branch.** The shipped render tree keeps `navError` as its own
      unconditional `Alert` — independent of the poll, because a rejected `goto` is not a poll
      failure — then renders the `parked`/`stale` note unconditionally too (mutually exclusive with
      each other, and with `navError`, above and independent of the content branching below), and
      only then branches on `pollError`/`loading`/the empty check/the table (`+page.svelte:220-253`
      as of this plan):
      ```svelte
      {#if navError}
        <Alert>{navError}</Alert>
      {/if}

      {#if jobsPoll.parked}
        <p role="status" class="text-xs text-faint">{m.queue_paused()}</p>
      {:else if jobsPoll.stale}
        <p role="status" class="text-xs text-warn">
          {m.queue_refresh_failed({
            reason: messageFor(jobsPoll.error, m.error_network()),
            age: relative(
              jobsPoll.updatedAt === undefined ? undefined : new Date(jobsPoll.updatedAt).toISOString()
            )
          })}
        </p>
      {/if}

      {#if pollError}
        <Alert>
          {pollError}
          {#if !jobsPoll.stopped && !jobsPoll.parked}{m.queue_retrying()}{/if}
        </Alert>
      {:else if loading}
        <Loading what={m.queue_loading()} />
      {:else if jobs.length === 0}
        <Empty title={filtered ? m.queue_empty_filtered() : m.queue_empty_title()}>
          <!-- existing Empty body, unchanged -->
        </Empty>
      {:else}
        <!-- existing table markup, unchanged -->
      {/if}
      ```
      The `parked`/`stale` note sits **above**, not inside, the content branch — putting it inside
      the final `{:else}` (table-only) branch was the exact bug two prior review rounds fixed: a
      stale or parked poll can coincide with `loading`, the empty state, or a `pollError`, and hiding
      the note behind a table-only branch would make it invisible in those cases. Double-check after
      editing that the `<Empty>` branch's own body (the "connect a repo" link, etc.) is still present
      verbatim — it is the one branch easiest to drop by accident when pattern-matching against the
      overview page's simpler two-way `{#if error}...{:else}` shape, which has no empty-state branch
      to preserve. Confirm `m.error_network` already exists (it is used by the overview page) rather
      than adding a new key for it.
- [x] **Type-check and build the page in isolation**: `cd web && npm run check`. Fix any TS error
      before moving on (in particular: `ApiError`'s exported shape, `session.clear()`'s signature —
      both already used identically in `o/[org]/+page.svelte`, so mirror it exactly rather than
      re-deriving the types).
- [x] **Give `QueueHarness.svelte` an optional reactive `url` prop**, so a test can drive a live
      filter change on an already-mounted instance instead of remounting (remounting would create a
      second, independent `Poller` subscription and could not demonstrate "a late response for the
      superseded filter is dropped," which requires one continuous subscription whose generation
      counter increments). Existing tests, which never pass `url`, are unaffected because the
      `Object.defineProperty` call below only runs when a `url` prop is actually supplied — the
      "Clear filters" test's own external `Object.defineProperty(page, 'url', ...)` override (set
      before mounting, on the `page` singleton directly) keeps working exactly as it does today:
      ```svelte
      <script lang="ts">
        import { page } from '$app/state';
        import { OrgContext, provideOrg } from '$lib/org.svelte';
        import Page from './+page.svelte';

        let { slug, url }: { slug: string; url?: URL } = $props();

        provideOrg(new OrgContext(() => slug));

        let current = $state(url);

        /** Lets a test drive a live filter/org change on an already-mounted instance. */
        export function setUrl(next: URL) {
          current = next;
        }

        if (url !== undefined) {
          Object.defineProperty(page, 'url', {
            configurable: true,
            get: () => current
          });
        }
      </script>

      <Page />
      ```
- [x] **Add the new test cases**, in `web/src/routes/o/[org]/queue/page.render.test.ts`, alongside
      (not replacing) the existing `describe('the queue filters', ...)` block. Follow
      `o/[org]/page.render.test.ts`'s pattern (a `serve(healthy: () => boolean)` fetch stub keyed on
      URL path, `vi.useFakeTimers()`, `REFRESH_INTERVAL` imported from `$lib/poll.svelte`,
      `vi.waitFor` for the real-`Response`-body race described in that file's comments). **Every
      stub in this new `describe` block must answer `/repos` and `/teams` (empty arrays, always
      `200`) in addition to `/jobs`**, because the page's untouched picker `$effect` fires its own
      `Promise.all([api.repos(slug), api.teams(slug)])` on mount independent of the job poll — a
      stub that only knows about `/jobs` will make that effect's calls fall through to whatever the
      `healthy`/404 branch returns, polluting assertions that are meant to be about the job list
      only:
      - `describe('the queue poller', ...)`:
        - **"keeps the table and shows the stale note when a refresh fails, then clears it on
          recovery"** — mount with a healthy stub returning one job, `settle()`, assert the job's
          title renders and no `[role="status"]` exists; flip to unhealthy (502 on `/jobs` only —
          `/repos`/`/teams` keep answering `200`), advance `REFRESH_INTERVAL * 1.4`, `vi.waitFor` a
          `[role="status"]` containing "Refresh failed" while the job's title still renders; flip
          back healthy, advance `REFRESH_INTERVAL * 4`, `vi.waitFor` the status node gone and the
          title still present.
        - **"stops polling and shows the error in place of the table on a 404"** — mount with a stub
          that 404s on `/jobs` (still 200 on `/repos`/`/teams`), `settle()`, assert the table is
          absent and an `Alert`-rendered error is present, then advance the clock by several
          intervals and assert the count of `fetch` calls whose URL contains `/jobs` specifically
          stays at 1 (filter `fetchMock.mock.calls` by URL substring — do not assert on the raw
          total call count, which also includes the one-time `/repos`/`/teams` calls). This is the
          `jobsPoll.stopped` behavior; the healthy case's job-render assertions must NOT appear.
        - **"restarts the poll when a filter changes, and drops a late response for the old
          filter"** — mount the harness with `props: { slug: 'acme', url: new
          URL('http://example.test/o/acme/queue') }` and a stub that, per `/jobs` call, hands back
          its own genuinely independent, still-pending deferred promise, keyed by whether the
          request URL is the unfiltered one or carries `status=pending` (as actually shipped: a
          `resolve()` obtained from `new Promise(...)` per call, stored by key, rather than a single
          promise resolved twice — a request already settled cannot be resolved "late" a second
          time). `settle()` leaves the initial unfiltered request's deferred promise deliberately
          pending — nothing resolves it yet. Call `instance.setUrl(new
          URL('http://example.test/o/acme/queue?status=pending'))` and `await settle()` — this
          re-runs the page's `$effect` (the `url` prop is `$state`-backed and reactive, per the
          harness change above), tearing down the unfiltered `Poller` subscription and starting a
          new one for `status=pending`, which gets its own new deferred promise. Assert the newest
          `/jobs` fetch call's URL contains `status=pending`, then resolve *that* deferred promise
          and assert its job renders. Only now resolve the original unfiltered request's
          still-pending deferred promise — late, from a request that was never touched before the
          filter switch — and assert the rendered table is unaffected by it — the response belongs
          to a superseded `Poller` generation and must not be applied
          (`docs/specs/2026-09-09-overview-polling-design.md` §3).
- [x] Run `cd web && npm test` (full suite, not just `queue`) and confirm everything passes,
      including the untouched existing two filter-navigation tests in the same file.
- [x] **Update `web/README.md`'s Layout table.** Change the `poll.svelte.ts` row from "Polling a
      page hands to an `$effect`. The overview uses it; `/queue` and `/repos` should." to "Polling
      a page hands to an `$effect`. The overview and `/queue` use it; `/repos` should."
- [x] **Format and commit.**
      ```bash
      cd web && npm run lint -- --write && npm run lint
      cd .. && git add web/src/routes/o/\[org\]/queue/+page.svelte web/messages/*.json \
        web/src/routes/o/\[org\]/queue/QueueHarness.svelte \
        web/src/routes/o/\[org\]/queue/page.render.test.ts web/README.md \
        web/src/lib/poll-fatal.ts web/src/routes/o/\[org\]/+page.svelte
      git commit -m "web: migrate the queue page's job list to Poller, keyed on filters too"
      ```
- [x] **Full gate, once more, from a clean state**: `cd web && npm run check && npm run lint && npm test && npm run build`.
