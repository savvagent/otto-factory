# Repos page lease presence design

> **Status:** IMPLEMENTED — gives the Repos page an at-a-glance "who's here" signal per repo and
> explains the underlying lease concept in place, per issue #168. Shipped in PR #182.

## Premise corrections

The ticket's "Actual" section states the toggle is labeled "Show leases" / "Hide leases". On
inspection, `repos_show_leases` (`web/messages/en.json`) already reads "Who is in here?" — it
was written that way when the i18n migration introduced the key (commit `c4cbcb7`) and has never
said "Show leases". Only `repos_hide_leases` ("Hide leases") still leads with the mechanism name.
This spec treats the collapse-state label as the one piece of AC bullet 2 not yet satisfied,
rather than relabeling a string that already reads correctly — re-wording `repos_show_leases`
too would be churn across all six locale files for no behavioral gain.

Every other premise in the ticket and its attached `[Spec]` comment holds: there is no
at-a-glance presence signal on the collapsed row, the expanded lease panel has no heading (unlike
the sibling Trackers block in the same expander), and the existing explanatory text
(`repos_leases_note`) only renders when the lease list is non-empty — exactly backwards from when
a first-time reader most needs it.

## Scope

**In:**

- `web/src/routes/o/[org]/repos/+page.svelte`: collapsed-row presence pill, `repos_hide_leases`
  relabel, a heading above the lease list, an always-visible short explainer under that heading,
  and a keyboard-reachable info affordance carrying one further sentence connecting "lease" to
  the MCP tool names.
- `web/src/lib/types.ts`: a `RepoListItem` type (`Repo` plus `hasActiveLease: boolean`) for what
  `api.repos()` now returns.
- `crates/of-web/src/routes/repos.rs`: `list_repos` computes `hasActiveLease` per repo from one
  grouped lease read, via a new `RepoListItem` response DTO local to `of-web`.
- `crates/of-web/src/openapi.rs`: a `RepoListItem` schema (`Repo` + `hasActiveLease`, via the same
  `allOf` pattern `JobDetail` already uses), and `RepoList`'s items updated to reference it.
- New/changed locale keys in `web/messages/{en,es,de,fr,it,hi}.json`.
- Tests: an of-web `#[sqlx::test]` integration test and a Vitest component test.

