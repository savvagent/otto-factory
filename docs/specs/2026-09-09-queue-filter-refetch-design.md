# Queue filter refetch design

> **Status:** IMPLEMENTED — shipped in `savvagent/otto-factory#85` (merged as `7dc4243`), closing
> `savvagent/otto-factory#82`. Selecting a queue-page filter (Status, Repo, Team, "only what I
> queued") re-fetches the job list immediately instead of only updating the URL. Verified via the
> merge's own `master` CI run (`34430848210`): all four jobs green, including `deploy` —
> confirmed live via the auto-deploy pipeline.

## Premise corrections

`savvagent/otto-factory#82`'s title says the filters "do not work." Reproduction (see Testing) shows
that is imprecise in a way that matters for the fix: the filter *controls* and the *URL* both update
correctly — selecting "Pending" changes the address bar to `?status=pending` and the `<select>` shows
"Pending" selected. What never happens is the re-fetch: the jobs table keeps showing every job,
unfiltered, until the page is hard-reloaded (at which point the URL's query string *is* honored
correctly by the initial load). The team filter is not named in the issue only because the reporting
org has no teams configured, so its picker never renders (`{#if teams.length > 0 || team}` in
`+page.svelte`) — it shares the exact same defect once a team exists to filter by.

## Goal & Success Criteria

Selecting a value in any of the queue page's filters (Status, Repo, Team, "only what I queued")
re-fetches and re-renders the job list immediately, without a page reload, and without adding a
browser-history entry per filter change (the page's original design intent, stated in its own
comment).

- Changing Status, Repo, Team, or "only what I queued" in the browser immediately narrows the
  rendered table to matching jobs, with no reload.
- The "Clear filters" button (visible once any filter is set) immediately restores the unfiltered
  list, with no reload.
- No new browser-history entry is created per filter change (repeated back-button presses do not
  step through filter states one at a time).
- `npm run check`, `npm run lint`, `npm test`, `npm run build` all pass.

## Scope

**In:**

- `web/src/routes/o/[org]/queue/+page.svelte` — the two call sites (`setFilter`, the "Clear filters"
  button's `onclick`) that currently call `replaceState` from `$app/navigation`, consolidated
  behind a shared `applyFilters` helper that also routes a rejected navigation into the page's
  existing `error`/`Alert` path (added during review, see Root cause).

**Out:**

- No change to the console API (`of-web`'s `list_jobs`/`ListJobsQuery`) — reproduction confirms the
  server-side filtering is correct; the bug never reaches the server because the client never issues
  the follow-up request.
- No change to `$lib/api.ts`'s `jobs()`/`query()` helpers — the query-string construction from a
  `filters` object is correct once the effect actually runs.
- No new console route, MCP tool, SQL, config surface, or migration — consistent with constraint 2
  (substrate, not workflow) and Non-Negotiable Rule 6 (no public-interface change here at all).
- No change to `$lib/org.svelte.ts` or any other page's filter/URL pattern. A repo-wide grep (see
  Assumptions) found no other page using this `replaceState`-for-filters pattern, so the fix is
  scoped to this one file.

## Root cause

`web/src/routes/o/[org]/queue/+page.svelte`'s `setFilter` (and the "Clear filters" button) call:

```js
import { replaceState } from '$app/navigation';
...
replaceState(url, page.state);
```

`replaceState` (and its sibling `pushState`) are SvelteKit's **shallow-routing** primitives.
Reading `@sveltejs/kit`'s own client runtime
(`node_modules/@sveltejs/kit/src/runtime/client/client.js`, `replaceState`/`pushState`) shows
exactly what they do: they call `history.replaceState(...)` / `history.pushState(...)` directly (so
the browser's address bar *does* change), and they set `page.state = state` — but they never
reassign `page.url`. The `clone_page(page)` call that follows clones the page object with its
existing (unchanged) `url`. This is intentional upstream behavior: these two functions exist so a
component can attach ad-hoc `state` to a history entry (e.g. "this modal is open") without
triggering a full navigation — the URL argument only updates what the browser chrome shows, not the
reactive `page.url` any component reads.

`+page.svelte`'s `status`, `repo`, `team`, and `mine` are each a `$derived` reading
`page.url.searchParams`, and the `$effect` that fetches the job list depends on exactly those four
values. Because `page.url` never changes after `replaceState`, none of the four `$derived`s
recompute, so the effect never re-runs and no second request is ever sent. The address bar and the
`<select>`'s own DOM value change (from the user's interaction and the browser's native form
behavior), which is what makes the bug easy to miss by eye — everything *looks* like it responded
except the one thing that matters.

The fix is `goto` from the same module, with `replaceState: true`, routed through a single
`applyFilters` helper both call sites share:

```ts
function applyFilters(url: URL | string) {
  void goto(url, { replaceState: true, keepFocus: true, noScroll: true }).catch((e: unknown) => {
    error = messageFor(e, m.queue_load_failed());
  });
}

function setFilter(key: string, value: string | undefined) {
  const url = new URL(page.url);
  if (value === undefined || value === '') url.searchParams.delete(key);
  else url.searchParams.set(key, value);
  applyFilters(url);
}
```

`goto` performs an actual (client-side) navigation — which is what updates `page.url` — while
`replaceState: true` keeps the original intent from the page's comment ("changing a filter is not a
place in history to go back to") by replacing the current history entry instead of pushing a new
one. `keepFocus: true` avoids moving focus away from the control the reader just used (a `<select>`
or the checkbox), and `noScroll: true` avoids SvelteKit's default post-navigation scroll-to-top,
since this is a same-page, same-scroll-position update.

The `.catch()` is not incidental. A security review of the original PR caught that a rejected
`goto` — the origin check `goto` runs internally, or a chunk-load failure — would otherwise vanish
silently, reproducing this exact bug's symptom: a filter that looks like it did something (the
control's own DOM state changes) but changed nothing, with no error shown. Routing the rejection
into the same `error`/`Alert` path the job-fetching `$effect` already uses closes that gap instead
of leaving a second, narrower way back into the original defect.

Both call sites go through `applyFilters`: `setFilter`, and the "Clear filters" button's `onclick`,
which now calls `applyFilters(page.url.pathname)` — a bare path, not a `new URL(page.url.pathname,
location.origin)` construction, since `goto` accepts a string directly. Neither call site
duplicates the `goto`/`catch` logic; both funnel through the one helper, so a future
error-handling fix only has one place to land.

## Assumptions

- **No other page in `web/` shares this bug.** `grep -rn "replaceState(" web/src` (excluding this
  file and the `$app/navigation` import line) returns nothing else — this is the only page in the
  console that builds a filter-in-URL pattern this way, so the fix does not need to be repeated
  elsewhere. Confirmed during Phase 1 investigation.
- **`goto`'s load-rerunning behavior has nothing to invalidate here, which is a more basic fact
  than "a no-op."** `invalidateAll` defaults to `false` — `goto` does not invalidate anything by
  default at all, so there is no default invalidation behavior to reason about in the first place.
  This route also has no `+page.ts`/`+page.server.ts` `load` function to invalidate even if it
  did (`web/routes/+layout.ts` sets `ssr = false`; this page fetches its own data in an `$effect`,
  not via `load`). The `$effect` re-running because `page.url` changed — not any `goto` invalidation
  — is what re-triggers the fetch.
- **`keepFocus: true` and `noScroll: true` are the right defaults for this interaction**, matching
  how a same-page filter control should behave (the reader stays where they are, with focus where
  they left it) — this is a judgment call the plan critique should confirm rather than a requirement
  from the issue, which only asks that filtering work at all.
- **A behavioral test for this defect is feasible, once you stop trying to exercise the real
  router.** `web/`'s only Svelte-page tests (`o/[org]/page.render.test.ts`, using a
  `*Harness.svelte` that mounts `+page.svelte` directly via `mount()`, bypassing SvelteKit's
  `app.js`/`start()`) cannot call the real `goto`, `pushState`, or `replaceState` — `@sveltejs/kit`'s
  client runtime throws `Cannot call goto(...)/replaceState(...) before router is initialized` in
  that harness, confirmed during Phase 1 investigation. The seam that unblocks it is `vi.mock`ing
  `$app/navigation` itself: with `goto`/`replaceState` replaced by spies, the component never
  touches the real router, and the test can assert on *which* primitive the component calls and
  with what arguments — the actual defect. The other detail that has to be right: Svelte 5
  delegates `change` at the mount root, so a synthetic `new Event('change')` with no options never
  reaches the handler — it must be dispatched with `{ bubbles: true }`. The PR adds this test at
  `web/src/routes/o/[org]/queue/page.render.test.ts`, using a `QueueHarness.svelte` mirroring the
  existing pattern.

## Error Handling & Edge Cases

- **Rapid sequential filter changes** (e.g. Status then Repo before the first fetch resolves) are
  already handled by the existing `latest`/`seq` guard in the `$effect` that fetches jobs — untouched
  by this fix, and still correct once the effect actually re-runs per change.
- **A filter set from a shared/bookmarked URL on first load** already works today (this is the path
  that made the bug easy to miss) and is unaffected — `goto` is only invoked from `setFilter`/"Clear
  filters", never on initial mount.
- **Back/forward navigation across filter states**: `goto(url, { replaceState: true })` keeps a
  single history entry for the queue page's filter state (matching current behavior's intent), so
  the browser back button leaves the queue page in one step, not one step per filter change.

