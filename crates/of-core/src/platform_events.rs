//! What the platform's signed lifecycle webhooks do to this database.
//!
//! The platform owns orgs, teams, and memberships; this database holds rows keyed
//! by their ids with no foreign key to them (they are in another database). So
//! when the platform deletes one, it tells every registered resource server, and
//! this module is the cleanup. Deliveries are at-least-once and unordered
//! (`otto_resource::webhook`), so everything here is idempotent twice over: each
//! delivery is recorded in `platform_events` **in the same transaction as its
//! effects** (a redelivery finds the row and does nothing), and each effect is
//! itself safe to repeat.
//!
//! | Event | Effect |
//! |---|---|
//! | `org.deleted` | Every row this database holds for the org is deleted: repos, jobs, leases, messages, tracker connections, counters, unshipped usage, and the org's domain audit trail. Only the dedupe marker survives. |
//! | `team.deleted` | **Scope is kept, not widened.** Repos, jobs, and messages keep their `team_id`. See below. |
//! | `member.removed` | The user's live leases are released, their in-progress or active job claims go back to `pending`, and their message read cursor is dropped. History they authored (jobs they created, messages they sent) is kept. |
//!
//! # Why `team.deleted` keeps the team id
//!
//! A null `team_id` means *org-wide*. The obvious cleanup, "null out the deleted
//! team's references", would therefore publish everything the team had to the
//! whole org, on the strength of a webhook. That is exactly the failure the old
//! schema had (`ON DELETE SET NULL`) and the old code guarded against by
//! refusing the delete. The platform does not ask us any more, so the guard
//! becomes this rule: **the dangling id is the tombstone.** The platform never
//! reuses a team id, so a deleted team's id matches no member's team list and no
//! `VerifiedTeam::verify`; the repo is visible to org owners and admins only
//! until one reassigns it (to a live team, or to org-wide, which is then a
//! deliberate, audited human act) through `PATCH .../repos/{repo}`. Nothing is
//! deleted and nothing is widened. The event is recorded in the audit trail with
//! the number of repos, jobs, and messages left orphaned, so an admin can find
//! them.

use crate::audit::action;
use crate::error::Result;
use otto_resource::webhook::{LifecycleEvent, WebhookEvent};
use otto_tenant::audit::Entry;
use otto_tenant::ids::{OrgId, TeamId, UserId};
use otto_tenant::Db;
use serde::Serialize;

/// What handling a delivery did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Outcome {
    /// Effects applied now.
    Applied { detail: serde_json::Value },
    /// This event id was applied by an earlier delivery. The clean-up was
    /// re-run anyway (it is idempotent), to catch work that raced the first run.
    Duplicate,
    /// An event type this version does not know. Acknowledged, not failed:
    /// failing it would only make the platform retry it forever.
    Ignored,
}

/// How long a removed member is refused by [`revoked`]. Past the introspection
/// cache's 60 s the platform itself answers "inactive", so this only has to
/// outlast that; it is short so a member who is re-added is not locked out.
pub const REMOVED_MEMBER_TTL_SECS: i64 = 300;

/// Whether `user`'s token for `org` must be refused regardless of what the
/// platform's cached introspection says: the org was deleted, or the user was
/// just removed from it.
///
/// Called by both HTTP surfaces on every authenticated request. An `Err` is a
/// database failure and must be answered `503`, not treated as "not revoked".
pub async fn revoked(db: &Db, org: OrgId, user: UserId) -> Result<bool> {
    let hit: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM deleted_orgs WHERE org_id = $1) \
             OR EXISTS (SELECT 1 FROM removed_members \
                        WHERE org_id = $1 AND user_id = $2 \
                          AND removed_at > now() - make_interval(secs => $3))",
    )
    .bind(org)
    .bind(user)
    .bind(REMOVED_MEMBER_TTL_SECS as f64)
    .fetch_one(db.pool())
    .await?;
    Ok(hit)
}

