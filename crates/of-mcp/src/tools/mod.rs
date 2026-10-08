//! The tool surface.
//!
//! Split by domain so each file stays readable; `rmcp`'s [`ToolRouter`] adds,
//! so the four routers are combined in [`router`].
//!
//! ## Two conventions every tool follows
//!
//! **Naming a repo.** Coordination is anchored on repos, so almost every tool
//! takes an optional `repo` (a registered slug) *or* `remote` (whatever
//! `git remote get-url origin` printed, in any spelling). The agent passes what
//! it has and the server normalizes. When neither resolves, the error lists the
//! registered slugs and points at `register_repo` — it never falls back to a
//! default, because queueing work against a repo nobody meant is a silent,
//! expensive failure and an error is a cheap one.
//!
//! **Output shape.** Every result is a one-field object — `{"job": …}`,
//! `{"jobs": […]}` — carrying `of-core`'s own domain types. See [`out`] for why
//! it is an envelope rather than the bare value, and why the payloads are not
//! mirrored into view structs.

use of_core::ids::{JobId, RepoId};
use of_core::jobs::{Job, JobsExt};
use of_core::repos::ReposExt;
use of_core::repos::{Repo, RepoRef};
use of_core::teams::TeamScope;
use otto_tenant::Tx;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::model::ErrorData;

use crate::server::{Factory, McpResult};

pub mod coord;
pub mod jobs;
pub mod org;
pub mod out;
pub mod repos;

/// Every tool this server exposes.
///
/// `#[tool_router]` generates its constructor as an associated function on the
/// type it is applied to, so these are `Factory::*` rather than module-level
/// functions even though each is written in its own file.
pub fn router() -> ToolRouter<Factory> {
    Factory::repos_router()
        + Factory::jobs_router()
        + Factory::coord_router()
        + Factory::org_router()
}

/// Scope names. Defined once, in [`of_core::scopes`], which is also what the
/// resource registry is populated from at startup.
pub mod scope {
    pub use of_core::scopes::{JOBS_READ, JOBS_WRITE, MESSAGES, REPOS_READ, REPOS_WRITE, TRACKERS};
}

/// Resolve a repo the caller named, or fail with the error that lists what is
/// registered.
///
/// Resolved as `team` may see it: a repo of a team the caller is not on is
/// reported exactly like an unregistered one, and the list of registered slugs
/// leaves it out.
pub(crate) async fn repo_of(
    tx: &mut Tx<'_>,
    team: &TeamScope,
    slug: Option<String>,
    remote: Option<String>,
) -> Result<Repo, ErrorData> {
    tx.resolve_repo_visible(&RepoRef { slug, remote }, team)
        .await
        .mcp()
}

/// Resolve a repo only if the caller named one.
///
/// For the tools where a repo narrows a query rather than anchoring a write —
/// `list_jobs`, `ready`, `stats`, `list_leases`. Passing nothing means "the
/// whole org", which is a legitimate question; passing something unresolvable
/// is still an error, because a filter that silently matched everything would
/// answer a question the caller did not ask.
pub(crate) async fn maybe_repo_of(
    tx: &mut Tx<'_>,
    team: &TeamScope,
    slug: Option<String>,
    remote: Option<String>,
) -> Result<Option<RepoId>, ErrorData> {
    let named = slug.as_deref().is_some_and(|s| !s.trim().is_empty())
        || remote.as_deref().is_some_and(|s| !s.trim().is_empty());

    if !named {
        return Ok(None);
    }
    Ok(Some(repo_of(tx, team, slug, remote).await?.id))
}

/// Fetch a job the caller may see. A job of a team the caller is not on is
/// `job_not_found`, the same as one that does not exist.
pub(crate) async fn visible_job(
    tx: &mut Tx<'_>,
    team: &TeamScope,
    id: &JobId,
) -> Result<Job, ErrorData> {
    let job = tx.get_job(id).await.mcp()?;
    if team.allows(job.team_id) {
        Ok(job)
    } else {
        Err(of_core::Error::JobNotFound(id.clone())).mcp()
    }
}

/// Refuse unless every job named is one the caller may see, so a write cannot
/// reach into a team the caller is not on. Unrestricted callers cost nothing
/// extra; a job that does not exist is left to the write itself to report.
pub(crate) async fn ensure_jobs_visible(
    tx: &mut Tx<'_>,
    team: &TeamScope,
    ids: &[JobId],
) -> Result<(), ErrorData> {
    if team.is_all() {
        return Ok(());
    }
    for id in ids {
        visible_job(tx, team, id).await?;
    }
    Ok(())
}

/// [`ensure_jobs_visible`] for one job.
pub(crate) async fn ensure_job_visible(
    tx: &mut Tx<'_>,
    team: &TeamScope,
    id: &JobId,
) -> Result<(), ErrorData> {
    ensure_jobs_visible(tx, team, std::slice::from_ref(id)).await
}
