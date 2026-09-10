# Leases: generalize (repo, branch) to (repo, resource) — implementation plan

Goal: rename `repo_leases.branch` to `resource` throughout the stack — schema, `of-core`,
the MCP surface, and the console — so agents can lease things that are not branches
(a staging slot, a migration lock, a shared fixture), while `branch` survives as a
deprecated input alias on `acquire_lease` so no deployed caller breaks. Closes
savvagent/otto-factory#69.

**Spec:** `docs/specs/2026-09-10-leases-generalize-resource-design.md` — read it first.
This plan implements it exactly.

---

## Global Constraints

These hold for every task in this plan:

- No AI self-attribution anywhere — commits, comments, docs, PR body.
- Run `cargo fmt --all` before every Rust commit.
- Every SQL statement lives in `of-core`. A query in `of-mcp`/`of-web` is a bug.
- Tests need a real Postgres: `podman compose up -d` (Postgres 16 on host port 15433) and
  a `.env` with `DATABASE_URL` (`cp .env.example .env`).
- A migration is a new file, never an edit to one already applied. `0027` is the next free
  number (`0026_job_claim_expiry.sql` is the latest applied).
- The resource string is never validated against a list or a format, anywhere in the stack.
- **Per-task gates are crate-scoped** (`-p of-core`, `-p of-mcp`) until the final Rust task.
  `Tx::acquire_lease`/`renew_lease`/`release_lease`/`list_leases` are positional-argument
  functions, so renaming their `branch` parameter to `resource` does **not** break any
  existing call site's compilation by itself (Rust does not name-check positional args) —
  confirmed by grep: no call site in `crates/of-core/tests/queue.rs`,
  `crates/of-core/tests/isolation.rs`, or `crates/of-web/src/routes/repos.rs` reads a
  `.branch` field or passes a named argument. The one real cross-crate compilation
  dependency is `crates/of-mcp/tests/tools.rs`'s three `AcquireLeaseArgs { branch: ..., }`
  struct literals (lines 1164, 1176, 1208), which break once `branch` changes from
  `String` to `Option<String>` in Task 2 — Task 2 fixes them in the same task, so no task
  is left in a non-compiling state at its own gate. Run `cargo test --workspace` +
  `cargo clippy --all-targets -- -D warnings` unscoped only after Task 2 (the last Rust
  task) completes, as that task's final gate.
- `web/` is a separate toolchain (`npm run check`/`lint`/`test`/`build`), unaffected by the
  Rust build; Task 3 owns it.

## File Structure

| File | Responsibility |
|---|---|
| **Create.** `crates/of-core/migrations/0027_lease_resource.sql` | Forward-only migration: rename column, backfill existing rows to `branch:<value>`, recreate the live-lease unique index. |
| **Modify.** `crates/of-core/src/leases.rs` | `Lease.resource`, `LEASE_COLS`, every function's parameter and SQL predicate, doc comments. |
| **Modify.** `crates/of-core/src/error.rs` | `Error::LeaseHeld { resource, ... }` and its message. |
| **Modify.** `crates/of-core/tests/queue.rs` | New test proving a non-branch resource works and is independent of a branch resource on the same repo (the actual AC). |
| **Modify.** `crates/of-mcp/src/tools/coord.rs` | `AcquireLeaseArgs` gains `resource: Option<String>`, `branch` becomes `Option<String>`; handler resolves one from the other; four tool descriptions updated. |
| **Modify.** `crates/of-mcp/tests/tools.rs` | Fix the three `AcquireLeaseArgs` literals for the new field shape; new tests for `resource`, the `branch` alias, and the empty-branch rejection. |
| **Modify.** `web/src/lib/types.ts` | `Lease.branch` → `Lease.resource`. |
| **Modify.** `web/src/routes/o/[org]/repos/+page.svelte` | `{lease.branch}` → `{lease.resource}` at line 364. |
| **Modify.** `crates/of-web/tests/console.rs` | New test for `GET /api/orgs/{org}/repos/{repo}/leases` — this route currently has zero test coverage, and its response shape is exactly what this change breaks/renames. |
| **Modify.** `docs/specs/2026-09-01-otto-factory-design.md` | Repo-leases paragraph and open risk 4 restated in resource terms. |

