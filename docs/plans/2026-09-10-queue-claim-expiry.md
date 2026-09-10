# Queue claim expiry and claimer-checked finalize — implementation plan

Goal: give `claim_jobs` a TTL so a crashed agent's job becomes claimable again instead of
staying `in-progress` forever, and make `complete_job`/`fail_job` refuse a caller who does
not hold the current claim — closing savvagent/otto-factory#65.

**Spec:** `docs/specs/2026-09-10-queue-claim-expiry-design.md` — read it first. This plan
implements it exactly.

---

## Status — 2026-09-10

Not started.

---

## Global Constraints

These hold for every task in this plan:

- No AI self-attribution anywhere — commits, comments, docs, PR body.
- Run `cargo fmt --all` before every Rust commit.
- Every SQL statement lives in `of-core`. A query in `of-mcp`/`of-web` is a bug.
- Tests need a real Postgres: `podman compose up -d` (Postgres 16 on host port 15433) and
  a `.env` with `DATABASE_URL` (`cp .env.example .env`).
- `cargo test` (or `cargo test --workspace`) + `cargo clippy --all-targets -- -D
  warnings` + `cargo fmt --all --check` must all be clean before a task is done.
- A migration is a new file, never an edit to one already applied. `0025` is the next free
  number (`0024` is the current head).
- A new MCP tool is not done until it is classified in `of-billing::classify` —
  `every_tool_has_a_price`/`exhaustive_over` enforce this.
- A tenant-scoped function is not done without a cross-org negative test — no *new* RLS
  policy registration is needed here (`claim_expires_at` is a new column on the
  already-covered `jobs` table, not a new table), but `renew_claim` and the changed
  `complete_job`/`fail_job` each still get one, per the spec's §2 reasoning.
- Anything touching `web/` adds `npm run check` (svelte-check + tsc; runs
  `scripts/check-messages.mjs` first) and `npm run lint` (prettier) to its gate.
- No console route changes anywhere in this plan — every write this feature adds or
  changes is an MCP tool; the console only gains read-only fields plus a client-side
  derived indicator (spec §8).

## File Structure

| File | Responsibility |
|---|---|
| **Create.** `crates/of-core/migrations/0025_job_claim_expiry.sql` | Adds nullable `claim_expires_at` to `jobs`. |
| **Modify.** `crates/of-core/src/jobs.rs` | `DEFAULT_CLAIM_TTL_SECS`/`MAX_CLAIM_TTL_SECS`, `Job.claim_expires_at`, `claim_jobs`'s reap step + `ttl_secs` param, `ready()`'s widened claimable predicate, `ensure_claim_held`, `finalize`/`complete_job`/`fail_job` taking `caller: UserId`, `renew_claim`, `repend_job` clearing the new column. |
| **Modify.** `crates/of-core/src/error.rs` | `AlreadyClaimed` gains `holder: String`. |
| **Modify.** `crates/of-core/tests/queue.rs` | New-behavior tests (see Task 1). |
| **Modify.** `crates/of-core/tests/isolation.rs` | Cross-org negative tests for `renew_claim` and for the claimer check on `complete_job`/`fail_job`. |
| **Modify.** `crates/of-mcp/src/tools/jobs.rs` | `ClaimJobsArgs.ttl`, `complete_job`/`fail_job` passing `caller.user_id`, new `RenewClaimArgs`/`renew_claim` tool, description updates. |
| **Modify.** `crates/of-billing/src/classify.rs` | `renew_claim` added to `FREE`. |
| **Modify.** `crates/of-mcp/tests/tools.rs` | End-to-end tests, tool-list/billing assertions. |
| **Modify.** `crates/of-web/src/openapi.rs` | `Job` schema gains `claimExpiresAt`, schema-assertion test. |
| **Modify.** `web/src/lib/types.ts` | `Job.claimExpiresAt: string \| null`. |
| **Create.** `web/src/lib/jobs.ts` | `isClaimStranded` predicate. |
| **Modify.** `web/messages/{en,es,de,fr,it,hi}.json` | `job_claim_stranded` key, all six. |
| **Modify.** `web/src/routes/o/[org]/queue/+page.svelte` | Stranded indicator under the status pill in the list table. |
| **Modify.** `web/src/routes/o/[org]/queue/[job]/+page.svelte` | Stranded indicator next to the cancellation-requested line. |