/// Housekeeping: forget dedupe markers older than `keep_days` and expired
/// removed-member tombstones. Safe at any time, because clean-up is idempotent
/// and a forgotten marker only means a (very) late redelivery writes its audit
/// entry again.
pub async fn sweep(db: &Db, keep_days: i32) -> Result<u64> {
    let events = sqlx::query(
        "DELETE FROM platform_events WHERE received_at < now() - make_interval(days => $1)",
    )
    .bind(keep_days)
    .execute(db.pool())
    .await?
    .rows_affected();
    let tombstones = sqlx::query(
        "DELETE FROM removed_members WHERE removed_at < now() - make_interval(secs => $1)",
    )
    .bind((REMOVED_MEMBER_TTL_SECS * 2) as f64)
    .execute(db.pool())
    .await?
    .rows_affected();
    Ok(events + tombstones)
}

/// Apply one verified delivery.
pub async fn apply(db: &Db, event: &WebhookEvent) -> Result<Outcome> {
    match &event.event {
        LifecycleEvent::OrgDeleted { org_id } => org_deleted(db, event, (*org_id).into()).await,
        LifecycleEvent::TeamDeleted { org_id, team_id } => {
            team_deleted(db, event, (*org_id).into(), (*team_id).into()).await
        }
        LifecycleEvent::MemberRemoved { org_id, user_id } => {
            member_removed(db, event, (*org_id).into(), (*user_id).into()).await
        }
        LifecycleEvent::Unknown { kind } => {
            tracing::warn!(kind = %kind, event_id = %event.id, "ignoring unknown platform event");
            Ok(Outcome::Ignored)
        }
        // `LifecycleEvent` is #[non_exhaustive].
        _ => Ok(Outcome::Ignored),
    }
}

/// Record the delivery; `false` if an earlier one already did.
async fn first_delivery(
    tx: &mut otto_tenant::Tx<'_>,
    event: &WebhookEvent,
    kind: &str,
) -> Result<bool> {
    let org = tx.org();
    let inserted = sqlx::query(
        "INSERT INTO platform_events (event_id, org_id, kind) VALUES ($1, $2, $3) \
         ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(event.id)
    .bind(org)
    .bind(kind)
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(inserted == 1)
}

async fn org_deleted(db: &Db, event: &WebhookEvent, org: OrgId) -> Result<Outcome> {
    // Tombstone first, on its own, so authentication refuses this org from now
    // on -- before, not as part of, the purge.
    sqlx::query("INSERT INTO deleted_orgs (org_id) VALUES ($1) ON CONFLICT (org_id) DO NOTHING")
        .bind(org)
        .execute(db.pool())
        .await?;

    // The org's audit trail is append-only to a pinned transaction (there is no
    // UPDATE policy and DELETE needs `current_org() IS NULL`), so it goes first,
    // on the pool, unpinned. Idempotent, and a failure here aborts before the
    // dedupe marker exists, so the platform's retry runs it again.
    let audit = sqlx::query("DELETE FROM audit_events WHERE org_id = $1")
        .bind(org)
        .execute(db.pool())
        .await?
        .rows_affected();

    let mut tx = db.begin(org).await?;
    let first = first_delivery(&mut tx, event, "org.deleted").await?;

    // Children before parents: messages reference jobs and repos, jobs and
    // tracker bindings reference repos, bindings and the connection index
    // reference connections. Every statement names the org explicitly (guard 1)
    // in addition to the pinned policy (guard 2), and the one table with no
    // policy, `tracker_connection_index`, relies on that predicate alone.
    let mut deleted = serde_json::Map::new();
    for table in [
        "messages",
        "message_cursors",
        "tracker_bindings",
        "tracker_connection_index",
        "tracker_connections",
        "repo_leases",
        "job_dependencies",
        "jobs",
        "repo_remotes",
        "repos",
        "org_counters",
        "usage_outbox",
    ] {
        let n = sqlx::query(&format!("DELETE FROM {table} WHERE org_id = $1"))
            .bind(org)
            .execute(tx.conn())
            .await?
            .rows_affected();
        deleted.insert(table.to_string(), n.into());
    }
    deleted.insert("audit_events".into(), audit.into());
    tx.commit().await?;

    tracing::info!(org = %org, event_id = %event.id, first, ?deleted, "purged a deleted org's data");
    Ok(if first {
        Outcome::Applied {
            detail: deleted.into(),
        }
    } else {
        Outcome::Duplicate
    })
}

async fn team_deleted(db: &Db, event: &WebhookEvent, org: OrgId, team: TeamId) -> Result<Outcome> {
    let mut tx = db.begin(org).await?;
    let first = first_delivery(&mut tx, event, "team.deleted").await?;

    let mut counts = serde_json::Map::new();
    for table in ["repos", "jobs", "messages"] {
        let n: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {table} WHERE org_id = $1 AND team_id = $2"
        ))
        .bind(org)
        .bind(team)
        .fetch_one(tx.conn())
        .await?;
        counts.insert(table.to_string(), n.into());
    }
    let detail: serde_json::Value = counts.into();

    // Deliberately no UPDATE: the dangling id is the tombstone (module docs).
    if first {
        tx.audit(
            Entry::new(action::TEAM_SCOPE_ORPHANED)
                .actor_label("platform")
                .target("team", team.to_string())
                .detail(detail.clone()),
        )
        .await?;
    }
    tx.commit().await?;

    tracing::info!(org = %org, team = %team, ?detail, "team deleted; its scoped rows stay team-scoped");
    Ok(if first {
        Outcome::Applied { detail }
    } else {
        Outcome::Duplicate
    })
}

