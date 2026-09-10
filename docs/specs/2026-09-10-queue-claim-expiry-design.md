# Queue claim expiry and claimer-checked finalize design

> **Status:** IMPLEMENTED — closes savvagent/otto-factory#65. Spec critique approved this
> design with two minor non-blocking corrections (both applied): §3's `JOB_COLS` ordering
> comment no longer implies positional `sqlx::FromRow` decoding, and §8 no longer claims a
> queue-list placement precedent that does not exist.
>
> **PR review round found six further corrections, all applied before merge** (see the
> `Post-review correction:` notes inline in §2–§4 and Risks & Open Questions for detail):
> the migration was renumbered `0025` → `0026` after `savvagent/otto-factory#99` merged
> `0025_idempotency_keys.sql` to master while this branch was in flight; `claim_jobs`'s
> reap step now runs *after* the deterministic sorted `FOR UPDATE` lock, not before, so it
> acquires no lock ordering of its own; the reap now also clears the same cancellation
> fields `repend_job` clears; `finalize`, `cancel_job`, and `close_from_ticket` now all
> clear `claim_expires_at`, making §3's `Job.claim_expires_at` doc comment true rather
> than aspirational; `cancel_job` now takes a `caller: UserId` and is fenced by
> `ensure_claim_held` exactly like `complete_job`/`fail_job` (Scope/Out's original
> characterization of `cancel_job` as having "no result to clobber" was wrong — it is a
> third finalizer, not a `activate_job`-shaped no-op); and `Error::AlreadyClaimed` is no
> longer `retriable()`, since every real caller of it (via `ensure_claim_held`) is a
> refusal that retrying cannot fix. A known, documented, not-fixed-in-this-PR limitation
> — fencing by account (`claimed_by: UserId`) rather than by agent instance — is recorded
> in Risks & Open Questions.

## Goal & Success Criteria

`0002_repos.sql:54` states the rule for leases out loud: *"A crashed agent's lease expires
rather than deadlocking the repo."* That reasoning was never applied to jobs, and jobs are
where the work is — a claim taken by `claim_jobs` has no TTL and nothing reaps it, so an
agent that dies mid-task (context exhaustion, a rate limit, a closed terminal) leaves its
job `in-progress` forever, recoverable only by a human calling `repend_job`. Separately,
`finalize` (backing `complete_job`/`fail_job`) checks only `status`, never `claimed_by`, so
any caller with `jobs:write` can finish a job a different agent claimed — harmless on its
own today, but the two defects must ship together: fixing claim expiry alone means a
reaped-and-reclaimed job's *original* holder can still wake up and clobber the *new*
holder's result with a stale `complete_job` call, corrupting the audit trail and marking
downstream dependencies ready off the wrong outcome.

Success:

- `claim_jobs` accepts an optional TTL and records `claim_expires_at`. A claim without an
  explicit TTL gets a sensible default; a client-requested TTL is clamped to a sane range,
  matching `repo_leases`'s existing shape exactly (§3 explains why the same numbers apply).
- A new tool, `renew_claim`, pushes a held claim's expiry forward without completing or
  failing it — the same shape as `renew_lease`, so an agent doing long-running work adopts
  a renewal cadence it already knows from leasing a branch.
- An expired claim is discoverable again through `ready()` without a background sweeper:
  the repro in GH#65 ("call `ready`, get `job-1` back") works because `ready()`'s own query
  treats a lapsed claim as claimable, and the actual row transition back to `pending`
  happens lazily, inside `claim_jobs`, exactly where `acquire_lease` reaps an expired lease
  before granting a new one.
- `attempts` is untouched by a reap — it already increments only on an actual claim, and a
  reap-then-reclaim inside one `claim_jobs` call is still exactly one claim.
- `complete_job` and `fail_job` refuse a caller who is not the job's current holder, naming
  the current holder, closing the exact gap `finalize`'s own doc comment already promises
  is closed ("Mark a job **you claimed** as completed").
- Fencing holds under the exact interleaving the issue describes: A claims, A's claim
  expires, B claims the same job, A calls `complete_job` — A's call fails naming B as the
  holder. A test proves this by interleaving two claims and using the new claimer check,
  not a new epoch/token field — see §3's reasoning for why the existing `claimed_by`
  column is already sufficient.
- The console shows a stranded claim (an `in-progress`/`active` job whose `claim_expires_at`
  has passed) as visibly different from healthy work in progress, without the row's
  `status` needing to change first — a human glancing at the queue should not read a dead
  agent's job as "someone is on this."
- Both `renew_claim` and the retained `claim_jobs` are correctly classified in
  `of-billing::classify` (`every_tool_has_a_price` must not fail).

## Public interface note

Per Non-Negotiable Rule 6, this is **additive, not breaking**:

- `jobs` gains one new nullable column (`claim_expires_at`). No column is renamed or
  removed.
- `claim_jobs`'s MCP input schema gains one new optional field (`ttl`). Existing callers
  that omit it keep today's behavior (server-chosen default TTL, same as omitting
  `acquire_lease`'s `ttlSeconds`).
- One new MCP tool, `renew_claim`, is added alongside the existing queue tools. No tool is
  renamed or removed, and its result is the existing `{"job": …}` envelope (`out::JobOut`).
- `Error::AlreadyClaimed` gains a `holder: String` field. This is an internal-to-of-core
  enum variant with no current production call site (`AlreadyClaimed` is defined but never
  constructed today — confirmed by `rg -n "AlreadyClaimed"` returning only its own
  definition and the two match arms that read it), so widening its shape breaks no wire
  contract: every caller sees it only through `Error::code()` (`"already_claimed"`,
  unchanged) and its `Display` message, both of which are allowed to gain detail.
- `complete_job`/`fail_job`/`cancel_job`'s **behavior** changes for a caller that is not
  the current holder — today that call silently succeeds; after this change it is
  refused. This is a bug fix matching each tool's own published description, not a schema
  or route change (no MCP input/output shape changes for any of the three), and is called
  out explicitly here per the architect reviewer's remit. **Post-review correction:**
  `cancel_job` was originally scoped out of this list (see Scope/Out) on the mistaken
  premise that it "has no result to clobber" the way `activate_job` doesn't — it does
  (`status = 'cancelled'`, `completed_at`, `error`), so it is now fenced identically to
  `complete_job`/`fail_job`.
