# Job cancellation design

> **Status:** DRAFT — lets anyone with `jobs:write` ask a job to stop, closing
> savvagent/otto-factory#67.

## Goal & Success Criteria

Every coordination signal in otto-factory today points *into* the queue: an agent claims,
activates, completes, or fails a job, but nothing points back out at an agent already
running. This adds a way to signal that a job should stop — for a human watching an agent
burn tokens down a wrong path, or a supervising agent that has learned a job is obsolete —
without giving the server any power it does not already have: it cannot stop a process any
more than it can stop a `git push`. It can only make the request visible and let the
holder act on it.

Success:

- §1 below states, explicitly and first, whether a cancellation request is compatible
  with `the_queue_is_read_only_over_the_console` (`crates/of-web/src/openapi.rs`) as it
  stands today. It is: this feature adds no console route, so that test needs no
  amendment. See §1 for why that is the right call rather than a dodge.
- A new MCP tool, `request_cancel`, lets any caller with `jobs:write` ask a `pending`,
  `in-progress`, or `active` job to stop. For a `pending` job — nobody is running it yet —
  the request finalizes the job immediately, to a new terminal `cancelled` status, because
  there is no holder to wait on. For `in-progress`/`active`, it only records the request;
  the job's status does not change.
- The request is visible to the holder through the ordinary channels that already exist —
  `get_job` returns the new fields, and the `UPDATE` that sets them fires the existing
  `jobs_notify` trigger, which wakes any `watch` call already in flight. No changes to
  `watch`/`Watcher` are needed.
- A second new tool, `cancel_job`, is how the holder of an `in-progress`/`active` job
  complies: it finalizes the job as `cancelled`, and refuses if no cancellation was ever
  requested — an agent stopping for its own reasons still calls `fail_job`.
- The audit trail (`of_core::audit`) records who requested a cancellation and, separately,
  who finalized it — two events, `job.cancel.requested` and `job.cancelled`, so a claimed
  job's record always shows both actors even when they differ, and a pending job's record
  shows both events from the same actor rather than collapsing them into one.
- Both new tools are classified in `of-billing::classify` (`every_tool_has_a_price` must
  not fail) and appear in `of-mcp`'s tool list with descriptions a caller with no other
  context can act on (`every_tool_documents_itself`).
- `Status::is_terminal()` includes `Cancelled`; `Stats` gains a `cancelled` counter so its
  buckets keep summing to `total`; `list_jobs`/`ready`/`blocked` behave correctly with the
  new status (`ready`/`blocked` are unaffected — both already filter to `pending`, and a
  `pending` job never stays `pending` after this feature touches it).
- A cross-org negative test proves guard 1 (the explicit `org_id` predicate) holds for
  both new `of-core::jobs` functions, following `cross_org_mutation_is_refused`'s pattern.
  No new RLS policy registration is needed — `jobs` is already a tenant table in
  `0007_rls.sql`, and RLS is row-scoped, not column-scoped.

## Public interface note

Per Non-Negotiable Rule 6, this is **additive, not breaking**: `job_status` gains a new
enum value (`cancelled`); `jobs` gains three new nullable columns; two new MCP tools are
added alongside the existing ones; `Stats`/`QueueStats` gain a `cancelled` field. No tool,
field, route, or column is renamed or removed, and no existing result envelope's shape
changes — `request_cancel` and `cancel_job` both return the existing `{"job": …}` envelope
(`out::JobOut`). No version bump is required. No console route is added or changed, so
`the_queue_is_read_only_over_the_console` needs no amendment — see §1.

## Scope

**In:**

- §1's resolution of the read-only-console tension (a design decision, not code).
- New migrations: `crates/of-core/migrations/0023_job_cancelled_status.sql` (the enum
  value, its own file per the `0016`/`0017` precedent) and
  `crates/of-core/migrations/0024_job_cancellation_columns.sql` (the three columns).
- `crates/of-core/src/jobs.rs`: `Status::Cancelled`, `Tx::request_cancel`,
  `Tx::cancel_job`, `Job`'s three new fields, `Stats.cancelled` and its query,
  `repend_job` clearing the cancellation fields on revival and accepting `Cancelled` as a
  starting state (already does, structurally — see §3).