async fn member_removed(
    db: &Db,
    event: &WebhookEvent,
    org: OrgId,
    user: UserId,
) -> Result<Outcome> {
    // Tombstone first (see `revoked`): from here on this user's cached token is
    // refused, so nothing new is leased or claimed in their name while we clean up.
    sqlx::query(
        "INSERT INTO removed_members (org_id, user_id) VALUES ($1, $2) \
         ON CONFLICT (org_id, user_id) DO UPDATE SET removed_at = now()",
    )
    .bind(org)
    .bind(user)
    .execute(db.pool())
    .await?;

    let mut tx = db.begin(org).await?;
    let first = first_delivery(&mut tx, event, "member.removed").await?;

    let leases = sqlx::query(
        "UPDATE repo_leases SET released_at = now() \
         WHERE org_id = $1 AND holder_user_id = $2 AND released_at IS NULL",
    )
    .bind(org)
    .bind(user)
    .execute(tx.conn())
    .await?
    .rows_affected();

    // The same involuntary re-pend an expired claim gets (see `claim_jobs`):
    // back to `pending`, cancellation bookkeeping cleared so it does not
    // misattribute to whoever claims the job next. Not counted as an attempt.
    let claims = sqlx::query(
        "UPDATE jobs SET status = 'pending', claimed_by = NULL, claimed_by_label = NULL, \
                started_at = NULL, claim_expires_at = NULL, \
                cancel_requested_at = NULL, cancel_requested_by = NULL, cancel_reason = NULL \
         WHERE org_id = $1 AND claimed_by = $2 AND status IN ('in-progress', 'active')",
    )
    .bind(org)
    .bind(user)
    .execute(tx.conn())
    .await?
    .rows_affected();

    let cursors = sqlx::query("DELETE FROM message_cursors WHERE org_id = $1 AND user_id = $2")
        .bind(org)
        .bind(user)
        .execute(tx.conn())
        .await?
        .rows_affected();

    let detail = serde_json::json!({
        "leases_released": leases,
        "claims_released": claims,
        "cursors_dropped": cursors,
    });
    if first {
        tx.audit(
            Entry::new(action::MEMBER_RELEASED)
                .actor_label("platform")
                .target("user", user.to_string())
                .detail(detail.clone()),
        )
        .await?;
    }
    tx.commit().await?;

    tracing::info!(org = %org, user = %user, ?detail, "member removed; released what they held");
    Ok(if first {
        Outcome::Applied { detail }
    } else {
        Outcome::Duplicate
    })
}