- `ready()`'s **result set** changes: a row it returns no longer implies `status ==
  'pending'` — an `in-progress`/`active` job with a lapsed claim now appears too. A caller
  that branched on `status` after `ready()` assuming it was always `'pending'` sees a
  behavior change, though not a schema change (the field was always there). Named here
  per the architect reviewer's finding that the original draft of this note covered only
  `complete_job`/`fail_job` and missed this.
- No version bump is required; every crate stays at the workspace `0.1.0`.

## Scope

**In:**

- `crates/of-core/migrations/0026_job_claim_expiry.sql`: the new nullable column.
- `crates/of-core/src/jobs.rs`: `DEFAULT_CLAIM_TTL_SECS`/`MIN_CLAIM_TTL_SECS`/
  `MAX_CLAIM_TTL_SECS` constants and a shared `clamp_claim_ttl` helper, `Job.claim_expires_at`,
  `JOB_COLS`, `claim_jobs`'s lock-then-reap-then-claim step (locking first, in the
  existing deterministic sorted order, so the reap acquires no new locks) and new
  `ttl_secs` parameter, the shared `ensure_claim_held` holder check, `finalize`/
  `complete_job`/`fail_job`/`cancel_job` taking a `caller: UserId` and clearing
  `claim_expires_at` on finalize, the new `renew_claim` function, `close_from_ticket`
  also clearing `claim_expires_at` (no caller check — see §3), and `ready()`'s query
  treating an expired claim as claimable.
- `crates/of-core/src/error.rs`: `AlreadyClaimed` gains `holder: String` and a more
  actionable message; `retriable()` no longer includes `AlreadyClaimed` (see §4).
- `crates/of-mcp/src/tools/jobs.rs`: `ClaimJobsArgs.ttl`, `complete_job`/`fail_job` passing
  `caller.user_id` through, the new `RenewClaimArgs`/`renew_claim` tool, and description
  updates for `claim_jobs`/`ready`/`complete_job`/`fail_job` naming the new behavior.
- `crates/of-billing/src/classify.rs`: `renew_claim` added to `FREE` (same reasoning as
  `renew_lease`/`release_lease` — coordination hygiene is never priced). `claim_jobs`
  stays `BILLABLE`, unchanged.
- `crates/of-web/src/openapi.rs`: `Job` schema gains `"claimExpiresAt"`.
- `web/`: `src/lib/types.ts`'s `Job.claimExpiresAt`, a small `isClaimStranded` predicate
  (new `src/lib/jobs.ts`), and both the queue list page and the job detail page rendering
  a distinct "stranded" indicator when it is true. A new `job_claim_stranded` message key
  in all six locale catalogs.
- Tests: `crates/of-core/tests/queue.rs` (claim TTL, reap-and-reclaim, `renew_claim`,
  claimer-checked finalize, the fencing interleave), `crates/of-core/tests/isolation.rs`
  (cross-org negative test for `renew_claim`, and for the claimer check on
  `complete_job`/`fail_job`), `crates/of-mcp/tests/tools.rs` (end-to-end `renew_claim`,
  the tool list/description/classification assertions), `web/` (`npm run check`,
  `npm run lint`, `npm test`).

**Out:**

- A background sweeper process. Explicitly rejected by the issue's AC ("no background
  sweeper, no new failure mode") and by precedent — `repo_leases` has never had one either.
- Applying the same claimer check to `activate_job` or `request_cancel`. `activate_job` is
  a one-shot status refinement with no result to clobber, so leaving it unfenced cannot
  corrupt a reclaimer's outcome the way an unfenced finalizer could — a stale holder can
  flip a job it no longer holds to `active`, which is a cosmetic/audit annoyance, not the
  clobbering bug this PR fixes (flagged as a follow-up below regardless).
  `request_cancel` is deliberately open to any org member by existing, separate design
  (`2026-09-10-job-cancellation-design.md`'s Scope/Out) — it only sets a flag, never
  finalizes anything, so it was never a candidate for this fence. **Post-review
  correction:** `cancel_job` was originally grouped with these two on the premise that it
  "has no result to clobber" — that premise was wrong (see the Public interface note
  above); `cancel_job` **is** now fenced identically to `complete_job`/`fail_job`,
  matching the exact clobbering risk the issue's Goal section describes. Only
  `activate_job`/`request_cancel` remain intentionally unfenced.
- An epoch/fencing-token column distinct from `claimed_by`. `claimed_by` already changes
  on every successful (re)claim and is already compared by identity in the new checks
  below, so it already provides fencing — a dedicated token would duplicate that guarantee
  for no new correctness.
- Retroactively stamping `claim_expires_at` onto jobs already `in-progress`/`active` at
  migration time. See §2 and Risks & Open Questions for why leaving them `NULL` (and
  therefore never reaped) is the correct, conservative default.
- A `renewed_at` column on `jobs` mirroring `repo_leases.renewed_at`. Nothing reads it for
  leases beyond informational display, and no AC here asks for it; `claim_expires_at`
  moving forward is observable enough for both correctness and the console's stranded
  indicator.
- Any change to the `blocked()`/`stats()` queries. Both already reason only about
  `status = 'pending'` jobs; an expired `in-progress`/`active` claim is not blocked (its
  dependencies, if any, were already satisfied at the original claim) and is not counted
  differently by `stats()` today — the console's stranded indicator is derived client-side
  from `status` + `claim_expires_at`, not a new server-computed status or counter.
- A dedicated audit event for a reap. `claim_jobs` already has no audit entry for an
  ordinary claim (`audit.rs` has no `JOB_CLAIMED` action), so a reap-then-reclaim needs
  none either — consistent, not a regression.

## §1 — Why `ready()` reinterprets, but only `claim_jobs` mutates

This is the one design decision worth explaining before the code, because it resolves an
apparent tension between two AC bullets: "expired claims return to pending on the read
path" and "the console shows a stranded claim as such, not as healthy work in progress."
If reaping physically rewrote `status` to `pending` the moment anything read the row, the
second bullet would be trivially true but uninteresting (a stranded claim just *becomes*
an ordinary pending job, indistinguishable from one nobody ever touched) — and worse, nothing
reads *every* row on a schedule, so nothing would ever trigger that rewrite until an agent
happened to call `claim_jobs` with that exact id, which it cannot do without first
discovering the id.

The discovery step is `ready()`, and `ready()` is a read tool — it commits nothing today
(`Tx::ready` is `SELECT`-only) and should stay that way; turning a read into a write because
of an unrelated liveness concern is the kind of implicit behavior CLAUDE.md's "no silent
fallback" spirit warns against. So `ready()`'s query is widened to also select an
`in-progress`/`active` job whose `claim_expires_at` has passed — exactly mirroring
`repo_leases::list_leases`'s existing pattern of filtering live-vs-expired at query time
without mutating anything. This directly satisfies the issue's own repro: step 3 of
scenario (a) is "Call `ready`", and the expected result is "`job-1` becomes claimable
again" — claimable, observed through `ready()`, not necessarily already flipped to
`pending` in storage.

The actual row mutation — the "reap" — happens exactly where `acquire_lease`'s reap
happens: inside the call that is actually trying to *act* on the resource, scoped to the
specific id(s) requested, in the same transaction as the claim it grants. `claim_jobs`
already takes a row lock on every requested id; extending its status check from "is this
row `pending`?" to "is this row `pending`, or an expired claim I'm allowed to take over?"
and running one extra `UPDATE` is the direct analog of `acquire_lease`'s "reap this
branch's expired lease, then insert." **Post-review correction:** the reap `UPDATE` must
run *after* the existing `SELECT ... ORDER BY id FOR UPDATE` locks every requested row in
deterministic order, not before it — `acquire_lease`'s reap is safe running first only
because it targets a single `(repo_id, branch)` row, a property that does not carry over
to `claim_jobs`'s multi-row batch. Running the reap first would take locks in
planner-chosen (not sorted) order and could deadlock two concurrent batch claims; see §3
for the corrected sequencing. Until an agent actually calls `claim_jobs` on the stale id,
the row's `status` column still honestly says
`in-progress`/`active` — which is exactly the state the console's stranded indicator
needs: a human looking at the job detail page sees "in-progress" and a `claim_expires_at`
in the past, and the console renders "stranded," not "pending" (an outcome that would look
like nobody ever picked it up) and not "in progress" unqualified (which would look
healthy). Both AC bullets hold simultaneously because they describe two different
observers — an agent calling `ready()` to find work, and a human reading a job's literal
`status` — and this design gives each the answer it needs without the two contradicting
each other.

## §2 — Migration

`crates/of-core/migrations/0026_job_claim_expiry.sql`:

```sql
-- A claim needs an expiry so a crashed agent's job becomes claimable again
-- instead of staying in-progress forever — the same reasoning 0002_repos.sql
-- states for repo_leases, applied to jobs (see savvagent/otto-factory#65).
--
-- Nullable, with no backfill: a job already in-progress/active when this
-- migration runs gets no retroactive TTL. NULL never compares <= now(), so
-- claim_jobs's reap step and ready()'s claimable check both simply never
-- treat such a row as expired — nothing already claimed is silently reaped
-- the moment this ships. Only a claim taken through the new claim_jobs sets
-- this column going forward; a job stuck in-progress from before this
-- migration is recoverable exactly as it is today, via repend_job, until an
-- agent repends or completes/fails it and the next claim on it carries a
-- real expiry.
ALTER TABLE jobs
    ADD COLUMN claim_expires_at timestamptz;
```

No RLS registration needed: `jobs` is already a tenant table in `0007_rls.sql` with policy
`jobs_tenant_isolation`, row-scoped (`USING (org_id = current_org())`), which already
covers this new column on every row. This is a new column on an existing tenant table, not
a new tenant table — Load-Bearing Invariant 1's cross-org-negative-test requirement
applies to new tenant *tables*; the tenant-scoped *functions* this spec adds or changes
(`renew_claim`, and the new caller check inside `finalize`) still each get one, per §5.

## §3 — `crates/of-core/src/jobs.rs`

### Constants

```rust
/// Default claim lifetime. Same value as `repo_leases::DEFAULT_TTL_SECS`, and
/// for the same reason: long enough that a working agent renewing on a
/// normal cadence never loses its claim mid-task, short enough that a
/// crashed agent's job becomes claimable again while a human is still in the
/// room. Defined separately from the lease constant rather than imported —
/// jobs and leases are different resources with independently tunable
/// lifetimes that only happen to start at the same number today.
pub const DEFAULT_CLAIM_TTL_SECS: i64 = 900;

/// Upper bound on a client-requested claim TTL, for the same reason
/// `repo_leases::MAX_TTL_SECS` caps leases: without one, a single
/// `claim_jobs` call could take an effectively permanent claim that only
/// `repend_job` could clear.
pub const MAX_CLAIM_TTL_SECS: i64 = 4 * 3600;
```

### `Job`

One new field, placed after `claimed_by_label` — "who claimed it, and until when":

```rust
/// When the current claim lapses and the job becomes claimable again via
/// `ready()`/`claim_jobs`, the way an expired `repo_leases` row frees its
/// branch. `None` for a job that has never been claimed, or whose claim was
/// finalized (`complete_job`/`fail_job`/`cancel_job`) or reset
/// (`repend_job`) — only an active `in-progress`/`active` claim has this
/// set. A job in `in-progress`/`active` with this timestamp in the past is
/// *stranded*: nobody is actually working it, but nothing has reclaimed it
/// yet. See `ready()` for how such a job becomes claimable again, and
/// `claim_jobs` for where the row itself is actually reaped.
pub claim_expires_at: Option<chrono::DateTime<chrono::Utc>>,
```

`JOB_COLS` gains `, claim_expires_at` at the end, matching the cancellation columns'
precedent of appending rather than reordering existing entries — `Job` derives
`sqlx::FromRow` and decodes every `JOB_COLS` query by column name, not position, so nothing
here is positionally load-bearing; appending is simply the smaller, more reviewable diff.

### `claim_jobs`

Gains one new parameter and a reap step:

```rust
pub async fn claim_jobs(
    &mut self,
    ids: &[JobId],
    claimer: UserId,
    label: Option<&str>,
    ttl_secs: Option<i64>,
) -> Result<Vec<Job>> {
    if ids.is_empty() {
        return Err(Error::Invalid(
            "claim_jobs needs at least one job id".into(),
        ));
    }
    let ttl = ttl_secs
        .unwrap_or(DEFAULT_CLAIM_TTL_SECS)
        .clamp(60, MAX_CLAIM_TTL_SECS);

    let org = self.org();
    let mut sorted: Vec<String> = ids.iter().map(|i| i.0.clone()).collect();
    sorted.sort();
    sorted.dedup();

    // Reap any of the *requested* jobs whose claim has lapsed, exactly the
    // way acquire_lease reaps an expired lease on the specific branch being
    // acquired before checking availability — scoped to these ids, not
    // organization-wide, because there is no background sweeper and none is
    // needed: a stale claim on a job nobody is trying to (re)claim simply
    // sits until someone does, and ready() already tells callers it is
    // claimable in the meantime (see the design note in jobs.rs's module
    // doc / the design spec §1).
    sqlx::query(
        "UPDATE jobs SET status = 'pending', claimed_by = NULL, \
                claimed_by_label = NULL, started_at = NULL, claim_expires_at = NULL \
         WHERE org_id = $1 AND id = ANY($2) AND status IN ('in-progress', 'active') \
           AND claim_expires_at <= now()",
    )
    .bind(org)
    .bind(&sorted)
    .execute(self.conn())
    .await?;

    let locked: Vec<(String, Status)> = sqlx::query_as(
        "SELECT id, status FROM jobs WHERE org_id = $1 AND id = ANY($2) \
         ORDER BY id FOR UPDATE",
    )
    .bind(org)
    .bind(&sorted)
    .fetch_all(self.conn())
    .await?;

    // ... existing "all requested ids exist" + "all are Pending" + "none
    // blocked by an incomplete dependency" checks, unchanged ...

    let jobs: Vec<Job> = sqlx::query_as(&format!(
        "UPDATE jobs SET status = 'in-progress', started_at = now(), \
                attempts = attempts + 1, claimed_by = $3, claimed_by_label = $4, \
                claim_expires_at = now() + make_interval(secs => $5) \
         WHERE org_id = $1 AND id = ANY($2) RETURNING {JOB_COLS}"
    ))
    .bind(org)
    .bind(&sorted)
    .bind(claimer)
    .bind(label)
    .bind(ttl as f64)
    .fetch_all(self.conn())
    .await?;

    Ok(jobs)
}
```

`attempts` is untouched by the reap `UPDATE` (it only clears claim/status fields), so the
subsequent claim `UPDATE`'s `attempts = attempts + 1` still increments exactly once per
actual claim — a reap-then-reclaim inside one `claim_jobs` call is one claim, not two,
satisfying the AC's "`attempts` is not reset by a reap" (nothing resets it either; it is
simply never touched by the reap statement).

### `ready()`

The `WHERE` clause's status condition widens from `j.status = 'pending'` to:

```sql
(j.status = 'pending'
 OR (j.status IN ('in-progress', 'active') AND j.claim_expires_at <= now()))
```

everything else (`repo_id` filter, the `NOT EXISTS` incomplete-dependency check, the
`ORDER BY`) is unchanged — a job whose claim lapsed already had its dependencies checked
at the time of its original claim, and the same `NOT EXISTS` predicate is still correct to
re-run (a dependency `set_dependencies` added *after* the original claim would correctly
block it from `ready()` again, which is desirable, not a regression).

### Claimer-checked finalize

A private helper factors the check `finalize` and `renew_claim` both need:

```rust
/// Lock the job row, confirm it is claimed (`in-progress` or `active`), and
/// confirm `caller` is the one who holds it. Shared by `finalize` and
/// `renew_claim` — both need the identical fencing check, and the one
/// invariant this whole feature depends on is that they never drift apart.
async fn ensure_claim_held(&mut self, id: &JobId, caller: UserId) -> Result<()> {
    let org = self.org();
    let row: Option<(Status, Option<UserId>, Option<String>)> = sqlx::query_as(
        "SELECT status, claimed_by, claimed_by_label FROM jobs \
         WHERE org_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(org)
    .bind(id)
    .fetch_optional(self.conn())
    .await?;
    let (status, claimed_by, claimed_by_label) =
        row.ok_or_else(|| Error::JobNotFound(id.clone()))?;

    if !matches!(status, Status::InProgress | Status::Active) {
        return Err(Error::WrongStatus {
            job: id.clone(),
            actual: status.as_str().to_string(),
            expected: "in-progress or active".into(),
        });
    }
    if claimed_by != Some(caller) {
        return Err(Error::AlreadyClaimed {
            job: id.clone(),
            holder: claimed_by_label
                .or_else(|| claimed_by.map(|u| u.to_string()))
                .unwrap_or_else(|| "nobody".into()),
        });
    }
    Ok(())
}
```

`finalize` calls it first, then proceeds exactly as today (its own `SELECT ... FOR UPDATE`
is removed — `ensure_claim_held` already took and holds the row lock for the rest of the
transaction):

```rust
async fn finalize(
    &mut self,
    id: &JobId,
    caller: UserId,
    to: Status,
    result: Option<&str>,
    error: Option<&str>,
) -> Result<Job> {
    self.ensure_claim_held(id, caller).await?;
    let org = self.org();
    let job = sqlx::query_as(&format!(
        "UPDATE jobs SET status = $3, completed_at = now(), result = $4, error = $5 \
         WHERE org_id = $1 AND id = $2 RETURNING {JOB_COLS}"
    ))
    .bind(org)
    .bind(id)
    .bind(to)
    .bind(result)
    .bind(error)
    .fetch_one(self.conn())
    .await?;
    Ok(job)
}

pub async fn complete_job(
    &mut self,
    id: &JobId,
    caller: UserId,
    result: Option<&str>,
) -> Result<Job> {
    self.finalize(id, caller, Status::Completed, result, None).await
}

pub async fn fail_job(&mut self, id: &JobId, caller: UserId, error: Option<&str>) -> Result<Job> {
    self.finalize(id, caller, Status::Failed, None, error).await
}
```

Both existing call sites are `crates/of-mcp/src/tools/jobs.rs`'s `complete_job`/`fail_job`
tool handlers, which already have `caller.user_id` in scope from `self.caller(&parts)?` —
the change there is passing it through, nothing new to resolve.

### `renew_claim`

```rust
/// Push a held claim's expiry forward without completing or failing the
/// job — the direct analog of `renew_lease`. Only the current holder may
/// renew, via the same `ensure_claim_held` check `finalize` uses: otherwise
/// any caller could keep another agent's claim alive indefinitely, which
/// would defeat the reap this whole feature exists to enable.
pub async fn renew_claim(
    &mut self,
    id: &JobId,
    caller: UserId,
    ttl_secs: Option<i64>,
) -> Result<Job> {
    self.ensure_claim_held(id, caller).await?;
    let ttl = ttl_secs
        .unwrap_or(DEFAULT_CLAIM_TTL_SECS)
        .clamp(60, MAX_CLAIM_TTL_SECS);
    let org = self.org();
    let job = sqlx::query_as(&format!(
        "UPDATE jobs SET claim_expires_at = now() + make_interval(secs => $3) \
         WHERE org_id = $1 AND id = $2 RETURNING {JOB_COLS}"
    ))
    .bind(org)
    .bind(id)
    .bind(ttl as f64)
    .fetch_one(self.conn())
    .await?;
    Ok(job)
}
```

### `repend_job`

Its `UPDATE` gains `, claim_expires_at = NULL` alongside the fields it already clears
(`claimed_by`, `claimed_by_label`, …) — a repended job has no live claim, and leaving a
stale timestamp in place would be inert today (status is `pending`, so nothing reads it)
but confusing to a future reader of the row.

### Post-review corrections to this section

Six real issues surfaced by the mandatory review trio plus the automated pr-review-toolkit
passes, all fixed in the shipped code (the blocks above are the original design; this is
what changed):

1. **`claim_jobs`'s lock/reap ordering.** The code above runs the reap `UPDATE` before the
   `SELECT ... ORDER BY id FOR UPDATE`. Shipped order is reversed: the sorted `SELECT ...
   FOR UPDATE` runs first (locking every requested row in the deterministic order the
   function's own doc comment already promises), *then* the reap `UPDATE` runs scoped to
   that same id array — since every row it touches is already locked by this transaction,
   it acquires no new locks and cannot invert the sort order. The reap's `RETURNING id`
   is collected into a set, and the subsequent per-row status check treats a row in that
   set as `Pending` regardless of what the initial `SELECT` saw, rather than re-querying.
2. **The reap now also clears `cancel_requested_at`, `cancel_requested_by`, and
   `cancel_reason`** — the same three fields `repend_job` clears, for the identical
   reason: a reap is an involuntary repend, and a stale "please stop" aimed at the agent
   that died must not follow the job to whoever reclaims it.
3. **`finalize`'s `UPDATE` now also sets `claim_expires_at = NULL`**, and so does
   `cancel_job`'s and `close_from_ticket`'s (see below) — making §3's `Job.claim_expires_at`
   doc comment ("only a live claim has this set") actually true instead of aspirational.
4. **`ensure_claim_held`'s `AlreadyClaimed` fallback string** changed from `"nobody"` to
   `"no recorded holder (data inconsistency)"` — the branch is unreachable today (every
   path to `in-progress`/`active` sets `claimed_by`), and the new wording reads as a bug
   report rather than a plausible normal state if that invariant is ever broken by a
   future change.
5. **`cancel_job` gains a `caller: UserId` parameter** and calls `ensure_claim_held`
   first, exactly like `finalize` — see the Public interface note and Scope corrections
   above for why. Its internal `SELECT` for `cancel_requested_at` no longer needs its own
   `FOR UPDATE` (`ensure_claim_held` already holds the row lock) and no longer duplicates
   the status check.
6. **`close_from_ticket`'s `UPDATE`** (unchanged in every other respect — this is a
   tracker-webhook-driven finalize path with no `UserId` to fence against, and stays
   that way) **now also clears `claim_expires_at`**, for the hygiene reason in point 3,
   with no claimer check added: a human closing the linked ticket is a separate,
   trusted-by-construction signal, not "someone claiming to be the holder."

## §4 — `crates/of-core/src/error.rs`

```rust
#[error(
    "job {job} is currently claimed by {holder}, not you — your claim likely expired \
     and was taken over. Call get_job to see its current state, or claim_jobs if it \
     becomes available again; do not retry this call as-is."
)]
AlreadyClaimed { job: JobId, holder: String },
```

(was `#[error("job {job} was claimed by someone else")] AlreadyClaimed { job: JobId }`).
`code()`'s existing `Error::AlreadyClaimed { .. } => "already_claimed"` arm needs no
change. **Post-review correction, superseding the paragraph originally here:** three
independent reviewers (rust-pro, the blind security review, and the automated
silent-failure pass) converged on the same finding — the original claim that
`AlreadyClaimed` "was already correctly marked retriable... which is exactly right for
this repurposed meaning too" does not hold. Before this PR the variant had zero
construction sites, so its `retriable() == true` was vacuous; this PR is the first thing
that ever raises it, exclusively from `ensure_claim_held`'s fencing check. Unlike
`LeaseHeld` (retriable because the lease's *holder* can let it lapse without acting, so
waiting and retrying the identical call can succeed), a caller fenced out of
`complete_job`/`fail_job`/`cancel_job`/`renew_claim` cannot make that call succeed by
retrying it — the claim is gone for good, and the only forward path is a different call
(`claim_jobs`, or `get_job` to see the state). `retriable()` is shipped as
`matches!(self, Error::LeaseHeld { .. } | Error::Db(_))` — `AlreadyClaimed` removed
entirely, rather than split into two variants, since it now has exactly one call site
(`ensure_claim_held`) and that call site is never retriable.

