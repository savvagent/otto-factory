//! Jobs — the queue.
//!
//! Every job is anchored to a repo. The lifecycle is
//! `pending → in-progress → completed | failed`, with `repend` returning a
//! terminal job to `pending` for one more dispatch. `in-progress` covers both
//! "claimed" and its optional refinement `active` — a claiming agent may call
//! `activate_job` to confirm it is actively working right now, but
//! `complete_job`/`fail_job` accept either as a starting state, so nothing
//! requires passing through `active` to finish.

use crate::db::Tx;
use crate::error::{Error, Result};
use crate::ids::{JobId, OrgId, RepoId, TeamId, UserId};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, schemars::JsonSchema,
)]
#[sqlx(type_name = "job_status", rename_all = "lowercase")]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Pending,
    #[sqlx(rename = "in-progress")]
    #[serde(rename = "in-progress")]
    InProgress,
    /// A refinement of `in-progress`: the claiming agent has confirmed it is
    /// actively working right now, not merely holding the claim. Never set
    /// or cleared by the server on its own — only an explicit `activate_job`
    /// call moves a job here, and `complete_job`/`fail_job` accept a job in
    /// either `in-progress` or `active` as their starting state, so calling
    /// `activate_job` is optional, not a gate.
    Active,
    Completed,
    Failed,
    /// Terminal. Reached either directly from `Pending` (via `request_cancel`,
    /// which finalizes immediately since there is no holder to wait on) or
    /// from `InProgress`/`Active` once the holder confirms it stopped (via
    /// `cancel_job`). Invariant: every `Cancelled` job has a non-null
    /// `cancel_requested_at`, enforced by `Tx::request_cancel`/`Tx::cancel_job`
    /// — never construct one otherwise.
    Cancelled,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::InProgress => "in-progress",
            Status::Active => "active",
            Status::Completed => "completed",
            Status::Failed => "failed",
            Status::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Status::Completed | Status::Failed | Status::Cancelled)
    }
}

impl std::str::FromStr for Status {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(Status::Pending),
            "in-progress" => Ok(Status::InProgress),
            "active" => Ok(Status::Active),
            "completed" => Ok(Status::Completed),
            "failed" => Ok(Status::Failed),
            "cancelled" => Ok(Status::Cancelled),
            other => Err(Error::Invalid(format!(
                "unknown status {other:?} (expected pending | in-progress | active | \
                 completed | failed | cancelled)"
            ))),
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, schemars::JsonSchema,
)]
#[sqlx(type_name = "tracker", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum Tracker {
    Jira,
    Github,
}