## Task Order & Rationale

1. **`of-core`** first — the schema and domain type everything else depends on.
2. **`of-mcp`** second — the only crate whose compilation actually depends on Task 1's
   type changes (via `AcquireLeaseArgs`'s relationship to the handler, not via the `Tx`
   call sites, which are positional). Last Rust task, so it carries the full-workspace gate.
3. **`web/`** third — independent toolchain, only needs `of-core`'s wire shape to match
   (which is already true once Task 1 lands, since `of-web` re-exports `Lease` verbatim).
4. **Docs** last — no code dependency, but reads more sensibly written after the change is
   real rather than before.

---

## Task 1 — `of-core`: schema, `Lease`, and `Error::LeaseHeld` ⬜

**Files:** `crates/of-core/migrations/0027_lease_resource.sql` (create),
`crates/of-core/src/leases.rs`, `crates/of-core/src/error.rs`,
`crates/of-core/tests/queue.rs`

**Interfaces:** produces `Lease.resource`, `Tx::acquire_lease(..., resource: &str, ...)` /
`renew_lease` / `release_lease` / `list_leases`, `Error::LeaseHeld { resource, ... }` —
consumed by Task 2 (`of-mcp`) and by `of-web::routes::repos::list_leases` (already
compiles unchanged, per the Global Constraints note).

- [ ] Write a failing test in `crates/of-core/tests/queue.rs`, appended after
      `leases_are_per_branch` (around line 1398-1417): a new test
      `leases_are_per_resource_not_just_branch` that follows the exact pattern of
      `leases_are_per_branch`/`a_second_agent_cannot_take_a_held_lease` immediately above
      it (`let db = db(pool); let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;`,
      a second `other` user via `db.upsert_user(...)` + `db.add_member(...)`, then
      `db.begin(t.org)` / `tx.acquire_lease(...)` / `tx.commit()`): acquire a lease on
      `"branch:main"` for `t.user` and `"deploy:staging"` for `other` on the **same** repo
      in one transaction, assert both succeed and `tx.list_leases(Some(t.repo))` returns 2
      (proving two different non-overlapping resources don't collide — the literal AC from
      GH#69); then, in a fresh transaction, assert a second `acquire_lease` on
      `"deploy:staging"` by a third user fails with `err.code() == "lease_held"` and
      `err.to_string().contains("deploy:staging")` (proving `Error::LeaseHeld` names the
      resource, not a hardcoded "branch").
- [ ] Run `cargo test -p of-core --test queue leases_are_per_resource_not_just_branch` —
      confirm it fails to compile or fails the assertion (the column is still named
      `branch`, but positional calls still compile against the current signature, so this
      will most likely fail on the message-content assertion, not a compile error — either
      failure mode confirms the test exercises the not-yet-built behavior).
- [ ] Create `crates/of-core/migrations/0027_lease_resource.sql`:
      ```sql
      ALTER TABLE repo_leases RENAME COLUMN branch TO resource;

      UPDATE repo_leases SET resource = 'branch:' || resource
        WHERE resource NOT LIKE 'branch:%';

      DROP INDEX repo_leases_live_key;
      CREATE UNIQUE INDEX repo_leases_live_key
        ON repo_leases (repo_id, resource)
        WHERE released_at IS NULL;
      ```
      Also add a one-line SQL comment above the `ALTER TABLE` explaining the `branch:`
      backfill convention, matching the spec's §1 reasoning.
- [ ] In `crates/of-core/src/leases.rs`: rename `Lease.branch: String` to
      `Lease.resource: String`; update `LEASE_COLS` to list `resource`; rename every
      function's `branch: &str` parameter to `resource: &str` and every SQL predicate
      (`branch = $n`) to `resource = $n`; update the empty-string guard's error message to
      `"lease resource must not be empty"`; update the module doc comment and
      `acquire_lease`'s doc comment to say "resource" instead of "branch" (keep the
      advisory/time-bounded reasoning verbatim).
- [ ] In `crates/of-core/src/error.rs`: rename `Error::LeaseHeld`'s `branch` field to
      `resource` and update the `#[error(...)]` message to
      `"{resource} of this repo is leased by {holder} until {expires_at}"`. `code()` and
      `retriable()` are unchanged.
