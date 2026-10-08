//! The shared agent-to-agent message channel.
//!
//! Coordination chatter, not a payload transport: "taking the auth refactor,
//! stay out of crates/auth", "job-42 is blocked on the migration, who owns it?".
//! Bodies are bounded because every unread message is re-served on every inbox
//! read by every member — an oversized body is paid for many times over.

use crate::error::{Error, Result};
use crate::ids::{JobId, RepoId};
use crate::teams::TeamScope;
use otto_tenant::ids::{OrgId, TeamId, UserId};
use otto_tenant::Tx;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Upper bound on a message body, in bytes.
///
/// Bytes rather than chars because the point is the storage and transfer cost,
/// and a multi-byte body can only be shorter in chars, never longer in bytes.
/// 16 KiB is several times the longest plausible hand-off note.
pub const MAX_BODY_LEN: usize = 16 * 1024;

/// Cap on one inbox read, so no caller can force an unbounded read out of a
/// shared server by asking for a huge limit.
pub const INBOX_LIMIT_MAX: i64 = 200;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    Serialize,
    Deserialize,
    sqlx::Type,
    schemars::JsonSchema,
)]
#[sqlx(type_name = "message_kind", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum MessageKind {
    #[default]
    Note,
    Request,
    Response,
}

/// Who typed a message.
///
/// A rendering hint, not a security claim: a human in the console and their
/// agent session authenticate as the same user, so this cannot be used to
/// distinguish them for authorization. The authoritative `sender_user_id` is
/// always set server-side from the authenticated principal and is never accepted
/// from the client.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    Serialize,
    Deserialize,
    sqlx::Type,
    schemars::JsonSchema,
)]
#[sqlx(type_name = "sender_kind", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum SenderKind {
    #[default]
    Agent,
    Human,
}

