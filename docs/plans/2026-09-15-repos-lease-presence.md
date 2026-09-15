# Repos page lease presence — implementation plan

**Spec:** [`docs/specs/2026-09-15-repos-lease-presence-design.md`](../specs/2026-09-15-repos-lease-presence-design.md)
— read it first. This plan implements it exactly.

Goal: the Repos page shows, without a click, whether a repo has anyone actively holding a
lease on it right now, and the expanded panel explains what a lease is (and that it is
advisory, not a lock) via a heading, an always-visible one-line explainer, and a separate
keyboard-reachable info affordance — closing out `savvagent/otto-factory#168`.

## Status — 2026-09-15

Not started. Two tasks: backend (the `hasActiveLease` field + its test) then frontend (the
page changes + locale catalogs + its test).

## Global Constraints

These hold for every task below:

- No AI self-attribution anywhere (commit messages, code comments, docs, PR body).
- Run `cargo fmt --all` before every Rust commit.
- Every SQL statement lives in `of-core`. This plan adds none — `Tx::list_leases(None)`
  already exists and is already tenant-scoped; Task 1 only adds a new *call site* for it in
  `of-web`.
- Tenant isolation: `Task 1` adds a cross-org negative test for the new `hasActiveLease`
  field even though the underlying read is not new — per `CLAUDE.md`, "a tenant-scoped
  function without a cross-org negative test is not done" applies to the new field's
  behavior, not just to new SQL.
- No new migration, no new `catalog.rs` entry, no MCP tool — nothing here needs
  `of-billing::classify` or a `0007_rls.sql` change.
- This is an additive console-API change (Non-Negotiable Rule 6): `GET /api/orgs/{org}/repos`
  keeps its path, method, and existing fields; it only gains one field on each item. No
  version bump, no breaking-change flag, no `docs/clients/matrix.md` update.
- Tests need a real Postgres: `podman compose up -d` and a `.env` with `DATABASE_URL`
  (`cp .env.example .env`) before `cargo test -p of-web`.
- `web/` changes are gated by `npm run check` (svelte-check + tsc, and the locale-catalog
  completeness check via `scripts/check-messages.mjs`), `npm run lint` (prettier), and
  `npm test` (vitest).
- Runes only in `+page.svelte` — `$state`, no Svelte 4 stores, no `export let`.
- No out-of-band artifact is touched: no `Dockerfile`/`fly.toml` change, no migration, no
  `.github/workflows/` change, no `OF_*` config. State this explicitly at close-out rather
  than skip it.

## File Structure

| File | Responsibility |
|---|---|
| **Modify.** `crates/of-web/src/routes/repos.rs` | `RepoListItem` DTO; `list_repos` computes `hasActiveLease` from one grouped `Tx::list_leases(None)` read. |
| **Modify.** `crates/of-web/src/openapi.rs` | `RepoListItem` schema (`allOf` over `Repo`, mirroring `JobDetail`); `RepoList` now references it. |
| **Modify.** `crates/of-web/tests/console.rs` | New `#[sqlx::test]`: `hasActiveLease` reflects a real lease and never leaks across orgs. |
| **Modify.** `web/src/lib/types.ts` | New `RepoListItem extends Repo { hasActiveLease: boolean }`, mirroring `JobDetail extends Job`. |
| **Modify.** `web/src/lib/api.ts` | `repos()` return type: `Repo[]` → `RepoListItem[]`. |
| **Modify.** `web/src/routes/o/[org]/repos/+page.svelte` | Presence pill; `repos_hide_leases` relabel; heading + always-visible explainer; keyboard-reachable info affordance; `repos` state retyped to `RepoListItem[]`. |
| **Create.** `web/src/routes/o/[org]/repos/RepoHarness.svelte` | Test harness mounting the repos page inside an `OrgContext`, mirroring `queue/QueueHarness.svelte`. |
| **Create.** `web/src/routes/o/[org]/repos/page.render.test.ts` | Vitest: presence pill, toggle text/`aria-expanded`, info-affordance keyboard behavior. |
| **Modify.** `web/messages/{en,es,de,fr,it,hi}.json` | New keys: `repos_leases_heading`, `repos_leases_info_label`, `repos_leases_info_detail`, `repos_badge_in_use`; changed value: `repos_hide_leases`. |

