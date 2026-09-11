//! The audit trail.
//!
//! Two entry points, matching the two transaction kinds:
//!
//! - [`Tx::audit`] for anything that happens inside an org — role changes, PAT
//!   mints, repo registration, tracker connections. Written in the *same*
//!   transaction as the change itself, so an action and its record commit or
//!   abort together and the trail can never disagree with reality.
//! - [`Db::audit_global`] for events that precede any org context: login
//!   attempts, passkey enrollment.
//!
//! A third entry point, [`Db::audit_global_on`], is for a caller that already
//! holds a transaction open for something else and needs its audit write to
//! commit or roll back with it, rather than best-effort on the pool.
//!
//! Actions are dotted and stable because they are queried by prefix and because
//! they end up in customers' SIEM exports. Renaming one is a breaking change.

use crate::db::{Db, Tx};
use crate::error::Result;
use crate::ids::{OrgId, UserId};
use serde::Serialize;
use sqlx::FromRow;

/// Stable action names. Use these constants rather than string literals at call
/// sites — a typo in a literal produces an event nobody will ever find.
pub mod action {
    // Authentication (usually global: no org context yet).
    pub const LOGIN_SUCCEEDED: &str = "auth.login.succeeded";
    pub const LOGIN_FAILED: &str = "auth.login.failed";
    pub const LOGOUT: &str = "auth.logout";
    /// Historical only. TOTP was removed from this product; rows with this
    /// action predate `PASSKEY_REGISTERED` and are not rewritten. Nothing
    /// writes this constant anymore.
    pub const TOTP_ENROLLED: &str = "auth.totp.enrolled";
    /// Historical only, for the same reason as `TOTP_ENROLLED`. Rows predate a
    /// split into two successors: `PASSKEY_CLEARED` (self-service, global) and
    /// `MEMBER_PASSKEYS_RESET` (admin-assisted, org-scoped). A historical row
    /// under this action does not say which of the two occurred — check
    /// whether it carries an `org_id` to tell them apart.
    pub const TOTP_RESET: &str = "auth.totp.reset";
    pub const RECOVERY_CODE_USED: &str = "auth.recovery_code.used";
    pub const MAGIC_LINK_SENT: &str = "auth.magic_link.sent";
    pub const MAGIC_LINK_CONSUMED: &str = "auth.magic_link.consumed";
    pub const EMAIL_VERIFIED: &str = "auth.email.verified";
    pub const PASSKEY_REGISTERED: &str = "auth.passkey.registered";
    pub const PASSKEY_CLEARED: &str = "auth.passkey.cleared";
    pub const PASSKEY_REMOVED: &str = "auth.passkey.removed";
    pub const PASSKEY_RENAMED: &str = "auth.passkey.renamed";
    /// A `claim/finish` request that rolled back — a ceremony/claim ownership
    /// mismatch, or a failure partway through registration. Best-effort,
    /// written outside the rolled-back transaction (see
    /// `of_web::routes::auth::claim_finish`), so the admin-assisted-recovery
    /// path still leaves a trace when a completion attempt is refused, not
    /// only when one succeeds (`PASSKEY_REGISTERED` with `via = "claim"`).
    pub const CLAIM_REFUSED: &str = "auth.claim.refused";

    // OAuth / tokens (org-scoped: the org is bound at authorization time).
    pub const CLIENT_REGISTERED: &str = "oauth.client.registered";
    pub const AUTHORIZATION_GRANTED: &str = "oauth.authorization.granted";
    pub const TOKEN_ISSUED: &str = "oauth.token.issued";
    pub const TOKEN_REFRESHED: &str = "oauth.token.refreshed";
    pub const TOKEN_REVOKED: &str = "oauth.token.revoked";
    /// A replayed refresh token. Treated as theft: the whole chain is revoked.
    /// This is the highest-signal line in the table — alert on it.
    pub const REFRESH_REUSE_DETECTED: &str = "oauth.refresh.reuse_detected";
    pub const PAT_MINTED: &str = "oauth.pat.minted";
    pub const PAT_REVOKED: &str = "oauth.pat.revoked";

    // Org administration.
    pub const MEMBER_INVITED: &str = "org.member.invited";
    pub const MEMBER_JOINED: &str = "org.member.joined";
    pub const MEMBER_ROLE_CHANGED: &str = "org.member.role_changed";
    pub const MEMBER_REMOVED: &str = "org.member.removed";
    pub const MEMBER_PASSKEYS_RESET: &str = "org.member.passkeys_reset";
    pub const IDP_CONNECTED: &str = "org.idp.connected";
    pub const IDP_DISCONNECTED: &str = "org.idp.disconnected";
    pub const DOMAIN_CLAIMED: &str = "org.domain.claimed";
    pub const DOMAIN_VERIFIED: &str = "org.domain.verified";
    pub const PLAN_CHANGED: &str = "org.plan.changed";

    // Resources.
    pub const REPO_REGISTERED: &str = "repo.registered";
    pub const REPO_UPDATED: &str = "repo.updated";
    pub const TRACKER_CONNECTED: &str = "tracker.connected";
    pub const TRACKER_DISCONNECTED: &str = "tracker.disconnected";
    pub const TRACKER_BOUND: &str = "tracker.repo.bound";
    pub const TRACKER_UNBOUND: &str = "tracker.repo.unbound";

    // Jobs.
    pub const JOB_CANCEL_REQUESTED: &str = "job.cancel.requested";
    pub const JOB_CANCELLED: &str = "job.cancelled";
}