#[derive(Debug, Clone, PartialEq, Serialize, FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: JobId,
    pub org_id: OrgId,
    pub repo_id: RepoId,
    pub team_id: Option<TeamId>,
    pub title: String,
    pub description: Option<String>,
    pub status: Status,
    pub ticket_ref: Option<String>,
    pub tracker: Option<Tracker>,
    pub remote_revision: Option<String>,
    pub agent_type: Option<String>,
    /// Opaque to otto-factory. Customers' own skills own the shape.
    pub metadata: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub attempts: i32,
    pub result: Option<String>,
    pub error: Option<String>,
    pub created_by: Option<UserId>,
    pub claimed_by: Option<UserId>,
    pub claimed_by_label: Option<String>,
    /// Set by `request_cancel`. `None` means nobody has ever asked this job to
    /// stop. **Check `status` before acting on this**: it is only a live "stop,
    /// please" while `status` is `in-progress` or `active` — that is when you,
    /// as the holder, should call `cancel_job` once you actually stop (or
    /// `fail_job`, if you are stopping for your own reasons unrelated to this
    /// request). `complete_job`, `fail_job`, and the immediate pending-finalize
    /// path inside `request_cancel` itself do **not** clear this field, so a
    /// `Completed`, `Failed`, or `Cancelled` job can still show it set — at
    /// that point it is history, not a pending request. Only `repend_job`
    /// clears it.
    pub cancel_requested_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Who asked, at the time `cancel_requested_at` was last set. Can be
    /// `None` even while a request is still live: the column is
    /// `ON DELETE SET NULL`, so a requester whose account is later deleted
    /// leaves `cancel_requested_at`/`cancel_reason` in place with this field
    /// cleared. So `cancel_requested_by == None` does **not** mean "no
    /// request" — only `cancel_requested_at == None` means that.
    pub cancel_requested_by: Option<UserId>,
    pub cancel_reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct NewJob {
    pub repo_id: RepoId,
    pub team_id: Option<TeamId>,
    pub title: String,
    pub description: Option<String>,
    pub ticket_ref: Option<String>,
    pub tracker: Option<Tracker>,
    pub agent_type: Option<String>,
    pub metadata: Option<serde_json::Value>,
    /// Job ids that must reach `completed` before this one is claimable.
    pub depends_on: Vec<JobId>,
    pub created_by: Option<UserId>,
    /// Caller-supplied replay key. `None` (the default) reproduces today's
    /// behavior exactly — no lookup, no extra column write beyond NULL. See
    /// the `idempotency` module doc and the idempotency-key design spec's §1
    /// for the correctness argument.
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct JobFilter {
    pub status: Option<Status>,
    pub repo_id: Option<RepoId>,
    pub team_id: Option<TeamId>,
    /// Restrict to jobs this user created. Used by "what did I queue?" views.
    pub created_by: Option<UserId>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub pending: i64,
    pub in_progress: i64,
    pub active: i64,
    pub completed: i64,
    pub failed: i64,
    pub cancelled: i64,
    pub blocked: i64,
    pub total: i64,
}

const JOB_COLS: &str = "id, org_id, repo_id, team_id, title, description, status, ticket_ref, \
                        tracker, remote_revision, agent_type, metadata, created_at, started_at, \
                        completed_at, attempts, result, error, created_by, claimed_by, \
                        claimed_by_label, cancel_requested_at, cancel_requested_by, \
                        cancel_reason";

/// Fingerprint the fields that define "the same `add_job` call" — see the
/// `idempotency` module doc. Named-field destructure with no `..`: adding a
/// field to `NewJob` without deciding whether it belongs here fails to
/// *compile*, not just to be missed on review.
///
/// `team_id` here is `new.team_id` as the caller supplied it, deliberately
/// *not* `add_job`'s locally-inherited `new.team_id.or(repo.team_id)` — the
/// fingerprint describes the request, not a value `add_job` derives from it,
/// so a repo whose default team changes between the original call and a
/// replay does not turn an otherwise-identical retry into a false conflict.
fn job_idempotency_fingerprint(new: &NewJob) -> Vec<u8> {
    let NewJob {
        repo_id,
        team_id,
        title,
        description,
        ticket_ref,
        tracker,
        agent_type,
        metadata,
        depends_on,
        created_by,
        // The key names the replay lookup; it is never part of what makes
        // two calls "the same call".
        idempotency_key: _,
    } = new;

    // Dependency order carries no meaning (set_dependencies takes a set, not
    // a sequence), so it is sorted before hashing — otherwise the same
    // dependency set supplied in a different order would look like a
    // different payload and turn a genuine replay into a false conflict.
    let mut dep_ids: Vec<&str> = depends_on.iter().map(|j| j.0.as_str()).collect();
    dep_ids.sort_unstable();

    // Normalized the same way the insert normalizes it (`None` becomes
    // `{}`), so a caller who spells the omitted field as `{}` on a retry
    // still replays cleanly instead of hitting a false conflict.
    let metadata = metadata.clone().unwrap_or_else(|| serde_json::json!({}));

    crate::idempotency::fingerprint(&serde_json::json!({
        "repoId": repo_id,
        "teamId": team_id,
        "title": title.trim(),
        "description": description,
        "ticketRef": ticket_ref,
        "tracker": tracker,
        "agentType": agent_type,
        "metadata": metadata,
        "dependsOn": dep_ids,
        "createdBy": created_by,
    }))
}

impl Tx<'_> {
    /// Resolve `new.idempotency_key` against an already-completed `add_job`
    /// call, doing no writes and touching no meter. `Ok(None)` covers both
    /// "no key was supplied" (the common case, at zero extra query cost) and
    /// "the key has not been used before" — either way the caller should
    /// proceed to `add_job`. `add_job` repeats this check race-safely via a
    /// unique index + SAVEPOINT retry for the rare case of two concurrent
    /// callers racing a brand-new key; this method exists separately so the
    /// MCP layer can skip metering a replay *before* calling `add_job`,
    /// which has no way to un-charge after the fact.
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

    /// Enqueue a job.
    ///
    /// The id comes from the org's own counter, bumped under that org's row lock
    /// inside this transaction — so ids are dense and per-tenant, and two
    /// concurrent inserts in the same org cannot collide. Inserts in *different*
    /// orgs take different row locks and do not contend.
    pub async fn add_job(&mut self, new: NewJob) -> Result<Job> {
        if new.title.trim().is_empty() {
            return Err(Error::Invalid("job title must not be empty".into()));
        }
        if let Some(key) = new.idempotency_key.as_deref() {
            crate::idempotency::validate(key)?;
        }

        let org = self.org();

        // Confirm the repo belongs to this org before anything else. RLS would
        // also catch it, but a foreign-key error is a far worse message for an
        // agent than "repo not found".
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
        // Inherit the repo's team unless the caller named one, so team-scoped
        // reads work without the caller having to know about teams at all.
        let team_id = new.team_id.or(repo.team_id);

        let (job, created): (Job, bool) = if let Some(key) = new.idempotency_key.as_deref() {
            let hash = job_idempotency_fingerprint(&new);
            // A savepoint, not a bare INSERT: Postgres aborts the *whole*
            // enclosing transaction on any statement error, so recovering
            // from a unique-violation by running another query in the same
            // Tx would otherwise fail with "current transaction is aborted"
            // instead of ever reaching the recovery query below — same
            // reasoning as create_from_ticket's own SAVEPOINT below.
            sqlx::query("SAVEPOINT add_job_idempotency")
                .execute(self.conn())
                .await?;
            let inserted = sqlx::query_as(&format!(
                "INSERT INTO jobs (id, org_id, repo_id, team_id, title, description, \
                                   ticket_ref, tracker, remote_revision, agent_type, \
                                   metadata, created_by, idempotency_key, \
                                   idempotency_payload_hash) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) \
                 RETURNING {JOB_COLS}"
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
            // Moved, not cloned: `new.metadata` is not read again (only the
            // disjoint `new.depends_on` is, below), and `hash` (computed
            // just above) already captured its content.
            .bind(new.metadata.unwrap_or_else(|| serde_json::json!({})))
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
                // A concurrent caller won the race on this brand-new key
                // between find_replayed_job's read and this insert. Converge
                // on its row exactly like create_from_ticket converges on
                // ticket_ref's winner, rather than failing a request that,
                // semantically, already succeeded — the seq number burned
                // above is simply unused, same as that precedent.
                //
                // The literal index name here must match 0025's
                // `CREATE UNIQUE INDEX jobs_org_idempotency_key_idx` exactly:
                // if that index is ever renamed without updating this match,
                // the guard below silently stops matching and this whole
                // recovery path falls through to the generic `Err(Error::Db)`
                // arm instead — a correctness regression with no compiler
                // error to catch it.
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
                            "add_job lost a unique-violation race for idempotency key \
                             {key:?} but no concurrently-created job was found"
                        ))
                    })?;
                    if winner.1 != hash {
                        return Err(Error::IdempotencyKeyConflict {
                            key: key.to_string(),
                            tool: "add_job",
                        });
                    }
                    // Reachable two ways, and `Tx::add_job` cannot tell
                    // which: a genuine concurrent race (two callers both saw
                    // `find_replayed_job` return `None`, one lost the
                    // insert), or a direct of-core caller that skips the
                    // find_replayed_job pre-check entirely and relies on
                    // this fallback alone. Via `tools::jobs::add_job` (the
                    // production path) only the first is possible, and it
                    // means the loser was already billed via Factory::charge
                    // for a call that created nothing — accepted and
                    // documented in the idempotency-key design spec §8, not
                    // a bug, logged so it is observable rather than silent.
                    tracing::warn!(
                        org = %org,
                        key,
                        "add_job's idempotency-key insert hit a unique violation and \
                         converged onto an existing job instead of failing; expected under \
                         concurrent replay of the same new key (see design spec §8) — \
                         unexpected otherwise"
                    );
                    (self.get_job(&winner.0).await?, false)
                }
                Err(error) => return Err(Error::Db(error)),
            }
        } else {
            // Unchanged from before this feature: no key, no idempotency
            // columns written, no savepoint.
            let job = sqlx::query_as(&format!(
                "INSERT INTO jobs (id, org_id, repo_id, team_id, title, description, \
                                   ticket_ref, tracker, remote_revision, agent_type, \
                                   metadata, created_by) \
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

        // Only for a row this call actually inserted: the race-fallback arm
        // converged onto a concurrent winner whose own add_job call already
        // set its dependencies from the identical payload (the hash
        // comparison above includes dependsOn, so a mismatch there is
        // already a conflict before this point). Using `job.id` here (not
        // the locally-burned `id`) matters for the same reason — in the
        // race-fallback arm they are different jobs.
        if created && !new.depends_on.is_empty() {
            self.set_dependencies(&job.id, &new.depends_on, &[]).await?;
        }

        Ok(job)
    }

    pub async fn get_job(&mut self, id: &JobId) -> Result<Job> {
        let org = self.org();
        sqlx::query_as(&format!(
            "SELECT {JOB_COLS} FROM jobs WHERE org_id = $1 AND id = $2"
        ))
        .bind(org)
        .bind(id)
        .fetch_optional(self.conn())
        .await?
        .ok_or_else(|| Error::JobNotFound(id.clone()))
    }

    /// Look a job up by its tracker ticket reference. Returns the most recent
    /// match: a ticket may legitimately be queued more than once (a retry, a
    /// follow-up), and the newest is the one an agent means.
    pub async fn get_job_by_ticket(&mut self, ticket_ref: &str) -> Result<Option<Job>> {
        let org = self.org();
        let job = sqlx::query_as(&format!(
            "SELECT {JOB_COLS} FROM jobs WHERE org_id = $1 AND ticket_ref = $2 \
             ORDER BY created_at DESC LIMIT 1"
        ))
        .bind(org)
        .bind(ticket_ref)
        .fetch_optional(self.conn())
        .await?;
        Ok(job)
    }

    /// Look a job up by its tracker ticket reference, scoped to one repo and
    /// tracker. The org-wide sibling remains for callers whose inputs are
    /// already unique enough; the sync engine needs the narrower version so two
    /// repos in one org cannot share a ticket ref by accident.
    pub async fn get_job_by_ticket_for_repo(
        &mut self,
        repo_id: RepoId,
        tracker: Tracker,
        ticket_ref: &str,
    ) -> Result<Option<Job>> {
        let org = self.org();
        let job = sqlx::query_as(&format!(
            "SELECT {JOB_COLS} FROM jobs \
             WHERE org_id = $1 AND repo_id = $2 AND tracker = $3 AND ticket_ref = $4 \
             ORDER BY created_at DESC LIMIT 1"
        ))
        .bind(org)
        .bind(repo_id)
        .bind(tracker)
        .bind(ticket_ref)
        .fetch_optional(self.conn())
        .await?;
        Ok(job)
    }

    /// Look up the *live* job holding a ticket ref, scoped to one repo and
    /// tracker. Unlike `get_job_by_ticket_for_repo`'s newest-wins semantics —
    /// deliberately tolerant of multiple *historical* jobs sharing a
    /// ticket_ref (a ticket that closes and reopens gets a fresh job) —
    /// this is for callers who just lost a race against
    /// `jobs_org_repo_tracker_ticket_open_idx`, which allows at most one
    /// `pending`/`in-progress`/`active` row per (repo, tracker, ticket_ref). "Newest
    /// created_at" is not the same job as "the live one": `repend_job` can
    /// revive an older job without changing its `created_at`, which could
    /// leave a newer but already-terminal job looking like the conflict
    /// instead of the actual live holder the index just rejected a second
    /// writer for.
    async fn get_live_job_by_ticket_for_repo(
        &mut self,
        repo_id: RepoId,
        tracker: Tracker,
        ticket_ref: &str,
    ) -> Result<Option<Job>> {
        let org = self.org();
        let job = sqlx::query_as(&format!(
            "SELECT {JOB_COLS} FROM jobs \
             WHERE org_id = $1 AND repo_id = $2 AND tracker = $3 AND ticket_ref = $4 \
             AND status IN ('pending', 'in-progress', 'active') LIMIT 1"
        ))
        .bind(org)
        .bind(repo_id)
        .bind(tracker)
        .bind(ticket_ref)
        .fetch_optional(self.conn())
        .await?;
        Ok(job)
    }

    /// Attach a tracker identity to an already-existing job. Per-job, not a
    /// repo-level binding: `tracker_bindings` (Task 1) already answers "which
    /// tracker does this repo use, and through which connection" —
    /// `link_ticket` answers a different question, "which ticket does *this*
    /// job correspond to", the half a job created by hand (`add_job`, whose
    /// `ticket_ref` is recorded but not resolved to a `tracker`) or claimed
    /// before anyone thought to link it does not have.
    pub async fn link_ticket(
        &mut self,
        id: &JobId,
        tracker: Tracker,
        ticket_ref: &str,
    ) -> Result<Job> {
        if ticket_ref.trim().is_empty() {
            return Err(Error::Invalid("ticket_ref must not be empty".into()));
        }

        let org = self.org();
        let repo_id: RepoId =
            sqlx::query_scalar("SELECT repo_id FROM jobs WHERE org_id = $1 AND id = $2")
                .bind(org)
                .bind(id)
                .fetch_optional(self.conn())
                .await?
                .ok_or_else(|| Error::JobNotFound(id.clone()))?;

        // A savepoint, not a bare UPDATE: Postgres aborts the whole enclosing
        // transaction on any statement error, so recovering from a
        // unique-violation by running the lookup query below in the same Tx
        // would otherwise fail with "current transaction is aborted" instead
        // of ever reaching that lookup.
        sqlx::query("SAVEPOINT link_ticket")
            .execute(self.conn())
            .await?;
        // remote_revision resets to NULL: it is loop-safety state for the
        // inbound stale/echo guard, scoped to whichever ticket the job
        // currently points at. Relinking to a different ticket without
        // clearing it would make that ticket's first genuine webhook look
        // like an old echo of the previous one and get silently dropped.
        let updated = sqlx::query_as(&format!(
            "UPDATE jobs SET tracker = $3, ticket_ref = $4, remote_revision = NULL \
             WHERE org_id = $1 AND id = $2 RETURNING {JOB_COLS}"
        ))
        .bind(org)
        .bind(id)
        .bind(tracker)
        .bind(ticket_ref)
        .fetch_one(self.conn())
        .await;

        let job = match updated {
            Ok(job) => {
                sqlx::query("RELEASE SAVEPOINT link_ticket")
                    .execute(self.conn())
                    .await?;
                job
            }
            // 0015_jobs_ticket_ref_uniqueness.sql's partial index already
            // guards one live job per (repo, tracker, ticket_ref) for
            // create_from_ticket's inbound race case; an explicit
            // link_ticket call hitting it means a caller asked to attach a
            // ticket a different live job already owns — a genuine conflict
            // to name, not a race to silently converge on (unlike
            // create_from_ticket's handling of the same index).
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                sqlx::query("ROLLBACK TO SAVEPOINT link_ticket")
                    .execute(self.conn())
                    .await?;
                sqlx::query("RELEASE SAVEPOINT link_ticket")
                    .execute(self.conn())
                    .await?;
                let holder = self
                    .get_live_job_by_ticket_for_repo(repo_id, tracker, ticket_ref)
                    .await?
                    .ok_or_else(|| {
                        Error::Invalid(format!(
                            "link_ticket lost a unique-violation for {ticket_ref:?} but no \
                             conflicting job was found"
                        ))
                    })?;
                return Err(Error::TicketAlreadyLinked {
                    ticket_ref: ticket_ref.to_string(),
                    job: holder.id,
                });
            }
            // The job named by `id` existed for the `SELECT repo_id` lookup
            // above but is gone by the time this `UPDATE ... RETURNING`
            // runs — e.g. `delete_job` ran concurrently in between. Report
            // it as the not-found it now is rather than letting `Error::Db`
            // redact it into an opaque "retry shortly": there is nothing to
            // retry, the job is gone.
            Err(sqlx::Error::RowNotFound) => {
                sqlx::query("ROLLBACK TO SAVEPOINT link_ticket")
                    .execute(self.conn())
                    .await?;
                sqlx::query("RELEASE SAVEPOINT link_ticket")
                    .execute(self.conn())
                    .await?;
                return Err(Error::JobNotFound(id.clone()));
            }
            Err(error) => return Err(Error::Db(error)),
        };

        Ok(job)
    }

    pub async fn create_from_ticket(
        &mut self,
        repo_id: RepoId,
        tracker: Tracker,
        ticket_ref: &str,
        title: &str,
        description: Option<&str>,
        remote_revision: Option<&str>,
    ) -> Result<Job> {
        if title.trim().is_empty() {
            return Err(Error::Invalid("job title must not be empty".into()));
        }

        let org = self.org();
        let repo = self
            .get_repo(repo_id)
            .await?
            .ok_or_else(|| Error::RepoNotFound(repo_id.to_string()))?;

        let seq: i64 = sqlx::query_scalar(
            "UPDATE orgs SET next_job_seq = next_job_seq + 1 WHERE id = $1 \
             RETURNING next_job_seq - 1",
        )
        .bind(org)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::OrgNotFound(org))?;

        let id = JobId::from_seq(seq);
        // A savepoint, not a bare INSERT: Postgres aborts the *whole*
        // enclosing transaction on any statement error, so recovering from
        // a unique-violation by running another query in the same Tx would
        // otherwise fail with "current transaction is aborted" instead of
        // ever reaching the recovery query below.
        sqlx::query("SAVEPOINT create_from_ticket")
            .execute(self.conn())
            .await?;
        let inserted = sqlx::query_as(&format!(
            "INSERT INTO jobs (id, org_id, repo_id, team_id, title, description, ticket_ref, \
                               tracker, remote_revision, metadata) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING {JOB_COLS}"
        ))
        .bind(&id)
        .bind(org)
        .bind(repo_id)
        .bind(repo.team_id)
        .bind(title.trim())
        .bind(description)
        .bind(ticket_ref)
        .bind(tracker)
        .bind(remote_revision)
        .bind(serde_json::json!({}))
        .fetch_one(self.conn())
        .await;

        let job = match inserted {
            Ok(job) => {
                sqlx::query("RELEASE SAVEPOINT create_from_ticket")
                    .execute(self.conn())
                    .await?;
                job
            }
            // Two concurrent webhook deliveries for the same freshly-labelled
            // issue can both observe "no existing job" from
            // get_job_by_ticket_for_repo and both reach this insert;
            // 0015_jobs_ticket_ref_uniqueness.sql's partial index turns the
            // loser into a unique-violation instead of a duplicate job. Treat
            // it as the race it is: hand back the job the other transaction
            // just created rather than failing the caller (the webhook route
            // would otherwise 500 on a request that, semantically, succeeded)
            // — never guessed, since the id burned above is simply unused.
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                sqlx::query("ROLLBACK TO SAVEPOINT create_from_ticket")
                    .execute(self.conn())
                    .await?;
                sqlx::query("RELEASE SAVEPOINT create_from_ticket")
                    .execute(self.conn())
                    .await?;
                self.get_job_by_ticket_for_repo(repo_id, tracker, ticket_ref)
                    .await?
                    .ok_or_else(|| {
                        Error::Invalid(format!(
                            "create_from_ticket lost a unique-violation race for {ticket_ref:?} \
                             but no concurrently-created job was found"
                        ))
                    })?
            }
            Err(error) => return Err(Error::Db(error)),
        };

        Ok(job)
    }

    pub async fn update_from_ticket(
        &mut self,
        id: &JobId,
        title: &str,
        description: Option<&str>,
        remote_revision: Option<&str>,
    ) -> Result<Job> {
        if title.trim().is_empty() {
            return Err(Error::Invalid("job title must not be empty".into()));
        }

        let job = sqlx::query_as(&format!(
            "UPDATE jobs SET title = $3, description = $4, \
                    remote_revision = COALESCE($5, remote_revision) \
             WHERE org_id = $1 AND id = $2 RETURNING {JOB_COLS}"
        ))
        .bind(self.org())
        .bind(id)
        .bind(title.trim())
        .bind(description)
        .bind(remote_revision)
        .fetch_optional(self.conn())
        .await?
        .ok_or_else(|| Error::JobNotFound(id.clone()))?;

        Ok(job)
    }

    pub async fn list_jobs(&mut self, f: &JobFilter) -> Result<Vec<Job>> {
        let org = self.org();
        let jobs = sqlx::query_as(&format!(
            "SELECT {JOB_COLS} FROM jobs \
             WHERE org_id = $1 \
               AND ($2::job_status IS NULL OR status = $2) \
               AND ($3::uuid IS NULL OR repo_id = $3) \
               AND ($4::uuid IS NULL OR team_id = $4) \
               AND ($5::uuid IS NULL OR created_by = $5) \
             ORDER BY created_at DESC \
             LIMIT $6"
        ))
        .bind(org)
        .bind(f.status)
        .bind(f.repo_id)
        .bind(f.team_id)
        .bind(f.created_by)
        .bind(f.limit.unwrap_or(200).clamp(1, 1000))
        .fetch_all(self.conn())
        .await?;
        Ok(jobs)
    }

    /// Claim one or more pending jobs atomically.
    ///
    /// All or nothing: if any requested job is missing, already claimed, or
    /// still blocked, the whole batch fails and none are claimed. A partial
    /// claim would leave an agent believing it owns work it does not, which is
    /// the exact race this queue exists to prevent.
    ///
    /// Rows are locked in a deterministic (sorted) order so two agents claiming
    /// overlapping batches cannot deadlock each other.
    pub async fn claim_jobs(
        &mut self,
        ids: &[JobId],
        claimer: UserId,
        label: Option<&str>,
    ) -> Result<Vec<Job>> {
        if ids.is_empty() {
            return Err(Error::Invalid(
                "claim_jobs needs at least one job id".into(),
            ));
        }

        let org = self.org();
        let mut sorted: Vec<String> = ids.iter().map(|i| i.0.clone()).collect();
        sorted.sort();
        sorted.dedup();

        let locked: Vec<(String, Status)> = sqlx::query_as(
            "SELECT id, status FROM jobs WHERE org_id = $1 AND id = ANY($2) \
             ORDER BY id FOR UPDATE",
        )
        .bind(org)
        .bind(&sorted)
        .fetch_all(self.conn())
        .await?;

        if locked.len() != sorted.len() {
            let found: std::collections::HashSet<&str> =
                locked.iter().map(|(i, _)| i.as_str()).collect();
            let missing = sorted
                .iter()
                .find(|i| !found.contains(i.as_str()))
                .cloned()
                .unwrap_or_default();
            return Err(Error::JobNotFound(JobId(missing)));
        }

        for (id, status) in &locked {
            if *status != Status::Pending {
                return Err(Error::WrongStatus {
                    job: JobId(id.clone()),
                    actual: status.as_str().to_string(),
                    expected: "pending".into(),
                });
            }
        }

        // Reject anything still blocked by an incomplete dependency.
        let blocked: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT d.job_id FROM job_dependencies d \
             JOIN jobs dep ON dep.org_id = d.org_id AND dep.id = d.depends_on \
             WHERE d.org_id = $1 AND d.job_id = ANY($2) AND dep.status <> 'completed'",
        )
        .bind(org)
        .bind(&sorted)
        .fetch_all(self.conn())
        .await?;

        if let Some(b) = blocked.first() {
            return Err(Error::WrongStatus {
                job: JobId(b.clone()),
                actual: "blocked by an incomplete dependency".into(),
                expected: "ready".into(),
            });
        }

        let jobs: Vec<Job> = sqlx::query_as(&format!(
            "UPDATE jobs SET status = 'in-progress', started_at = now(), \
                    attempts = attempts + 1, claimed_by = $3, claimed_by_label = $4 \
             WHERE org_id = $1 AND id = ANY($2) RETURNING {JOB_COLS}"
        ))
        .bind(org)
        .bind(&sorted)
        .bind(claimer)
        .bind(label)
        .fetch_all(self.conn())
        .await?;

        Ok(jobs)
    }

    /// Mark an in-progress (or active) job completed.
    pub async fn complete_job(&mut self, id: &JobId, result: Option<&str>) -> Result<Job> {
        self.finalize(id, Status::Completed, result, None).await
    }

    /// Mark an in-progress (or active) job failed.
    pub async fn fail_job(&mut self, id: &JobId, error: Option<&str>) -> Result<Job> {
        self.finalize(id, Status::Failed, None, error).await
    }

    /// Confirm a claimed job is being actively worked on, not merely claimed.
    /// This is a refinement signal only — `complete_job`/`fail_job` accept a
    /// job in `in-progress` OR `active` as their starting state, so calling
    /// this is optional, never a gate an agent must pass through to finish
    /// work.
    pub async fn activate_job(&mut self, id: &JobId) -> Result<Job> {
        let org = self.org();
        let current: Option<Status> =
            sqlx::query_scalar("SELECT status FROM jobs WHERE org_id = $1 AND id = $2 FOR UPDATE")
                .bind(org)
                .bind(id)
                .fetch_optional(self.conn())
                .await?;

        let current = current.ok_or_else(|| Error::JobNotFound(id.clone()))?;
        if current != Status::InProgress {
            return Err(Error::WrongStatus {
                job: id.clone(),
                actual: current.as_str().to_string(),
                expected: "in-progress".into(),
            });
        }

        let job = sqlx::query_as(&format!(
            "UPDATE jobs SET status = 'active' WHERE org_id = $1 AND id = $2 \
             RETURNING {JOB_COLS}"
        ))
        .bind(org)
        .bind(id)
        .fetch_one(self.conn())
        .await?;

        Ok(job)
    }

    async fn finalize(
        &mut self,
        id: &JobId,
        to: Status,
        result: Option<&str>,
        error: Option<&str>,
    ) -> Result<Job> {
        let org = self.org();
        let current: Option<Status> =
            sqlx::query_scalar("SELECT status FROM jobs WHERE org_id = $1 AND id = $2 FOR UPDATE")
                .bind(org)
                .bind(id)
                .fetch_optional(self.conn())
                .await?;

        let current = current.ok_or_else(|| Error::JobNotFound(id.clone()))?;
        if !matches!(current, Status::InProgress | Status::Active) {
            return Err(Error::WrongStatus {
                job: id.clone(),
                actual: current.as_str().to_string(),
                expected: "in-progress or active".into(),
            });
        }

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

    /// Ask a job to stop.
    ///
    /// A `Pending` job has no holder to wait on — there is nobody to notice the
    /// flag and act on it — so this finalizes it to `Cancelled` immediately, in
    /// the same statement that stamps the request. An `InProgress`/`Active` job
    /// only gets the three cancellation fields set: its holder is the only
    /// party that can say when it has actually stopped, and does so by calling
    /// `cancel_job`. Calling this again on an already-flagged job re-stamps the
    /// request rather than failing, so an agent that lost track of an earlier
    /// call can simply ask again.
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

        let job = if current == Status::Pending {
            sqlx::query_as(&format!(
                "UPDATE jobs SET status = 'cancelled', completed_at = now(), \
                        cancel_requested_at = now(), cancel_requested_by = $3, \
                        cancel_reason = $4 \
                 WHERE org_id = $1 AND id = $2 RETURNING {JOB_COLS}"
            ))
            .bind(org)
            .bind(id)
            .bind(requested_by)
            .bind(reason)
            .fetch_one(self.conn())
            .await?
        } else {
            sqlx::query_as(&format!(
                "UPDATE jobs SET cancel_requested_at = now(), cancel_requested_by = $3, \
                        cancel_reason = $4 \
                 WHERE org_id = $1 AND id = $2 RETURNING {JOB_COLS}"
            ))
            .bind(org)
            .bind(id)
            .bind(requested_by)
            .bind(reason)
            .fetch_one(self.conn())
            .await?
        };

        Ok(job)
    }

    /// Confirm a claimed job has stopped in response to an earlier
    /// `request_cancel` call.
    ///
    /// Refuses a job with no cancellation request on file: a holder stopping
    /// for its own reasons, unrelated to a request, should call `fail_job`
    /// instead — the distinction is what lets an observer tell "asked to stop
    /// and did" apart from "gave up on its own".
    pub async fn cancel_job(&mut self, id: &JobId, note: Option<&str>) -> Result<Job> {
        let org = self.org();
        let row: Option<(Status, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
            "SELECT status, cancel_requested_at FROM jobs \
             WHERE org_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(org)
        .bind(id)
        .fetch_optional(self.conn())
        .await?;

        let (current, cancel_requested_at) = row.ok_or_else(|| Error::JobNotFound(id.clone()))?;
        if !matches!(current, Status::InProgress | Status::Active) {
            return Err(Error::WrongStatus {
                job: id.clone(),
                actual: current.as_str().to_string(),
                expected: "in-progress or active".into(),
            });
        }
        if cancel_requested_at.is_none() {
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

    pub async fn close_from_ticket(
        &mut self,
        id: &JobId,
        to: Status,
        result: Option<&str>,
        error: Option<&str>,
        remote_revision: Option<&str>,
    ) -> Result<Job> {
        if !matches!(to, Status::Completed | Status::Failed) {
            return Err(Error::Invalid(format!(
                "close_from_ticket must move a job to completed or failed, not {}",
                to.as_str()
            )));
        }

        let org = self.org();
        let current: Option<Status> =
            sqlx::query_scalar("SELECT status FROM jobs WHERE org_id = $1 AND id = $2 FOR UPDATE")
                .bind(org)
                .bind(id)
                .fetch_optional(self.conn())
                .await?;

        let current = current.ok_or_else(|| Error::JobNotFound(id.clone()))?;
        if !matches!(
            current,
            Status::Pending | Status::InProgress | Status::Active
        ) {
            return Err(Error::WrongStatus {
                job: id.clone(),
                actual: current.as_str().to_string(),
                expected: "pending, in-progress, or active".into(),
            });
        }

        // `remote_revision` folds into the same statement as the status
        // transition (COALESCE, matching `update_from_ticket`) rather than a
        // follow-up `set_remote_revision` call, so a ticket close and its
        // revision stamp can never drift apart into two partially-applied
        // writes.
        let job = sqlx::query_as(&format!(
            "UPDATE jobs SET status = $3, completed_at = now(), result = $4, error = $5, \
                    remote_revision = COALESCE($6, remote_revision) \
             WHERE org_id = $1 AND id = $2 RETURNING {JOB_COLS}"
        ))
        .bind(org)
        .bind(id)
        .bind(to)
        .bind(result)
        .bind(error)
        .bind(remote_revision)
        .fetch_one(self.conn())
        .await?;

        Ok(job)
    }

    /// Return a terminal job to `pending` for one more dispatch. `attempts` is
    /// preserved so a job that keeps coming back is visible as such.
    pub async fn repend_job(&mut self, id: &JobId) -> Result<Job> {
        let org = self.org();
        let current: Option<Status> =
            sqlx::query_scalar("SELECT status FROM jobs WHERE org_id = $1 AND id = $2 FOR UPDATE")
                .bind(org)
                .bind(id)
                .fetch_optional(self.conn())
                .await?;

        let current = current.ok_or_else(|| Error::JobNotFound(id.clone()))?;
        if current == Status::Pending {
            return Err(Error::WrongStatus {
                job: id.clone(),
                actual: "pending".into(),
                expected: "completed, failed, in-progress, active, or cancelled".into(),
            });
        }

        let job = sqlx::query_as(&format!(
            "UPDATE jobs SET status = 'pending', started_at = NULL, completed_at = NULL, \
                    result = NULL, error = NULL, claimed_by = NULL, claimed_by_label = NULL, \
                    cancel_requested_at = NULL, cancel_requested_by = NULL, cancel_reason = NULL \
             WHERE org_id = $1 AND id = $2 RETURNING {JOB_COLS}"
        ))
        .bind(org)
        .bind(id)
        .fetch_one(self.conn())
        .await?;

        Ok(job)
    }

    pub async fn delete_job(&mut self, id: &JobId) -> Result<()> {
        let org = self.org();
        let n = sqlx::query("DELETE FROM jobs WHERE org_id = $1 AND id = $2")
            .bind(org)
            .bind(id)
            .execute(self.conn())
            .await?
            .rows_affected();
        if n == 0 {
            return Err(Error::JobNotFound(id.clone()));
        }
        Ok(())
    }

    pub async fn set_remote_revision(&mut self, id: &JobId, revision: &str) -> Result<()> {
        let updated =
            sqlx::query("UPDATE jobs SET remote_revision = $3 WHERE org_id = $1 AND id = $2")
                .bind(self.org())
                .bind(id)
                .bind(revision)
                .execute(self.conn())
                .await?
                .rows_affected();
        if updated == 0 {
            return Err(Error::JobNotFound(id.clone()));
        }
        Ok(())
    }

    /// Edit a pending job. Every field is optional; `None` leaves it unchanged.
    pub async fn update_job(
        &mut self,
        id: &JobId,
        title: Option<&str>,
        description: Option<&str>,
        agent_type: Option<&str>,
        metadata: Option<&serde_json::Value>,
    ) -> Result<Job> {
        if title.is_none() && description.is_none() && agent_type.is_none() && metadata.is_none() {
            return Err(Error::Invalid(
                "update_job needs at least one of title, description, agentType, metadata".into(),
            ));
        }

        let org = self.org();
        let job = sqlx::query_as(&format!(
            "UPDATE jobs SET title = COALESCE($3, title), \
                    description = COALESCE($4, description), \
                    agent_type = COALESCE($5, agent_type), \
                    metadata = COALESCE($6, metadata) \
             WHERE org_id = $1 AND id = $2 RETURNING {JOB_COLS}"
        ))
        .bind(org)
        .bind(id)
        .bind(title)
        .bind(description)
        .bind(agent_type)
        .bind(metadata)
        .fetch_optional(self.conn())
        .await?
        .ok_or_else(|| Error::JobNotFound(id.clone()))?;

        Ok(job)
    }

    /// Add and/or remove dependencies, rejecting anything that would create a
    /// cycle.
    ///
    /// The check runs *after* the inserts inside this transaction and rolls back
    /// on failure. Checking reachability before inserting would race a
    /// concurrent edit that closes the loop from the other side; letting the
    /// database hold the rows and then asking "is this job now reachable from
    /// itself?" cannot.
    pub async fn set_dependencies(
        &mut self,
        id: &JobId,
        add: &[JobId],
        remove: &[JobId],
    ) -> Result<Vec<JobId>> {
        if add.is_empty() && remove.is_empty() {
            return Err(Error::Invalid(
                "set_dependencies needs at least one dependency to add or remove".into(),
            ));
        }

        let org = self.org();

        for dep in add {
            if dep == id {
                return Err(Error::DependencyCycle(id.clone(), id.clone()));
            }
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM jobs WHERE org_id = $1 AND id = $2)",
            )
            .bind(org)
            .bind(dep)
            .fetch_one(self.conn())
            .await?;
            if !exists {
                return Err(Error::JobNotFound(dep.clone()));
            }

            sqlx::query(
                "INSERT INTO job_dependencies (org_id, job_id, depends_on) VALUES ($1,$2,$3) \
                 ON CONFLICT DO NOTHING",
            )
            .bind(org)
            .bind(id)
            .bind(dep)
            .execute(self.conn())
            .await?;
        }

        for dep in remove {
            sqlx::query(
                "DELETE FROM job_dependencies WHERE org_id = $1 AND job_id = $2 AND depends_on = $3",
            )
            .bind(org)
            .bind(id)
            .bind(dep)
            .execute(self.conn())
            .await?;
        }

        // Is `id` now reachable from itself by following depends_on edges?
        let cycle: Option<String> = sqlx::query_scalar(
            "WITH RECURSIVE reach(job_id) AS ( \
                 SELECT depends_on FROM job_dependencies WHERE org_id = $1 AND job_id = $2 \
                 UNION \
                 SELECT d.depends_on FROM job_dependencies d \
                   JOIN reach r ON r.job_id = d.job_id WHERE d.org_id = $1 \
             ) SELECT job_id FROM reach WHERE job_id = $2 LIMIT 1",
        )
        .bind(org)
        .bind(id)
        .fetch_optional(self.conn())
        .await?;

        if let Some(via) = cycle {
            return Err(Error::DependencyCycle(id.clone(), JobId(via)));
        }

        let deps: Vec<String> = sqlx::query_scalar(
            "SELECT depends_on FROM job_dependencies WHERE org_id = $1 AND job_id = $2 \
             ORDER BY depends_on",
        )
        .bind(org)
        .bind(id)
        .fetch_all(self.conn())
        .await?;

        Ok(deps.into_iter().map(JobId).collect())
    }

    pub async fn dependencies_of(&mut self, id: &JobId) -> Result<Vec<JobId>> {
        let org = self.org();
        let deps: Vec<String> = sqlx::query_scalar(
            "SELECT depends_on FROM job_dependencies WHERE org_id = $1 AND job_id = $2 \
             ORDER BY depends_on",
        )
        .bind(org)
        .bind(id)
        .fetch_all(self.conn())
        .await?;
        Ok(deps.into_iter().map(JobId).collect())
    }

    /// Pending jobs whose dependencies are all completed — i.e. claimable now.
    pub async fn ready(&mut self, repo_id: Option<RepoId>) -> Result<Vec<Job>> {
        let org = self.org();
        let jobs = sqlx::query_as(&format!(
            "SELECT {JOB_COLS} FROM jobs j \
             WHERE j.org_id = $1 AND j.status = 'pending' \
               AND ($2::uuid IS NULL OR j.repo_id = $2) \
               AND NOT EXISTS ( \
                 SELECT 1 FROM job_dependencies d \
                 JOIN jobs dep ON dep.org_id = d.org_id AND dep.id = d.depends_on \
                 WHERE d.org_id = j.org_id AND d.job_id = j.id AND dep.status <> 'completed') \
             ORDER BY j.created_at"
        ))
        .bind(org)
        .bind(repo_id)
        .fetch_all(self.conn())
        .await?;
        Ok(jobs)
    }

    /// Pending jobs still waiting on at least one incomplete dependency.
    pub async fn blocked(&mut self, repo_id: Option<RepoId>) -> Result<Vec<Job>> {
        let org = self.org();
        let jobs = sqlx::query_as(&format!(
            "SELECT {JOB_COLS} FROM jobs j \
             WHERE j.org_id = $1 AND j.status = 'pending' \
               AND ($2::uuid IS NULL OR j.repo_id = $2) \
               AND EXISTS ( \
                 SELECT 1 FROM job_dependencies d \
                 JOIN jobs dep ON dep.org_id = d.org_id AND dep.id = d.depends_on \
                 WHERE d.org_id = j.org_id AND d.job_id = j.id AND dep.status <> 'completed') \
             ORDER BY j.created_at"
        ))
        .bind(org)
        .bind(repo_id)
        .fetch_all(self.conn())
        .await?;
        Ok(jobs)
    }

    pub async fn stats(&mut self, repo_id: Option<RepoId>) -> Result<Stats> {
        let org = self.org();
        let stats = sqlx::query_as(
            "SELECT \
               COUNT(*) FILTER (WHERE status = 'pending')     AS pending, \
               COUNT(*) FILTER (WHERE status = 'in-progress') AS in_progress, \
               COUNT(*) FILTER (WHERE status = 'active')      AS active, \
               COUNT(*) FILTER (WHERE status = 'completed')   AS completed, \
               COUNT(*) FILTER (WHERE status = 'failed')      AS failed, \
               COUNT(*) FILTER (WHERE status = 'cancelled')   AS cancelled, \
               COUNT(*) FILTER (WHERE status = 'pending' AND EXISTS ( \
                 SELECT 1 FROM job_dependencies d \
                 JOIN jobs dep ON dep.org_id = d.org_id AND dep.id = d.depends_on \
                 WHERE d.org_id = j.org_id AND d.job_id = j.id \
                   AND dep.status <> 'completed'))            AS blocked, \
               COUNT(*)                                       AS total \
             FROM jobs j WHERE j.org_id = $1 AND ($2::uuid IS NULL OR j.repo_id = $2)",
        )
        .bind(org)
        .bind(repo_id)
        .fetch_one(self.conn())
        .await?;
        Ok(stats)
    }
}
