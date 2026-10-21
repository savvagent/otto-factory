//! Usage rows queued under a meter key that has since been renamed.
//!
//! `0006_meter_otto_tower.sql` rewrote every queued `usage_outbox` row from
//! `otto-factory/jobs_completed` to `otto-tower/jobs_completed`, once. That is
//! not enough on its own: roll back to an image that predates the rename and
//! it keeps enqueuing completions under the old key — the migration is already
//! recorded as applied, so nothing rewrites them — and the next roll forward
//! ships them under a meter the shipper no longer declares. The platform
//! refuses an undeclared meter as `unknown_meter`, which is final: every billed
//! completion the older image recorded would be dead-lettered into
//! `usage_outbox_rejected` instead of billed.
//!
//! So `of-server` runs [`rewrite`] at every startup, after migrations and
//! before the usage shipper starts. It is idempotent: with nothing on an old
//! key it reads the outbox once per renamed key and writes nothing.
//!
//! `usage_outbox_rejected` is deliberately left alone, as in `0006`: those rows
//! are the history of what was refused, under the key they were sent with.
//!
//! ## Scoping
//!
//! This rewrites tenant data with no org of its own, so it follows the one
//! pattern that is safe under every deployment shape (CLAUDE.md, "A migration
//! that touches a tenant table's data"): it finds the orgs that have old-key
//! rows, and for each one pins `app.org_id` to that org **and** names it in the
//! `UPDATE`'s own predicate. The predicate is what scopes the statement when
//! the connecting role bypasses RLS; `app.org_id` is what lets it match any
//! rows at all when RLS applies. Discovering the orgs is an org-less read,
//! which `usage_outbox_tenant_isolation`'s `current_org() IS NULL` branch allows — the
//! same branch the shipper drains through — and `tests/meter_rename.rs` runs
//! the whole thing under both shapes.

use otto_tenant::Db;
use sqlx::PgConnection;
use uuid::Uuid;

/// Every meter key that has been renamed, as `(old, new)`. Each `new` must be
/// one the shipper declares (`of_billing::METER_DECLARATIONS`); a test in
/// `of-billing` holds this list to that.
pub const RENAMED: &[(&str, &str)] =
    &[("otto-factory/jobs_completed", "otto-tower/jobs_completed")];

/// Rewrite every queued usage row still on a retired key to its successor, for
/// every org, in one transaction. Returns how many rows moved.
pub async fn rewrite(db: &Db) -> crate::Result<u64> {
    let mut tx = db.pool().begin().await?;
    let moved = rewrite_in(&mut tx).await?;
    tx.commit().await?;
    Ok(moved)
}

/// [`rewrite`] on a connection the caller already holds, inside whatever
/// transaction it has open — which is how the tests run it as a role RLS
/// applies to. `app.org_id` is set transaction-locally and restored to unset
/// before this returns, so nothing after it in the same transaction inherits an
/// org it did not ask for.
pub async fn rewrite_in(conn: &mut PgConnection) -> crate::Result<u64> {
    let mut moved = 0;
    for (old, new) in RENAMED {
        // Unpinned for the discovery read, or on a shape RLS binds it would
        // see only the org the previous key's loop left pinned.
        unpin(conn).await?;
        let orgs: Vec<Uuid> =
            sqlx::query_scalar("SELECT DISTINCT org_id FROM usage_outbox WHERE meter = $1")
                .bind(old)
                .fetch_all(&mut *conn)
                .await?;
        for org in orgs {
            sqlx::query("SELECT set_config('app.org_id', $1, true)")
                .bind(org.to_string())
                .execute(&mut *conn)
                .await?;
            moved +=
                sqlx::query("UPDATE usage_outbox SET meter = $3 WHERE org_id = $1 AND meter = $2")
                    .bind(org)
                    .bind(old)
                    .bind(new)
                    .execute(&mut *conn)
                    .await?
                    .rows_affected();
        }
    }
    unpin(conn).await?;
    Ok(moved)
}

/// Restore `app.org_id` to unset for the rest of the transaction.
async fn unpin(conn: &mut PgConnection) -> crate::Result<()> {
    sqlx::query("SELECT set_config('app.org_id', '', true)")
        .execute(&mut *conn)
        .await?;
    Ok(())
}
