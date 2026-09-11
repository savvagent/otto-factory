# Claim-generation fencing for complete_job/fail_job/cancel_job/renew_claim design

> **Status:** IMPLEMENTED — shipped in savvagent/otto-factory#161, closing
> savvagent/otto-factory#103, the known limitation recorded in
> `docs/specs/2026-09-10-queue-claim-expiry-design.md`'s Risks & Open Questions. Shipped as a
> **breaking** change (`fix(of-core)!: ...`) per the "Addendum" section below, not the purely
> additive change originally drafted.

> **Depends on:** `docs/specs/2026-09-10-queue-claim-expiry-design.md` (shipped in
> savvagent/otto-factory#102) — this spec extends `ensure_claim_held`, the claimer fence
> that design introduced.

## Addendum: the expiry fence (added during PR review, and non-additive)

The independent `security-auditor` pass on the PR implementing this spec raised a High
finding this spec did not anticipate: `expected_attempts` closes the race for a caller that
*learns about it*, but an expired-and-never-reclaimed claim was still finalizable/renewable
by its original holder with no argument at all — `ready()` and `claim_jobs` already treat
that claim as available the instant it lapses, so the stale holder finalizing over it is the
identical #65 race, left open. `ensure_claim_held` was extended, in the same PR, with a
server-side expiry check ahead of the account and generation checks this spec describes —
see `ensure_claim_held`'s doc comment in `crates/of-core/src/jobs.rs` for the exact
mechanics.

**This changes the "Public interface note" below: the overall change is not purely
additive.** Unlike `expected_attempts` (opt-in, silent for a caller that omits it), the
expiry check applies to *every* caller unconditionally: a caller that previously renewed or
finalized a claim after its TTL had quietly lapsed — relying on today's resurrection
behavior, with no argument to opt out — now gets `Error::AlreadyClaimed` instead. This is a
genuine behavior change to an existing, unconditional code path, not an addition alongside
it, and the PR carries a `!`/`BREAKING CHANGE:` marker for it per Non-Negotiable Rule 6. See
the updated "Public interface note" for the full accounting.

A second, narrower fix landed in the same round: `cancel_job` is exempted from the new
expiry check specifically when `cancel_requested_at IS NOT NULL` on the row — a human
already asked the job to stop before the claim lapsed, and letting the holder record
compliance is strictly more honest than forcing `fail_job`, which erases the "asked to stop,
and did" distinction `cancel_job` exists to preserve. This exemption is additive relative to
the expiry check it carves out of (it only ever makes a call succeed that the unqualified
expiry check would have refused), but the expiry check itself is not, so the PR's breaking
classification stands regardless.

## Premise correction

The issue's own writeup (and the spec it follows up on) states: *"`repo_leases::renew_lease`/
`release_lease` have the identical shape today (`holder_user_id` only, no per-process
token) — so #102 shipped consistent with that existing, accepted trust model."*

That characterization does not survive contact with `crates/of-core/src/leases.rs`. Leases
are **not** vulnerable to the exact race this issue describes, because `acquire_lease`
mints a **new row with a new `id: uuid::Uuid`** every time a resource is reclaimed after
its previous lease expired (`leases.rs:104-146`): the reap step marks the expired row
`released_at = now()`, `existing` then comes back `None`, and a fresh `INSERT` follows.
`renew_lease`/`release_lease` are keyed on that per-acquisition `id`
(`leases.rs:150-191`), not on `(org, repo, resource)` alone — so a stale holder's
old `lease_id` fails with `lease_not_held` the instant a new acquisition has run, because
the *old row* (not a new one sharing its identity) is what got marked released. This was
verified directly: a throwaway test (acquire → force-expire → reacquire under the same
`holder_user_id` with a different label → assert the returned `id` differs → assert
`renew_lease`/`release_lease` on the *old* `id` both fail `lease_not_held`) passed against
`master` as committed, with no code change. `acquire_lease`'s own re-acquire branch
(`if live.holder_user_id == holder`) only reuses the same `id` when the **existing lease
is still live** (not expired) — a different, already-documented, intentional convergence
behavior ("an agent that lost track of its own state converges rather than deadlocking
against itself"), not the crash-then-reclaim race this issue is about.

Jobs have no equivalent: a job's row (and its `JobId`) is the same for the job's entire
lifetime, and `claim_jobs`'s reap-then-reclaim (`jobs.rs:929-952`) does not change the job's
identity — only `claimed_by`/`claimed_by_label`/`claim_expires_at` move. There is no
existing column that changes identity on reclaim the way a lease's `id` does. `attempts`
(incremented once per actual claim, untouched by a reap `UPDATE`, per the 2026-09-10
spec's §3) is the closest thing jobs have to that, and today nothing checks it.

**This changes scope decision §4 below**: `repo_leases` does not need the analogous fix,
because it is not exposed to the race in question. This is stated as a decision, not
silently assumed — see §4.

## Goal & Success Criteria

`ensure_claim_held` (`jobs.rs:1085-1121`, added in #102) fences `complete_job`/`fail_job`/
`cancel_job`/`renew_claim` by comparing `claimed_by: UserId` — the authenticated account —
against the caller. That closes the race #65 describes (two *different* accounts racing
to finalize a job) but not two agent *instances* sharing one account/token: `claim_jobs`'s
`agent` label exists specifically so teammates can tell such instances apart in the queue
view, meaning this deployment already anticipates the shape it does not fence against.

Concretely: agent A claims a job under account R (getting back `attempts = 3`, say), A
crashes, the claim expires, agent A′ (also account R, a different process) reclaims it
(`attempts` becomes `4`) and starts real work, then A wakes up and calls `complete_job` —
`claimed_by == Some(R)` still holds, so the call succeeds and clobbers A′'s in-flight
attempt with a stale result, corrupting the audit trail and marking downstream
dependencies ready off the wrong outcome. Same shape as GH#65, with "someone else" being a
different process under the same identity rather than a different identity.

Success:

- `complete_job`, `fail_job`, `cancel_job`, and `renew_claim` each accept a new, optional
  `expected_attempts` (wire name `expectedAttempts`) argument.
- **Omitted (the default): `expected_attempts` itself adds nothing new to check.** Only
  `claimed_by == caller` is checked beyond the (separately added, see the Addendum above)
  expiry check, exactly as `ensure_claim_held` did before this spec's own argument existed.
  An existing caller that never learns about `expected_attempts` sees no *additional*
  refusal from it — but is not otherwise unaffected by this PR, because the Addendum's
  expiry check applies regardless of whether this argument is ever supplied. This is what
  makes `expected_attempts` itself additive; it is not what makes the PR as a whole additive
  — see the Addendum and the Public interface note.
- **Supplied:** the call additionally refuses (with the existing `Error::AlreadyClaimed`,
  same `code()` — see §3 for why no new error variant is introduced) unless the job's
  current `attempts` equals the supplied value. A caller that captured `attempts` at claim
  time (already present on every `Job` returned by `claim_jobs`, or refreshable via
  `get_job`) and supplies it back is refusing to act on a claim generation it no longer
  actually holds, closing the exact interleaving in Goal above: A's stale
  `expected_attempts = 3` call fails naming A′'s label (via the current
  `claimed_by_label`) once `attempts` has moved to `4`; A′'s own call, with
  `expected_attempts = 4`, still succeeds.
- Tool descriptions for all four tools explain `expectedAttempts` to an LLM caller that has
  never read this spec: what it guards against, where to get the value, and that omitting
  it keeps today's behavior.
- `repo_leases::renew_lease`/`release_lease` are explicitly decided **out of scope**, with
  the reasoning in §4 — not silently excluded.
- A test proves the exact interleaving: same account claims, claim expires, same account
  reclaims under a different label, the stale caller's finalize/renew call with the
  original `attempts` value fails naming the new holder's label, and the new holder's own
  call with the current `attempts` value succeeds. A second test proves omitting
  `expected_attempts` reproduces every pre-existing `ensure_claim_held` test unchanged.
- A permanent regression test locks in §"Premise correction"'s finding for leases, so a
  future change to `acquire_lease`'s reclaim path that accidentally reused the old lease
  `id` would be caught here, not discovered again as a live incident.

## Scope

**In:**
- `crates/of-core/src/jobs.rs`: `ensure_claim_held`, `finalize`, `complete_job`, `fail_job`,
  `cancel_job`, `renew_claim` — new `expected_attempts: Option<i32>` parameter, threaded
  through.
- `crates/of-mcp/src/tools/jobs.rs`: `CompleteJobArgs`, `FailJobArgs`, `CancelJobArgs`,
  `RenewClaimArgs` gain `expected_attempts: Option<i32>` (wire: `expectedAttempts`); the
  four tool handlers pass it through; the four tool descriptions explain it.
- Tests: new `of-core` tests proving the fix (`crates/of-core/tests/queue.rs`) and a new
  leases regression test locking in §"Premise correction". New `of-mcp` tests proving the
  MCP-level wiring (`crates/of-mcp/tests/tools.rs`).
- Every existing call site of the four changed `Tx` methods across the workspace updated
  to pass `None` (mechanical; the compiler enumerates every site once the signature
  changes — see the plan's verification step).

**Out:**
- `repo_leases::renew_lease`/`release_lease` — decided out of scope; see §4. Not a
  silent omission.
- `activate_job` gaining the same fence — already an open, separately-scoped question per
  the 2026-09-10 spec's Risks & Open Questions (`activate_job` is a refinement signal, not
  a finalizer; nothing clobbers by racing it). Unchanged by this spec.
- Any new MCP tool. No `of-billing::classify` entry is needed — no tool is added, and the
  four affected tools are already classified.
- A schema/migration change. `attempts` already exists on `jobs` (added long before #102);
  nothing new is persisted.
- `close_from_ticket` — it finalizes a job from an inbound tracker webhook, not an MCP
  caller, and by its own existing doc comment never calls `ensure_claim_held` at all (there
  is no `UserId` to check against a webhook). Unaffected by this spec; not touched.
- Reworking `Error::AlreadyClaimed`'s wording to distinguish "different account" from
  "same account, different generation" — see §3 for why the existing message already
  covers both correctly.

Checked against the three constraints in `CLAUDE.md`: this only tightens who may finalize
a repo-anchored job's own row (constraint 1, unaffected); it adds no workflow opinion — the
server still does not care *what* `expected_attempts` means to the caller's own retry
policy, only that it must match if supplied (constraint 2); and it is enforced identically
regardless of which coding agent supplied it — no client-specific behavior (constraint 3).

## §1 — Why `attempts` and not a new column or a random token

Three alternatives were considered:

1. **A new dedicated `claim_token` column** (e.g. a fresh UUID minted on every claim,
   mirroring what leases get "for free" from their new-row-per-reclaim shape) — would work,
   but needs a migration, a new `Job` field, and a new thing for every caller to learn,
   for a guarantee `attempts` already provides. Rejected as unnecessary surface for the
   same outcome.
2. **Reusing `claim_expires_at`** as an implicit generation marker — rejected: two
   sequential claims of the same job with the same `ttl` could coincidentally produce
   close timestamps, and comparing timestamps for exact equality is exactly the kind of
   fragile check a `-D warnings` clippy pass and a reviewer would rightly flag; `attempts`
   is an integer that increments by exactly 1 per claim, with no such ambiguity.
3. **`attempts`** — already exists, already documented as "a de facto claim-generation
   counter" in the 2026-09-10 spec's own Risks & Open Questions (the issue's own suggested
   fix), already returned on every `Job` a caller sees (`claim_jobs`'s response, `get_job`,
   `ready`), and already proven immune to being touched by a reap
   (`claim_jobs`'s reap `UPDATE` at `jobs.rs:941-952` clears `claimed_by`/
   `claimed_by_label`/`started_at`/`claim_expires_at`/the three cancellation fields, but
   never `attempts`). Chosen.

## §2 — `crates/of-core/src/jobs.rs`

### `ensure_claim_held`

Gains one parameter and one additional check, after the existing identity check:

```rust
async fn ensure_claim_held(
    &mut self,
    id: &JobId,
    caller: UserId,
    expected_attempts: Option<i32>,
) -> Result<()> {
    let org = self.org();
    let row: Option<(Status, Option<UserId>, Option<String>, i32)> = sqlx::query_as(
        "SELECT status, claimed_by, claimed_by_label, attempts FROM jobs \
         WHERE org_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(org)
    .bind(id)
    .fetch_optional(self.conn())
    .await?;
    let (status, claimed_by, claimed_by_label, attempts) =
        row.ok_or_else(|| Error::JobNotFound(id.clone()))?;

    if !matches!(status, Status::InProgress | Status::Active) {
        return Err(Error::WrongStatus {
            job: id.clone(),
            actual: status.as_str().to_string(),
            expected: "in-progress or active".into(),
        });
    }

    let holder = claimed_by_label
        .or_else(|| claimed_by.map(|u| u.to_string()))
        .unwrap_or_else(|| "no recorded holder (data inconsistency)".into());

    if claimed_by != Some(caller) {
        return Err(Error::AlreadyClaimed { job: id.clone(), holder });
    }

    // Claim-generation fence, closing savvagent/otto-factory#103: `attempts`
    // increments exactly once per actual claim (`claim_jobs`) and is never
    // touched by a reap, so it doubles as a generation counter distinguishing
    // *which* claim a caller holds, not merely *whose account* holds it —
    // the gap the identity check above leaves open when one account runs
    // multiple concurrent agent processes (claim_jobs's own `agent` label
    // exists precisely so those processes can be told apart in the queue
    // view). A caller that never learns about this argument is unaffected:
    // `expected_attempts: None` skips this check entirely, so nothing about
    // today's behavior changes for it.
    if let Some(expected) = expected_attempts {
        if attempts != expected {
            return Err(Error::AlreadyClaimed { job: id.clone(), holder });
        }
    }

    Ok(())
}
```

Deliberately the *same* `Error::AlreadyClaimed` the identity check already raises, not a
new variant — see §3.

### `finalize`, `complete_job`, `fail_job`, `renew_claim`, `cancel_job`

Each gains a trailing `expected_attempts: Option<i32>` parameter, threaded straight to
`ensure_claim_held`:

```rust
pub async fn complete_job(
    &mut self,
    id: &JobId,
    caller: UserId,
    result: Option<&str>,
    expected_attempts: Option<i32>,
) -> Result<Job> {
    self.finalize(id, caller, Status::Completed, result, None, expected_attempts)
        .await
}

pub async fn fail_job(
    &mut self,
    id: &JobId,
    caller: UserId,
    error: Option<&str>,
    expected_attempts: Option<i32>,
) -> Result<Job> {
    self.finalize(id, caller, Status::Failed, None, error, expected_attempts)
        .await
}
```

`finalize` and `cancel_job` pass it straight to their own `ensure_claim_held` call.
`renew_claim` does the same, unchanged otherwise.

**Why extend the existing signatures rather than add parallel `*_with_generation`
methods:** this repo's own precedent (the 2026-09-10 plan, in the very PR that created
`ensure_claim_held`) already extended `cancel_job`'s signature in place to add the
`caller: UserId` fence, updating every call site rather than keeping an unfenced sibling
method alive. A second public entry point per finalizer would be exactly the kind of
type-design smell (two ways to do the same thing, one of them silently weaker) this
repo's `type-design-analyzer` review step exists to catch. The cost is mechanical: every
existing call site across the workspace must add one trailing argument. The plan makes
the compiler responsible for finding every site (a missed one is a build error, not a
silent gap) rather than trusting a grep to be exhaustive.

## §3 — Reusing `Error::AlreadyClaimed`, not a new error variant

`Error::AlreadyClaimed`'s message is already generation-agnostic and correct for both
cases:

> "job {job} is currently claimed by {holder}, not you — your claim likely expired and was
> taken over. Call get_job to see its current state, or claim_jobs if it becomes available
> again; do not retry this call as-is."

For the identity mismatch (different account), `{holder}` names the other account's
label. For the generation mismatch this spec adds (same account, different process),
`{holder}` names the *reclaiming instance's* `claimed_by_label` — exactly as useful,
since `claim_jobs`'s `agent` label is what a fleet uses to tell its own processes apart in
the first place. Both are, from the caller's perspective, the identical remedy: stop,
call `get_job` to see who holds it now, do not retry this exact call. Minting a second
error code for the same remedy would only cost every existing `err.code() ==
"already_claimed"` branch a second case to handle for no behavioral difference.
`retriable()` is already `false` for `AlreadyClaimed` (per the 2026-09-10 spec's PR-review
correction) and stays so — a generation mismatch is exactly as un-retriable as an
identity mismatch: the *same* call can never succeed again on retry, only a different one
(`get_job`, `claim_jobs`) can.

## §4 — Scope decision: `repo_leases` does **not** get the analogous fix

The issue asks this to be decided and stated explicitly, not silently included or
excluded. Decision: **out of scope**, because — per the Premise correction above —
`repo_leases` is not exposed to the race this issue describes. `acquire_lease` already
mints a new `id` on every reclaim-after-expiry, and `renew_lease`/`release_lease` are
keyed on that per-acquisition `id`, so a stale holder's old `id` already fails
`lease_not_held` the moment a new acquisition has happened — there is no window where a
stale renew/release call can succeed against a lease a different process has since
reclaimed. Adding an `expected_...` parameter to `renew_lease`/`release_lease` would
duplicate a guarantee those functions already have under a different name (the lease
`id` itself already *is* the generation token; leases never needed `attempts`'s
stand-in because they never lost their own).

What §"Premise correction" does **not** claim: that leases have *no* residual sharp edge
in the same-account, multiple-instances scenario generally. `acquire_lease`'s
still-live-re-acquire branch (`if live.holder_user_id == holder`) lets a second process
under the same account silently take over (well, share) a *still-valid* lease by
re-acquiring it — documented, intentional convergence behavior for an agent that lost
track of its own state, not a crash-then-reclaim race, and not the shape #103 describes.
Widening leases' re-acquire semantics is a different, separately-scoped design question if
it is ever raised — not implied or requested by this issue, and not addressed here.

## §5 — `crates/of-mcp/src/tools/jobs.rs`

Each args struct gains one field:

```rust
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompleteJobArgs {
    pub job: String,
    /// What you did, for whoever reads this later.
    #[serde(default)]
    pub result: Option<String>,
    /// Optional: the `attempts` value you saw when you claimed this job (from
    /// claim_jobs's response, or a later get_job) — a generation number, not
    /// a retry count. If a different process under your own account has
    /// since reclaimed this job after your claim lapsed, `attempts` has
    /// moved on; supplying the value you actually hold makes this call fail
    /// (naming the current holder) instead of silently overwriting their
    /// in-flight work. Omit it to keep matching by account alone, as before.
    #[serde(default)]
    pub expected_attempts: Option<i32>,
}
```

`FailJobArgs`, `CancelJobArgs`, `RenewClaimArgs` gain the identical field and doc comment,
adjusted for each tool's own verb ("failing"/"cancelling"/"renewing"). Each of the four
tool `description`s gains one sentence naming `expectedAttempts` and its purpose, per this
repo's "descriptions are the documentation" convention — the reader is an LLM that has
never read this spec.

The four handlers (`complete_job`, `fail_job`, `cancel_job`, `renew_claim`) pass
`args.expected_attempts` through to the corresponding `Tx` method.

**Not modified:** `tools::out` (no envelope shape changes — every one of these tools
already returns `{"job": …}`, and `Job.attempts` is already serialized), `of-billing`
(no new tool), any migration (no schema change), `docs/clients/matrix.md` (nothing here
depends on a specific client's behavior — the argument is optional and generic).

## Tenant isolation

No tenant table is added and no column is added to one. `jobs` is already a tenant table
registered in `0007_rls.sql`'s `tenant_tables` array, guarded by the existing
`jobs_tenant_isolation` policy (`org_id = current_org()`), which already covers `attempts`
on every row exactly as it covers every other column — `ensure_claim_held`'s new `SELECT`
runs inside the same `Tx` (same `org_id = $1` predicate, same RLS context) as its existing
one. Load-Bearing Invariant 1's cross-org-negative-test requirement applies to new tenant
*tables*; this spec adds neither a table nor a column, only a new comparison inside an
existing, already-org-scoped query. The existing cross-org tests in
`crates/of-core/tests/isolation.rs` that call `complete_job`/`fail_job`/`cancel_job`/
`renew_claim` (asserting org B cannot touch org A's job) are updated only mechanically
(the new trailing parameter) and continue to prove the same cross-org boundary; no new
cross-org test is required for this change specifically, since it introduces no new
tenant-scoped surface, only an additional in-org check on an existing one.

## Public interface note

**Revised per the Addendum above: this PR is a breaking change overall, per Non-Negotiable
Rule 6, and carries a `!`/`BREAKING CHANGE:` marker.** The `expected_attempts` argument
itself is additive in isolation; the server-side expiry check added in the same PR is not,
because it changes behavior on a code path every existing caller already exercises
unconditionally:

- Four MCP tools (`complete_job`, `fail_job`, `cancel_job`, `renew_claim`) each gain one
  new **optional** input field, `expectedAttempts`. No field is renamed or removed, no
  tool is renamed or removed, and every result envelope is unchanged
  (`out::JobOut`, i.e. `{"job": …}`). This part is additive on its own.
- **This part is not additive:** an existing caller that renews or finalizes a claim after
  it has already expired — with no argument, new or old, that opts out — previously
  succeeded (the claim was quietly resurrected) and now fails with `Error::AlreadyClaimed`.
  `expected_attempts` omitted does *not* preserve today's behavior in this case; it only
  preserves it for the already-reclaimed case (`ensure_claim_held`'s doc comment and
  `expected_attempts_omitted_preserves_todays_behavior_for_a_reclaimed_claim` in
  `crates/of-core/tests/queue.rs` are both named for exactly this narrower scope, not for
  blanket compatibility).
- `cancel_job` additionally gains a carve-out that is additive *relative to* the expiry
  check above (it only turns a refusal into a success, never the reverse): when
  `cancel_requested_at IS NOT NULL` on the row, `cancel_job` still succeeds on an expired,
  unreclaimed claim. This narrows the breaking surface for `cancel_job` specifically but
  does not make the overall change additive, since `complete_job`/`fail_job`/`renew_claim`
  carry the unqualified break.
- `crates/of-core`'s `complete_job`/`fail_job`/`cancel_job`/`renew_claim`/`finalize`/
  `ensure_claim_held` Rust function signatures change (new trailing parameters, including
  `honor_pending_cancel: bool` from the Addendum). This part is **not** a customer-facing
  interface under Rule 6's own list (MCP tool surface, console REST API, OAuth/discovery,
  config surface, DB schema) — `of-core` has no HTTP and no external callers of its own;
  every call site inside the workspace is updated in the same PR, and the compiler — not a
  grep — is what proves none was missed. (This bullet is about the Rust signatures only; it
  does not extend to the MCP tool behavior, which is where the actual break lives.)
- `Error::AlreadyClaimed` gains no new field and no new variant (§3) — its wire shape
  (`code: "already_claimed"`, a `message` string) is unchanged. The break is in when this
  error is returned, not in its shape.

## Error Handling & Edge Cases

- **`expected_attempts` supplied but the job has never been claimed (pending):** the
  existing status check in `ensure_claim_held` fires first (`WrongStatus`, not
  `AlreadyClaimed`) — unchanged from today, since the generation check only runs after
  the status and identity checks pass.
- **`expected_attempts` supplied, identity matches, generation matches:** proceeds exactly
  as if the argument were omitted.
- **`expected_attempts` supplied, identity does not match (different account):** the
  existing identity check fires first; the generation check is never reached. No change in
  behavior from today's `AlreadyClaimed` for that case.
- **`expected_attempts` supplied and stale (identity matches, generation does not):** new
  behavior — `AlreadyClaimed`, naming the current `claimed_by_label`.
- **Terminal job (`completed`/`failed`/`cancelled`), `expected_attempts` supplied:** the
  existing status check fires first, exactly as for the identity-only case today.
- **`cancel_job` with a matching `expected_attempts` but no cancellation request on
  file:** unchanged — `ensure_claim_held` (including its new check) runs first inside
  `cancel_job`, then the existing `cancel_requested_at IS NULL` check runs exactly as
  today, independent of `expected_attempts`.
- **Claim expired, caller is the account that held it, nobody has reclaimed it yet:**
  refused (`AlreadyClaimed`, "was claimed by you, but that claim has expired…") for
  `complete_job`/`fail_job`/`renew_claim`. For `cancel_job` specifically, this succeeds
  instead when `cancel_requested_at IS NOT NULL` on the row (see the Addendum) — the one
  place the caller's own expired claim does not block it.
- **Claim expired, caller never held this claim at all (a different account probing an
  expired job):** the expiry-specific wording is never shown to this caller — it would
  falsely assert "was claimed by you" — and the call falls straight through to the ordinary
  account-mismatch refusal ("claimed by X, not you"), whether or not the claim happens to
  be expired. Covered by
  `an_account_that_never_held_the_claim_is_not_told_its_own_claim_expired` in
  `crates/of-core/tests/queue.rs`.

## Assumptions

- **The field is named `expected_attempts` (wire: `expectedAttempts`), not `generation` or
  `claimGeneration`.** Rationale: it is literally the job's own `attempts` value, already
  visible to every caller under that name; introducing new vocabulary for the same number
  would cost a caller a second concept to learn for no added clarity. Flagged for spec
  critique to weigh in on directly, since naming is genuinely a judgment call.
- **All four tools get the argument in the same PR, not phased.** They share one
  `ensure_claim_held` choke point already, so there is no partial-rollout state where one
  tool has the fence and a sibling does not — implementing one without the others would
  leave the exact race open on whichever is skipped.
- **No new audit event.** `cancel_job` and `request_cancel` already audit
  `JOB_CANCELLED`/`JOB_CANCEL_REQUESTED`; a generation mismatch is a refusal (nothing
  changes state), and refusals are not audited elsewhere in this file today
  (`complete_job`-from-a-non-holder isn't audited either) — consistent, not a gap this
  spec introduces.

## Risks & Open Questions

- **Naming judgment call:** `expected_attempts` vs. a fresh name — see Assumptions. If
  spec critique or PR review prefers different wording, the field is renamed before
  merge; either way it is a one-time naming choice with no migration cost since nothing is
  persisted under this name.
- **Residual leases sharp edge, explicitly out of scope (§4):** `acquire_lease`'s
  still-live re-acquire branch lets two processes under one account share a still-valid
  lease's `id`. Not this issue's shape, not touched here; noted so it is not rediscovered
  as if it were new.
- **This does not stop an agent instance from simply never learning its own `attempts`
  value** (e.g. an agent that discards `claim_jobs`'s response and never calls `get_job`)
  — such a caller keeps omitting `expected_attempts` and keeps today's account-only
  fencing. This is inherent to an *optional*, additive argument: the fix is available to
  every caller, not forced on any of them, per Non-Negotiable Rule 6's own additive-change
  contract. A future, separately-scoped decision could make it mandatory for a given
  client's own skill to always supply it — a customer's own skill choice (constraint 2:
  substrate, not workflow), not a server-side requirement.
