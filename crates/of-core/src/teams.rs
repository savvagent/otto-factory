//! Team deletion, guarded by what otto-factory scopes to a team.
//!
//! Teams themselves (create, list, rename, membership) are identity and live in
//! `otto_core::teams`. Deleting one is the single team operation that cannot:
//! `repos.team_id`, `jobs.team_id`, and `messages.team_id` are all
//! `ON DELETE SET NULL`, and a null `team_id` means *org-wide*, so the platform
//! crate — which has never heard of repos or jobs — would happily delete a team
//! and silently publish everything scoped to it to the whole org.
//!
//! **Call [`delete_team`] here, never `otto_core::teams::TeamsExt::delete_team`.**
//! The latter still compiles anywhere `TeamsExt` is imported, and nothing but
//! this note stops a caller reaching for it; `tests/guards.rs` fails the
//! build if a source file in this workspace does.

use crate::error::{Error, Result};
use otto_core::teams::TeamsExt;
use otto_tenant::ids::TeamId;
use otto_tenant::Tx;

/// Delete a team, refusing while anything is still scoped to it.
///
/// The schema would allow the delete. That is exactly why it is refused here —
/// cascading would silently widen a deliberately narrowed scope, and the admin
/// who deleted a stale team would have published its repos, job history, and
/// messages to the whole org without being told. Jobs and messages are checked
/// too, not just repos: a job keeps its `team_id` after its repo is unassigned,
/// so team-scoped history can outlive the repo link entirely. The error names
/// the repos so the fix is obvious: reassign them, then delete.
pub async fn delete_team(tx: &mut Tx<'_>, id: TeamId) -> Result<()> {
    let org = tx.org();

    let scoped: Vec<String> = sqlx::query_scalar(
        "SELECT slug FROM repos WHERE org_id = $1 AND team_id = $2 ORDER BY slug",
    )
    .bind(org)
    .bind(id)
    .fetch_all(tx.conn())
    .await?;

    if !scoped.is_empty() {
        return Err(Error::TeamInUse {
            repos: scoped.join(", "),
        });
    }

    let jobs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE org_id = $1 AND team_id = $2")
            .bind(org)
            .bind(id)
            .fetch_one(tx.conn())
            .await?;

    if jobs > 0 {
        return Err(Error::TeamInUse {
            repos: format!("{jobs} job(s) still reference this team"),
        });
    }

    let messages: i64 =
        sqlx::query_scalar("SELECT count(*) FROM messages WHERE org_id = $1 AND team_id = $2")
            .bind(org)
            .bind(id)
            .fetch_one(tx.conn())
            .await?;

    if messages > 0 {
        return Err(Error::TeamInUse {
            repos: format!("{messages} message(s) still reference this team"),
        });
    }

    // The existence check and the delete itself are the platform's.
    TeamsExt::delete_team(tx, id).await?;
    Ok(())
}