- [ ] Run `cargo test -p of-core --test queue` and `cargo test -p of-core --test isolation`
      — both suites must pass unmodified (per the Global Constraints note, their existing
      lease tests compile and pass against the renamed parameter without any edit).
- [ ] Run `cargo clippy -p of-core --all-targets -- -D warnings`.
- [ ] Format and commit: `cargo fmt --all` then
      `git commit -m "of-core: generalize repo_leases from (repo, branch) to (repo, resource)"`.

## Task 2 — `of-mcp`: `resource` input, `branch` alias, tool descriptions ⬜

**Files:** `crates/of-mcp/src/tools/coord.rs`, `crates/of-mcp/tests/tools.rs`

**Interfaces:** consumes Task 1's `Tx::acquire_lease` / `Error::LeaseHeld`; produces the
public `acquire_lease` MCP tool's new input shape and all four lease tools' output (via
`LeaseOut`/`LeasesOut`, unchanged code, new wire shape).

- [ ] Write failing tests in `crates/of-mcp/tests/tools.rs`, in the "coordination" section
      (after `a_held_lease_names_its_holder_to_the_next_agent`, ~line 1242):
      - `acquire_lease_accepts_a_free_form_resource`: call `acquire_lease` with
        `resource: Some("deploy:staging".into())`, `branch: None`, assert the returned
        `lease.resource == "deploy:staging"`.
      - `acquire_lease_branch_alias_prefixes_and_still_works`: call with
        `branch: Some("main".into())`, `resource: None`, assert the returned
        `lease.resource == "branch:main"` (the deprecated alias translates correctly).
      - `acquire_lease_rejects_an_empty_branch_alias`: call with
        `branch: Some("   ".into())`, `resource: None`, assert an `invalid_params`-shaped
        error (not a lease created on `"branch:"`).
      - `acquire_lease_needs_resource_or_branch`: call with both `None`, assert an
        `invalid_params` error naming `resource`.