## §5 — `crates/of-mcp/src/tools/jobs.rs`

`ClaimJobsArgs` gains:

```rust
/// Seconds before this claim expires if never renewed. Defaults to a
/// server-chosen TTL (900s) if omitted, clamped to at most 4 hours. Extend
/// it with renew_claim while you keep working — an unrenewed claim expires
/// and the job becomes claimable by someone else.
#[serde(default)]
pub ttl: Option<i64>,
```

`claim_jobs`'s handler passes `args.ttl` through to `tx.claim_jobs(...)`. Its description
gains a sentence: *"Claims expire (900s by default, or your ttl); renew_claim pushes a
claim you hold forward, and an expired claim becomes claimable again — see ready."*

`complete_job`/`fail_job`'s handlers pass `caller.user_id` as the new second positional
argument to `tx.complete_job`/`tx.fail_job`. Both descriptions gain: *"Fails if you are not
the job's current claim holder."*

New args + tool:

```rust
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RenewClaimArgs {
    /// The job you are still working on.
    pub job: String,
    /// Seconds to extend the claim by, from now. Same default and cap as
    /// claim_jobs's ttl if omitted.
    #[serde(default)]
    pub ttl: Option<i64>,
}
```

```rust
#[tool(
    name = "renew_claim",
    description = "Push a claim you hold forward, the way renew_lease extends a branch \
                   lease. Call this on a cadence comfortably shorter than the claim's TTL \
                   while a long-running job is still in progress: an unrenewed claim \
                   expires and the job becomes claimable by someone else, which is what \
                   lets a crashed agent's abandoned job be picked back up. Fails if you are \
                   not the job's current claim holder."
)]
pub async fn renew_claim(
    &self,
    Extension(parts): Extension<http::request::Parts>,
    Parameters(args): Parameters<RenewClaimArgs>,
) -> Result<Json<out::JobOut>, ErrorData> {
    let caller = self.caller(&parts)?;
    caller.require_scope(scope::JOBS_WRITE).mcp()?;

    let mut tx = self.tx(&caller).await?;
    // Recorded like every other call (of-billing's own doc comment: "record
    // every call regardless of class") even though it is classified Free —
    // renew_lease follows the identical pattern.
    self.charge(&mut tx, &caller, "renew_claim").await?;
    let job = tx
        .renew_claim(&JobId::from(args.job), caller.user_id, args.ttl)
        .await
        .mcp()?;
    tx.commit().await.mcp()?;

    Ok(Json(out::JobOut { job }))
}
```

