# Overview polling design

> **Status:** DRAFT — the console overview refreshes itself every 30 seconds instead of showing
> whatever was true when the tab was opened

> **Implements:** `savvagent/otto-factory#57`

## Premise corrections

- **The issue says "the dashboard".** There is no page by that name. The page meant is the org
  overview, `web/src/routes/o/[org]/+page.svelte`, mounted at `/o/{org}` — the tiles, the recent-jobs
  list, the period meter, and the repo chips. This spec uses "the overview" throughout.
- **The issue's interval ("every 30 seconds") is a product choice, not a derived one**, and it is
  recorded here as an assumption rather than a calculation.
- **The issue says the existing `if (org.slug !== slug) return;` guard "stays".** It does, in
  `o/[org]/+layout.svelte`, which fetches once and is not converted here. The _page's_ copy of that
  idiom is superseded by the generation counter in §3, which covers strictly more (§3 says why).

## Goal & Success Criteria

The overview describes a queue that changes while nobody is touching the browser — agents claim jobs
over MCP, leases expire, counters move — and it is the page most likely to be left open on a second
monitor. Today it fetches once, in an `$effect` keyed on the org slug, and never again: the numbers
on screen are from whenever the tab was opened and nothing on the page admits it. It should refresh
itself, on an interval, without becoming worse than the static page it replaces.

- The overview refreshes every 30 seconds with no visible loading state on a refresh.
- A hidden tab does not poll, and refreshes as soon as it is shown again.
- A refresh that fails leaves the last good data on screen, says why and how old it is, and recovers
  on its own when the next refresh succeeds.
- A failure that means the credential or the grant is gone stops the poll and surfaces, rather than
  being retried behind a small warning.
- Two refreshes never run concurrently, a failing one backs off, and a response belonging to an org
  the reader has navigated away from is never rendered.
- An open tab does not silently defeat the server's 14-day idle session deadline.
- `npm run check`, `npm run lint`, `npm test`, `npm run build` all pass.

## Scope

**In:**

- A new `web/src/lib/poll.svelte.ts` exporting `Poller<T>` — the polling state machine, reusable by
  any console page that displays live state.
- `web/src/lib/poll.svelte.test.ts` and `web/src/lib/poll.dom.test.ts` — the rules under fake
  timers with injected seams, and the shipped `document`-backed seams under jsdom.
- `web/src/routes/o/[org]/+page.svelte` rewired to fetch through the poller, plus
  `page.render.test.ts` and its harness.
- Three message keys — `overview_refresh_failed`, `overview_paused`, `overview_retrying` — in all
  six catalogs (`web/messages/*.json`).
- One row in `web/README.md`'s Layout table and one bullet in the root `CLAUDE.md`'s `web/` section,
  so the next page that needs polling finds the helper instead of writing a `setInterval`.

**Out:**

- **No server change of any kind.** No MCP tool, no console route, no SQL, no `of-core` function, no
  migration, no `OF_*` config key. The endpoints polled (`queueStats`, `jobs`, `repos`, `usage`)
  already exist and are unchanged.
- **No push transport.** `of-core`'s `watch.rs` opens with "Agents need to react to queue changes
  without polling", and the MCP `watch` tool carries this data — but a feed to the _browser_ would
  be a new console route in `catalog.rs` plus a held connection per open tab, fanned out to readers
  rather than agents, for a page whose data is a second stale at worst. Polling an existing read API
  costs the server nothing new. Constraint 2: a capability that could live outside the server does.
- **No adoption by the other pages.** `/queue` and `/repos` have the same staleness problem and the
  helper is written for them, but converting them is not in this change; each has its own filter and
  pagination state to think about.
- **No user-configurable interval and no manual refresh button.** A control that changes how often
  the console reads is a workflow opinion, and a refresh button on a page that refreshes itself is
  furniture. `PollOptions.interval` exists for the _code_ to vary, floored at one second so a typo
  cannot become a hot loop against the API.
- **No "updated N seconds ago" label while things are healthy** — see Assumptions. The age _is_
  shown once a refresh has failed, because then it is the content of the message rather than chrome.
- **No bound on the queries the tick repeats.** `Jobs::stats` is a full per-org aggregate and
  `list_repos` has no `LIMIT`; both were paid once per visit and are now paid every 30 seconds per
  open tab. That is a server-side change with its own spec — see Risks.

Checked against the three constraints in `CLAUDE.md`: coordination stays anchored on repos (nothing
here touches repos or leases); the server gains no workflow opinion (this is entirely client-side,
over read endpoints that already exist); coding-agent agnosticism is untouched (the console is a
human surface and no agent client sees this).

## Assumptions

- **30 seconds, from the issue.** Roughly 120 requests an hour per open tab against a read-only API,
  for a queue whose jobs run for minutes. Fast enough that a job's status change is noticed within
  one glance-cycle; slow enough that an open tab is not a load concern.
