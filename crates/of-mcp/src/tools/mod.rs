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
use of_core::repos::{Hold, Repo, RepoRef};
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

/// [`repo_of`] for a tool that writes to the repo or to rows hanging off it:
/// the repo stays held until the transaction ends, so it cannot be moved out of
/// the caller's teams between this check and the write (see
/// `ReposExt::hold_repo_visible`).
pub(crate) async fn repo_for_write(
    tx: &mut Tx<'_>,
    team: &TeamScope,
    slug: Option<String>,
    remote: Option<String>,
    hold: Hold,
) -> Result<Repo, ErrorData> {
    tx.resolve_repo_visible_held(&RepoRef { slug, remote }, team, hold)
        .await
        .mcp()
}

/// [`maybe_repo_of`] for a write; see [`repo_for_write`].
pub(crate) async fn maybe_repo_for_write(
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
    Ok(Some(
        repo_for_write(tx, team, slug, remote, Hold::Shared)
            .await?
            .id,
    ))
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
    tx.get_job_visible(id, team).await.mcp()
}

/// Strip hidden jobs out of a list of jobs the caller was handed by a query that
/// does not know about teams (`ready`, `blocked`): a job is visible only if its
/// own team **and** its repo's team are.
pub(crate) async fn retain_visible(
    tx: &mut Tx<'_>,
    team: &TeamScope,
    jobs: &mut Vec<Job>,
) -> Result<(), ErrorData> {
    if team.is_all() {
        return Ok(());
    }
    let hidden = tx.hidden_repo_ids(team).await.mcp()?;
    jobs.retain(|j| team.allows(j.team_id) && !hidden.contains(&j.repo_id));
    Ok(())
}

/// Keep the ids the caller may see. For job ids that ride along in an answer.
pub(crate) async fn visible_ids(
    tx: &mut Tx<'_>,
    team: &TeamScope,
    ids: Vec<JobId>,
) -> Result<Vec<JobId>, ErrorData> {
    if team.is_all() {
        return Ok(ids);
    }
    tx.visible_job_ids(&ids, team).await.mcp()
}

/// Errors that name a row the caller never asked about (the job a ticket is
/// already linked to, the repo a remote belongs to, the far end of a dependency
/// chain) must not name one they cannot see. The message is withheld and the
/// code kept, so an agent branching on the code is unaffected. Unrestricted
/// callers get the error unchanged.
pub(crate) fn redact_foreign_ids(team: &TeamScope, e: of_core::Error) -> of_core::Error {
    if team.is_all() {
        return e;
    }
    let message = match &e {
        of_core::Error::TicketAlreadyLinked { .. } => {
            "that ticket_ref is already linked to another job in this repo; use a different \
             ticket_ref, or ask an administrator which job holds it"
        }
        of_core::Error::RemoteTaken(..) => {
            "that remote is already registered to another repo; ask an administrator which"
        }
        of_core::Error::DependencyCycle(..) => {
            "that change would make a job depend on itself, directly or through a chain; \
             drop the dependency that closes the loop"
        }
        _ => return e,
    };
    of_core::Error::Redacted {
        code: e.code(),
        message,
    }
}

/// Refuse unless every job named is one the caller may see, so a write cannot
/// reach into a team the caller is not on. Each job's repo stays held until the
/// transaction ends, so an admin cannot move it out of the caller's teams
/// between this check and the write. Unrestricted callers cost nothing extra.
pub(crate) async fn ensure_jobs_visible(
    tx: &mut Tx<'_>,
    team: &TeamScope,
    ids: &[JobId],
) -> Result<(), ErrorData> {
    if team.is_all() {
        return Ok(());
    }
    for id in ids {
        tx.get_job_visible_held(id, team).await.mcp()?;
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