`ready`'s tool description gains a clause noting it also lists jobs whose claim has
expired.

**Post-review corrections:** `RenewClaimArgs.ttl`'s doc comment shipped as *"Seconds from
now until the claim expires — not added to whatever time was left on it. Same default
(900s) and clamp range (60 seconds to 4 hours) as claim_jobs's ttl if omitted"* — the
original "extend... by" wording was found to read as additive when the actual behavior
recomputes the expiry outright from `now()`, which could pull expiry *closer* if a
caller passed a shorter `ttl` than the time remaining on their current claim.
`cancel_job`'s existing handler (predating this feature — not shown above) now forwards
`caller.user_id` as `Tx::cancel_job`'s new second argument, and its description gains the
same *"or if you are not its current claim holder"* clause `complete_job`/`fail_job`
carry — see §3's post-review corrections for why.

## §6 — `crates/of-billing/src/classify.rs`

`renew_claim` is added to `FREE`, next to `renew_lease`/`release_lease`, with a comment
extending their existing one: *"...and renew_claim, for the same reason: it is
coordination hygiene on a claim you already paid to take, not new work."* `claim_jobs`
needs no change to its existing `BILLABLE` entry.

## §7 — `crates/of-web`

`openapi.rs`'s `Job` schema gains, placed after `"claimedByLabel"`:

```rust
"claimExpiresAt": { "type": ["string", "null"], "format": "date-time" },
```

No route changes: `routes/jobs.rs` returns `of_core::jobs::Job` directly, so the new field
is just more data flowing through the existing read-only `GET`s, consistent with every
prior job-field addition (§7 of the cancellation design made the identical point).

## §8 — `web/`

- `src/lib/types.ts`: `Job.claimExpiresAt: string | null;`, placed after `claimedByLabel`.
- New `src/lib/jobs.ts`:
  ```ts
  import type { Job } from '$lib/types';

  /**
   * True for an in-progress/active job whose claim has lapsed — nobody is
   * actually working it, but nothing has reclaimed it yet. Derived
   * client-side from fields the server already returns; no new server
   * state, matching how the console derives every other display-only fact.
   */
  export function isClaimStranded(job: Pick<Job, 'status' | 'claimExpiresAt'>): boolean {
    if (job.status !== 'in-progress' && job.status !== 'active') return false;
    if (!job.claimExpiresAt) return false;
    return new Date(job.claimExpiresAt).getTime() <= Date.now();
  }
  ```
- `src/routes/o/[org]/queue/+page.svelte`: no existing precedent to follow here — today's
  status cell is a bare `<StatusPill status={job.status} />` with nothing beneath it in the
  table. This establishes a new placement: a small `text-bad` line under the pill in the
  same cell (not a new column — the table is already dense), matching only the *color*
  convention the detail page's cancellation-requested line already uses.
