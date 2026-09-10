# Idempotency key design

> **Status:** DRAFT — adds an optional caller-supplied idempotency key to `add_job` and
> `send_message`, closing savvagent/otto-factory#68.

## Goal & Success Criteria

An agent calling otto-factory over a connection that can drop between commit and response
has no way to make a retry safe: `add_job` over a dropped MCP connection commits, the
response is lost, the agent retries because retrying is what agents do, and the org ends
up with two `job-N` rows for one piece of work. `0015_jobs_ticket_ref_uniqueness.sql`
already solved exactly this for tracker-created jobs by making `create_from_ticket`
naturally idempotent on `ticket_ref`; plain `add_job` — the common path, and the one an
agent calls without a tracker — has no equivalent. `send_message` has the same exposure,
lower stakes (a duplicate note is noise, not a duplicate unit of work), but the issue asks
for the same treatment or an explicit reason not to.

Success:

- `add_job` accepts an optional `idempotencyKey` string.
- Replaying a call with the same key, same org, and the same payload returns the original
  job unchanged — no second row, no error.
- Replaying a key with a *different* payload is a loud, named error (`idempotency_key_conflict`)
  rather than a silent wrong answer.
- Keys are org-scoped (enforced by a partial unique index on `(org_id, idempotency_key)`,
  never a read-then-write) — see §1 for why the up-front lookup this design also does is
  an optimization, not the correctness mechanism.
- Omitting the key reproduces today's behavior exactly, including its cost: the no-key path
  takes zero extra queries and charges in the same place it does today.
- Keys do not create an unboundedly growing table — see §1 for why storing the key on the
  row it produced closes this without a TTL/reaper.
- `send_message` gets the identical treatment (§4) — the spec's answer to "or the spec says
  why not" is "no reason not to, it costs almost nothing to add once `add_job` has the
  pattern."
- A replayed call is metered once, not twice (§3, §5).

## Public interface note

Per Non-Negotiable Rule 6, this is **additive, not breaking**: `add_job` and `send_message`
each gain one new optional argument; `jobs` and `messages` each gain two new nullable
columns; no existing tool, field, route, or result envelope's shape changes.
`out::JobOut`/`out::MessageOut` are untouched — a replay returns the identical one-field
envelope a fresh call would, per the MCP surface convention that every result is a
one-field object (`tools::out`). No version bump is required.

## Scope

**In:**

- New migration `crates/of-core/migrations/0025_idempotency_keys.sql`: two nullable
  columns each on `jobs` and `messages`, plus a partial unique index per table.
- `crates/of-core/src/idempotency.rs` (new, small): key validation and the payload
  fingerprint shared by both tools.
- `crates/of-core/src/jobs.rs`: `NewJob.idempotency_key`, `Tx::find_replayed_job`,
  `Tx::add_job`'s insert gaining the idempotency columns and a savepoint-guarded
  unique-violation fallback for the concurrent-race case.
- `crates/of-core/src/messages.rs`: the identical shape — `NewMessage.idempotency_key`,
  `Tx::find_replayed_message`, `Tx::send_message`'s insert gaining the same treatment.
- `crates/of-core/src/error.rs`: `Error::IdempotencyKeyConflict { key, tool }`.
- `crates/of-mcp/src/tools/jobs.rs`: `AddJobArgs.idempotency_key`, `add_job`'s handler
  restructured to skip `charge` on a replay (§3).
- `crates/of-mcp/src/tools/coord.rs`: `SendMessageArgs.idempotency_key`, `send_message`'s
  handler gets the same restructuring.
- Tests: `crates/of-core/tests/queue.rs`, a new test module or extension in
  `crates/of-core/tests/isolation.rs` for the cross-org negative case, and
  `crates/of-mcp/tests/tools.rs` for the end-to-end/metering behavior.

**Out:**

