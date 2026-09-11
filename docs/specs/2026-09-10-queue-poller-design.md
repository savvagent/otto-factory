# Queue page Poller migration design

> **Status:** DRAFT — migrate the queue page's job list from fetch-once-per-filter-change to a
> `Poller` subscription, folding in the four-filter restart requirement `savvagent/otto-factory#92`
> already flagged for this exact migration.

> **Implements:** `savvagent/otto-factory#104`, `savvagent/otto-factory#92`

## Premise corrections

None. The issue's description of the current state matches the code: `web/src/routes/o/[org]/queue/+page.svelte`
fetches the job list from a plain `$effect` keyed on the org slug and the four filter values
(`status`, `repo`, `team`, `mine`), and never again until the effect's dependencies change.

## Goal & Success Criteria

The queue page is the second-most-likely page to be left open on a second monitor after the
overview (`docs/specs/2026-09-09-overview-polling-design.md`, already shipped), and its whole
purpose is "what is happening to my jobs right now." Today a job moving `pending` → `active` →
`completed` is invisible until the reader reloads. This migrates the job-list fetch to
`Poller` (`web/src/lib/poll.svelte.ts`), the same helper the overview already uses, and — per
`#92` — keys the poll's restart on the org **and** all four filter values, not the org alone.

- The queue's job list refreshes every 30 seconds with no visible loading flash on a healthy tick.
- Changing any filter (Status, Repo, Team, "only what I queued") or navigating to a different org
  restarts the poll against the new query immediately — no stale poll keeps running against the
  filter set the reader just left, and no response for an abandoned filter set is ever rendered.
- A refresh that fails leaves the last good table on screen, says why and how old it is, and
  clears itself on the next successful tick.
- A `401`/`403`/`404` (session gone, org gone, org access lost) stops the poll and shows the error
  in place of the table, exactly as it does today for a first-load failure — it does not retry
  forever against a credential or a target that will not come back.
- Two refreshes never overlap, a failing one backs off, and a tab hidden or idle for hours behaves
  per the six rules already governing the overview.
- The repo/team picker fetch (used to populate the filter `<select>`s) is unchanged — it is a
  best-effort convenience fetch, not the live data this migration is about.
- `npm run check`, `npm run lint`, `npm test`, `npm run build` all pass.

## Scope

**In:**

- `web/src/routes/o/[org]/queue/+page.svelte` — the job-list `$effect` rewired through `Poller<Job[]>`,
  keyed on `org.slug` plus the four filter values (via `Poller`'s own generation guard, not a
  hand-rolled `seq` counter — see §2).
- Three new message keys — `queue_refresh_failed`, `queue_paused`, `queue_retrying` — added to all
  six catalogs (`web/messages/*.json`), matching the overview's three keys in shape and tone.
- `web/src/routes/o/[org]/queue/page.render.test.ts` gains poller-behavior cases (stale-keeps-data,
  fatal-stops, restart-on-filter-change), added alongside the existing filter-navigation tests —
  neither set of tests is removed.