- `src/routes/o/[org]/queue/[job]/+page.svelte`: the header block gains a stranded line
  next to the existing cancellation-requested line, shown when `isClaimStranded(job)`.
- `web/messages/{en,es,de,fr,it,hi}.json`: a new `job_claim_stranded` key in each, next to
  `job_cancellation_requested`, translated at the same register ("Stranded — claim
  expired" in English). `npm run check` fails on any locale missing it.

Deliberately not touched: `StatusPill`'s `tones`/`JobStatus` (no new wire status is
introduced — a stranded job is still, on the wire, `in-progress` or `active`), the queue
page's status filter list (same reason), and the org overview's stat tiles (no new
counter is introduced, per Scope/Out).

## §9 — Testing

- `crates/of-core/tests/queue.rs`:
  - `claim_jobs` with no `ttl` sets `claim_expires_at` to roughly
    `now() + DEFAULT_CLAIM_TTL_SECS`; with an explicit `ttl` it is honored; a `ttl` above
    `MAX_CLAIM_TTL_SECS` or below 60 is clamped, not rejected (matching `acquire_lease`'s
    existing clamp-not-reject behavior).
  - A job whose `claim_expires_at` is forced into the past (via a raw
    `UPDATE jobs SET claim_expires_at = now() - interval '1 second' WHERE id = $1`, since
    nothing else can create that state quickly in a test) appears in `ready()`.
  - Calling `claim_jobs` on that same expired-claim job succeeds for a **different**
    claimer, sets the new `claimed_by`, and `attempts` is exactly `2` (one for the
    original claim, one for the reclaim — never reset, never double-counted).
  - `renew_claim` by the current holder extends `claim_expires_at`; by a different caller
    returns `AlreadyClaimed` naming the actual holder's label; on a `pending` or terminal
    job returns `WrongStatus`.
  - **Fencing**: claim as user A with a short `ttl`, force-expire it (or use `ttl: 60`
    and a manipulated `claim_expires_at` as above), claim as user B, then assert A's
    `complete_job`/`fail_job` call fails with `AlreadyClaimed { holder: <B's label> }`
    while B's own `complete_job` call succeeds. This is the exact interleaving from
    GH#65's rationale for filing the two defects together.
  - `complete_job`/`fail_job` called by the actual holder still succeed exactly as today
    (regression coverage for every existing test in this file that calls them).
  - `repend_job` on a claimed job clears `claim_expires_at` (extend the existing
    `repend_job` test coverage rather than adding a new test).
  - **Added during PR review**, closing gaps the pr-test-analyzer pass found: an
    `active` (not just `in-progress`) expired claim reaps, reappears in `ready()`, and
    fences identically — every prior test in this file happened to leave the job in
    `in-progress`, even though the reap/`ready()`/`ensure_claim_held` predicates all
    explicitly branch on `in-progress OR active`; `ready()` leaves the row itself
    untouched (`status`/`claimed_by` unchanged) after an expired claim appears in its
    result, proving §1's "reinterprets, never mutates" claim directly rather than only
    asserting the query's output; a reap clears a stale `cancel_requested_at`/
    `cancel_requested_by`/`cancel_reason` inherited from the original holder (proving the
    §3 post-review correction); and the same stale-holder-vs-reclaimer fencing test as
    `complete_job`/`fail_job`, now also for `cancel_job`.