- [ ] Run `cargo test -p of-mcp --test tools` — confirm these four fail to compile (the
      struct literals reference fields that don't exist yet in the shape the tests need).
- [ ] In `crates/of-mcp/src/tools/coord.rs`: change `AcquireLeaseArgs.branch` to
      `Option<String>` with an updated doc comment marking it deprecated; add
      `resource: Option<String>` above it with the doc comment from spec §4. In the
      `acquire_lease` handler, resolve `resource` from `(args.resource, args.branch)`
      exactly per spec §4 — **the `branch` arm must reject an empty/whitespace value
      before prefixing**, returning `invalid_params` rather than producing the string
      `"branch:"`. Update the `acquire_lease` tool description verbatim from spec §4.
      Update `renew_lease` and `release_lease` descriptions to say "the resource" in place
      of "the branch" (both currently read "...another agent may take the branch..." and
      "...freeing the branch immediately..."). Update `list_leases`' description from "the
      holder, the branch, and when each expires" to "the holder, the resource, and when
      each expires."
- [ ] Fix the three existing `AcquireLeaseArgs` struct literals in
      `crates/of-mcp/tests/tools.rs` for the new field shape (`branch: "main".into()` no
      longer compiles against `Option<String>`): line ~1164's direct literal and the
      `take` closure at ~1176 both need `resource: None,` added and `branch: "main".into()`
      changed to `branch: Some("main".into())`; the override literal at ~1208
      (`branch: "feature/x".into(), ..take()`) needs `branch: Some("feature/x".into())`.
- [ ] Run `cargo test -p of-mcp --test tools` — all coordination tests pass, including the
      four new ones and the fixed existing test.
- [ ] Run `cargo clippy -p of-mcp --all-targets -- -D warnings`.
- [ ] **Final Rust gate for this plan:** `cargo test --workspace` and
      `cargo clippy --all-targets -- -D warnings`, unscoped, both green.
- [ ] Format and commit: `cargo fmt --all` then
      `git commit -m "of-mcp: accept resource on acquire_lease, keep branch as a deprecated alias"`.

## Task 3 — `web/`: console type and Repos page ⬜

**Files:** `web/src/lib/types.ts`, `web/src/routes/o/[org]/repos/+page.svelte`,
`crates/of-web/tests/console.rs`

**Interfaces:** consumes the renamed `Lease` JSON shape from `GET
/api/orgs/{org}/repos/{repo}/leases` (already correct once Task 1 lands, since `of-web`
returns `of_core::leases::Lease` verbatim).

- [ ] Add a new test in `crates/of-web/tests/console.rs` (this route currently has zero
      coverage), following the file's established pattern
      (`let h = harness(pool); let rob = onboard(&h, "rob@acme.test").await; let acme =
      org_with_owner(&h, "acme", &rob).await;`, then create a repo via
      `Call::post("/api/orgs/acme/repos")...`, matching `the_queue_view_lists_filters_and_counts`'s
      setup): seed a lease by opening `h.db.begin(acme)` and calling `tx.acquire_lease(...)`
      directly (mirroring the file's `enqueue` helper's own comment on why reaching past
      the API to seed fixtures is correct here — leases, like jobs, are written by an agent
      over MCP, never by the console), then `Call::get("/api/orgs/acme/repos/api/leases")
      .with_session(&rob.session).send(&h.router).await` and assert the JSON response's
      lease object has a `"resource"` field (not `"branch"`) with the expected value. This
      is not a failing-first step in the usual sense: `of-web` requires no code change (the
      route already returns `of_core::leases::Lease` unchanged, and Task 1 already renamed
      that field), so the test is expected to pass as soon as it is written — its value is
      closing the pre-existing zero-coverage gap on a route whose response shape this plan
      just renamed.
- [ ] Run `cargo test -p of-web --test console <new test name>` — passes immediately, per
      the note above; treat a failure here as a signal that Task 1 did not fully land.
- [ ] In `web/src/lib/types.ts`: rename the `Lease` interface's `branch: string` field to
      `resource: string`; update the doc comment above it to describe a resource generally
      rather than "one branch."
- [ ] In `web/src/routes/o/[org]/repos/+page.svelte` at line 364: change
      `{lease.branch}` to `{lease.resource}`.
- [ ] Run `cd web && npm run check` — `svelte-check`/`tsc` must pass (this is what catches
      any remaining `.branch` reference on a `Lease` value at compile time).
- [ ] Run `cd web && npm run lint` — `prettier --check` must pass.
- [ ] Run `cd web && npm test` — vitest (the Cloudflare Worker's routing tests) must pass;
      unaffected by this change but must not regress.
- [ ] Run `cd web && npm run build` — confirm the static bundle builds (out-of-band check
      per the skill's Phase 5, done early here since it is cheap and this is the last
      `web/`-touching task).
- [ ] Format and commit: `cargo fmt --all` (for `console.rs`) then
      `git commit -m "web: rename lease.branch to lease.resource in the console"`.

## Task 4 — Docs: restate open risk 4 ⬜

**Files:** `docs/specs/2026-09-01-otto-factory-design.md`

**Interfaces:** none — documentation only.

- [ ] In `docs/specs/2026-09-01-otto-factory-design.md`, update the "Repo leases" paragraph
      (currently: "An agent takes a lease on `(repo, branch)` for a bounded TTL...") to say
      `(repo, resource)`, and mention the `branch:main` convention for the branch case in
      one sentence.
- [ ] Update open risk 4 from "Lease semantics are advisory. The server cannot see git
      operations, so a determined agent can ignore a lease..." to the generalized wording
      from spec §"Risks & Open Questions": "The server cannot see what an agent actually
      does with a leased resource — a git operation, a migration run, a deploy — so a
      determined agent can ignore any lease it holds. This is documented, not hidden;
      leases make collisions visible, not impossible."
- [ ] Format and commit: `git commit -m "docs: restate lease advisory risk in resource terms"`.

---

## Out-of-Band Artifacts Touched

- **Migration** (`crates/of-core/migrations/0027_lease_resource.sql`): additive/new file,
  confirmed in Task 1. Verify at Phase 5 with `podman compose down -v && podman compose up
  -d` then `cargo test -p of-core` for a fresh-cluster apply.
- **Console bundle**: `web/` changes in Task 3; `npm run build` confirms the static bundle
  builds, per Task 3's own step.
- **Container image, Cloudflare Worker config, CI workflow, config surface**: not touched
  by this plan — vacuously satisfied.