- One `web/README.md` Layout-table row edit (the `poll.svelte.ts` row already says "the overview
  uses it; `/queue` and `/repos` should" — updated to reflect that `/queue` now does).

**Out:**

- **No server change of any kind.** No MCP tool, no console route, no SQL, no `of-core` function,
  no migration, no `OF_*` config key. `api.jobs()` is called with the same `filters` object as
  today; only what triggers the call changes.
- **No change to the filter controls, the URL-as-state design, or `applyFilters`/`setFilter`.**
  `docs/specs/2026-09-09-queue-filter-refetch-design.md` already fixed filter changes to navigate
  through `goto`; this change only replaces what happens once `page.url` (and therefore `status`/
  `repo`/`team`/`mine`) has changed. The existing filter-navigation tests keep passing unmodified.
- **No change to the repo/team picker fetch.** It stays a separate, un-pollled, best-effort
  `$effect` exactly as it is today — polling it would mean refetching the full repo/team list every
  30 seconds for two `<select>`s that rarely change, for no reader-visible benefit.
- **No adoption on `/repos`.** Same helper, same six rules, but `/repos` has its own state to think
  through and is not part of either `#104` or `#92`.
- **No change to `Poller`/`poll.svelte.ts` itself.** The overview's `start()` call already resets
  every field on each call (§3 of the overview spec: "no `key`, so every effect re-run is a full
  reload") — exactly the behavior a filter change wants, since a new filter set is a different
  query, not a retry of the old one. No new capability is needed from the helper.

Checked against the three constraints in `CLAUDE.md`: coordination stays anchored on repos
(nothing here touches repo/lease semantics); the server gains no workflow opinion (client-side
only, over an endpoint that already exists); coding-agent agnosticism is untouched (the console is
a human surface, no agent client sees this).

## Assumptions

- **The same 30-second interval and defaults as the overview.** No product reason to poll the
  queue at a different rate; introducing a second interval constant would need its own
  justification this issue does not give one for.
- **Filter changes are not "refreshes."** `Poller.start()` fully resets `value`/`loading`/`stale`/
  etc. on every call — the overview spec's own Risks section names this ("wanted before `/queue`
  adopts the helper with filters in scope"), and it is exactly right here: selecting a different
  Status is a new question, not a retry of the old answer, so a brief loading state on filter
  change is correct, not a regression of rule 1 ("a refresh is not a load" — that rule is about
  *ticks* within one subscription, not about starting a new one).
- **`fatal` classification mirrors the overview's exactly.** A `401` clears the session (letting
  the root layout's guard redirect to `/login`); a `403`/`404` means this org — or this exact
  filter combination is treated as a permanent failure of the *current subscription*, but "current
  subscription" already means "current org + current filters" per the point above, so a `404` from
  an unregistered `repo`/`team` slug in the URL stops retrying it every 30 seconds until the reader
  picks a different filter or navigates — which is strictly better than the status quo (today's
  code sets `error` once and never revisits, no polling at all).
- **The repo/team-picker effect's failure handling is untouched.** It already swallows errors
  silently by design ("The pickers are a convenience... the filters still work by URL") — this
  migration does not touch it and that comment stays accurate.
- **`Poller`'s existing generation counter is what satisfies `#92`, not a new mechanism.** Because
  the `$effect` reads `org.slug` and (transitively, through the `$derived` filters object) all four
  filter values, any change to any of them re-runs the effect, whose cleanup tears down the current
  `Poller` subscription before the new one starts — see §2. This is the same shape the overview
  already uses for the org alone; extending the effect's dependencies is the entire fix.

## §1 Current shape (for reference)

```svelte
let latest = 0; // "seq" — a hand-rolled generation counter for exactly one hazard: stale rewrites
$effect(() => {
  const slug = org.slug;
  const filters = { status, repo, team, mine, limit: 200 };
  if (!slug) return;
  const seq = ++latest;
  loading = true;
  error = undefined;
  void (async () => {
    try {
      const found = await api.jobs(slug, filters);
      if (seq !== latest) return;
      jobs = found;
    } catch (e) {
      if (seq !== latest) return;
      error = messageFor(e, m.queue_load_failed());
      jobs = [];
    } finally {
      if (seq === latest) loading = false;
    }
  })();
});
```

This fetches once per (org, filters) tuple and never again. `Poller` already solves the identical
staleness hazard with its own generation counter (`docs/specs/2026-09-09-overview-polling-design.md`
§3), so migrating removes the hand-rolled `latest`/`seq` pair rather than duplicating it.

## §2 The new shape

```svelte
import { Poller } from '$lib/poll.svelte';
import { ApiError } from '$lib/api';
import { session } from '$lib/session.svelte';

const jobsPoll = new Poller<Job[]>();

const filters = $derived({ status, repo, team, mine, limit: 200 });

$effect(() => {
  const slug = org.slug;
  if (!slug) return;
  // Reading `filters` here (a $derived over status/repo/team/mine) makes this
  // effect depend on all four in addition to the org — #92's requirement.
  // Any one of the five changing tears down the current subscription (the
  // cleanup below) before the next one starts, so a filter change never
  // leaves a poll running against the filter set the reader just left.
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

`jobsPoll.start` returns its teardown, which Svelte calls before re-running the effect — the same
idiom `docs/specs/2026-09-09-overview-polling-design.md` §2 documents. The effect's dependency set
now includes the four filters because `active` reads `filters`, a `$derived` that itself reads
`status`/`repo`/`team`/`mine`; Svelte's reactivity tracks that transitively; there is nothing to
name explicitly for it to work, but it is worth stating because it is the entire content of the
`#92` fix and is otherwise invisible in a diff.

The render tree keeps the same three-way branch it has today (error / loading-with-nothing-shown /
table), with one added state:

```svelte
{#if error}
  <Alert>
    {error}
    {#if !jobsPoll.stopped}{m.queue_retrying()}{/if}
  </Alert>
{:else if loading}
  <Loading what={m.queue_loading()} />
{:else}
  {#if jobsPoll.parked}
    <p role="status" class="text-xs text-faint">{m.queue_paused()}</p>
  {:else if jobsPoll.stale}
    <p role="status" class="text-xs text-warn">
      {m.queue_refresh_failed({
        reason: messageFor(jobsPoll.error, m.error_network()),
        age: relative(jobsPoll.updatedAt === undefined ? undefined : new Date(jobsPoll.updatedAt).toISOString())
      })}
    </p>
  {/if}
  <!-- existing Empty / table markup, reading `jobs` as it does today -->
{/if}
```

The filter bar (the four controls plus "Clear filters") is unconditional today and stays that
way — only the results region below it switches on `error`/`loading`/`parked`/`stale`.

## §2a What was actually built

The description above is wrong in two ways a PR review caught, and both are now fixed in the
shipped code rather than in this spec's prose only.

First, the "three-way branch" was never accurate even for the code as designed here: `error` /
`loading` / the results region is really a four-way split once the results region's own
`Empty`-vs-`table` choice is counted, and the stale/parked note was written nested *inside* the
table's branch. That meant a successful-but-empty poll (a filter matching nothing, or a fresh org)
followed by a failing or long-idle tick showed a confident "no jobs" empty state with no
staleness or pause indication at all — exactly what the six rules in `poll.svelte.ts` exist to
prevent. The shipped render tree puts the `parked`/`stale` note above the `Empty`/table split, so
it fires whenever the poll has ever produced a value (fresh, stale, or parked, including an empty
one) rather than only when that value happens to be non-empty.

Second, this spec never mentions `navError` (the existing rejected-`goto` notice from
`docs/specs/2026-09-09-queue-filter-refetch-design.md`) at all, but the shipped page has to
render both it and the poll's own `error` somewhere. An earlier revision merged them into one
`error` slot with `pollError ?? navError` precedence. That was wrong on its own terms: `stale`
(data present, refresh failed non-fatally) leaves `pollError` `undefined`, so a stale poll fell
through to a stale `navError` and replaced a perfectly good table with a nav-rejection alert; and
an effect that cleared `navError` on every successful poll tick retired a real navigation failure
report on a random 0-30s timer that proved nothing about whether the navigation itself ever
actually got fixed. The shipped code renders `navError` as its own independent, always-visible
`Alert` — a nav-rejection is a notice about a *control*, not a reason to blank the results region
— and retires it only from the existing reset at the top of the job-poll `$effect` (on a genuine
org/filter change, the only event that actually supersedes a failed navigation). `pollError`
drives its own `{#if pollError}` branch with no precedence rule against `navError` at all.

## §3 What does not change

- `setFilter`, `applyFilters`, the `filtered` derived, and every control in the filter bar —
  untouched. `docs/specs/2026-09-09-queue-filter-refetch-design.md` already made these navigate
  through `goto`; this spec only changes what happens after `page.url` updates.
- The repo/team picker `$effect` — untouched, still fetch-once-per-org, still swallows failures.
- `api.jobs()` — called with the same shape of `filters` object as today.
- The `200`-row cap and its "showing N, capped" footer note — reads `jobs.length` as it does today,
  now sourced from `jobsPoll.value` instead of the local `jobs` state variable.

## Testing

- `web/src/routes/o/[org]/queue/page.render.test.ts` — the existing two tests (which navigation
  primitive `setFilter`/"Clear filters" call) are untouched. New cases, mirroring
  `o/[org]/page.render.test.ts`'s pattern with a stubbed `fetch` and fake timers:
  - A refresh that fails (502) keeps the last-rendered jobs and shows the stale note; a subsequent
    success clears it.
  - A `404` (simulating an unregistered `repo`/`team` filter, or an org the account no longer has
    access to) stops the poll (`jobsPoll.stopped`) and renders the error branch instead of a table.
  - Changing a filter mid-poll (simulated by re-rendering the harness with a different `page.url`,
    the same technique the existing filter tests already use) restarts the subscription: the new
    filter's `fetch` call is the one whose result renders, and a response for the old filter that
    resolves late is not.
- Existing `web/src/lib/poll.svelte.test.ts` / `poll.dom.test.ts` are unchanged — this migration
  adds no new behavior to `Poller` itself.
- Gates: `npm run check`, `npm run lint`, `npm test`, `npm run build`.

## Error Handling & Edge Cases

- **A refresh fails with a table on screen** → `stale`, table kept with its age, retried on a
  widening interval, same as the overview.
- **The first load for a given (org, filters) fails** → `failed`, the error branch replaces the
  table, "still retrying" shown unless the failure was fatal.
- **A `401`** → session cleared, poll stops; the root layout's own guard routes to `/login`, same
  as the overview.
- **A `404`/`403`** (org gone, org access lost, or an unregistered `repo`/`team` slug in the URL)
  → poll stops; the error persists until the reader changes the org or a filter, which starts a new
  subscription per §2 — this is a strict improvement over today's code, which sets the error once
  and polls not at all.
- **The reader changes a filter while a request is in flight** → the in-flight response belongs to
  a superseded generation and is dropped by `Poller` itself (`docs/specs/2026-09-09-overview-polling-design.md`
  §3); the new subscription's own first load drives the loading state.
- **The tab is hidden or idle** → identical to the overview: no tick while hidden, refresh on
  return; parks after four hours of no input, resumes on the next one.

## Risks & Open Questions

- **The picker fetch (repos/teams) still runs once per org and is not polled.** If a repo or team
  is renamed or added while the tab is open, the picker's `<select>` will not show it until the
  page is reloaded or the org is revisited. Not a regression — this is exactly today's behavior —
  and out of scope per this spec's own Scope section; worth a follow-up issue only if it turns out
  to matter in practice.
- **A stricter fix for `#92` would give `Poller.start()` an explicit key** (as the overview spec's
  own Risks section speculates) so a same-key restart could preserve `value` across a filter
  change instead of resetting to `loading`. Not pursued here: a filter change is a materially
  different query, and briefly showing the loading state for it is correct, not a rough edge to
  polish away.