## Task Order & Rationale

1 → 2 → 3 → 4, strictly sequential: `of-core` (Task 1) is the foundation everything else
compiles against — `of-mcp` (Task 2) cannot compile until `Tx::claim_jobs`'s new
signature, `Tx::renew_claim`, and `Error::AlreadyClaimed`'s new field exist. `of-web`
(Task 3) only needs `Job.claim_expires_at` (from Task 1) to serialize it into the
hand-written OpenAPI schema — done after Task 2 to keep the "domain → MCP surface →
console API → UI" story told in one direction. `web/` (Task 4) is last: purely
presentational, depends on nothing upstream breaking, and is the only task touching
`web/messages/*.json`, so it is also the only task that needs `npm run check`'s
message-catalog gate.

---

## Task 1 — `of-core`: claim expiry, reap, and the claimer check

**Files:** `crates/of-core/migrations/0025_job_claim_expiry.sql` (create),
`crates/of-core/src/jobs.rs`, `crates/of-core/src/error.rs`, `crates/of-core/tests/queue.rs`,
`crates/of-core/tests/isolation.rs`.

**Interfaces produced:** `Job.claim_expires_at`, `DEFAULT_CLAIM_TTL_SECS`/
`MAX_CLAIM_TTL_SECS`, `Tx::claim_jobs`'s new `ttl_secs: Option<i64>` parameter,
`Tx::renew_claim`, `Tx::complete_job`/`Tx::fail_job`'s new `caller: UserId` parameter,
`Error::AlreadyClaimed { job, holder }`. Consumed by Task 2.

- [ ] Write `crates/of-core/migrations/0025_job_claim_expiry.sql` exactly as spec §2: one
      nullable `timestamptz` column, no backfill, with the comment explaining why NULL
      rows are never reaped.
- [ ] Run `cargo test -p of-core` once with no other code changes, purely to confirm a
      fresh throwaway database migrates cleanly through the new file. Expect: passes
      (nothing references the new column yet).
- [ ] In `crates/of-core/tests/error.rs` or inline where `Error::AlreadyClaimed` is
      constructed in tests (there are none yet), note that this step has no standalone
      test — its behavior is exercised entirely through the `queue.rs` tests below.