- `crates/of-mcp/src/tools/jobs.rs`: `request_cancel` and `cancel_job` tools,
  `sync_ticket`'s status match gaining a `Cancelled` arm, `ListJobsArgs`/`ready`/`stats`
  tool description updates listing the new status.
- `crates/of-core/src/audit.rs`: `JOB_CANCEL_REQUESTED` and `JOB_CANCELLED` action
  constants.
- `crates/of-billing/src/classify.rs`: both new tools classified `BILLABLE`.
- `crates/of-web/src/openapi.rs`: `Job` schema gains the three new properties, the status
  `enum` gains `"cancelled"`, `QueueStats` gains `"cancelled"`. `routes/jobs.rs`'s
  `ListJobsQuery.status` doc comment.
- `web/`: `JobStatus` type, `StatusPill` tone, `labels.ts`'s `statusLabel` switch and the
  `status_cancelled` message key in all six locale catalogs, the queue page's status
  filter list. (Required, not optional — `tones: Record<JobStatus, string>` and the
  `statusLabel` switch are exhaustive over `JobStatus`; adding `'cancelled'` to the type
  without updating both fails `npm run check`.)
- Tests: `crates/of-core/tests/queue.rs`, `crates/of-mcp/tests/tools.rs`,
  `crates/of-core/tests/isolation.rs`, `crates/of-web/tests/console.rs` (schema
  assertions), `web/` (`npm run check`, `npm run lint`).

**Out:**

- Any console route or UI control that *sends* a cancellation request. §1 explains why:
  the capability is MCP-only, matching how every other job write already works, and
  keeping the console's read-only invariant intact rather than amending it.
- Actually terminating a process. The server has no channel into an agent's runtime; it
  can only record and surface intent, exactly like `repo_leases`.
- A dedicated tracker-side (GitHub/JIRA) outbound signal for cancellation — no
  `not_planned` GitHub close reason, no distinct JIRA status category. §4 originally
  planned to reuse the existing `JobTransition::Failed` outbound plumbing for this; PR
  review found that wrong, since `outbound_decision`'s `(Failed, Jira)` arm transitions a
  ticket to the "new" status category, announcing "still needs doing" about work someone
  asked to stop. A `JobTransition::Cancelled` variant was added specifically to avoid that
  wrong signal, so — corrected from the original claim below — `of_trackers::sync` was
  not left untouched. A bespoke external signal is still out of scope: `Cancelled` gets
  a comment only, no GitHub close and no JIRA transition, matching `Failed`'s own
  no-op-beyond-a-comment shape on both.
- Inbound recognition of an externally-cancelled ticket (e.g. a human closing a GitHub
  issue as "not planned" flowing back to a local `Cancelled` job). Out for the same
  reason: not required by the issue, and the existing inbound mapping to `Failed` already
  covers that external signal without this feature touching it.