## Testing

- Reproduced pre-fix with a real signed-in session, a real Postgres-backed `of-server`, and seeded
  jobs across two repos/three statuses/two creators: selecting "Pending" in Status changed the URL to
  `?status=pending` and left the `<select>` showing "Pending," but the table kept all four jobs and
  the "Clear filters" button (a `$derived` off the same state) never appeared. A hard navigation to
  `.../queue?status=pending` filtered correctly, isolating the defect to the client-side update path.
  Same result for Repo and "only what I queued."
- Post-fix: repeated the same manual sequence (Status, Repo, "only what I queued", then "Clear
  filters") against the same seeded data. The table narrowed or restored immediately each time,
  with no reload, and `window.history.length` stayed constant across the sequence rather than
  growing per filter change — see the plan's Manual verification record for the exact run.
- `npm run check`, `npm run lint`, `npm test`, `npm run build` all pass.

## Risks & Open Questions

- The regression test added at `web/src/routes/o/[org]/queue/page.render.test.ts` guards the
  specific "which navigation primitive is called, with what arguments" defect this issue was about
  — a future edit that reintroduces `replaceState` here now fails that test. It does not, and
  cannot from this harness, prove the full behavioral path (that a real navigation actually
  re-renders the table with the new filter applied): that needs a live SvelteKit router, which is
  an e2e concern this repo has no runner for. The narrower claim is what's tested; the code comment
  at the call site remains as a second line of defense for the part no test covers.
- A future adoption of `Poller` (`web/src/lib/poll.svelte.ts`) on this page would need to key its
  restart on the four filter values (`status`, `repo`, `team`, `mine`), not just the org — a known
  follow-up concern raised in review, noted here rather than left to be rediscovered. Tracked as
  `savvagent/otto-factory#92`.