- `crates/of-core/tests/isolation.rs`: extend `cross_org_mutation_is_refused` (or add a
  sibling following its exact pattern) with `renew_claim` called from the wrong org, and
  with `complete_job`/`fail_job`/`cancel_job` called from the wrong org against a job
  claimed in the right org — guard 1's proof for every new/changed statement.
- `crates/of-mcp/tests/tools.rs`:
  - End-to-end: `add_job` → `claim_jobs` (with an explicit `ttl`) → `renew_claim` extends
    it → `complete_job` succeeds.
  - End-to-end: `claim_jobs` → `complete_job` called with a *different* caller's token
    fails.
  - `renew_claim` appears in the tool list with a non-empty description
    (`every_tool_documents_itself`'s existing assertion pattern).
  - `every_tool_has_a_price`/`exhaustive_over`: `renew_claim` present and classified free;
    `claim_jobs` still billable.
- `crates/of-web`'s schema tests: extend the existing `Job` property assertions with
  `"claimExpiresAt"`, following the `"cancelRequestedAt"` precedent.
- `web/`: `npm run check` (message-catalog completeness, `isClaimStranded`'s type
  checking), `npm run lint`, `npm test` (no Worker routing change expected — a regression
  gate).

## Assumptions

- `DEFAULT_CLAIM_TTL_SECS = 900` and `MAX_CLAIM_TTL_SECS = 4 * 3600`, identical to
  `repo_leases`'s existing constants. *Rationale: a job claim and a branch lease cover the
  same real-world span — "however long the agent is actively working" — and the codebase
  already has an established, working answer for that span with a documented renewal
  convention (the MCP server's own onboarding instructions already tell agents to "take a
  lease... and renew it while you work"; `renew_claim` extends the identical convention to
  jobs). Reusing the number avoids inventing an arbitrary new one with no evidence behind
  it; the two constants are defined independently in `jobs.rs` so they can diverge later
  without one change accidentally retuning both resources.*
- No claimer check is added to `activate_job`, `cancel_job`, or `request_cancel`. See
  Scope/Out — the issue's AC names only `complete_job`/`fail_job`, and the prior
  cancellation design already made the "no asymmetric enforcement" call for the other two
  once; revising it for three more tools inside a spec about a different pair would be
  scope creep. Flagged below as a follow-up worth its own ticket.
- Jobs already `in-progress`/`active` at migration time get no retroactive
  `claim_expires_at` and are therefore not reapable until their current lifecycle ends
  (`complete_job`/`fail_job`/`repend_job`). *Rationale: a migration that started reaping
  live claims the instant it ran would be a surprise state change with no user action
  behind it — the conservative, forward-only-migration-consistent choice is "nothing
  changes for existing rows until the normal lifecycle touches them again."*
- The stranded indicator is computed client-side from `status` + `claim_expires_at` rather
  than as a new server-side computed field or wire status value. *Rationale: it is exactly
  the same kind of derived-display fact the console already computes for other things
  (`relative()` timestamps, `filtered` in the queue page); no other consumer of the API
  needs a "stranded" concept, and inventing a seventh wire status for what is really just
  "in-progress, but late" would ripple through `StatusPill`'s exhaustive `tones` record,
  the status filter list, and `Stats`, for a UI-only concern.*
- `renew_claim` calls `self.charge(...)` despite being classified `Free`, matching
  `renew_lease`'s existing pattern exactly (charge records usage unconditionally; only
  `Class::Billable` actually deducts from the plan's bucket). *Rationale: `of-billing`'s
  own module doc says "Both classes are recorded regardless" — this is not new reasoning,
  just applying the existing rule.*

## Error Handling & Edge Cases

- `claim_jobs` on a batch where one id is a live (non-expired) claim held by someone else:
  unchanged — still `WrongStatus` for the whole batch, all-or-nothing, exactly as today.
- `claim_jobs` on a batch mixing a genuinely-pending job and a stale expired claim: both
  reap-then-claim in the same statement pair, atomically — either the whole batch succeeds
  or (if a dependency is unmet, or another id is genuinely unavailable) none of it does,
  preserving the existing all-or-nothing guarantee.
- Two concurrent `claim_jobs` calls racing to reclaim the same expired job: both take the
  `FOR UPDATE` lock in the same deterministic sorted order the existing code already
  establishes, so they serialize; the loser's `SELECT` (after the winner commits) sees the
  row already back in `in-progress` under the winner's `claimed_by` and fails with
  `WrongStatus`, never a torn write. No new race is introduced — this is the same
  serialization the existing code already relies on for two ordinary concurrent claims.
- `renew_claim` racing a `complete_job`/`fail_job` on the same job by its actual holder:
  both go through `ensure_claim_held`'s `FOR UPDATE`, so they serialize; whichever commits
  first wins, and if `complete_job` wins, the job is now terminal and a subsequent
  `renew_claim` (from a would-be immediately-following heartbeat) correctly fails
  `WrongStatus`.
- `renew_claim`/`complete_job`/`fail_job` called by the *original* holder immediately after
  a *different* agent reclaims the same (now-expired) job: both go through
  `ensure_claim_held`, which now sees `claimed_by` pointing at the new holder, so the
  stale caller gets `AlreadyClaimed` naming the new holder — this is the fencing property
  the issue's AC requires, and it falls directly out of comparing `claimed_by`, needing no
  additional epoch/version column.
- A `ttl` of `0` or negative: clamped to the 60-second floor, same as `acquire_lease`
  already does for `ttlSeconds` — not rejected, since a client sending a degenerate value
  probably meant "as short as possible," and clamping is friendlier than an error for a
  value this harmless.

## Risks & Open Questions

- **Follow-up worth its own ticket, not in this scope:** whether `activate_job` should
  eventually get the same claimer check `complete_job`/`fail_job`/`cancel_job` get here.
  **Post-review update:** `cancel_job` was originally grouped in this note alongside
  `activate_job`/`request_cancel`; PR review found that grouping wrong (`cancel_job` is a
  third finalizer, not a `activate_job`-shaped no-op) and it is now fenced in this PR —
  see §3's post-review corrections. Only `activate_job` remains a real open question;
  `request_cancel` is excluded on its own, separate, already-settled design (it never
  finalizes anything).
- **Known limitation, not fixed in this PR:** the claimer fence (`ensure_claim_held`)
  checks `claimed_by: UserId` — the authenticated account — not a per-agent-instance
  identity. In a deployment where one token/account runs multiple concurrent agent
  processes (the org's own onboarding already anticipates this: `claim_jobs`'s `agent`
  label exists precisely so teammates can tell such instances apart in the queue view),
  the exact race this PR closes for two *different* accounts is **not** closed for two
  *instances under the same account*: agent A claims, crashes, its claim expires, agent
  A′ (same account) reclaims and starts real work, and A wakes up and calls
  `complete_job` — `claimed_by == Some(same account)` still holds, so the call succeeds
  and clobbers A′'s in-flight attempt exactly as GH#65 describes, just with the "someone
  else" being a different process under the same identity rather than a different
  identity. This is not a regression this PR introduces: `repo_leases::renew_lease`/
  `release_lease` have the identical shape today (`holder_user_id = $x`, no per-process
  token), and this PR's fencing is deliberately consistent with that existing, accepted
  trust model rather than inventing a stronger one unilaterally mid-review. Closing it
  properly needs a fencing token distinct from account identity (e.g. surfacing
  `attempts`, which already increments as a de facto claim generation counter, as a
  value `complete_job`/`fail_job`/`cancel_job`/`renew_claim` could optionally require to
  match) — a real design decision affecting three tool signatures, appropriately scoped
  to its own issue rather than expanded into this one during a review round.
- Reusing `repo_leases`'s exact TTL constants is a judgment call, not something the issue
  specifies numerically — flagged for the spec critique to weigh in on directly, since a
  coding job and a branch lease are similar but not identical in typical duration (a
  lease covers "however long this branch is checked out," a job claim covers "however
  long this specific unit of work takes," and the two could reasonably diverge). If 15
  minutes proves too short in practice once `renew_claim` ships, the fix is a config
  change to the constant, not a design change.
- **Process note, not a design risk:** this branch's migration collided with
  `0025_idempotency_keys.sql` (savvagent/otto-factory#99), which merged to master while
  this feature was in review — different filename, same leading version number, so `git`
  merged the branch cleanly with no conflict marker and the collision surfaced only when
  the architect and rust-pro reviews actually ran the CI-equivalent commands against a
  rebased branch. Renumbered to `0026`; no schema or design change resulted. Recorded here
  because it is the kind of gap a purely textual review (reading the diff without running
  it against current `origin/master`) cannot catch — the fix, in both this PR and as
  a general practice, is confirming the next free migration number against `origin/master`
  at merge time, not at branch-creation time.
- The reap step in `claim_jobs` is scoped to only the ids in the current request, matching
  `acquire_lease`'s per-branch scoping. This means a stale claim on a job nobody has
  listed via `ready()` and then explicitly named in a `claim_jobs` call stays visibly
  `in-progress` (and stranded, per the console) indefinitely — which is correct per the
  "no background sweeper" requirement, but worth stating plainly: recovery is
  discovery-driven, not time-driven. An org whose agents never call `ready()` again for a
  dead job would never see it reclaimed automatically, same as `repo_leases` today.