- An ownership check restricting `request_cancel`/`cancel_job` to specific callers (e.g.
  only an admin, or only the job's creator). Matches the existing, deliberate lack of a
  claimant check on `complete_job`/`fail_job` — any caller with `jobs:write` in the org
  may act on any job today, and introducing asymmetric enforcement for these two tools
  alone would be a new, separate policy decision.
- A dedicated "Cancelled" tile on the console's org overview page. The existing tiles
  (`In progress`, `Active`, `Blocked`, `Failed`) are reserved for states that need
  attention; a cancellation was requested on purpose, so it does not need the same visual
  alarm. `stats.cancelled` is available for a future dashboard to use.
- Rate-limiting or deduplicating repeated `request_cancel` calls against the same job.
  Each call simply re-stamps the request fields and emits a fresh
  `job.cancel.requested` audit event — harmless, and simpler than tracking whether "the
  same" request was already made.

## §1 — Is a cancellation request compatible with `the_queue_is_read_only_over_the_console`?

**Yes, and without amending the test, because this feature adds no console-facing write
at all.**

The issue's own framing argues that a cancellation *request* is a different category from
what that test guards against: it is an intent, not a claim about what happened, and the
holding agent's own `fail_job`/`cancel_job` call is still what records the outcome. That
argument is correct as far as it goes — but it is an argument for amending the test to
let a **console** endpoint send this intent. This design does not need that argument,
because the human-facing half of the problem ("a human watching an agent burn tokens")
does not require a console button to solve.

otto-factory is MCP-first by constitution (constraint 3: every coding agent is equally
first-class) and substrate-not-workflow by constitution (constraint 2: a capability that
could live in a customer's own tooling belongs there, not in a bespoke server feature).
A human today already has a way to call any MCP tool without running a full coding
agent: a personal access token and a one-line script or CLI invocation, the same path
that lets a human call `send_message` or `add_job` by hand. Adding a console-only write
path for *this one tool* — and no other job-mutating tool — would special-case
cancellation as a first-party console feature in a product that otherwise deliberately
gives the console zero write access to the queue, for every other job transition, on
principle. It would also mean asserting, for the first time, that a human's *intent* is
allowed to reach the queue through a channel an *agent's outcome* is not — a distinction
real enough to explain in prose but a strange one to encode as "this one write endpoint
is fine because it's only advisory," when the console's existing rule draws no such line
today.

So: **the console stays exactly as read-only as it is now.** `GET
/api/orgs/{org}/jobs`/`.../jobs/{job}` already return whatever fields `Job` has, so a
human watching the console already sees `cancelRequestedAt`/`cancelRequestedBy`/
`cancelReason` the moment anyone — themselves via a PAT, or a supervising agent — calls
`request_cancel`. What they cannot do is click a button in the browser to send the
request; they call the tool, the same way they would call it to queue a job by hand.
`the_queue_is_read_only_over_the_console` needs no code change and no amendment to its
stated reasoning, because nothing in this feature contradicts it: every write this
feature introduces is an MCP tool, exactly like every job write before it.

If a future change wants a console button for this, that is the moment to revisit this
test's reasoning explicitly — not now, when the simpler answer already satisfies the
issue's stated acceptance criteria.

The issue's separate AC bullet — "the MCP tool description tells an LLM caller what it is
expected to do when it sees the flag" — is satisfied by the doc comment on
`Job::cancel_requested_by` in §3, not by prose in `get_job`'s own tool description: `Job`
derives `schemars::JsonSchema`, so that field-level doc comment becomes the property
description in every schema a caller reads it through (`get_job`'s, `list_jobs`'s, and
both new tools' output schemas), which is the more precise place for "here is what to do
about this field" to live than a paragraph on `get_job` that would have to repeat it.

## §2 — Migrations

`crates/of-core/migrations/0023_job_cancelled_status.sql`:

```sql
-- Adds 'cancelled' to job_status: a terminal state reached either immediately
-- (a pending job that nobody has claimed yet, cancelled by request_cancel) or
-- by the holder of a claimed job complying with a cancellation request (via
-- cancel_job). Additive only, its own file — see 0016/0017's precedent:
-- Postgres refuses to let a new enum value be *used* in the same transaction
-- that added it, and sqlx runs each migration file in its own transaction.
ALTER TYPE job_status ADD VALUE 'cancelled';
```

`crates/of-core/migrations/0024_job_cancellation_columns.sql`:

```sql
-- Cancellation is a request riding on the job row it targets, not a new
-- table: it has exactly one live instance per job (a second request just
-- re-stamps these same three columns) and needs no history of its own beyond
-- what the audit trail already keeps (job.cancel.requested/job.cancelled).
ALTER TABLE jobs
  ADD COLUMN cancel_requested_at timestamptz,
  ADD COLUMN cancel_requested_by uuid REFERENCES users (id) ON DELETE SET NULL,
  ADD COLUMN cancel_reason       text;
```

No RLS registration is needed: `jobs` is already in `0007_rls.sql`'s `tenant_tables` with
policy `jobs_tenant_isolation`, and that policy is `USING (org_id = current_org())` —
row-scoped, so it already covers these new columns on every existing row. This is a new
column on an existing tenant table, not a new tenant table (Load-Bearing Invariant 1
applies to the latter).

## §3 — `crates/of-core/src/jobs.rs`

```rust
pub enum Status {
    Pending,
    #[sqlx(rename = "in-progress")]
    #[serde(rename = "in-progress")]
    InProgress,
    Active,
    Completed,
    Failed,
    Cancelled,
}
```

`as_str` gains `Status::Cancelled => "cancelled"`. `FromStr` gains a `"cancelled" =>
Ok(Status::Cancelled)` arm, and its error message lists all six values. `is_terminal`
becomes `matches!(self, Status::Completed | Status::Failed | Status::Cancelled)`.

`Job` gains three fields, placed after `claimed_by_label` to read as "who claimed it,
then who asked it to stop":

```rust
pub cancel_requested_at: Option<chrono::DateTime<chrono::Utc>>,
/// Set by `request_cancel`. `None` means nobody has asked this job to stop.
/// If you are this job's holder and this is set, call `cancel_job` once you
/// actually stop — or `fail_job` if you are stopping for your own reasons
/// unrelated to this request.
pub cancel_requested_by: Option<UserId>,
pub cancel_reason: Option<String>,
```

`JOB_COLS` gains `, cancel_requested_at, cancel_requested_by, cancel_reason`.

Two new `Tx` methods, both taking the same row lock pattern as `finalize`:

```rust
/// Ask a job to stop. For a `pending` job — nobody is running it — this
/// finalizes it immediately to `cancelled`, since there is no holder to wait
/// on: "distinguish cancelling pending vs claimed jobs" from the issue means
/// exactly this, that the former needs no agent cooperation. For an
/// `in-progress`/`active` job this only records the request; the caller
/// (`request_cancel`'s tool handler) is responsible for deciding whether to
/// also write the `job.cancel.requested` and, for the immediate-finalize
/// case, `job.cancelled` audit events, and for syncing the outcome to any
/// linked tracker ticket in the pending case.
pub async fn request_cancel(
    &mut self,
    id: &JobId,
    requested_by: UserId,
    reason: Option<&str>,
) -> Result<Job> {
    let org = self.org();
    let current: Option<Status> =
        sqlx::query_scalar("SELECT status FROM jobs WHERE org_id = $1 AND id = $2 FOR UPDATE")
            .bind(org)
            .bind(id)
            .fetch_optional(self.conn())
            .await?;
    let current = current.ok_or_else(|| Error::JobNotFound(id.clone()))?;

    if current.is_terminal() {
        return Err(Error::WrongStatus {
            job: id.clone(),
            actual: current.as_str().to_string(),
            expected: "pending, in-progress, or active".into(),
        });
    }

    let sql = if current == Status::Pending {
        // No holder to wait on: finalize now, same shape as `finalize`'s
        // terminal writes.
        format!(
            "UPDATE jobs SET status = 'cancelled', completed_at = now(), \
                    cancel_requested_at = now(), cancel_requested_by = $3, \
                    cancel_reason = $4 \
             WHERE org_id = $1 AND id = $2 RETURNING {JOB_COLS}"
        )
    } else {
        // Advisory only — status is untouched.
        format!(
            "UPDATE jobs SET cancel_requested_at = now(), cancel_requested_by = $3, \
                    cancel_reason = $4 \
             WHERE org_id = $1 AND id = $2 RETURNING {JOB_COLS}"
        )
    };

    let job = sqlx::query_as(&sql)
        .bind(org)
        .bind(id)
        .bind(requested_by)
        .bind(reason)
        .fetch_one(self.conn())
        .await?;
    Ok(job)
}

/// Finalize a claimed job as cancelled, in response to a prior
/// `request_cancel`. Refuses if no cancellation was ever requested — an
/// agent stopping for its own reasons calls `fail_job` instead, which is
/// what keeps `job.cancelled` in the audit trail meaning "asked to stop, and
/// did" rather than an ordinary failure wearing a different name.
pub async fn cancel_job(&mut self, id: &JobId, note: Option<&str>) -> Result<Job> {
    let org = self.org();
    let current: Option<(Status, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT status, cancel_requested_at FROM jobs \
         WHERE org_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(org)
    .bind(id)
    .fetch_optional(self.conn())
    .await?;
    let (current, requested_at) = current.ok_or_else(|| Error::JobNotFound(id.clone()))?;

    if !matches!(current, Status::InProgress | Status::Active) {
        return Err(Error::WrongStatus {
            job: id.clone(),
            actual: current.as_str().to_string(),
            expected: "in-progress or active".into(),
        });
    }
    if requested_at.is_none() {
        return Err(Error::Invalid(format!(
            "job {id} has no cancellation request on file — call fail_job if you are \
             stopping for your own reasons, not in response to a request_cancel call"
        )));
    }

    let job = sqlx::query_as(&format!(
        "UPDATE jobs SET status = 'cancelled', completed_at = now(), error = $3 \
         WHERE org_id = $1 AND id = $2 RETURNING {JOB_COLS}"
    ))
    .bind(org)
    .bind(id)
    .bind(note)
    .fetch_one(self.conn())
    .await?;
    Ok(job)
}
```

`error` is reused to hold `cancel_job`'s optional `note` — the column already means "why
this job ended the way it did" for `Failed`, and a cancelled job ending with an
explanation is the same kind of fact, not a new one needing its own column.

`repend_job`'s `UPDATE` gains `, cancel_requested_at = NULL, cancel_requested_by = NULL,
cancel_reason = NULL` alongside its existing resets — a repended job is a fresh dispatch,
and leaving a stale cancellation request in place would let its *next* holder call
`cancel_job` without anyone having asked *this* attempt to stop. `repend_job`'s existing
guard (`current == Status::Pending` is the only rejected state) needs no logic change —
`Cancelled` already falls into "anything but pending" — but its `WrongStatus.expected`
string gains `"or cancelled"`, and its tool description (`crates/of-mcp/src/tools/jobs.rs`)
changes from "Return a completed or failed job to pending" to "Return a completed,
failed, or cancelled job to pending."

`Stats` gains `pub cancelled: i64` (placed after `failed`, before `blocked`, matching the
lifecycle-then-blocked ordering the struct already uses), and `stats()`'s query gains
`COUNT(*) FILTER (WHERE status = 'cancelled') AS cancelled`.

`get_live_job_by_ticket_for_repo`'s `status IN ('pending', 'in-progress', 'active')`
filter is untouched — `cancelled` is terminal, so a cancelled job is correctly never the
"live" holder of its `ticket_ref`, exactly like `completed`/`failed` today.

## §4 — `crates/of-mcp/src/tools/jobs.rs`

```rust
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestCancelArgs {
    /// The job to ask to stop.
    pub job: String,
    /// Why, for whoever is holding it and for the audit trail.
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CancelJobArgs {
    /// The job you were working on and are stopping.
    pub job: String,
    /// What you were doing when you stopped, for whoever reads this later.
    #[serde(default)]
    pub note: Option<String>,
}
```

```rust
#[tool(
    name = "request_cancel",
    description = "Ask a job to stop. If nobody has claimed it yet, this cancels it \
                   immediately — there is no agent to wait on. If it is claimed \
                   (in-progress or active), this only sets a flag: the server cannot \
                   stop the agent holding it, any more than it can stop a git push. The \
                   holder sees the request the next time it calls get_job or wakes from \
                   watch, and is expected to call cancel_job once it actually stops (or \
                   fail_job, if it disagrees and finishes anyway). Fails if the job is \
                   already completed, failed, or cancelled."
)]
pub async fn request_cancel(
    &self,
    Extension(parts): Extension<http::request::Parts>,
    Parameters(args): Parameters<RequestCancelArgs>,
) -> Result<Json<out::JobOut>, ErrorData> {
    let caller = self.caller(&parts)?;
    caller.require_scope(scope::JOBS_WRITE).mcp()?;

    let mut tx = self.tx(&caller).await?;
    self.charge(&mut tx, &caller, "request_cancel").await?;
    let id = JobId::from(args.job);
    let job = tx
        .request_cancel(&id, caller.user_id, args.reason.as_deref())
        .await
        .mcp()?;

    tx.audit(
        Entry::new(action::JOB_CANCEL_REQUESTED)
            .actor(caller.user_id)
            .target("job", job.id.to_string())
            .detail(serde_json::json!({ "reason": args.reason })),
    )
    .await
    .mcp()?;

    // A pending job with nobody to wait on is finalized in the same call —
    // record that outcome too, distinctly, so the audit trail always shows
    // both events regardless of which path a job took.
    let finalized_now = job.status == Status::Cancelled;
    if finalized_now {
        tx.audit(
            Entry::new(action::JOB_CANCELLED)
                .actor(caller.user_id)
                .target("job", job.id.to_string()),
        )
        .await
        .mcp()?;
    }
    tx.commit().await.mcp()?;

    let out = Json(out::JobOut { job: job.clone() });
    if finalized_now {
        let detail = args
            .reason
            .clone()
            .unwrap_or_else(|| "Cancelled before being claimed.".to_string());
        self.sync_jobs_after_transition(std::slice::from_ref(&job), JobTransition::Cancelled, Some(&detail))
            .await;
    }
    Ok(out)
}

#[tool(
    name = "cancel_job",
    description = "Finalize a job you were working on as cancelled, because you were \
                   asked to stop via request_cancel and are complying. Fails if this job \
                   never had a cancellation requested — if you are stopping for your own \
                   reasons, call fail_job instead, so the audit trail keeps distinguishing \
                   'asked to stop, and did' from an ordinary failure."
)]
pub async fn cancel_job(
    &self,
    Extension(parts): Extension<http::request::Parts>,
    Parameters(args): Parameters<CancelJobArgs>,
) -> Result<Json<out::JobOut>, ErrorData> {
    let caller = self.caller(&parts)?;
    caller.require_scope(scope::JOBS_WRITE).mcp()?;

    let mut tx = self.tx(&caller).await?;
    self.charge(&mut tx, &caller, "cancel_job").await?;
    let job = tx
        .cancel_job(&JobId::from(args.job), args.note.as_deref())
        .await
        .mcp()?;
    tx.audit(
        Entry::new(action::JOB_CANCELLED)
            .actor(caller.user_id)
            .target("job", job.id.to_string()),
    )
    .await
    .mcp()?;
    tx.commit().await.mcp()?;

    let out = Json(out::JobOut { job: job.clone() });
    let detail = args
        .note
        .clone()
        .or_else(|| job.cancel_reason.clone())
        .unwrap_or_else(|| "Cancelled.".to_string());
    self.sync_jobs_after_transition(std::slice::from_ref(&job), JobTransition::Cancelled, Some(&detail))
        .await;
    Ok(out)
}
```

Both call `self.tx(&caller)` and `Entry`/`action` need importing
(`use of_core::audit::{action, Entry};`) — the first use of the audit trail from
`of-mcp`; every prior call site is in `of-web`'s console routes (see §1's reasoning for
why that asymmetry is fine: audit-worthiness is about who-did-what-to-what, not about
which crate happens to call it first).

`sync_ticket`'s exhaustive match over `job.status` gains:

```rust
Status::Cancelled => (
    JobTransition::Cancelled,
    Some(
        job.error
            .clone()
            .or_else(|| job.cancel_reason.clone())
            .unwrap_or_else(|| "Cancelled.".into()),
    ),
),
```

placed after the `Status::Failed` arm, and the transition it pairs with is
`JobTransition::Cancelled` — **not** `Failed` as originally planned here (see Scope/Out):
reusing `Failed` fires `outbound_decision`'s `(Failed, Jira)` arm, which transitions a
cancelled ticket back to the "new" status category. `Cancelled` gets its own arm instead —
a comment only, no GitHub close, no JIRA transition — added to `of_trackers::sync` for
this reason. The `Some(...)` always carries a non-empty, accurate string, so the tracker
comment never falls back to `outbound_decision`'s literal `"Cancelled."` default when a
better one (the job's `error` or `cancel_reason`) is available.

`ListJobsArgs.status`'s doc comment, `list_jobs`'s description, `stats`'s description, and
`request_cancel`'s own description above already list every valid state including
`cancelled`; audit that "pending", "in-progress", "active", "completed" or "failed" become
"...or cancelled" everywhere they appear as a literal list in this file.

## §5 — `crates/of-core/src/audit.rs`

New section at the end of `pub mod action`:

```rust
// Jobs.
pub const JOB_CANCEL_REQUESTED: &str = "job.cancel.requested";
pub const JOB_CANCELLED: &str = "job.cancelled";
```

## §6 — `crates/of-billing/src/classify.rs`

`request_cancel` and `cancel_job` are added to `BILLABLE`, alongside `claim_jobs`/
`complete_job`/`fail_job` — both change durable state (a flag or a terminal status), on
the same footing as every other job write.

## §7 — `crates/of-web`

- `openapi.rs`'s `Job` schema gains:
  ```rust
  "cancelRequestedAt": { "type": ["string", "null"], "format": "date-time" },
  "cancelRequestedBy": { "type": ["string", "null"], "format": "uuid" },
  "cancelReason": { "type": ["string", "null"] },
  ```
  The `status` enum gains `"cancelled"`. `QueueStats` gains `"cancelled": { "type":
  "integer" }`, added to `required` alongside the others (placed after `"failed"`,
  before `"blocked"`, matching §3's field ordering).
- `routes/jobs.rs`'s `ListJobsQuery.status` doc comment: `` `pending` | `in-progress` |
  `active` | `completed` | `failed` | `cancelled` ``.

No route changes — the console stays read-only over the queue (§1); `cancelled` and the
three new fields are just more data the existing `GET` endpoints pass through, since
`routes/jobs.rs` returns `of_core::jobs::Job` directly rather than a mirrored DTO.

## §8 — `web/`

- `src/lib/types.ts`: `JobStatus` gains `'cancelled'`; `QueueStats` gains `cancelled:
  number`. If any object literal typed as `QueueStats` in the codebase (a mock, a test
  fixture) does not yet set `cancelled`, `npm run check` will name it — fix at the same
  time.
- `src/lib/labels.ts`: `statusLabel`'s `switch` gains `case 'cancelled': return
  m.status_cancelled();`.
- `web/messages/{en,es,de,fr,it,hi}.json`: a new `status_cancelled` key in each, following
  the existing five `status_*` keys' placement and translation register. `npm run check`
  runs `scripts/check-messages.mjs` first, which fails on a key missing from any locale —
  this is not optional in any one file.
- `src/lib/components/StatusPill.svelte`: `tones` (a `Record<JobStatus, string>`, so
  exhaustive by the type system) gains a `cancelled` entry. Use `--color-faint` (the same
  family `pending` already uses: `border-faint/40 bg-faint/10 text-muted`) rather than a
  new tone — a cancellation is a deliberate, calm outcome, not one of the two colors
  reserved for "needs attention" (`busy`/`bad`) or the one reserved for "worked as
  intended" (`ok`).
- `src/routes/o/[org]/queue/+page.svelte`: `STATUSES` gains `'cancelled'` at the end,
  after `'failed'`.

Deliberately **not** touched: the org overview page's stat tiles (see Scope/Out).

## §9 — Testing

- `crates/of-core/tests/queue.rs`:
  - `request_cancel` on a `pending` job finalizes it to `cancelled` immediately, with
    `cancel_requested_at`/`cancel_requested_by`/`cancel_reason` all set and
    `completed_at` set; `is_terminal()` is true.
  - `request_cancel` on an `in-progress`/`active` job leaves status unchanged but sets
    the three request fields; a second `request_cancel` call re-stamps them.
  - `request_cancel` on an already-terminal job (`completed`/`failed`/`cancelled`)
    returns `WrongStatus` naming `"pending, in-progress, or active"` as expected.
  - `cancel_job` after a `request_cancel` on a claimed job succeeds, sets `status =
    cancelled`, `completed_at`, and `error` from `note`.
  - `cancel_job` on a claimed job that was never asked to stop returns `Error::Invalid`.
  - `cancel_job` on a `pending` job (never claimed) returns `WrongStatus` naming
    `"in-progress or active"`.
  - `repend_job` on a `cancelled` job succeeds and clears all three cancellation fields;
    a subsequent `cancel_job` on the repended-then-reclaimed job fails with
    `Error::Invalid` (no cancellation is on file for the new attempt).
  - `stats()` reports the new `cancelled` counter, and `pending + in_progress + active +
    completed + failed + cancelled == total`.
- `crates/of-core/tests/isolation.rs`: extend `cross_org_mutation_is_refused` with
  `request_cancel` and `cancel_job` called from the wrong org, both asserted to fail —
  guard 1's proof for the two new statements.
- `crates/of-mcp/tests/tools.rs`:
  - End-to-end: `add_job` → `request_cancel` (still pending) → job is `cancelled`, both
    audit events present in order.
  - End-to-end: `add_job` → `claim_jobs` → `request_cancel` → job still `in-progress`
    with the request fields set → `cancel_job` → job `cancelled`, both audit events
    present with the correct, possibly-different actors.
  - `cancel_job` without a prior request returns an error.
  - Both tools appear in the tool list with non-empty descriptions
    (`every_tool_documents_itself`'s existing assertion pattern).
  - `every_tool_has_a_price`/`exhaustive_over`: both tools present and classified
    billable.
- `crates/of-web/tests/console.rs` (or wherever `openapi.rs`'s schema tests live):
  extend the existing `status_enum.iter().any(...)` / `QueueStats` property assertions
  with `"cancelled"`, following the `"active"` precedent exactly.
- `web/`: `npm run check` (message-catalog completeness plus the `Record<JobStatus,
  string>`/`switch` exhaustiveness), `npm run lint`, `npm test` (no Worker routing
  change expected, so this is a regression gate).

## Assumptions

- The read-only-console tension is resolved by adding no console write at all, rather
  than by amending `the_queue_is_read_only_over_the_console`'s reasoning. *Rationale:
  see §1 — the human-facing half of the problem is already solvable via a personal
  access token, matching how every other by-hand job write works, and it avoids
  asserting a new, first-of-its-kind distinction between "intent" and "outcome" writes
  that the existing test draws no line for today.*
- `Cancelled` is a genuinely new terminal status, not a repurposing of `Failed` with a
  flag. *Rationale: the issue explicitly proposes this ("a new terminal `cancelled`"),
  and it gives `stats()`/the console an honest count of "asked to stop" distinct from
  "attempted and did not succeed."*
- `cancel_job` refuses when no cancellation was ever requested, rather than being a
  general-purpose "I quit, mark me cancelled" tool. *Rationale: keeps `job.cancelled` in
  the audit trail meaning what its name says; an agent stopping unprompted already has
  `fail_job`.*
- A `JobTransition::Cancelled` variant *is* added to `of_trackers::sync` — this reverses
  the original plan to reuse `JobTransition::Failed`'s outbound plumbing, corrected during
  PR review once reuse was found to fire `outbound_decision`'s `(Failed, Jira)` arm and
  transition a cancelled ticket back to "new". *Rationale: the issue's AC still does not
  ask for a distinct external signal, so `Cancelled` gets the same comment-only, no-close,
  no-transition shape `Failed` has — the variant exists only to keep the wrong JIRA signal
  from firing, not to add a new outbound capability.*
- No ownership/claimant check on either new tool, matching `complete_job`/`fail_job`'s
  existing lack of one. *Rationale: consistency over incidental hardening for two tools
  only; a real ownership model is a larger, separate change.*
- Repeated `request_cancel` calls are idempotent-ish (they re-stamp, not error), and
  each still writes an audit event. *Rationale: simpler than tracking "is this the same
  request", and a second nudge to stop is meaningful information, not noise.*
- `error` (not a new column) holds `cancel_job`'s optional `note`. *Rationale: the column
  already means "why this job ended this way" for `Failed`; a fourth string column
  meaning almost the same thing would be redundant.*

## Error Handling & Edge Cases

- Cancelling a job that is already `completed`/`failed`/`cancelled`: `request_cancel`
  returns `WrongStatus` naming the valid starting states, matching every other
  transition tool's error shape.
- Racing a `complete_job`/`fail_job` against a `request_cancel`: both `request_cancel`
  and `finalize` (backing `complete_job`/`fail_job`) take `SELECT ... FOR UPDATE` on the
  same row, so the two serialize — whichever commits first wins, and the loser sees the
  now-terminal status and returns `WrongStatus`, never a torn write.
- `cancel_job` racing a second `cancel_job` (or a `complete_job`/`fail_job`) on the same
  claimed job: same row lock, same serialization; the second caller sees `WrongStatus`
  once the first has committed.
- A `pending` job with a linked tracker ticket cancelled via `request_cancel`: the
  outbound sync fires exactly as it would for `fail_job` on the same job, posting a
  "Cancelled" comment — verified by §9's tool-level test asserting `sync_jobs_after_transition`
  is invoked (or, more simply, by an integration-style test with a tracker connection
  configured, if one already exists for `fail_job`; otherwise this is covered at the
  unit level via `outbound_decision`'s existing `Failed` coverage plus the new detail-string
  logic being plain Rust with no I/O).

## Risks & Open Questions

- `ALTER TYPE ... ADD VALUE` inside a migration transaction already works in this
  codebase (0016 proved it against the same Postgres 16 the test suite runs against), so
  this is not a fresh risk — flagged only because 0023 repeats that pattern and must stay
  its own migration file, never combined with 0024.
- The choice to store `cancel_job`'s `note` in the existing `error` column (rather than a
  fourth `cancel_note` column) trades a small semantic overload for one fewer column;
  flagged for the spec critique to weigh in on directly, since it is the one place this
  design reuses an existing field for a new-ish meaning rather than adding a dedicated
  one.