- A generic idempotency-key facility applied to every mutating tool. The issue names
  exactly `add_job` and `send_message`; `claim_jobs`/`complete_job`/`fail_job`/leases carry
  their own natural idempotency already (claiming is already all-or-nothing, completing an
  already-completed job is already a `WrongStatus` the caller can reasonably interpret) or
  are out of scope for this issue. A capability a customer's own retry wrapper can already
  get by checking `get_job`/`list_jobs` after a dropped call belongs in the customer's
  skill, not as a blanket server feature (constraint 2) — this issue is scoped to the two
  tools that genuinely cannot be made safe that way, because there is no natural
  correlating key to look the result up by afterwards.
- A separate `idempotency_keys` table. §1 explains why the key rides on the row it
  produced instead.
- A configurable TTL / expiry job. §1's resolution of the growth concern makes one
  unnecessary; revisit only if a customer's usage pattern proves otherwise.
- Exposing a `replayed: true/false` flag on the result. The MCP result-envelope convention
  is one field (`tools::out`); the AC only asks that a replay "return the original job,
  unchanged," which the existing envelope already does verbatim.
- Perfect metering under concurrent duplicate-key races. §3 names the accepted, narrow
  edge case this design does not close and why closing it would cost the common,
  sequential-retry path (the one the issue is actually about) its current zero-extra-query
  behavior.

## §1 — Where the key lives, and why that answers two AC bullets at once

The key and its payload fingerprint are stored as two new nullable columns directly on the
row they produce — `jobs.idempotency_key`/`jobs.idempotency_payload_hash` for `add_job`,
the same pair on `messages` for `send_message` — guarded by a partial unique index scoped
to `(org_id, idempotency_key) WHERE idempotency_key IS NOT NULL`. This is the same shape
`0015_jobs_ticket_ref_uniqueness.sql` already uses for `ticket_ref`, chosen over a
dedicated `idempotency_keys` table for two reasons that also resolve two of the issue's AC
bullets directly:

**"Keys are org-scoped... enforced by a unique index, not a read-then-write that races."**
The index is the enforcement. `Tx::add_job` (and `Tx::send_message`) still perform an
up-front `SELECT` by `(org_id, idempotency_key)` before attempting an insert — but that
read is a *fast-path optimization* to avoid burning a job-id sequence number on every
retry, not the correctness mechanism. Correctness comes from the insert itself: it always
carries the key and its hash, and on a unique-violation (the constraint name distinguishes
this from `0015`'s own partial index, which can also fire on the same table) the code rolls
back to a `SAVEPOINT`, re-reads the row the concurrent winner just created, and either
converges on it (hashes match) or returns `IdempotencyKeyConflict` (they don't) — exactly
`create_from_ticket`'s existing pattern for the identical race shape. Two concurrent calls
racing on a brand-new key can both pass the up-front `SELECT` seeing nothing; only the
index stops them from both being inserted.

**"Keys expire or are bounded, so the table does not grow without limit."** A dedicated
idempotency table needs a TTL and a reaper because its rows have no natural lifetime of
their own — they exist purely to remember "this key was used," and would accumulate
forever if nothing swept them. Putting the key on the job/message row sidesteps the
question rather than answering it with a timer: every idempotency-key row *is* a job or a
message row, which exists regardless of whether a key was supplied, for as long as the
product already keeps jobs and messages. There is no separate structure growing
independently of ordinary usage — the growth bound is exactly the bound already accepted
for `jobs`/`messages` themselves. The tradeoff this buys: a key is scoped to the lifetime
of the row it created and is *never* freed for reuse, even long after the job is
`completed`. That is the safer default for an idempotency key (a key's contract is "this
exact call, at most once," not "this exact call, at most once per some window") and it is
what "bounded" means here — bounded by, and coupled to, the entity it names, not
unbounded.

Cost of this choice: `Job`/`Message` do not need new caller-visible fields for this (the
two columns are written and read by `of-core` only, never included in `JOB_COLS`/`MSG_COLS`
or returned to a caller) — an idempotency key is bookkeeping for the write path, not
information a reader of the row needs back.

## §2 — Migration

`crates/of-core/migrations/0025_idempotency_keys.sql`:

```sql
-- An idempotency key rides on the row it produces rather than living in its
-- own table, exactly like 0015's ticket_ref: the row already has the
-- lifetime and the org scoping an idempotency record needs, so it does not
-- need a TTL or a reaper of its own — see the idempotency-key design spec
-- §1. NULL (the default, and the only value for every call that omits a
-- key) never participates in the unique index below, so this is invisible
-- to every existing caller.
ALTER TABLE jobs
  ADD COLUMN idempotency_key text,
  ADD COLUMN idempotency_payload_hash bytea;

CREATE UNIQUE INDEX jobs_org_idempotency_key_idx
    ON jobs (org_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

ALTER TABLE messages
  ADD COLUMN idempotency_key text,
  ADD COLUMN idempotency_payload_hash bytea;

CREATE UNIQUE INDEX messages_org_idempotency_key_idx
    ON messages (org_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
```

No RLS registration is needed: `jobs` and `messages` are both already tenant tables in
`0007_rls.sql`'s `tenant_tables` array with row-scoped policies — new nullable columns on
an existing tenant table need no new policy (Load-Bearing Invariant 1 is about new
*tables*, not new columns on one already covered).

## §3 — `crates/of-core/src/idempotency.rs` (new)

Shared validation and fingerprinting, used identically by `jobs.rs` and `messages.rs`:

```rust
//! Shared plumbing for the optional caller-supplied idempotency key on
//! `add_job` and `send_message`.
//!
//! The key never proves uniqueness by itself — a partial unique index on
//! `(org_id, idempotency_key)` does that, checked with a SAVEPOINT and a
//! retry on conflict exactly like 0015's ticket_ref index. This module only
//! owns validation and the payload fingerprint that tells "the same call,
//! replayed" apart from "a different call that happens to reuse a key" —
//! the latter must error loudly, never silently return the wrong row.

use crate::error::{Error, Result};
use sha2::{Digest, Sha256};

/// Long enough for a UUID or a short caller-chosen string; short enough that
/// the column cannot be used as unbounded free storage.
pub const MAX_KEY_LEN: usize = 200;

pub fn validate(key: &str) -> Result<()> {
    if key.trim().is_empty() {
        return Err(Error::Invalid("idempotency_key must not be empty".into()));
    }
    if key.len() > MAX_KEY_LEN {
        return Err(Error::Invalid(format!(
            "idempotency_key is {} bytes; the limit is {MAX_KEY_LEN}",
            key.len()
        )));
    }
    Ok(())
}

/// Fingerprint the fields that define "the same call". Not a secret —
/// SHA-256 here is for a stable, fixed-size comparison key, not
/// confidentiality. `serde_json::Value::Object` in this workspace serializes
/// with sorted keys (the `preserve_order` feature is not enabled), so this
/// is deterministic regardless of caller-supplied field order.
pub fn fingerprint(payload: &serde_json::Value) -> Vec<u8> {
    Sha256::digest(payload.to_string().as_bytes()).to_vec()
}
```

`of-core/Cargo.toml` gains `sha2.workspace = true` (already a workspace dependency, used
today only by `of-auth`).

## §4 — `crates/of-core/src/jobs.rs`

`NewJob` gains one field:

```rust
/// Caller-supplied replay key. `None` (the default) reproduces today's
/// behavior exactly — no lookup, no extra column write beyond NULL. See
/// `idempotency` module doc and the design spec's §1 for the correctness
/// argument.
pub idempotency_key: Option<String>,
```

A new `Tx` method, used by the MCP layer to decide *before* charging whether this call is
a replay:

```rust
/// Resolve `new.idempotency_key` against an already-completed `add_job`
/// call, doing no writes and touching no meter. `Ok(None)` covers both "no
/// key was supplied" (the overwhelmingly common case, at zero extra query
/// cost) and "the key has not been used before" — either way the caller
/// should proceed to `add_job`. `add_job` repeats this check race-safely
/// via the unique index + SAVEPOINT retry for the rare case of two
/// concurrent callers racing a brand-new key; this method exists
/// separately so the MCP layer can skip metering a replay *before* calling
/// `add_job`, which has no way to un-charge after the fact.
pub async fn find_replayed_job(&mut self, new: &NewJob) -> Result<Option<Job>> {
    let Some(key) = new.idempotency_key.as_deref() else {
        return Ok(None);
    };
    crate::idempotency::validate(key)?;

    let org = self.org();
    let existing: Option<(JobId, Vec<u8>)> = sqlx::query_as(
        "SELECT id, idempotency_payload_hash FROM jobs \
         WHERE org_id = $1 AND idempotency_key = $2",
    )
    .bind(org)
    .bind(key)
    .fetch_optional(self.conn())
    .await?;
    let Some((id, stored_hash)) = existing else {
        return Ok(None);
    };

    if stored_hash != job_idempotency_fingerprint(new) {
        return Err(Error::IdempotencyKeyConflict {
            key: key.to_string(),
            tool: "add_job",
        });
    }
    Ok(Some(self.get_job(&id).await?))
}
```

`job_idempotency_fingerprint` (private, module-level helper):

```rust
fn job_idempotency_fingerprint(new: &NewJob) -> Vec<u8> {
    let mut depends_on: Vec<&str> = new.depends_on.iter().map(|j| j.0.as_str()).collect();
    depends_on.sort_unstable();
    crate::idempotency::fingerprint(&serde_json::json!({
        "repoId": new.repo_id,
        "teamId": new.team_id,
        "title": new.title.trim(),
        "description": new.description,
        "ticketRef": new.ticket_ref,
        "tracker": new.tracker,
        "agentType": new.agent_type,
        "metadata": new.metadata,
        "dependsOn": depends_on,
        "createdBy": new.created_by,
    }))
}
```

`dependsOn` is sorted before hashing so dependency-list order (which carries no meaning —
`set_dependencies` takes a set, not a sequence) cannot turn a genuine replay into a false
conflict. `createdBy` is included: two different callers supplying the same key is a
different call even if every other field matches, not a replay of each other's request.

`Tx::add_job`'s insert gains the idempotency columns, following `create_from_ticket`'s own
SAVEPOINT pattern for the race case, and gains a `created` flag so dependency-setting is
only attempted for a row this call actually inserted:

```rust
pub async fn add_job(&mut self, new: NewJob) -> Result<Job> {
    if new.title.trim().is_empty() {
        return Err(Error::Invalid("job title must not be empty".into()));
    }
    if let Some(key) = new.idempotency_key.as_deref() {
        crate::idempotency::validate(key)?;
    }

    let org = self.org();
    let repo = self
        .get_repo(new.repo_id)
        .await?
        .ok_or_else(|| Error::RepoNotFound(new.repo_id.to_string()))?;

    let seq: i64 = sqlx::query_scalar(
        "UPDATE orgs SET next_job_seq = next_job_seq + 1 WHERE id = $1 \
         RETURNING next_job_seq - 1",
    )
    .bind(org)
    .fetch_optional(self.conn())
    .await?
    .ok_or(Error::OrgNotFound(org))?;

    let id = JobId::from_seq(seq);
    let team_id = new.team_id.or(repo.team_id);

    let (job, created) = if let Some(key) = new.idempotency_key.as_deref() {
        let hash = job_idempotency_fingerprint(&new);
        sqlx::query("SAVEPOINT add_job_idempotency")
            .execute(self.conn())
            .await?;
        let inserted = sqlx::query_as(&format!(
            "INSERT INTO jobs (id, org_id, repo_id, team_id, title, description, ticket_ref, \
                               tracker, remote_revision, agent_type, metadata, created_by, \
                               idempotency_key, idempotency_payload_hash) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) RETURNING {JOB_COLS}"
        ))
        .bind(&id)
        .bind(org)
        .bind(new.repo_id)
        .bind(team_id)
        .bind(new.title.trim())
        .bind(new.description.as_deref())
        .bind(new.ticket_ref.as_deref())
        .bind(new.tracker)
        .bind(Option::<&str>::None)
        .bind(new.agent_type.as_deref())
        .bind(new.metadata.clone().unwrap_or_else(|| serde_json::json!({})))
        .bind(new.created_by)
        .bind(key)
        .bind(&hash)
        .fetch_one(self.conn())
        .await;

        match inserted {
            Ok(job) => {
                sqlx::query("RELEASE SAVEPOINT add_job_idempotency")
                    .execute(self.conn())
                    .await?;
                (job, true)
            }
            // A concurrent caller won the race on this brand-new key between
            // find_replayed_job's read and this insert. Converge on its row
            // exactly like create_from_ticket converges on ticket_ref's
            // winner, rather than failing a request that, semantically,
            // already succeeded — the seq number burned above is simply
            // unused, same as that precedent.
            Err(sqlx::Error::Database(db))
                if db.is_unique_violation()
                    && db.constraint() == Some("jobs_org_idempotency_key_idx") =>
            {
                sqlx::query("ROLLBACK TO SAVEPOINT add_job_idempotency")
                    .execute(self.conn())
                    .await?;
                sqlx::query("RELEASE SAVEPOINT add_job_idempotency")
                    .execute(self.conn())
                    .await?;
                let winner: (JobId, Vec<u8>) = sqlx::query_as(
                    "SELECT id, idempotency_payload_hash FROM jobs \
                     WHERE org_id = $1 AND idempotency_key = $2",
                )
                .bind(org)
                .bind(key)
                .fetch_optional(self.conn())
                .await?
                .ok_or_else(|| {
                    Error::Invalid(format!(
                        "add_job lost a unique-violation race for idempotency key {key:?} \
                         but no concurrently-created job was found"
                    ))
                })?;
                if winner.1 != hash {
                    return Err(Error::IdempotencyKeyConflict {
                        key: key.to_string(),
                        tool: "add_job",
                    });
                }
                (self.get_job(&winner.0).await?, false)
            }
            Err(error) => return Err(Error::Db(error)),
        }
    } else {
        // Unchanged from today: no key, no idempotency columns, no savepoint.
        let job = sqlx::query_as(&format!(
            "INSERT INTO jobs (id, org_id, repo_id, team_id, title, description, ticket_ref, \
                               tracker, remote_revision, agent_type, metadata, created_by) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) RETURNING {JOB_COLS}"
        ))
        .bind(&id)
        .bind(org)
        .bind(new.repo_id)
        .bind(team_id)
        .bind(new.title.trim())
        .bind(new.description.as_deref())
        .bind(new.ticket_ref.as_deref())
        .bind(new.tracker)
        .bind(Option::<&str>::None)
        .bind(new.agent_type.as_deref())
        .bind(new.metadata.unwrap_or_else(|| serde_json::json!({})))
        .bind(new.created_by)
        .fetch_one(self.conn())
        .await?;
        (job, true)
    };

    if created && !new.depends_on.is_empty() {
        self.set_dependencies(&job.id, &new.depends_on, &[]).await?;
    }

    Ok(job)
}
```

The `created` flag matters beyond the obvious: if the race-fallback arm ran
`set_dependencies(&id, ...)` using the locally burned `id` (not `job.id`, the winner's real
id), it would attempt to set dependencies on a job that does not exist. Skipping it on the
non-`created` arm is correct, not just cheaper — the winner's own `add_job` call already
set its dependencies from the identical payload (the hash comparison includes `dependsOn`,
so a mismatch there is already caught as a conflict before this point is reached).

## §5 — `crates/of-mcp/src/tools/jobs.rs`

`AddJobArgs` gains:

```rust
/// A caller-chosen key. Replaying add_job with the same key and the same
/// arguments returns the original job unchanged instead of creating a
/// second one — call this every time if your connection to the server can
/// drop between the call committing and its response arriving, which is
/// the situation a retry cannot otherwise tell apart from "never happened".
/// Reusing a key with different arguments is an error. Omit it and every
/// call creates a new job, as today.
#[serde(default)]
pub idempotency_key: Option<String>,
```

`add_job`'s handler is restructured so metering only happens for a call that actually does
something — see `Factory::charge`'s doc comment in `crates/of-mcp/src/server.rs`, which
this adds as a third named exception alongside `watch` and `sync_ticket`:

```rust
pub async fn add_job(
    &self,
    Extension(parts): Extension<http::request::Parts>,
    Parameters(args): Parameters<AddJobArgs>,
) -> Result<Json<out::JobOut>, ErrorData> {
    let caller = self.caller(&parts)?;
    caller.require_scope(scope::JOBS_WRITE).mcp()?;

    let mut tx = self.tx(&caller).await?;
    let repo = repo_of(&mut tx, args.repo, args.remote).await?;
    let new_job = NewJob {
        repo_id: repo.id,
        title: args.title,
        description: args.description,
        ticket_ref: args.ticket_ref,
        agent_type: args.agent_type,
        metadata: args.metadata,
        depends_on: ids(args.depends_on),
        created_by: Some(caller.user_id),
        idempotency_key: args.idempotency_key,
        ..Default::default()
    };

    // A replay must not be billed (see Factory::charge's doc comment) — so
    // resolve replay-vs-new before calling charge, not after. This is the
    // one thing that has to happen before charge in this handler; the
    // no-key path returns None immediately and pays no extra query for it.
    if let Some(existing) = tx.find_replayed_job(&new_job).await.mcp()? {
        tx.commit().await.mcp()?;
        return Ok(Json(out::JobOut { job: existing }));
    }

    self.charge(&mut tx, &caller, "add_job").await?;
    let job = tx.add_job(new_job).await.mcp()?;
    tx.commit().await.mcp()?;

    Ok(Json(out::JobOut { job }))
}
```

The tool description gains one clause: `"...Returns the created job, including its id. \
Pass idempotencyKey if your connection can drop before you see the response, so a retry \
returns the original job instead of creating a duplicate."`

## §6 — `crates/of-core/src/messages.rs` and `crates/of-mcp/src/tools/coord.rs`

The identical shape, applied to `send_message`/`NewMessage`/`SendMessageArgs`:

- `NewMessage.idempotency_key: Option<String>`, same doc comment shape as `NewJob`'s.
- `Tx::find_replayed_message(&mut self, sender: UserId, new: &NewMessage) -> Result<Option<Message>>`,
  mirroring `find_replayed_job` exactly (`sender` is a parameter of `send_message`, not a
  `NewMessage` field, so it is threaded through separately and included in the fingerprint
  payload).
- `message_idempotency_fingerprint` fingerprints `{sender, body: new.body.trim(),
  recipientUserId, teamId, kind, senderKind, senderLabel, repoId, jobId, inReplyTo}` —
  `senderKind`/`senderLabel` included because they are caller-supplied and change what the
  message actually says on screen even for byte-identical `body`.
- `Tx::send_message`'s insert gains the same idempotency columns +
  SAVEPOINT/unique-violation-fallback shape, matched against constraint name
  `messages_org_idempotency_key_idx`. `send_message` has no dependency step, so there is no
  `created`-flag subtlety to carry — the `created` bool is still threaded through only so a
  future change adding one does not silently reintroduce it.
- `SendMessageArgs.idempotency_key: Option<String>`, `#[serde(default)]`, doc comment
  identical in spirit to `AddJobArgs`'s.
- `send_message`'s handler gets the identical restructuring: resolve `find_replayed_message`
  before `self.charge(...)`, return early on a hit, otherwise charge-then-insert as today.

## §7 — `crates/of-core/src/error.rs`

```rust
#[error(
    "idempotency_key {key:?} was already used for a different {tool} call in this \
     organization. Use a new key for a different request, or omit idempotency_key to \
     always create a new one."
)]
IdempotencyKeyConflict { key: String, tool: &'static str },
```

`code()` gains `Error::IdempotencyKeyConflict { .. } => "idempotency_key_conflict"`.
`retriable()` does **not** include it — retrying the identical call with the identical key
will fail identically every time; the caller must change the key or the payload, which is
what "loudly" in the AC means (`DependencyCycle` is the existing precedent for a
non-retriable, caller-must-rethink error).

## §8 — Metering: what this design closes and what it accepts

**Closed.** The sequential retry the issue describes — a call commits, the response is
lost, the same agent retries with the same key — never charges twice: `find_replayed_job`/
`find_replayed_message` runs before `charge` and returns early on a hit, so the second call
never reaches `charge` at all.

**Accepted, and named rather than silently left.** Two callers racing the exact same
brand-new key concurrently (not a sequential retry — an actual simultaneous double-send)
can both pass the up-front `find_replayed_*` check seeing nothing, both proceed to
`charge`, and only then have `add_job`/`send_message` discover via the unique-violation
fallback that one of them lost the race and should return the winner's row. The loser is
billed once for a call that, semantically, created nothing. Closing this fully would mean
moving `charge` to run *after* the insert for every call, including the common no-key path,
which would cost every `add_job`/`send_message` call — not just idempotent ones — the
"refuse at hard-stop before doing any work" property `Factory::charge`'s doc comment
describes as the reason charge runs first. This design keeps that property intact for the
overwhelmingly common cases (no key; key present but no concurrent racer) and accepts a
narrow, documented imperfection in a case that requires genuine millisecond-scale
concurrency on an *identical, never-before-seen* key to trigger — which is a different,
much rarer event than the sequential retry the issue is written to fix.

## §9 — Testing

- `crates/of-core/tests/queue.rs`:
  - `add_job` with an idempotency key, replayed with the same title/description/etc.,
    returns a `Job` with the same `id` and no second row exists (`list_jobs` count check).
  - Replaying the same key with a different `title` returns `Error::IdempotencyKeyConflict`.
  - Two different `depends_on` orderings of the same set, same key, are treated as the same
    payload (no conflict).
  - Omitting the key twice with identical arguments creates two distinct jobs (today's
    behavior, unchanged).
  - `idempotency_key` longer than `idempotency::MAX_KEY_LEN`, or empty/whitespace-only,
    returns `Error::Invalid`.
  - The identical four cases for `send_message`/`Tx::find_replayed_message`.
- `crates/of-core/tests/isolation.rs`: extend the cross-org coverage — the same
  `idempotency_key` string used by org A and org B for `add_job` (and `send_message`)
  produces two independent rows, one per org, neither visible to the other; the partial
  unique index's `(org_id, ...)` scoping is what this proves, following
  `cross_org_mutation_is_refused`'s pattern of running the same call from both tenants.
- `crates/of-mcp/tests/tools.rs`:
  - End-to-end: `add_job` with a key, called twice, returns the same job id both times, and
    `usage`'s billable-call count increases by exactly one (metering proof for §8's closed
    case).
  - End-to-end: `add_job` with a key, called with a different `title` the second time,
    returns an error whose `code` is `idempotency_key_conflict`.
  - The same two cases for `send_message`.
  - `add_job`/`send_message` continue to appear in `every_tool_documents_itself` and
    `every_tool_has_a_price` unchanged (no new tool was added, so no new classify entry is
    needed — `exhaustive_over` should be unaffected, asserted as a regression guard).

## Assumptions

- Only `add_job` and `send_message` get this treatment. *Rationale: these are the two
  tools the issue names, and they are the two tools with genuinely no other way for a
  caller to discover after the fact "did my last call actually go through" — everything
  else in the queue already has a natural, cheap idempotency path the caller already has
  (check `get_job`, or accept that re-claiming/re-completing an already-terminal job is
  self-evidently a no-op/error).*
- The key lives on the row it creates, not in a dedicated table. *Rationale: see §1 — it
  answers both the uniqueness-enforcement and the unbounded-growth AC bullets without new
  machinery, at the cost of never freeing a key for reuse, which is the safer default for
  what an idempotency key is for.*
- The payload fingerprint includes the resolved `repo_id` (not the caller's raw `repo`/
  `remote` string) and, for jobs, `created_by`; for messages, the sender and both
  `sender_kind`/`sender_label`. *Rationale: two different textual ways of naming the same
  repo should still match; a different caller or a different displayed identity reusing the
  same key is a materially different request, not a replay of someone else's.*
- A narrow metering imperfection under concurrent (not sequential) duplicate-key races is
  accepted rather than closed. *Rationale: §8 — closing it fully costs the common, no-key
  path the "refuse before doing work" property it has today; the issue's own motivating
  scenario is a sequential retry, which this design closes completely.*
- No `replayed` flag on the output envelope. *Rationale: the one-field-object convention
  (`tools::out`) and the AC's literal wording ("returns the original job, unchanged") don't
  ask for one; a caller that cares can already tell by checking whether it already had that
  job id.*

## Error Handling & Edge Cases

- Idempotency key supplied but empty/whitespace-only, or over `MAX_KEY_LEN`:
  `Error::Invalid`, matching the existing `ticket_ref`/message-body validation style —
  checked once, in `find_replayed_job`/`find_replayed_message` and again in `add_job`/
  `send_message`'s own insert path (the two entry points share `idempotency::validate`, so
  this is one implementation, not two to keep in sync).
- A key reused across `add_job` and `send_message` (same string, different tool): no
  conflict — the two tools' unique indexes are on different tables, so this is not
  guarded against and does not need to be; `Error::IdempotencyKeyConflict.tool` still names
  which tool's call is in conflict when it does fire, so an agent reusing a key sees which
  half of a mixed workflow collided.
- Concurrent identical-key race on `add_job`/`send_message`: closed for correctness (never
  two rows) via the unique index + SAVEPOINT fallback; not closed for metering (§8,
  accepted and documented).
- A replay whose payload matches except for `metadata`'s key order: not a conflict —
  `serde_json::Value::Object` serializes with sorted keys in this workspace (no
  `preserve_order` feature), so `fingerprint` is order-insensitive by construction; no
  special-casing needed.

## Risks & Open Questions

- The payload fingerprint is deliberately narrow (the fields that define "what this call
  does"), not a hash of the raw wire request. If a future field is added to `NewJob`/
  `NewMessage` and the corresponding fingerprint literal is not updated to include it, two
  calls differing only in that field would incorrectly be treated as the same payload and
  replay-collapsed. Mitigated by keeping the fingerprint construction next to `NewJob`/
  `NewMessage` in the same file (§4/§6) rather than in a separate module, so the two are
  visually adjacent to whoever next edits either struct — flagged here for the spec
  critique to weigh in on whether a compile-time assertion (e.g. destructuring `new` by
  name inside the fingerprint function, which already forces this file to name every field)
  is sufficient, given `NewJob`/`NewMessage` already derive `Default` and neither is
  `#[non_exhaustive]`.
- `db.constraint()` is used to disambiguate `jobs_org_idempotency_key_idx` from
  `jobs_org_repo_tracker_ticket_open_idx` (0015) on a unique-violation from `add_job`'s
  insert. In practice `add_job`'s own insert sets `tracker = NULL` always (only
  `create_from_ticket`/`link_ticket` set a `Tracker`), so 0015's index — which requires a
  non-NULL `tracker` to even participate meaningfully across rows — should never be the one
  that fires from this code path; the constraint-name check is defensive rather than
  load-bearing today, and is called out here so a reviewer checking "is this actually
  reachable" does not need to re-derive it.