- **The interval is not billable and must never read as a cost control.** `of-billing` meters MCP
  tool calls; every request the console makes is a `GET` to `of-web`. Written down here because the
  next person to touch the number will otherwise wonder.
- **All four endpoints are refetched together on every tick** and land as one value, so a tick
  repaints the whole page or none of it. Four pollers, or a staggered fetch to spread load, would
  let the usage meter and the job list come from different ticks.
- **A transient failure keeps the last good data.** Swapping a working dashboard for an error alert
  on one 502 is strictly worse than being one interval behind. `CLAUDE.md`'s "no silent fallbacks"
  rule is about _resolution_ failures, where guessing produces a wrong answer; this is a display
  labelled as not-current, which is the opposite of a silent guess — and §5 is why "labelled" is
  load-bearing rather than decorative.
- **There is no "updated 12 seconds ago" label on a healthy page.** Keeping one honest needs a
  second timer running at human resolution, which is a lot of machinery to say "nothing has gone
  wrong".
- **Polling continues after a failed first load**, so a page opened during a blip fills itself in.
- **`document.visibilityState` is the pause signal**, not `window.blur`. A tab behind another window
  is still visible and still being read; a backgrounded tab is not.

## §1 The six rules

The helper exists because polling is easy and _bearable_ polling is a set of rules that are each
easy to omit one at a time. Each is stated with the failure it prevents:

1. **A refresh is not a load.** `loading` is true only while the first result of a subscription is
   outstanding. Flipping it on every tick replaces a working page with a skeleton twice a minute.
2. **A failed refresh keeps the last good result**, sets `stale`, and records why and when the data
   on screen was fetched, so the page can say how far behind it is.
3. **A failure the caller calls `fatal` stops the poll** and surfaces through `failed`/`error`
   instead — §5.
4. **A hidden tab does not poll**, and refreshes the moment it is shown again. The **first** load
   runs regardless of visibility: a tab opened in the background should have data ready when it is
   looked at; it is the repeat that is suppressed.
5. **Refreshes never overlap and a failing one backs off.** The interval runs from the end of one
   refresh to the start of the next — a chained `setTimeout`, never `setInterval` — so a slow
   response delays the next tick instead of stacking behind it. A healthy poll keeps the interval
   exactly — "every 30 seconds" should mean that — while consecutive failures double the gap to an
   8× cap with ±15% jitter, so an outage is neither met at full rate by every open tab nor by all
   of them at the same instant. A load that never settles is failed by a `timeout`: a hung `fetch` would otherwise leave `#inFlight`
   set with no timer armed — a dead poll wearing a healthy page's face.
6. **A tab nobody has touched parks itself** — §6.

## §2 `Poller<T>` — shape

`web/src/lib/poll.svelte.ts`. A rune-backed class, per the `web/` convention that shared reactive
state lives in a `.svelte.ts` module (`session.svelte.ts`, `org.svelte.ts`).

The reactive fields are **private `$state` behind getters**. The legal combinations below are not
expressible in the types, and public fields would let a page put the object into a state its own
documentation calls impossible.

| Member                              | Meaning                                                                      |
| ----------------------------------- | ---------------------------------------------------------------------------- |
| `value: T \| undefined`             | The last successful result; `undefined` before the first lands.              |
| `loading: boolean`                  | True only while the first result of the current subscription is outstanding. |
| `failed: boolean`                   | There is nothing to render. A flag, not `error !== undefined` — see below.   |
| `error: unknown`                    | Why. The page maps it with `messageFor`, which takes `unknown` by design.    |
| `stale: boolean`                    | The last refresh failed and `value` is what worked before it.                |
| `failures: number`                  | Consecutive failures; drives the backoff, back to 0 on any success.          |
| `updatedAt: number \| undefined`    | When `value` was fetched, so a stale page can say how old it is.             |
| `parked: boolean`                   | Paused for want of a human (§6).                                             |
| `stopped: boolean`                  | A `fatal` failure ended the subscription; nothing will retry.                |
| `start(load, options?): () => void` | Begins a subscription; returns its teardown.                                 |
| `refresh(): Promise<boolean>`       | Refresh now; answers whether it actually ran.                                |

```ts
interface PollOptions {
  interval?: number; // default REFRESH_INTERVAL (30_000), floored at 1_000
  timeout?: number; // default LOAD_TIMEOUT (15_000)
  idleAfter?: number; // default IDLE_AFTER (4h)
  fatal?: (failure: unknown) => boolean;
  visibility?: Visibility;
  activity?: Activity;
  random?: () => number; // jitter source, injectable for deterministic tests
}
```