- [ ] In `crates/of-core/tests/queue.rs`, write the failing tests first (they will not
      compile until the `jobs.rs`/`error.rs` changes below land — expected):
      - `claim_jobs` with `ttl_secs: None` sets `claim_expires_at` to within a few seconds
        of `now() + DEFAULT_CLAIM_TTL_SECS` (assert a bounded range, not exact equality —
        wall-clock skew between the test and the database).
      - `claim_jobs` with an explicit `ttl_secs` honors it (same bounded-range assertion).
      - `claim_jobs` with `ttl_secs` above `MAX_CLAIM_TTL_SECS` or below 60 is clamped, not
        rejected — assert the resulting `claim_expires_at` matches the clamped value, not
        the requested one.
      - Claim a job, then force its `claim_expires_at` into the past with a raw
        `sqlx::query("UPDATE jobs SET claim_expires_at = now() - interval '1 second'
        WHERE org_id = $1 AND id = $2").bind(org).bind(&job.id).execute(tx.conn())`-style
        statement (there is no public API to create this state quickly — a raw query
        against the test's own `Tx`/pool is the established pattern elsewhere in this file
        for setting up edge-case row state). Assert the job now appears in
        `tx.ready(None)`'s result.
      - Call `claim_jobs` again on that same expired-claim job as a **different** user.
        Assert it succeeds, `claimed_by` is now the new user, and `attempts == 2` (one
        from the original claim, one from the reclaim).
      - `renew_claim` by the current holder extends `claim_expires_at` (assert it moved
        forward — compare to the value before the call).
      - `renew_claim` by a different user returns `Error::AlreadyClaimed` naming the
        actual holder's label.
      - `renew_claim` on a `pending` job, and on a `completed`/`failed`/`cancelled` job,
        both return `Error::WrongStatus` naming `"in-progress or active"`.
      - **Fencing** (the test GH#65 explicitly asks for): claim a job as user A. Force its
        `claim_expires_at` into the past via the same raw-`UPDATE` technique above. Claim
        it again as user B (succeeds). Assert A's `complete_job(&job.id, user_a, ...)`
        call now fails with `Error::AlreadyClaimed { holder }` naming B's label, **and**
        assert B's own `complete_job(&job.id, user_b, ...)` call succeeds. This is one
        test proving both halves of the interleaving in the same body.
      - `complete_job`/`fail_job` called by the actual holder still succeed — update every
        existing call site in this file that calls `tx.complete_job(&id, Some(...))` or
        `tx.fail_job(&id, Some(...))` to pass the claiming user's id as the new second
        argument (grep this file for `.complete_job(` and `.fail_job(` — there are
        multiple existing calls; every one needs its signature updated, not just the new
        tests, or the whole file fails to compile).
      - `complete_job`/`fail_job` called by a caller who never claimed the job (a fresh
        user with no claim at all, not just a different one) returns `Error::AlreadyClaimed`
        naming `"nobody"` — the `unwrap_or_else(|| "nobody".into())` branch in
        `ensure_claim_held`. Cover this distinctly from the fencing case above, since it
        exercises a different code path (`claimed_by_label` and `claimed_by` both absent
        vs. both present but pointing at someone else — impossible in practice since a
        `pending`/terminal job's status check fails first, so use a job claimed by someone
        else, not an entirely unclaimed one, to reach the `AlreadyClaimed` branch; a
        genuinely never-claimed job hits `WrongStatus` first, which is also worth its own
        one-line assertion for completeness).
      - `repend_job` on a claimed job clears `claim_expires_at` — extend the existing
        `repend_job` test's assertions rather than adding a new test.
- [ ] Run `cargo test -p of-core --test queue` — expect compile failure (new column, new
      function signatures, and `renew_claim` do not exist yet).
- [ ] Implement `crates/of-core/src/jobs.rs` per spec §3 exactly: the two constants;
      `Job.claim_expires_at` (placed after `claimed_by_label`) with its doc comment;
      `JOB_COLS` gains `, claim_expires_at`; `claim_jobs`'s new `ttl_secs` parameter, the
      reap `UPDATE` before the existing lock/check block, and the final claim `UPDATE`'s
      new `claim_expires_at = now() + make_interval(secs => $5)` clause; `ready()`'s
      widened `WHERE` predicate; the new private `ensure_claim_held` helper; `finalize`
      taking `caller: UserId` and calling `ensure_claim_held` first (removing its own now-
      redundant `SELECT ... FOR UPDATE`); `complete_job`/`fail_job` taking and forwarding
      `caller: UserId`; the new `renew_claim` function; `repend_job`'s `UPDATE` gaining
      `, claim_expires_at = NULL`.
- [ ] Implement `crates/of-core/src/error.rs` per spec §4: `AlreadyClaimed`'s new `holder:
      String` field and updated `#[error(...)]` message. Confirm (do not just assume) that
      `code()`'s and `retriable()`'s existing `{ .. }` match arms still compile unchanged.
- [ ] Run `cargo test -p of-core --test queue` — expect all pass.
- [ ] In `crates/of-core/tests/isolation.rs`, extend `cross_org_mutation_is_refused` (or
      add a sibling following its exact existing pattern) with: `renew_claim` called from
      org b against a job claimed in org a (`.is_err()`); `complete_job`/`fail_job` called
      from org b against a job claimed in org a (`.is_err()` — this exercises guard 1 for
      the changed statements, distinct from the claimer check in `queue.rs`, which runs
      inside one org).
- [ ] Run `cargo test -p of-core --test isolation` — expect all pass.
- [ ] `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --all`.
- [ ] Commit: `git commit -m "of-core: expire job claims and check the claimer on finalize"`.

## Task 2 — `of-mcp` + `of-billing`: `renew_claim`, `ttl`, and the claimer check

**Files:** `crates/of-mcp/src/tools/jobs.rs`, `crates/of-billing/src/classify.rs`,
`crates/of-mcp/tests/tools.rs`.

**Interfaces consumed:** everything from Task 1. **Interfaces produced:** MCP tool
`renew_claim` (returns `out::JobOut`, no new envelope type); `claim_jobs`'s new optional
`ttl` argument.

- [ ] In `crates/of-mcp/tests/tools.rs`, write the failing tests first:
      - End-to-end: `add_job` → `claim_jobs` with an explicit `ttl` → `renew_claim`
        extends the claim (assert the returned job's `claimExpiresAt` moved forward
        relative to the value right after claiming) → `complete_job` succeeds.
      - End-to-end: `claim_jobs` as one caller → `complete_job` attempted with a
        **different** caller's token/principal fails with an error whose `code` is
        `"already_claimed"`.
      - `renew_claim` appears in the tool list with a non-empty description — extend
        whichever test enumerates the tool list by name (follow the `request_cancel`/
        `cancel_job` precedent from the cancellation feature's Task 2 exactly — find that
        hardcoded `expected` vec and add `"renew_claim"` to it).
      - `every_tool_has_a_price`/`exhaustive_over` (in `of-billing`, or wherever the
        existing "tool list vs. price list" test lives) still passes once `renew_claim`
        is classified — this needs Task 2's classify.rs step below to actually pass; note
        it here as the acceptance check for that step. If `of-billing`'s
        `work_is_billable_and_looking_is_not`-style test hardcodes a free-tools list
        (check the existing test body, following the cancellation plan's Task 2
        precedent), extend it with `"renew_claim"`.
- [ ] Run `cargo test -p of-mcp --test tools` — expect compile failure (`renew_claim` does
      not exist, `claim_jobs`/`complete_job`/`fail_job` handler signatures haven't
      changed to match `of-core` yet — this crate will already fail to compile against
      Task 1's new `Tx` signatures until this task's handler changes land, which is
      expected and is why Task 1 must merge into the same branch before this task starts,
      not why it should be treated as this task's own failing-test signal).
- [ ] Implement per spec §5 exactly: `ClaimJobsArgs.ttl` (new optional field, forwarded to
      `tx.claim_jobs(...)` as the new fifth positional argument); `complete_job`/
      `fail_job`'s handlers forwarding `caller.user_id`; the new `RenewClaimArgs` struct
      and `renew_claim` tool (including the `self.charge(&mut tx, &caller,
      "renew_claim").await?;` call, matching `renew_lease`'s pattern of charging —
      recording, not billing — even a free tool); description text updates for
      `claim_jobs`, `ready`, `complete_job`, `fail_job` per spec §5's exact wording.
- [ ] In `crates/of-billing/src/classify.rs`, add `"renew_claim"` to `FREE`, next to
      `"renew_lease"`/`"release_lease"`, extending their existing comment per spec §6.
- [ ] Run `cargo test -p of-mcp --test tools` and `cargo test -p of-billing` — expect all
      pass, including `every_tool_has_a_price`/`exhaustive_over`.
- [ ] `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --all`.
- [ ] Commit: `git commit -m "of-mcp: add renew_claim and a claim TTL for claim_jobs"`.

## Task 3 — `of-web`: schema update

**Files:** `crates/of-web/src/openapi.rs`.

**Interfaces consumed:** `Job.claim_expires_at` (Task 1), passed through unchanged —
`routes/jobs.rs` returns `of_core::jobs::Job` directly, so no route code changes.

- [ ] In `crates/of-web/src/openapi.rs`'s test module (the same block asserting
      `"cancelRequestedAt"` etc. per the cancellation feature's precedent), write the
      failing assertion first: the `Job` schema's `properties` contains `claimExpiresAt`.
- [ ] Run the relevant `cargo test -p of-web` schema test — expect failure.
- [ ] Add `"claimExpiresAt": { "type": ["string", "null"], "format": "date-time" }` to the
      `Job` schema in `openapi.rs`, placed after `"claimedByLabel"` per spec §7.
- [ ] Run the schema test — expect pass. Then run `cargo test -p of-web` in full,
      including `the_queue_is_read_only_over_the_console` explicitly (confirm it still
      passes unchanged — this task adds no route).
- [ ] `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --all`.
- [ ] Commit: `git commit -m "of-web: surface claim expiry in the console schema"`.

## Task 4 — `web/`: the stranded-claim indicator

**Files:** `web/src/lib/types.ts`, `web/src/lib/jobs.ts` (create),
`web/messages/{en,es,de,fr,it,hi}.json`, `web/src/routes/o/[org]/queue/+page.svelte`,
`web/src/routes/o/[org]/queue/[job]/+page.svelte`.

**Interfaces consumed:** `claimExpiresAt`, already flowing through the read-only console
API from Task 3 — this task is purely presentational, no new API calls.

- [ ] In `web/messages/en.json`, add `"job_claim_stranded": "Stranded — claim expired"`
      immediately after `"job_cancellation_requested"`. Add the equivalent translated
      string, matching each locale's existing register, to `es.json`, `de.json`,
      `fr.json`, `it.json`, `hi.json`. Land this before the TypeScript/Svelte changes
      below, same reasoning as the cancellation feature's Task 4 ordering — so later
      failures are about `m.job_claim_stranded` not existing yet, not about the catalog
      being incomplete.
- [ ] Run `cd web && npm run check` — expect it to still pass (an unused catalog key is
      not an error).
- [ ] In `web/src/lib/types.ts`, add `claimExpiresAt: string | null;` to `Job`, placed
      after `claimedByLabel`.
- [ ] Create `web/src/lib/jobs.ts` with `isClaimStranded` exactly as spec §8's code block.
- [ ] Write a Vitest unit test for `isClaimStranded` (new file
      `web/src/lib/jobs.test.ts` or alongside existing `.test.ts` files under `src/lib/`
      if that is where sibling pure-function tests live — check for one, e.g. near
      `format.ts`'s tests, and match its location convention): a `pending` job is never
      stranded regardless of `claimExpiresAt`; an `in-progress` job with `claimExpiresAt`
      in the future is not stranded; an `in-progress` job with `claimExpiresAt` in the
      past is stranded; an `active` job with a past `claimExpiresAt` is stranded; an
      `in-progress` job with `claimExpiresAt: null` is not stranded (never claimed, or a
      pre-migration row).
- [ ] Run `npm test` — expect the new test file to pass and nothing else to regress.
- [ ] In `web/src/routes/o/[org]/queue/+page.svelte`'s table body, under the existing
      `<StatusPill status={job.status} />` cell, add a conditional small `text-bad` line
      (`{#if isClaimStranded(job)}<div class="text-xs text-bad">{m.job_claim_stranded()}</div>{/if}`)
      — this is a new placement, not an existing pattern to copy (spec §8's correction).
      Import `isClaimStranded` from `$lib/jobs`.
- [ ] In `web/src/routes/o/[org]/queue/[job]/+page.svelte`'s header block, add a stranded
      line next to the existing `{#if (job.status === 'in-progress' || job.status ===
      'active') && job.cancelRequestedAt}` cancellation-requested line, following that
      line's exact conditional/styling shape but keyed on `isClaimStranded(job)` instead.
- [ ] Run `npm run check && npm run lint && npm test` — expect all pass. `npm test` should
      show no Worker-routing regression — treat any failure there as a regression to fix.
- [ ] Commit: `git commit -m "web: show a stranded claim distinctly from healthy work in progress"`.

---

## Rule for every task

A task is done when its own tests pass, `cargo clippy --all-targets -- -D warnings` is
clean, and `cargo fmt --all --check` is clean (Tasks 1–3), or `npm run check && npm run
lint && npm test` is clean (Task 4). Tenant-scoped functions are not done without a
cross-org negative test (Task 1: `renew_claim`, `complete_job`, `fail_job`). A new MCP
tool is not done without a `of-billing::classify` entry (Task 2: `renew_claim`).