## Task Order & Rationale

Backend first (Task 1): the frontend task reads `repo.hasActiveLease` off the real API
response, and its own component test stubs `fetch` directly rather than hitting a live
server, so the two tasks are not runtime-coupled — but writing the field's shape and its
test first means Task 2's manual verification pass (`cargo run -p of-server` + a real
browser) has real data to look at, and Task 2's plan text can cite the exact JSON field
name Task 1 committed rather than a placeholder.

## Task 1 — `of-web`: `hasActiveLease` on the repos list ⬜

**Files:** `crates/of-web/src/routes/repos.rs`, `crates/of-web/src/openapi.rs`,
`crates/of-web/tests/console.rs`

**Interfaces:** consumes `of_core::Tx::list_leases` (unchanged) and `of_core::repos::Repo`
(unchanged); produces `of-web`'s new `RepoListItem` DTO, serialized as `Repo`'s fields
flattened plus `hasActiveLease: boolean`, from the existing `GET /api/orgs/{org}/repos`.

- [ ] Add the failing test first. In `crates/of-web/tests/console.rs`, near the existing
      `// ----------------------------------------------------------------- leases` section
      (right after `the_lease_route_reports_the_resource_field`), add:

      ```rust
      #[sqlx::test(migrations = "../of-core/migrations")]
      async fn repos_list_reports_lease_presence_and_never_another_orgs(pool: PgPool) {
          let h = harness(pool);
          let rob = onboard(&h, "rob@acme.test").await;
          let mallory = onboard(&h, "mallory@evil.test").await;
          let acme = org_with_owner(&h, "acme", &rob).await;
          let evil = org_with_owner(&h, "evil", &mallory).await;

          let leased: of_core::ids::RepoId = {
              let created = Call::post("/api/orgs/acme/repos")
                  .with_session(&rob.session)
                  .json(serde_json::json!({ "slug": "api" }))
                  .send(&h.router)
                  .await;
              created.expect(StatusCode::CREATED);
              created.body["id"].as_str().unwrap().parse().unwrap()
          };
          Call::post("/api/orgs/acme/repos")
              .with_session(&rob.session)
              .json(serde_json::json!({ "slug": "quiet" }))
              .send(&h.router)
              .await
              .expect(StatusCode::CREATED);
          Call::post("/api/orgs/evil/repos")
              .with_session(&mallory.session)
              .json(serde_json::json!({ "slug": "api" }))
              .send(&h.router)
              .await
              .expect(StatusCode::CREATED);

          {
              let mut tx = h.db.begin(acme).await.unwrap();
              tx.acquire_lease(leased, "src/main.rs", rob.user, Some("claude-code"), None, None)
                  .await
                  .unwrap();
              tx.commit().await.unwrap();
          }

          let acme_list = Call::get("/api/orgs/acme/repos")
              .with_session(&rob.session)
              .send(&h.router)
              .await;
          acme_list.expect(StatusCode::OK);
          let by_slug = |body: &serde_json::Value, slug: &str| {
              body.as_array()
                  .unwrap()
                  .iter()
                  .find(|r| r["slug"] == slug)
                  .unwrap_or_else(|| panic!("no repo named {slug} in {body}"))
                  .clone()
          };
          assert_eq!(by_slug(&acme_list.body, "api")["hasActiveLease"], true);
          assert_eq!(by_slug(&acme_list.body, "quiet")["hasActiveLease"], false);

          let evil_list = Call::get("/api/orgs/evil/repos")
              .with_session(&mallory.session)
              .send(&h.router)
              .await;
          evil_list.expect(StatusCode::OK);
          assert_eq!(
              by_slug(&evil_list.body, "api")["hasActiveLease"], false,
              "evil's own unleased 'api' repo must never report acme's active lease"
          );
      }
      ```

- [ ] Run `cargo test -p of-web repos_list_reports_lease_presence_and_never_another_orgs` and
      confirm it fails to compile/run (the field does not exist yet).