`failed` is a flag rather than the `error !== undefined` sentinel because a promise may reject with
`undefined`; treating "no error object" as "no failure" would render that as a confident empty
dashboard — tiles at zero, two empty states, and a meter spinning forever.

`error` is `unknown` rather than a string because the page owns the fallback sentence: `messageFor`
takes the thrown value and a page-specific default, and a helper that formatted the message itself
would have to know which page it was serving.

`refresh()` answers `boolean` because it declines when one is in flight and when there is no
subscription; a caller that awaited a `Promise<void>` and reported success would be reporting work
that did not happen.

`start` returns its teardown so that it is the whole body of an `$effect`:

```ts
$effect(() => {
  const slug = org.slug;
  if (!slug) return;
  return overview.start(() => load(slug), { fatal });
});
```

That ties a subscription's lifetime to the page _and_ to the org: the effect re-runs when the
route's org changes, the cleanup stops the previous subscription, and a new one starts against the
new slug. A new subscription resets every field above, so no warning, error, or stale marker
survives a navigation into a healthy org.

## §3 Generations — why a counter and not a slug comparison

`o/[org]/+layout.svelte:47` guards an out-of-order response by re-reading the slug and comparing
(`if (context.slug !== wanted) return;`). That works, but it is a rule every caller has to remember
and one that only detects the _org-changed_ case.

`Poller` holds a private `#generation` counter, incremented on every `start` and on every teardown.
A run captures the generation it belongs to and, on settling, applies its result only if the counter
still matches — on the success path, the failure path, and in the `finally`. A superseded run
therefore writes nothing at all: not `value`, not `error`, not `loading`, and not the in-flight flag
that the run replacing it now owns. This covers the org change, an unmount, and a restart while a
request is outstanding, in one predicate.

Two consequences that are easy to get wrong and are tested directly:

- **The teardown `start` returns is itself guarded.** Svelte can run a replaced effect's cleanup
  _after_ the new effect body, and an unguarded teardown would stop the subscription that had just
  replaced it — leaving a page that renders correctly and then never updates again.
- **Teardown drops the loader.** `refresh()` passes the current generation, so the counter cannot
  catch it; without clearing `#load`, a `refresh()` after teardown would run the previous org's
  loader, write its result, and arm a timer that the returned teardown can no longer stop.

The layout keeps its own hand-rolled guard for now: it fetches once rather than polling, and
converting it is a separate change.

## §4 Visibility and activity, injected

```ts
export interface Visibility {
  hidden(): boolean;
  subscribe(onChange: () => void): () => void;
}
export interface Activity {
  subscribe(onActive: () => void): () => void;
}
```

`documentVisibility` and `documentActivity` are the defaults, reading `document.visibilityState` and
listening for `visibilitychange` / input events respectively. Both are guarded on `typeof document`,
because `adapter-static` builds in Node where reaching for it breaks the build rather than a page —
and **the no-`document` answer to `hidden()` is `true`**: somewhere with no document has nobody
looking, and the safe answer to "can I tell?" is the one that does not poll.

The activity listeners are passive and capturing, so they never delay a scroll and are not swallowed
by a handler that stops propagation — a missed event reads as absence and would park a poll somebody
is watching.

Injecting both is the same move `locale.ts` makes with `LocaleStore` — for the same reason rather
than in the same shape: the interesting behaviour becomes ordinary state a test can drive instead of
a stubbed global. The shipped implementations get their own jsdom test, because a suite that only
ever sees the fakes cannot notice that the real seam broke.

## §5 What a failure means, by kind

`Poller` knows nothing about `ApiError` and must not: it is generic over `T` and the page owns the
API client. So the _page_ supplies the classifier:

```ts
function fatal(failure: unknown): boolean {
  if (!(failure instanceof ApiError)) return false;
  if (failure.isUnauthenticated) {
    session.clear();
    return true;
  }
  return failure.isNotFound || failure.status === 403;
}
```

- **`401`** — the session is gone (expired, or revoked from another device). Clearing the local copy
  is what lets the root layout's guard send the tab to `/login`, the same way every other flow does;
  the console never decides for itself that a cookie is still good.
- **`404`/`403`** — the org is gone, or this account is no longer in it. `CLAUDE.md`'s rule is that
  an org you are not in answers `404` exactly so it cannot be told from one that does not exist;
  either way the data on screen belongs to a page this reader can no longer see.
- **Everything else** — a `502`, a dropped connection, a timeout — is a blip, and rule 2 applies.

Without this, every failure after the first success is transient by definition, and a revoked
session renders as hours-old numbers under a 12px warning that reads like a hiccup — the console's
only signal that a credential stopped working, removed. `of-mcp` takes the opposite care for
exactly this reason: the principal is introspected per request so revocation takes effect on the
next call rather than at some unbounded later point.

## §6 Parking, and the idle session deadline it protects

