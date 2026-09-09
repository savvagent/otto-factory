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

## Goal & Success Criteria

The overview describes a queue that changes while nobody is touching the browser — agents claim jobs
over MCP, leases expire, counters move — and it is the page most likely to be left open on a second
monitor. Today it fetches once, in an `$effect` keyed on the org slug, and never again: the numbers
on screen are from whenever the tab was opened and nothing on the page admits it. It should refresh
itself, on an interval, without becoming worse than the static page it replaces.

- The overview refreshes every 30 seconds with no visible loading state on a refresh.
- A hidden tab does not poll, and refreshes as soon as it is shown again.
- A refresh that fails leaves the last good data on screen and says so; the page recovers on its own
  when the next refresh succeeds.
- Two refreshes never run concurrently, and a response belonging to an org the reader has navigated
  away from is never rendered.
- `npm run check`, `npm run lint`, `npm test`, `npm run build` all pass.

## Scope

**In:**

- A new `web/src/lib/poll.svelte.ts` exporting `Poller<T>` — the polling state machine, reusable by
  any console page that displays live state.
- `web/src/lib/poll.svelte.test.ts` — vitest coverage of the four rules in §1.
- `web/src/routes/o/[org]/+page.svelte` rewired to fetch through the poller.
- One new message key, `overview_refresh_failed`, in all six catalogs (`web/messages/*.json`).

**Out:**

- **No server change of any kind.** No MCP tool, no console route, no SQL, no `of-core` function, no
  migration, no `OF_*` config key. The endpoints polled (`queueStats`, `jobs`, `repos`, `usage`)
  already exist and are unchanged.
- **No push transport.** A WebSocket or SSE feed from `of-web` would make the console current
  faster, and it is deliberately not built here: it is a new public interface, a new connection to
  hold per reader, and a server-side opinion about how a client should observe the queue. Polling an
  existing read API needs none of that. Constraint 2 — a capability that could live outside the
  server does.
- **No adoption by the other pages.** `/queue` and `/repos` have the same staleness problem and the
  helper is written for them, but converting them is not in this change; each has its own filter and
  pagination state to think about.
- **No user-configurable interval and no manual refresh button.** A control that changes how often
  the console reads is a workflow opinion, and a refresh button on a page that refreshes itself is
  furniture.
- **No "updated N seconds ago" label** — see Assumptions.

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
- **All four endpoints are refetched together on every tick.** Staggering them to spread load would
  let the tiles and the recent-jobs list describe the queue at two different instants — five pending
  above a list of six — which is a worse failure than a fractionally larger burst.
- **A failed refresh keeps the last good data.** The alternative — swapping a working dashboard for
  an error alert on one 502 — is strictly worse than being one interval behind. `CLAUDE.md`'s "no
  silent fallbacks" rule is about *resolution* failures, where guessing produces a wrong answer;
  this is a display already labelled as not-current, which is the opposite of a silent guess.
- **There is no "updated 12 seconds ago" label.** Keeping one honest needs a second timer running at
  human resolution (the string rots between ticks otherwise), which is a lot of machinery to say
  "nothing has gone wrong". The page says something only when a refresh has failed.
- **Polling continues after a failed first load.** A page opened during a blip fills itself in
  rather than waiting for someone to press reload.
- **`document.visibilityState` is the pause signal**, not `window.blur`. A tab behind another window
  is still visible and still being read; a backgrounded tab is not.

## §1 The four rules

The helper exists because polling is easy and *bearable* polling is four rules that are each easy to
omit one at a time. Each is stated with the failure it prevents:

1. **A refresh is not a load.** `loading` is true only while the first result of a subscription is
   outstanding. Flipping it on every tick replaces a working page with a skeleton twice a minute —
   the numbers are supposed to change in place.
2. **A failed refresh keeps the last good result.** `error` means "there is nothing to render"; a
   failure on top of data sets `stale` and leaves `value` alone.
3. **A hidden tab does not poll**, and refreshes the moment it is shown again. A backgrounded tab
   polling all weekend is waste; a returning reader wants current data, not up to a full interval of
   stale data.
4. **Refreshes never overlap.** The interval runs from the end of one refresh to the start of the
   next — a chained `setTimeout`, never `setInterval` — so a slow response delays the next tick
   instead of stacking behind it. `refresh()` while one is in flight is a no-op for the same reason.

## §2 `Poller<T>` — shape

`web/src/lib/poll.svelte.ts`. A rune-backed class, per the `web/` convention that shared reactive
state lives in a `.svelte.ts` module (`session.svelte.ts`, `org.svelte.ts`).

| Member | Meaning |
| --- | --- |
| `value: T \| undefined` | The last successful result; `undefined` before the first lands. |
| `loading: boolean` | True only while the first result of the current subscription is outstanding. |
| `error: unknown` | Set only when there is nothing to render. The page maps it through `messageFor`. |
| `stale: boolean` | The last refresh failed and `value` is what worked before it. |
| `start(load, options?): () => void` | Begins a subscription; returns its teardown. |
| `refresh(): Promise<void>` | Refresh now; a no-op while one is in flight. |