/// One recorded event. Built with the fluent constructors rather than a struct
/// literal so adding a field later does not break every call site.
#[derive(Debug, Clone, Default)]
pub struct Entry {
    pub actor_user_id: Option<UserId>,
    pub actor_label: Option<String>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub detail: Option<serde_json::Value>,
}

impl Entry {
    pub fn new(action: &str) -> Self {
        Self {
            action: action.to_string(),
            ..Default::default()
        }
    }

    pub fn actor(mut self, user: UserId) -> Self {
        self.actor_user_id = Some(user);
        self
    }

    pub fn actor_label(mut self, label: impl Into<String>) -> Self {
        self.actor_label = Some(label.into());
        self
    }

    pub fn target(mut self, kind: &str, id: impl Into<String>) -> Self {
        self.target_type = Some(kind.to_string());
        self.target_id = Some(id.into());
        self
    }

    /// Caller IP and user agent, as seen at the HTTP boundary.
    pub fn from_request(mut self, ip: Option<&str>, user_agent: Option<&str>) -> Self {
        self.ip = ip.map(str::to_string);
        self.user_agent = user_agent.map(str::to_string);
        self
    }

    /// Extra context. **Never put a secret, token, or credential here** — org
    /// admins read this table in the console.
    pub fn detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = Some(detail);
        self
    }
}

impl Entry {
    async fn write<'e, E>(self, org: Option<OrgId>, conn: E) -> Result<()>
    where
        E: sqlx::PgExecutor<'e>,
    {
        sqlx::query(INSERT_SQL)
            .bind(org)
            .bind(self.actor_user_id)
            .bind(self.actor_label)
            .bind(self.action)
            .bind(self.target_type)
            .bind(self.target_id)
            .bind(self.ip)
            .bind(self.user_agent)
            .bind(self.detail.unwrap_or_else(|| serde_json::json!({})))
            .execute(conn)
            .await?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub id: i64,
    pub org_id: Option<OrgId>,
    pub actor_user_id: Option<UserId>,
    pub actor_label: Option<String>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub detail: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

const AUDIT_COLS: &str = "id, org_id, actor_user_id, actor_label, action, target_type, \
                          target_id, ip, user_agent, detail, created_at";

const INSERT_SQL: &str = "INSERT INTO audit_events \
     (org_id, actor_user_id, actor_label, action, target_type, target_id, ip, user_agent, detail) \
     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)";

impl Tx<'_> {
    /// Record an org-scoped event **in the same transaction as the change it
    /// describes**. If the change rolls back, so does its record.
    pub async fn audit(&mut self, e: Entry) -> Result<()> {
        let org = self.org();
        e.write(Some(org), self.conn()).await
    }

    /// Read the org's audit trail, newest first. Powers the console's security
    /// page and any customer SIEM export.
    pub async fn audit_trail(
        &mut self,
        action_prefix: Option<&str>,
        limit: i64,
    ) -> Result<Vec<AuditEvent>> {
        let org = self.org();
        let events = sqlx::query_as(&format!(
            "SELECT {AUDIT_COLS} FROM audit_events \
             WHERE org_id = $1 AND ($2::text IS NULL OR action LIKE $2 || '%') \
             ORDER BY created_at DESC, id DESC LIMIT $3"
        ))
        .bind(org)
        .bind(action_prefix)
        .bind(limit.clamp(1, 1000))
        .fetch_all(self.conn())
        .await?;
        Ok(events)
    }
}

impl Db {
    /// Record an event with no org context — a login attempt, a passkey
    /// enrollment.
    ///
    /// Best-effort by design: this returns `Result`, but callers on the failed-
    /// login path should log an error and continue rather than turning an audit
    /// write failure into an authentication outage. Losing one audit row is bad;
    /// refusing every login because the audit table is unavailable is worse.
    pub async fn audit_global(&self, e: Entry) -> Result<()> {
        e.write(None, self.pool()).await
    }

    /// Record an org-scoped event from the control plane, where no tenant
    /// transaction is open (e.g. membership changes made during signup).
    pub async fn audit_for_org(&self, org: OrgId, e: Entry) -> Result<()> {
        e.write(Some(org), self.pool()).await
    }

    /// Record a global (no-org) event on a connection the caller already
    /// holds open — typically a transaction that also carries the change the
    /// event describes, so both commit or roll back together.
    ///
    /// Unlike [`Self::audit_global`], a failure here is **not** swallowed: it
    /// propagates to the caller, who is expected to let it abort the
    /// transaction. Use this only when a lost audit row would be worse than
    /// failing the whole operation — `Db::audit_global`'s own doc comment
    /// explains why the *pool* variant is deliberately best-effort for the
    /// ordinary login/enrollment path; this is the exception for a caller
    /// that decided the tradeoff the other way.
    ///
    /// **Never pass a pinned [`Tx`]'s connection here.** This writes a
    /// `NULL`-org row, which is only legal for a connection with no
    /// `app.org_id` set. On a connection where guard 2 sets `app.org_id`
    /// (i.e. `Tx::conn()`), `audit_events`'s `audit_events_append` policy's
    /// `WITH CHECK` rejects the row outright wherever RLS is enforced — and
    /// where RLS is bypassed instead, it silently writes a row no tenant's
    /// own audit trail will ever show. Either outcome depends on the
    /// deployment's RLS shape, which is exactly the kind of thing nothing in
    /// this crate may assume (see `Db::begin`'s doc comment). Use
    /// [`Tx::audit`] for anything running on a pinned connection.
    pub async fn audit_global_on<'e, E>(conn: E, e: Entry) -> Result<()>
    where
        E: sqlx::PgExecutor<'e>,
    {
        e.write(None, conn).await
    }
}