- [ ] In `crates/of-web/src/routes/repos.rs`, add the `RepoListItem` struct directly above
      `list_repos` and change `list_repos`'s signature and body per the spec's §1 (import
      `of_core::ids::RepoId` is not required — `HashSet<_>` infers it from `Lease::repo_id`).
      Keep `get_repo`, `register_repo`, `update_repo` returning bare `Repo`, unchanged.
- [ ] Run `cargo test -p of-web repos_list_reports_lease_presence_and_never_another_orgs` again
      and confirm it passes. Then run the pre-existing lease/repo tests in the same file
      (`cargo test -p of-web --test console`) to confirm no regression — in particular
      `the_lease_route_reports_the_resource_field` and
      `another_orgs_data_is_not_merely_forbidden_it_is_invisible`.
- [ ] Update `crates/of-web/src/openapi.rs`: add the `RepoListItem` schema (spec §2, `allOf`
      pattern mirroring `JobDetail`) and change `RepoList`'s `items` to
      `reference("RepoListItem")`.
- [ ] Run `cargo test -p of-web` (the whole crate) to catch any OpenAPI-document test that
      snapshots the schema shape (e.g. a test asserting `paths["/api/orgs/{org}/repos"]` is an
      object, per `console.rs:1606` — confirm it still passes; it does not assert the referenced
      schema's contents, only that the path exists).
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all`.
- [ ] Commit: `git commit -m "of-web: report per-repo lease presence on the repos list"`.

## Task 2 — `web/`: presence pill, heading, explainer, info affordance ⬜

**Files:** `web/src/lib/types.ts`, `web/src/lib/api.ts`,
`web/src/routes/o/[org]/repos/+page.svelte`, `web/src/routes/o/[org]/repos/RepoHarness.svelte`,
`web/src/routes/o/[org]/repos/page.render.test.ts`, `web/messages/*.json`

**Interfaces:** consumes `GET /api/orgs/{org}/repos`'s new `hasActiveLease` field (Task 1);
produces no new interface — this is the console-only presentation layer.

- [ ] `web/src/lib/types.ts`: add, immediately after the existing `Repo` interface:

      ```ts
      export interface RepoListItem extends Repo {
        hasActiveLease: boolean;
      }
      ```

- [ ] `web/src/lib/api.ts`: change `repos: (org: string, includeInactive = false) =>
      get<Repo[]>(...)` to `get<RepoListItem[]>(...)`, adding `RepoListItem` to the file's
      type imports.
- [ ] Add locale keys to all six `web/messages/*.json` files (`en`, `es`, `de`, `fr`, `it`,
      `hi`) near the existing `repos_*` keys: `repos_leases_heading`, `repos_leases_info_label`,
      `repos_leases_info_detail`, `repos_badge_in_use`. Change `repos_hide_leases`'s value in
      all six (from "Hide leases"/its translation to a plain collapse word not naming the
      mechanism — sanity-check tone per locale rather than a literal word-for-word translation
      of the English "Hide"). Leave `repos_show_leases` untouched in all six.
- [ ] Run `npm run check` from `web/` — `check:messages` will fail until every new key has a
      value in every locale and no plural category is needed for these (no `{count}` in any of
      them); confirms the catalogs are complete before touching the component.
- [ ] Write the failing component test first. Create
      `web/src/routes/o/[org]/repos/RepoHarness.svelte`, mirroring
      `web/src/routes/o/[org]/OrgPageHarness.svelte` (not `queue/QueueHarness.svelte` — this page
      reads no query params, so it needs no `url` prop, no `setUrl` export, and no
      `$app/navigation` mocking; `OrgPageHarness.svelte`'s plain `provideOrg(new OrgContext(() =>
      slug))` + `<Page />` is the right-sized precedent).
- [ ] Create `web/src/routes/o/[org]/repos/page.render.test.ts` with (at minimum) these cases,
      following the `beforeEach`/`afterEach`/fake-timers/`vi.stubGlobal('fetch', ...)` harness
      pattern from `queue/page.render.test.ts`:
      - stubbing `/repos` to return one repo with `hasActiveLease: true` and one with `false`,
        and `/teams` to `[]`: after mount+settle, the "in use" pill text
        (`m.repos_badge_in_use()`'s English value) appears exactly once in `container.textContent`
        (present for the leased repo, absent for the other).
      - the toggle button's `aria-expanded` is `false` initially and its visible text is the
        current `repos_show_leases` value; clicking it sets `aria-expanded` to `true` and its
        text to the (updated) `repos_hide_leases` value.
      - with a row expanded: the new heading text (`repos_leases_heading`'s English value) and
        the always-visible explainer text (`repos_leases_note`'s value) are present even when the
        stubbed `/leases` response for that repo is `[]` (the empty-list branch) — this is the
        regression the ticket exists to fix, so assert it explicitly rather than only in the
        non-empty case.
      - the info-affordance button: a bare focus event must NOT open the popover (a real click
        fires focus before click, so an earlier onfocus-opens draft raced the click toggle and
        lost — the shipped version opens on click only, which a keyboard user reaches the same
        way via Enter/Space on a focused button); clicking it shows the popover text
        (`repos_leases_info_detail`'s value); dispatching a `keydown` with `key: 'Escape'` on it
        while open closes it; blurring it (dispatch a `focusout`/call `.blur()`) also closes it;
        and switching directly to a different row's toggle must not carry a stale open popover
        into that row.
- [ ] Run the new test file (`npx vitest run src/routes/o/[org]/repos/page.render.test.ts` from
      `web/`) and confirm it fails (the component has none of this yet).
- [ ] Implement the changes in `web/src/routes/o/[org]/repos/+page.svelte` per spec §3–§4:
      - Add `RepoListItem` to the `$lib/types` import; retype `let repos = $state<Repo[]>([]);`
        to `$state<RepoListItem[]>([]);` (spec §3 — this is the one file where the wider type
        must be named, not merely accepted structurally).
      - Add `let infoOpen = $state(false);`; reset it to `false` unconditionally at the top of
        `toggle()` on every call — not only when a row collapses — so switching directly from one
        expanded row to another never carries a stale open popover into the new row.
      - Add the presence pill next to the slug (spec §4), gated on `repo.hasActiveLease`.
      - Add the heading + info-affordance button + always-visible explainer block at the top of
        the lease `<div>` (spec §4's template), and delete `repos_leases_note`'s old
        rendering after the non-empty lease list (its content moved to the always-visible spot).
      - Nothing else in the expander (tracker-bindings block, admin buttons) changes.
- [ ] Run the component test again and confirm it passes.
- [ ] Run `npm run check`, `npm run lint`, and `npm test` (the whole suite) from `web/` and
      confirm no regression — in particular the existing `queue/page.render.test.ts` and
      `o/[org]/page.render.test.ts`, since both consume `api.repos()`'s return type.
- [ ] `npm run build` once, to confirm the SPA still builds cleanly with the retyped API client.
- [ ] Commit: `git commit -m "web: show at-a-glance lease presence and explain leases on the repos page"`.

## Addendum — `hasActiveLease` gated behind `?includeLeaseStatus=true` (post-review)

Both tasks above describe `hasActiveLease` as unconditionally computed. Review on the PR found
that `api.repos()` is also polled every 30 seconds by the org overview page and fetched by the
queue pages' repo picker, neither of which uses `hasActiveLease` — so the unconditional design
made every one of those call sites pay for an org-wide lease scan they never needed, far more
often than "once per Repos-page visit." Fixed post-review: `ListReposQuery` gained an
`include_lease_status` flag (default off); `list_repos` skips `list_leases` entirely unless it's
set; `RepoListItem.has_active_lease` became `Option<bool>` (omitted from the JSON response, not
sent as `false`, when not requested); `api.repos()` gained a matching third parameter, passed
`true` only from the Repos page's three call sites; the OpenAPI schema dropped `hasActiveLease`
from `RepoListItem`'s `required` array; and the backend test was extended to also assert the
field is absent (not `false`) when the flag is omitted. See the design spec's own addendum for
the full rationale. Committed as
`git commit -m "of-web: gate the repos-list lease-presence read behind an opt-in query param"`.

## Final gate (both tasks)

- [ ] `cargo test --workspace`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo fmt --all --check`
- [ ] `cd web && npm run check && npm run lint && npm test && npm run build`