`error` is `unknown` rather than a string because the page owns the fallback sentence: `messageFor`
takes the thrown value and a page-specific default (`m.overview_load_failed()`), and a helper that
formatted the message itself would have to know which page it was serving.

`start` returns its teardown so that it is the whole body of an `$effect`:

```ts
$effect(() => {
  const slug = org.slug;
  if (!slug) return;
  return overview.start(() => load(slug));
});
```

That is what ties a subscription's lifetime to the page *and* to the org: the effect re-runs when
the route's org changes, the cleanup stops the previous subscription, and a new one starts against
the new slug.

## §3 Generations — why a counter and not a slug comparison

`o/[org]/+layout.svelte:47` guards an out-of-order response by re-reading the slug and comparing
(`if (context.slug !== wanted) return;`). That works, but it is a rule every caller has to remember
and one that only detects the *org-changed* case.

`Poller` holds a private `#generation` counter, incremented on every `start` and on every teardown.
A run captures the generation it belongs to and, on settling, applies its result only if the counter
still matches. A superseded run therefore writes nothing at all — not `value`, not `error`, not
`loading`, and not the in-flight flag that the run replacing it now owns. This covers the org change,
an unmount, and a restart while a request is outstanding, in one predicate, expressed once.

## §4 Visibility, injected

```ts
export interface Visibility {
  hidden(): boolean;
  subscribe(onChange: () => void): () => void;
}
```

`documentVisibility` is the default implementation, reading `document.visibilityState` and listening
for `visibilitychange`, guarded on `typeof document` so importing the module is safe anywhere.

Injecting it is the same move `locale.ts` makes with `LocaleStore`, and for the same reason: the
interesting behaviour (pause while hidden, refresh on return) becomes ordinary state a test can
drive, instead of a stubbed global that only works under jsdom.

## §5 The page

`web/src/routes/o/[org]/+page.svelte` fetches its four endpoints in one `Promise.all` inside the
loader, and reads `stats` / `recent` / `repos` / `usage` back out of `overview.value` as `$derived`.
The `{#if error}` / `{:else if loading}` / `{:else}` structure is unchanged; `loading` no longer
needs its `&& !stats` companion, because `Poller.loading` is already first-load-only.

One new string, shown in the page header only while `overview.stale`:

- `overview_refresh_failed` — "Refresh failed — showing the last result." — in all six catalogs, per
  the console's i18n rule; `scripts/check-messages.mjs` fails the build if any locale lacks it.

## Testing

`web/src/lib/poll.svelte.test.ts`, vitest with `vi.useFakeTimers()` and a fake `Visibility`. No jsdom
and no stubbed `document`: injection is what makes that possible.

- The first load sets `loading`, and a subsequent refresh does not — rule 1.
- A first-load failure sets `error` with no `value`, and a later tick recovers it.
- A refresh failure keeps `value`, sets `stale`, and clears `stale` on the next success — rule 2.
- A hidden tab does not poll across several intervals, refreshes immediately on becoming visible,
  and resumes its interval from there; going hidden mid-flight still parks the poll — rule 3.
- Concurrent `refresh()` calls issue one request; a slow refresh delays rather than stacks the next
  tick — rule 4.
- Teardown clears the timer and unsubscribes from visibility.
- A superseded subscription's response is dropped, and the replacement runs even while the old
  request is still outstanding — §3.

Gates: `npm run check` (svelte-check + tsc + the message-catalog check), `npm run lint`, `npm test`,
`npm run build`.

## Error Handling & Edge Cases

- **A refresh fails with data on screen** → `stale`, data kept, retry next tick.
- **The first load fails** → `error`, the page renders `Alert`, polling continues, and a later
  success clears it.
- **The org changes mid-request** → the in-flight response is dropped (§3) and the page shows the
  new org's loading state.
- **The tab is hidden mid-request** → the response is applied, and no further tick is scheduled
  until it is visible again.
- **A `401` mid-session** (the session cookie expired while the tab sat open) → surfaces as `stale`,
  not as a sign-out. The console's session handling lives in the root layout, and a poller that
  redirected on a `401` would be a second, competing authority on who is signed in.

## Risks & Open Questions

- **`stale` does not distinguish a transient `502` from an expired session.** Both read as "refresh
  failed". If that proves confusing in use, the fix is for the page — not the poller — to branch on
  `ApiError.isUnauthenticated` and say something different.
- **The interval is a constant, not configuration.** If a deployment ever needs a different value,
  it should arrive as a build-time constant rather than an `OF_*` key: the server holding an opinion
  about the console's refresh rate is the kind of workflow coupling constraint 2 exists to prevent.