#[derive(Debug, Clone, PartialEq, Serialize, FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: i64,
    pub org_id: OrgId,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub sender_user_id: UserId,
    pub sender_label: Option<String>,
    pub sender_kind: SenderKind,
    pub recipient_user_id: Option<UserId>,
    pub team_id: Option<TeamId>,
    pub kind: MessageKind,
    pub body: String,
    pub repo_id: Option<RepoId>,
    pub job_id: Option<String>,
    pub in_reply_to: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct NewMessage {
    pub body: String,
    /// `None` broadcasts to the org (or the team, when `team_id` is set).
    pub recipient_user_id: Option<UserId>,
    pub team_id: Option<crate::teams::VerifiedTeam>,
    pub kind: MessageKind,
    pub sender_kind: SenderKind,
    pub sender_label: Option<String>,
    pub repo_id: Option<RepoId>,
    pub job_id: Option<JobId>,
    pub in_reply_to: Option<i64>,
    /// Caller-supplied replay key. `None` (the default) reproduces today's
    /// behavior exactly. See `jobs::NewJob::idempotency_key` — the identical
    /// shape, applied here.
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InboxQuery {
    /// Only messages past my cursor that I did not send. Default true.
    pub unread_only: bool,
    pub limit: i64,
    /// Newest first, so a limit-capped read keeps the most recent messages
    /// rather than the oldest. Default false (oldest first, conversational order).
    pub newest_first: bool,
    /// The reader's team visibility: when set, messages tied to a team, repo, or
    /// job outside it are left out. `None` is unrestricted.
    pub visible_teams: Option<Vec<TeamId>>,
}

impl Default for InboxQuery {
    fn default() -> Self {
        Self {
            unread_only: true,
            limit: 50,
            newest_first: false,
            visible_teams: None,
        }
    }
}

/// SQL: message `m` is visible under the `uuid[]` bound as `$n` (NULL = all).
/// A message tied to a team, a repo, or a job the reader cannot see is not
/// theirs to read, whoever it was addressed to.
fn message_visible_sql(n: usize) -> String {
    format!(
        "(${n}::uuid[] IS NULL OR ((m.team_id IS NULL OR m.team_id = ANY(${n})) \
           AND (m.repo_id IS NULL OR NOT EXISTS (SELECT 1 FROM repos vr \
                 WHERE vr.org_id = m.org_id AND vr.id = m.repo_id \
                   AND vr.team_id IS NOT NULL AND NOT vr.team_id = ANY(${n}))) \
           AND (m.job_id IS NULL OR NOT EXISTS (SELECT 1 FROM jobs vj \
                 JOIN repos vr ON vr.org_id = vj.org_id AND vr.id = vj.repo_id \
                 WHERE vj.org_id = m.org_id AND vj.id = m.job_id \
                   AND ((vj.team_id IS NOT NULL AND NOT vj.team_id = ANY(${n})) \
                     OR (vr.team_id IS NOT NULL AND NOT vr.team_id = ANY(${n})))))))"
    )
}

fn team_uuids(teams: Option<&Vec<TeamId>>) -> Option<Vec<uuid::Uuid>> {
    teams.map(|v| v.iter().map(|t| t.as_uuid()).collect())
}

const MSG_COLS: &str = "id, org_id, created_at, sender_user_id, sender_label, sender_kind, \
                        recipient_user_id, team_id, kind, body, repo_id, job_id, in_reply_to";

/// Fingerprint the fields that define "the same `send_message` call" — see
/// the `idempotency` module doc and `jobs::job_idempotency_fingerprint`,
/// which this mirrors. `sender` is threaded through separately (it is a
/// `send_message` parameter, not a `NewMessage` field) because two different
/// callers reusing the same key is a materially different request, not a
/// replay of each other's.
fn message_idempotency_fingerprint(sender: UserId, new: &NewMessage) -> Vec<u8> {
    let NewMessage {
        body,
        recipient_user_id,
        team_id,
        kind,
        sender_kind,
        sender_label,
        repo_id,
        job_id,
        in_reply_to,
        // The key names the replay lookup; it is never part of what makes
        // two calls "the same call".
        idempotency_key: _,
    } = new;

    crate::idempotency::fingerprint(&serde_json::json!({
        "sender": sender,
        "body": body.trim(),
        "recipientUserId": recipient_user_id,
        "teamId": team_id.map(|t| t.id()),
        "kind": kind,
        "senderKind": sender_kind,
        "senderLabel": sender_label,
        "repoId": repo_id,
        "jobId": job_id.as_ref().map(|j| j.0.as_str()),
        "inReplyTo": in_reply_to,
    }))
}

/// Extension methods on [`Tx`] for this module's domain (see the crate docs for why
/// these are extension traits rather than inherent methods).
pub trait MessagesExt {
    /// Resolve `new.idempotency_key` against an already-completed
    /// `send_message` call, doing no writes and touching no meter. See
    /// `jobs::Tx::find_replayed_job` — the identical shape.
    fn find_replayed_message(
        &mut self,
        sender: UserId,
        new: &NewMessage,
    ) -> impl std::future::Future<Output = Result<Option<Message>>> + Send;

    fn send_message(
        &mut self,
        sender: UserId,
        new: NewMessage,
    ) -> impl std::future::Future<Output = Result<Message>> + Send;

    /// Messages visible to `reader`: broadcasts plus anything addressed to them.
    fn inbox(
        &mut self,
        reader: UserId,
        q: &InboxQuery,
    ) -> impl std::future::Future<Output = Result<Vec<Message>>> + Send;

    /// Advance the read cursor.
    ///
    /// Clamped to the newest existing message id, so an over-large value cannot
    /// suppress messages that have not been written yet. Returns the cursor that
    /// actually landed, which is rarely what a careless caller passed.
    fn ack_messages(
        &mut self,
        reader: UserId,
        up_to: i64,
    ) -> impl std::future::Future<Output = Result<i64>> + Send;

    fn unread_count(
        &mut self,
        reader: UserId,
    ) -> impl std::future::Future<Output = Result<i64>> + Send;

    /// [`Self::ack_messages`], clamped to the newest message `scope` may see, so
    /// the cursor that lands says nothing about hidden traffic.
    fn ack_messages_for(
        &mut self,
        reader: UserId,
        up_to: i64,
        scope: &TeamScope,
    ) -> impl std::future::Future<Output = Result<i64>> + Send;

    /// [`Self::unread_count`], counting only messages `scope` may see.
    fn unread_count_for(
        &mut self,
        reader: UserId,
        scope: &TeamScope,
    ) -> impl std::future::Future<Output = Result<i64>> + Send;
}

impl MessagesExt for Tx<'_> {
    async fn find_replayed_message(
        &mut self,
        sender: UserId,
        new: &NewMessage,
    ) -> Result<Option<Message>> {
        let Some(key) = new.idempotency_key.as_deref() else {
            return Ok(None);
        };
        crate::idempotency::validate(key)?;
        // Bound the body before it is ever hashed, not just before it is
        // ever stored: without this, a caller replaying an already-used key
        // with an oversized body would pay for a full SHA-256 over it on
        // every retry, on a path that runs before metering.
        let body = new.body.trim();
        if body.is_empty() {
            return Err(Error::Invalid("message body must not be empty".into()));
        }
        if body.len() > MAX_BODY_LEN {
            return Err(Error::Invalid(format!(
                "message body is {} bytes; the limit is {MAX_BODY_LEN}",
                body.len()
            )));
        }

        let org = self.org();
        let existing: Option<(i64, Vec<u8>)> = sqlx::query_as(
            "SELECT id, idempotency_payload_hash FROM messages \
             WHERE org_id = $1 AND idempotency_key = $2",
        )
        .bind(org)
        .bind(key)
        .fetch_optional(self.conn())
        .await?;
        let Some((id, stored_hash)) = existing else {
            return Ok(None);
        };

        if stored_hash != message_idempotency_fingerprint(sender, new) {
            return Err(Error::IdempotencyKeyConflict {
                key: key.to_string(),
                tool: "send_message",
            });
        }

        let msg = sqlx::query_as(&format!(
            "SELECT {MSG_COLS} FROM messages WHERE org_id = $1 AND id = $2"
        ))
        .bind(org)
        .bind(id)
        .fetch_one(self.conn())
        .await?;
        Ok(Some(msg))
    }

    async fn send_message(&mut self, sender: UserId, new: NewMessage) -> Result<Message> {
        let body = new.body.trim();
        if body.is_empty() {
            return Err(Error::Invalid("message body must not be empty".into()));
        }
        if body.len() > MAX_BODY_LEN {
            return Err(Error::Invalid(format!(
                "message body is {} bytes; the limit is {MAX_BODY_LEN}",
                body.len()
            )));
        }
        if let Some(key) = new.idempotency_key.as_deref() {
            crate::idempotency::validate(key)?;
        }

        let org = self.org();

        let msg = if let Some(key) = new.idempotency_key.as_deref() {
            let hash = message_idempotency_fingerprint(sender, &new);
            // A savepoint, not a bare INSERT — see jobs::add_job's identical
            // reasoning: Postgres aborts the whole transaction on a
            // statement error, so recovering from a unique-violation in the
            // same Tx needs a savepoint to roll back to.
            sqlx::query("SAVEPOINT send_message_idempotency")
                .execute(self.conn())
                .await?;
            let inserted = sqlx::query_as(&format!(
                "INSERT INTO messages (org_id, sender_user_id, sender_label, sender_kind, \
                                       recipient_user_id, team_id, kind, body, repo_id, \
                                       job_id, in_reply_to, idempotency_key, \
                                       idempotency_payload_hash) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) \
                 RETURNING {MSG_COLS}"
            ))
            .bind(org)
            .bind(sender)
            .bind(new.sender_label.as_deref())
            .bind(new.sender_kind)
            .bind(new.recipient_user_id)
            .bind(new.team_id.map(|t| t.in_org(self.org())).transpose()?)
            .bind(new.kind)
            .bind(body)
            .bind(new.repo_id)
            .bind(new.job_id.as_ref().map(|j| j.0.as_str()))
            .bind(new.in_reply_to)
            .bind(key)
            .bind(&hash)
            .fetch_one(self.conn())
            .await;

            match inserted {
                Ok(msg) => {
                    sqlx::query("RELEASE SAVEPOINT send_message_idempotency")
                        .execute(self.conn())
                        .await?;
                    msg
                }
                // A concurrent caller won the race on this brand-new key
                // between find_replayed_message's read and this insert —
                // converge on its row exactly like jobs::add_job does. See
                // that function's identical comment on why the literal
                // index name below must stay in sync with 0025's
                // `CREATE UNIQUE INDEX messages_org_idempotency_key_idx`.
                Err(sqlx::Error::Database(db))
                    if db.is_unique_violation()
                        && db.constraint() == Some("messages_org_idempotency_key_idx") =>
                {
                    sqlx::query("ROLLBACK TO SAVEPOINT send_message_idempotency")
                        .execute(self.conn())
                        .await?;
                    sqlx::query("RELEASE SAVEPOINT send_message_idempotency")
                        .execute(self.conn())
                        .await?;
                    let winner: (i64, Vec<u8>) = sqlx::query_as(
                        "SELECT id, idempotency_payload_hash FROM messages \
                         WHERE org_id = $1 AND idempotency_key = $2",
                    )
                    .bind(org)
                    .bind(key)
                    .fetch_optional(self.conn())
                    .await?
                    // Unlike its three siblings (add_job's own idempotency
                    // recovery, link_ticket, create_from_ticket), this
                    // specific branch's precondition — the winner's row
                    // vanishing between the violation and this re-query —
                    // cannot currently occur: no code path in this codebase
                    // ever deletes a `users` or `orgs` row, and this insert
                    // has no other foreign key whose loss could produce the
                    // same shape. It stays classified as `RaceLost`
                    // (retriable) for structural consistency with the other
                    // three SAVEPOINT-recovery sites, and in case a future
                    // account- or org-deletion feature changes that. If such
                    // a feature is ever added, re-check this reasoning
                    // rather than assume it still holds — a retry here would
                    // resend the same `sender_user_id`, which could just as
                    // easily fail again on a foreign-key constraint
                    // (surfacing as `Error::Db`, not a second `RaceLost`)
                    // instead of succeeding.
                    .ok_or_else(|| {
                        Error::RaceLost(format!(
                            "send_message lost a unique-violation race for idempotency key {key:?} \
                             but no concurrently-created message was found — this is a transient \
                             server-side race; retry after a short backoff"
                        ))
                    })?;
                    if winner.1 != hash {
                        return Err(Error::IdempotencyKeyConflict {
                            key: key.to_string(),
                            tool: "send_message",
                        });
                    }
                    // See jobs::add_job's identical comment: this is
                    // reachable via a genuine concurrent race through the
                    // production (MCP) path, or via a direct of-core caller
                    // that skips the find_replayed_message pre-check — this
                    // function cannot tell which.
                    tracing::warn!(
                        org = %org,
                        key,
                        "send_message's idempotency-key insert hit a unique violation and \
                         converged onto an existing message instead of failing; expected \
                         under concurrent replay of the same new key (see design spec §8) \
                         — unexpected otherwise"
                    );
                    sqlx::query_as(&format!(
                        "SELECT {MSG_COLS} FROM messages WHERE org_id = $1 AND id = $2"
                    ))
                    .bind(org)
                    .bind(winner.0)
                    .fetch_one(self.conn())
                    .await?
                }
                Err(error) => return Err(Error::Db(error)),
            }
        } else {
            // Unchanged from before this feature: no key, no idempotency
            // columns written, no savepoint.
            sqlx::query_as(&format!(
                "INSERT INTO messages (org_id, sender_user_id, sender_label, sender_kind, \
                                       recipient_user_id, team_id, kind, body, repo_id, \
                                       job_id, in_reply_to) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING {MSG_COLS}"
            ))
            .bind(org)
            .bind(sender)
            .bind(new.sender_label.as_deref())
            .bind(new.sender_kind)
            .bind(new.recipient_user_id)
            .bind(new.team_id.map(|t| t.in_org(self.org())).transpose()?)
            .bind(new.kind)
            .bind(body)
            .bind(new.repo_id)
            .bind(new.job_id.as_ref().map(|j| j.0.as_str()))
            .bind(new.in_reply_to)
            .fetch_one(self.conn())
            .await?
        };

        Ok(msg)
    }

    async fn inbox(&mut self, reader: UserId, q: &InboxQuery) -> Result<Vec<Message>> {
        let org = self.org();
        let limit = q.limit.clamp(1, INBOX_LIMIT_MAX);

        // Two orderings rather than one parameterized `ORDER BY`: the direction
        // of an ORDER BY cannot be bound as a parameter, and building it by
        // string concatenation from client input is how injection happens.
        let order = if q.newest_first { "DESC" } else { "ASC" };

        let msgs = sqlx::query_as(&format!(
            "SELECT {MSG_COLS} FROM messages m \
             WHERE m.org_id = $1 \
               AND (m.recipient_user_id IS NULL OR m.recipient_user_id = $2) \
               AND (NOT $3 OR ( \
                     m.sender_user_id <> $2 \
                     AND m.id > COALESCE( \
                       (SELECT last_read_id FROM message_cursors \
                        WHERE org_id = $1 AND user_id = $2), 0))) \
               AND {} \
             ORDER BY m.id {order} LIMIT $4",
            message_visible_sql(5)
        ))
        .bind(org)
        .bind(reader)
        .bind(q.unread_only)
        .bind(limit)
        .bind(team_uuids(q.visible_teams.as_ref()))
        .fetch_all(self.conn())
        .await?;

        Ok(msgs)
    }

    async fn ack_messages(&mut self, reader: UserId, up_to: i64) -> Result<i64> {
        self.ack_messages_for(reader, up_to, &TeamScope::All).await
    }

    async fn ack_messages_for(
        &mut self,
        reader: UserId,
        up_to: i64,
        scope: &TeamScope,
    ) -> Result<i64> {
        let org = self.org();
        let teams = team_uuids(scope.restriction().as_ref());
        let newest: i64 = sqlx::query_scalar(&format!(
            "SELECT COALESCE(MAX(m.id), 0) FROM messages m WHERE m.org_id = $1 AND {}",
            message_visible_sql(2)
        ))
        .bind(org)
        .bind(teams)
        .fetch_one(self.conn())
        .await?;

        let target = up_to.clamp(0, newest);

        let landed: i64 = sqlx::query_scalar(
            "INSERT INTO message_cursors (org_id, user_id, last_read_id) VALUES ($1,$2,$3) \
             ON CONFLICT (org_id, user_id) DO UPDATE \
               SET last_read_id = GREATEST(message_cursors.last_read_id, EXCLUDED.last_read_id), \
                   updated_at = now() \
             RETURNING last_read_id",
        )
        .bind(org)
        .bind(reader)
        .bind(target)
        .fetch_one(self.conn())
        .await?;

        Ok(landed)
    }

    async fn unread_count(&mut self, reader: UserId) -> Result<i64> {
        self.unread_count_for(reader, &TeamScope::All).await
    }

    async fn unread_count_for(&mut self, reader: UserId, scope: &TeamScope) -> Result<i64> {
        let org = self.org();
        let teams = team_uuids(scope.restriction().as_ref());
        let n: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM messages m \
             WHERE m.org_id = $1 \
               AND (m.recipient_user_id IS NULL OR m.recipient_user_id = $2) \
               AND m.sender_user_id <> $2 \
               AND m.id > COALESCE( \
                 (SELECT last_read_id FROM message_cursors \
                  WHERE org_id = $1 AND user_id = $2), 0) \
               AND {}",
            message_visible_sql(3)
        ))
        .bind(org)
        .bind(reader)
        .bind(teams)
        .fetch_one(self.conn())
        .await?;
        Ok(n)
    }
}