`of-auth`'s `sessions.rs` gives a browser session a 14-day **idle** deadline and a 90-day absolute
one, and `sessions::resolve` slides the idle deadline forward on every authenticated request —
which silently assumes that requests imply a person. A polling console breaks that assumption: a
visible tab issues eight authenticated requests a minute forever, so the idle window never gets
half-spent and the session lives to the absolute cap regardless of whether anyone is at the desk.
The visibility pause does not cover it, because `visibilityState` stays `'visible'` for an
unattended second monitor — the exact scenario this feature is for.

So the poll parks itself after `IDLE_AFTER` (4 hours) with no pointer, key, wheel, touch, or focus
event: long enough to survive a working day at a desk, far short of 14 days. The activity
subscription stays live while parked, and the next input resumes polling immediately. The page says
it is parked, because a poll that stopped without saying so is the failure this whole design is
about.

The alternative — having the server not slide the deadline for requests the client marks as
background — was rejected: it puts a client-supplied hint into an auth decision.

## §7 The page

`web/src/routes/o/[org]/+page.svelte` fetches its four endpoints in one `Promise.all` inside the
loader, and reads `stats` / `recent` / `repos` / `usage` back out of `overview.value` as `$derived`.
The loading branch keys off `!overview.value` rather than `loading`, so the first paint before the
effect has run is honest rather than an empty dashboard presented as fact.

Three strings, all in six catalogs:

- `overview_refresh_failed({ reason, age })` — the stale line, in warn tone, with `role="status"`
  so the one reader who cannot see it turn amber is told too. `reason` is `messageFor(error,
m.error_network())`: an `ApiError` renders its own sentence, and everything else reaching there is
  a timeout or a dead socket, for which "check your connection" is the actionable version.
- `overview_paused()` — shown while `parked`.
- `overview_retrying()` — appended to the first-load error alert while the poll is still trying, so
  a page that says it failed does not also look abandoned.

## Testing

- `web/src/lib/poll.svelte.test.ts` — vitest with `vi.useFakeTimers()` and injected `Visibility`,
  `Activity`, and jitter source. 24 cases: each rule in §1, the fatal classification and its
  permanence, backoff widening and recovering, the timeout, parking and resuming, and every branch
  of the generation guard — including the late-cleanup case, the superseded _rejection_, and that
  teardown leaves nothing for `refresh()` to resurrect.
- `web/src/lib/poll.dom.test.ts` — jsdom, covering the shipped `documentVisibility` and
  `documentActivity` that every other test replaces.
- `web/src/routes/o/[org]/page.render.test.ts` — mounts the page in an org context with a stubbed
  `fetch`: a failed tick keeps the tiles and shows the warning; a recovered tick clears it; a failed
  first load replaces the page and says it is still retrying.

Gates: `npm run check` (svelte-check + tsc + the message-catalog check), `npm run lint`, `npm test`,
`npm run build`.

## Error Handling & Edge Cases

- **A refresh fails with data on screen** → `stale`, data kept with its age, retry after a widening
  interval.
- **The first load fails** → `failed`, the page renders `Alert` plus "still retrying", polling
  continues, and a later success clears it.
- **A rejection carrying `undefined`** → `failed` is still true; the page cannot render it as empty.
- **A `401`/`403`/`404`** → the poll stops, the data is dropped, and a `401` additionally clears the
  session so the root layout routes to `/login`.
- **A load that never settles** → failed as `PollTimeout` after 15 seconds; the poll continues.
- **The org changes mid-request** → the in-flight response is dropped (§3) and the page shows the
  new org's loading state.
- **The tab is hidden mid-request** → the response is applied, and no further tick is scheduled
  until it is visible again.
- **Nobody touches the tab for four hours** → parked, and said so (§6).

## Risks & Open Questions

- **The tick repeats two unbounded server queries.** `Jobs::stats` is a full aggregate over the
  org's jobs with a correlated `EXISTS` per pending row, and `list_repos` has no `LIMIT`. Both were
  paid once per page visit and are now paid every 30 seconds per open tab. Nothing crosses a tenant
  boundary and nothing is billable, so this is cost and availability — but a large tenant's console
  becomes a background load generator. Bounding them is a server-side change with its own spec.
- **No console `GET` is rate limited.** `of-auth`'s throttles cover login and client registration
  only, so the interval and the backoff are the only limits that exist on this traffic.
- **`parked` and `stale` are page-level notices, not a status bar.** If more pages adopt the poller,
  the two lines want a shared component rather than a copy each.
- **The interval is a constant, not configuration.** If a deployment ever needs a different value,
  it should arrive as a build-time constant rather than an `OF_*` key: the server holding an opinion
  about the console's refresh rate is the kind of workflow coupling constraint 2 exists to prevent.