**Out (per the ticket and per `CLAUDE.md`'s three constraints):**

- Lease semantics, TTL, or enforcement — leases remain advisory. Nothing here makes a lease
  block, expire differently, or become visible to `of-mcp` differently; `list_leases` and
  `acquire_lease`'s behavior and descriptions are unchanged.
- Any new `of-core` SQL function or migration. `Tx::list_leases(None)` already returns every live
  lease for the calling org in one query (`crates/of-core/src/leases.rs`); this change is a new
  *use* of that existing, already-tenant-scoped read from a new call site, not a new read. No
  new tenant table, no `0007_rls.sql` change — the two-guard tenant-isolation rule already
  covers this data through the `Tx` this handler already holds.
- A new console route or `catalog.rs` entry. `GET /api/orgs/{org}/repos` keeps its path and
  method; only its response items grow one field, which is the additive case Non-Negotiable
  Rule 6 says needs no version bump and no breaking-change flag.
- Restructuring the tracker-bindings sub-panel, the admin registration form, or anything else in
  the same expander.
- Real-time/live-updating presence. This is a page-load/refresh-time signal, matching how the
  rest of this page already works (fetch-on-load, fetch-on-expand — never a held connection).
- A count of active leases. The ticket's AC asks only for a boolean ("whether the repo has any
  active lease"); see Assumptions for why this spec does not build the richer count option.

## Assumptions

- **Boolean presence, not a count.** The AC's own wording is "whether the repo has any active
  lease right now" — a boolean. A count would need a `GROUP BY … COUNT(*)` instead of a
  membership check, a locale-plural key (`one`/`other`/`many`, genuinely per-locale per
  `CLAUDE.md`'s i18n section) instead of a fixed string, and buys no clarity the AC actually
  asks for. Boolean is the smaller, fully-compliant surface.
- **Approach (a) — a field added to `GET /api/orgs/{org}/repos`'s existing response — over a
  dedicated endpoint.** The ticket's own `[Spec]` comment leaves this as an implementer choice.
  This spec picks (a) for a reason the comment did not have available: `Tx::list_leases(None)`
  already exists, is already tenant-scoped, and already returns every live lease for the whole
  org in one query — the exact shape a batched per-page read needs. Approach (b) would add a new
  route, a new `catalog.rs` entry, and a new OpenAPI schema for a read that is already one query
  away. Approach (a) is strictly less surface for the same guarantee.
- **The "info affordance" (AC bullet 5) is a distinct, real widget, not merely a synonym for the
  always-visible inline explainer (AC bullet 3).** The two are separate bullets in the ticket
  body, not the `[Spec]` comment's elaboration — bullet 3 asks for a heading plus a short inline
  explanation; bullet 5 separately requires that "the info affordance is keyboard-reachable, not
  hover-only," which is only a meaningful constraint if something interactive exists to be
  hover-only. This spec builds both: an always-visible one-line explainer under the new heading
  (satisfies bullet 3 unconditionally, regardless of whether anyone touches the affordance), and
  a small `<button>` next to the heading that reveals one further sentence on click or focus,
  closing on Escape or on losing focus (satisfies bullet 5 with real content, not a duplicate of
  the always-visible line).
- **No new icon library.** Per the ticket's own note, this console has no Phosphor (or other)
  icon import anywhere in `web/src`. The info affordance is a plain `<button>` with a text glyph
  (`i`), matching the existing plain-`<button>`/plain-`<span>` vocabulary already used for the
  toggle and the retired badge on this same page.
- **The presence pill reuses the existing badge/pill vocabulary on this page, with a semantic
  color token.** `repos_badge_retired` already renders as a bordered pill next to the slug; the
  new "in use" pill matches that shape but borrows the `busy` color token `StatusPill.svelte`
  already uses for `in-progress` jobs (`border-busy/50 bg-busy/10 text-busy`) — "someone is
  actively working here" is the same shade of "in progress," and introduces no new color
  vocabulary. Per `StatusPill`'s own documented rule ("colour *and* the word, never colour
  alone"), the pill always carries text, never a bare dot.
- **`hasActiveLease` is optional on the shared frontend `Repo` type's call sites, not part of
  `Repo` itself.** `POST`/`PATCH`/`GET .../{repo}` continue to return `of_core::repos::Repo`
  verbatim and never populate this field; mixing it into the shared `Repo` interface would claim
  a field that most of that type's call sites never send. A `RepoListItem extends Repo` type
  (mirroring the existing `JobDetail extends Job` pattern in `web/src/lib/types.ts`) keeps the
  two honest. The three other pages that consume `api.repos()`'s return type structurally accept
  the wider type without any change on their part.

## Goal & Success Criteria

Goal: a person scanning the Repos page can tell, without clicking anything, whether each repo
has anyone active on it right now, and — if they open a row — immediately understands what a
"lease" is and that it is not a lock.

Success criteria:

1. A repo with `hasActiveLease: true` shows a pill next to its slug on the collapsed row; a repo
   with no active lease shows nothing extra there.
2. The demoted-toggle button's visible text no longer leads with "lease" in either state
   (`repos_show_leases` already satisfies this; `repos_hide_leases` is fixed to match).
3. The expanded lease panel has a heading matching the visual weight of the sibling `Trackers`
   heading, with a short explanatory sentence always visible beneath it — including when the
   lease list is empty.
4. A separate, keyboard-reachable info affordance next to that heading reveals one further
   sentence connecting "lease" to the `list_leases`/`acquire_lease` MCP tool names — a disclosure
   button (`aria-expanded`/`aria-controls`) opened by click (equally reachable via Tab then
   Enter/Space, since that is native `<button>` behavior) and closed by Escape or losing focus.
   An earlier draft of this spec also opened it on bare focus, but a real mouse click fires
   `focus` before `click`, so that handler raced the click toggle and lost — removed during
   implementation once the code review below caught it.
5. The word "lease" still appears in the expanded panel (the explainer sentence, the info
   affordance's sentence, and the lease-row fields all use it) — demoted from the entry point,
   never erased.
6. `npm run check` passes with the new/changed strings present, complete, and placeholder-correct
   across all six locale catalogs.
7. `GET /api/orgs/{org}/repos`'s new `hasActiveLease` field is computed inside the same `Tx` the
   handler already holds (`ctx.org.id`-scoped) and is verified by a `#[sqlx::test]` that a repo
   in one org never reports another org's lease activity.
8. No regression to the tracker-bindings sub-panel's layout, heading, or admin edit flow in the
   same expander.

## Approach

### 1. `of-web`: one grouped read added to the existing list handler

`crates/of-web/src/routes/repos.rs`'s `list_repos` currently returns `Vec<Repo>`. It changes to:

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoListItem {
    #[serde(flatten)]
    pub repo: Repo,
    /// Whether an unexpired lease is held on any resource in this repo right
    /// now. Computed once for the whole page from the same live-lease read
    /// the per-repo panel already uses (`Tx::list_leases`), not fetched per
    /// row — the N+1 concern this page's own module doc already calls out
    /// for why per-repo lease fetch is lazy today.
    pub has_active_lease: bool,
}

pub async fn list_repos(
    State(state): State<AppState>,
    ctx: OrgCtx,
    axum::extract::Query(q): axum::extract::Query<ListReposQuery>,
) -> ApiResult<Json<Vec<RepoListItem>>> {
    let mut tx = state.db.begin(ctx.org.id).await?;
    let repos = tx.list_repos(q.include_inactive, None).await?;
    let repos = visible_repos(&mut tx, &ctx, repos).await?;

    let active: std::collections::HashSet<_> = tx
        .list_leases(None)
        .await?
        .into_iter()
        .map(|l| l.repo_id)
        .collect();
    tx.commit().await?;

    Ok(Json(
        repos
            .into_iter()
            .map(|repo| RepoListItem {
                has_active_lease: active.contains(&repo.id),
                repo,
            })
            .collect(),
    ))
}
```

`Tx::list_leases(None)` is unmodified — it already exists, already filters `org_id = $1` and
`expires_at > now()` (`crates/of-core/src/leases.rs`), and already has a cross-org negative test
(`leases_are_invisible_across_orgs`, `crates/of-core/tests/isolation.rs:454`). Calling it with
`None` inside the same `Tx` as `list_repos`/`visible_repos` keeps everything on one round trip
and one transaction, matching this handler's existing shape.

A repo the caller cannot see (filtered out by `visible_repos`) never appears in the response at
all, so its lease-presence boolean — even though `list_leases(None)` reads leases across every
repo in the org — is never exposed for a repo the caller could not already see by slug and name.

### 2. `openapi.rs`: a new schema, not a change to `Repo`

`Repo` stays exactly as documented today — `GET /repos/{repo}`, `POST /repos`, and
`PATCH /repos/{repo}` keep returning that shape unchanged. A new `RepoListItem` schema, following
the same `allOf` pattern `JobDetail` already uses for `Job`:

```rust
"RepoListItem": {
    "allOf": [
        reference("Repo"),
        {
            "type": "object",
            "properties": {
                "hasActiveLease": {
                    "type": "boolean",
                    "description":
                        "Whether an unexpired lease is held on any resource in this \
                         repo right now — computed once for the whole list, never \
                         fetched per repo.",
                },
            },
            "required": ["hasActiveLease"],
        },
    ],
},
"RepoList": { "type": "array", "items": reference("RepoListItem") },
```

### 3. Frontend types and API client

`web/src/lib/types.ts` gains, mirroring the existing `JobDetail extends Job` pattern:

```ts
export interface RepoListItem extends Repo {
  hasActiveLease: boolean;
}
```

`web/src/lib/api.ts`'s `repos()` return type changes from `Repo[]` to `RepoListItem[]`. The three
other call sites (`o/[org]/+page.svelte`, `o/[org]/queue/+page.svelte`,
`o/[org]/queue/[job]/+page.svelte`) type their local state as `Repo[]`; `RepoListItem[]` is
structurally assignable to `Repo[]` (a superset of its fields), so none of them need a code
change — `hasActiveLease` is simply an extra field they never read.

This does **not** extend to `web/src/routes/o/[org]/repos/+page.svelte` itself — the one file §4
edits. Its `let repos = $state<Repo[]>([]);` (currently line 37) must become
`let repos = $state<RepoListItem[]>([]);`, with `RepoListItem` added to the existing `import type
{ Lease, Repo, Team, TrackerBinding, TrackerProvider } from '$lib/types';` line, because §4's
template reads `repo.hasActiveLease` off exactly this array. Left as `Repo[]`, the assignment from
`api.repos()` still compiles (an incoming `RepoListItem[]` is assignable into a `Repo[]`-typed
variable), but `repo.hasActiveLease` inside the `{#each repos as repo}` block does not exist on
`Repo` and fails `svelte-check`/`tsc` — i.e. `npm run check`. This is the one place the wider type
must actually be named, not merely accepted structurally.

### 4. `+page.svelte`: presence pill, heading, explainer, info affordance

- **Presence pill.** In the same `<div class="flex items-center gap-2">` that already renders
  the slug and the retired badge, add, when `repo.hasActiveLease`:

  ```svelte
  <span class="rounded-full border border-busy/50 bg-busy/10 px-2 py-0.5 text-xs text-busy">
    {m.repos_badge_in_use()}
  </span>
  ```

- **Toggle relabel.** `repos_show_leases` is unchanged. `repos_hide_leases`'s value changes (all
  six locales) from "Hide leases" to a plain collapse word ("Hide") that does not name the
  mechanism, matching the framing `repos_show_leases` already uses.

- **Heading + always-visible explainer**, added at the top of the existing lease `<div>` (the one
  currently gated on `expanded === repo.slug`), before the loading/empty/list branches:

  ```svelte
  <div class="flex items-center gap-1.5">
    <h3 class="text-xs font-semibold tracking-wide text-muted uppercase">
      {m.repos_leases_heading()}
    </h3>
    <div class="relative">
      <button
        type="button"
        class="flex h-4 w-4 items-center justify-center rounded-full border border-edge text-[10px] text-faint hover:text-ink"
        aria-label={m.repos_leases_info_label()}
        aria-expanded={infoOpen}
        aria-controls={`repos-lease-info-${repo.slug}`}
        onclick={() => (infoOpen = !infoOpen)}
        onblur={() => (infoOpen = false)}
        onkeydown={(e) => {
          if (e.key === 'Escape') infoOpen = false;
        }}
      >
        i
      </button>
      <div
        id={`repos-lease-info-${repo.slug}`}
        hidden={!infoOpen}
        class="absolute z-10 mt-1 w-64 rounded-md border border-edge bg-raised p-2 text-xs text-faint shadow-lg"
      >
        {m.repos_leases_info_detail()}
      </div>
    </div>
  </div>
  <p class="mt-1 text-xs text-faint">{m.repos_leases_note()}</p>
  ```

  `repos_leases_note`'s existing copy ("Leases are advisory. The server cannot see a git push, so
  a lease makes a collision visible — it does not prevent one.") moves here, unconditionally
  visible, replacing its current placement after the non-empty lease list (where it never
  rendered on the empty-state branch — exactly the ticket's complaint). `infoOpen` is one
  `$state<boolean>` per page instance; only one row can be expanded at a time already (`expanded`
  is a single value), so it does not need to be keyed per repo. `toggle()` resets it to `false`
  unconditionally at the top, on every call — not only on collapse — so switching directly from
  one expanded row to another never carries a stale open popover into the new row.

  This is a disclosure widget, not a tooltip: the trigger's `aria-expanded`/`aria-controls` pair
  is what a screen reader needs, so the popover carries no ARIA role of its own. It is rendered
  unconditionally once its row is expanded and toggles visibility via the `hidden` attribute
  rather than `{#if}`, so `aria-controls` always resolves to a real element instead of dangling
  while collapsed. `onclick` is the sole open/close trigger — a native `<button>` already fires
  `click` from Enter/Space while focused, so a keyboard user gets the same behavior as a mouse
  click without a separate `onfocus` handler. (An `onfocus`-opens handler was tried first and
  removed: a real mouse click fires `focus` before `click`, so the click's toggle immediately
  undid what focus had just opened, and clicking the affordance never actually opened it.)
  Closing on blur relies on the platform behavior that a click on a non-focusable element (the
  popover has no focusable content) moves focus to `<body>`, which blurs the button — covering
  "outside click" without a manual document-level listener. Escape is handled explicitly because
  it does not blur the button on its own.

  New keys: `repos_leases_heading` ("Who's here"), `repos_leases_info_label` ("About leases" —
  the button's accessible name), `repos_leases_info_detail` (one sentence connecting "lease" to
  `list_leases`/`acquire_lease`, e.g. "Leases are what the `list_leases` and `acquire_lease` MCP
  tools coordinate through, and expire on their own if never renewed."), `repos_badge_in_use`
  ("In use").

### 5. i18n

Every new/changed key goes through the existing `m.repos_*()` Paraglide convention, added to all
six `web/messages/*.json` catalogs. None of the new strings take a plural-sensitive count, so no
locale needs `one`/`other`/`many` variants (per Assumptions). `npm run check` (which runs
`scripts/check-messages.mjs`) is the gate that would catch a missing key, a dropped placeholder,
or an incomplete catalog.

## Error Handling & Edge Cases

- **Repo with zero leases**: no pill on the collapsed row (not a "0 active" badge) — the ticket's
  own framing: avoid noise on the common case.
- **The grouped lease read itself failing**: `list_repos` already propagates any `Tx` error via
  `ApiResult`'s `?`; a failure here fails the whole repos-list call the same way a `list_repos`
  or `visible_repos` failure already would. There is no separate degraded mode to design, because
  this is one query added to an existing single-transaction handler, not an added network hop
  approach (b) would have introduced.
- **Non-admin viewers**: the pill, heading, and explainer are all read-only info with no new
  permission gate — leases are already visible to any org member who can see the repo at all
  (`require_visible`/`visible_repos`), unchanged by this ticket.
- **A team-scoped repo the caller cannot see**: never appears in the response at all (existing
  `visible_repos` filtering, unchanged), so its lease-presence boolean is never computed for
  display to that caller in the first place.

## Testing Approach

- **`of-web` integration test** (`crates/of-web/tests/console.rs`, `#[sqlx::test]`, mirroring the
  existing `the_lease_route_reports_the_resource_field` and
  `another_orgs_data_is_not_merely_forbidden_it_is_invisible` fixtures): two repos in one org, a
  live lease acquired on only one of them via `tx.acquire_lease` (the same "seed past the API"
  pattern the existing lease test already uses, since leases are written by an agent over MCP,
  never by the console); `GET /api/orgs/{org}/repos` reports `hasActiveLease: true` for the
  leased repo and `false` for the other. A second org with its own unleased repo, checked in the
  same test, confirms its `hasActiveLease` is `false` and is never affected by the first org's
  active lease — the cross-org negative test `CLAUDE.md` requires for a tenant-scoped read, even
  though the underlying `list_leases` query itself is not new.
- **Vitest component test**, following the existing `page.render.test.ts` +
  `*Harness.svelte` pattern (`web/src/routes/o/[org]/queue/page.render.test.ts` /
  `QueueHarness.svelte`): a `RepoHarness.svelte` mounting the repos page inside an `OrgContext`,
  covering: the presence pill renders when `hasActiveLease` is true and is absent when false; the
  toggle's `aria-expanded` state and visible text; the info button opens its tooltip on focus and
  on click, and closes on Escape and on blur.
- **`npm run check`**: must pass with the new locale entries complete across all six catalogs.
- Manual pass: confirm the new "Who's here" heading matches the "Trackers" heading's weight and
  spacing, and that the collapsed-row pill does not crowd the provider/branch/team meta line at
  narrow viewport widths.

## Risks & Open Questions

- **Low risk**: the info-affordance's blur-closes-the-popover behavior assumes no focusable
  content inside the popover `<div>` (true today — it is a single sentence). If a future change
  adds an interactive element inside it (a link, say), the blur handler would need to check
  `event.relatedTarget` before closing, or a click on that new element would close the popover
  out from under it.
- **Low risk**: reusing the `busy` color token for the presence pill borrows an existing semantic
  (`in-progress` job status) for a new meaning (a repo currently in use). Both mean roughly "work
  is happening here now," and no new color is introduced, but a reviewer with a stronger opinion
  on this may prefer a different existing token (`ok`, `accent`) — either is a one-line change if
  requested.

## Addendum — `hasActiveLease` made opt-in (post-review)

The rest of this document describes `hasActiveLease` as unconditionally computed on every
`GET /api/orgs/{org}/repos` call. Review on the PR caught a real cost this spec underestimated:
`api.repos()` is not called only by the Repos page — the org overview page polls it every 30
seconds (`o/[org]/+page.svelte`'s `Poller`), and the queue pages' repo picker fetches it too, and
neither reads `hasActiveLease` at all. Unconditionally adding an org-wide `list_leases(None)` scan
to this handler meant every one of those unrelated call sites paid for a read it never used, on a
30-second cadence rather than once per Repos-page visit — a materially different cost profile than
"one extra query on page load," and one this spec did not weigh correctly in the Assumptions above.

**Fix:** `hasActiveLease` is computed only when the caller passes `?includeLeaseStatus=true`.
`ListReposQuery` gains that boolean (default `false`); `list_repos` skips the `list_leases` call
entirely when it is unset; `RepoListItem.has_active_lease` becomes `Option<bool>`, `#[serde(skip_serializing_if
= "Option::is_none")]` — omitted from the wire response rather than sent as `false`, so a caller
that never asked can't mistake "not computed" for "known absent." `web/src/lib/api.ts`'s `repos()`
gains a matching third parameter; only the Repos page's three call sites pass `true`. The
OpenAPI schema drops `hasActiveLease` from `RepoListItem`'s `required` array to match. Everything
else in this document — the boolean-not-count choice, approach (a) over a dedicated endpoint, the
UI shape, the i18n keys — is unaffected; this addendum changes only how the field's presence in
the *response* is gated, not what it means or how the UI reads it.
