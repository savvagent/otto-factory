//! Domain errors.
//!
//! These are written to be readable by an LLM tool caller that has never seen
//! the docs: a failure says what went wrong, what the valid options were, and
//! what to call next. `NotFound` naming the org, `UnknownRepo` listing the
//! registered slugs, and `LeaseHeld` naming the holder all exist for that
//! reason — a bare "not found" makes an agent guess, and a guessing agent
//! retries wrongly.

use crate::ids::{JobId, OrgId, UserId};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("job {0} not found")]
    JobNotFound(JobId),

    #[error("repo not found: {0}")]
    RepoNotFound(String),

    /// The agent's working directory could not be resolved to a registered repo.
    /// Deliberately never falls back to a default repo: silently queueing work
    /// against the wrong repository is worse than failing.
    #[error(
        "could not resolve a repo from {attempted}. Registered repos: {known}. \
         Register this one with register_repo, or pass an explicit repo slug."
    )]
    RepoUnresolved { attempted: String, known: String },

    #[error("repo slug {0:?} is already registered in this org")]
    RepoSlugTaken(String),

    #[error("remote {0:?} is already registered to repo {1:?}")]
    RemoteTaken(String, String),

    #[error("job {job} is {actual}, not {expected}")]
    WrongStatus {
        job: JobId,
        actual: String,
        expected: String,
    },

    /// `reason` is pre-rendered per call site rather than built from a single
    /// fixed template, because the three cases `ensure_claim_held` raises this
    /// for are not interchangeable: a different account holds the claim, an
    /// expired claim has not yet been picked up by anyone, or a different
    /// *instance* of your own account has reclaimed it (the claim-generation
    /// fence, savvagent/otto-factory#103) — and "not you" is simply false in
    /// the last case, since the account genuinely is you. Every branch still
    /// ends with the same guidance: `get_job` is for the caller's own
    /// orientation only, never a source for a fresh `expected_attempts` to
    /// retry with — reading `attempts` back from `get_job` and feeding it into
    /// a retry is the exact bypass this fence exists to prevent — and
    /// `claim_jobs` is the only way to actually take the job back.
    #[error("job {job} {reason}")]
    AlreadyClaimed { job: JobId, reason: String },

    #[error(
        "ticket {ticket_ref} is already linked to job {job} — unlink it there first, \
         or use a different ticket_ref"
    )]
    TicketAlreadyLinked { ticket_ref: String, job: JobId },

    #[error("dependency cycle: {0} would depend on itself through {1}")]
    DependencyCycle(JobId, JobId),

    #[error("{resource} of this repo is leased by {holder} until {expires_at}")]
    LeaseHeld {
        resource: String,
        holder: String,
        expires_at: chrono::DateTime<chrono::Utc>,
    },

    #[error("lease {0} is not held by you")]
    LeaseNotHeld(String),

    #[error("org {0} not found")]
    OrgNotFound(OrgId),

    #[error("no team {slug:?} in this org. Teams: {known}")]
    TeamNotFound { slug: String, known: String },

    #[error("team slug {0:?} is already taken in this org")]
    TeamSlugTaken(String),

    /// Refused rather than cascaded: a null `team_id` means org-wide, so
    /// deleting a team that still owns repos would publish them to the whole
    /// org without saying so.
    #[error(
        "this team still owns repos ({repos}). Reassign or unassign them first, \
         then delete the team — deleting it now would make them visible org-wide."
    )]
    TeamInUse { repos: String },

    #[error("user {0} is not a member of this org")]
    NotAMember(UserId),

    #[error("{email} is already a member of this org, as {role}")]
    AlreadyAMember { email: String, role: String },

    /// Unknown, already accepted, and expired collapse into one answer — which
    /// of the three it was is not something the holder of a failing token
    /// should be able to determine.
    #[error("this invitation is no longer valid. Ask an admin of the org to send a new one.")]
    InviteInvalid,

    #[error(
        "this invitation was sent to {invited}, but you are signed in as {signed_in_as}. \
         Sign in as {invited} to accept it."
    )]
    InviteWrongAccount {
        invited: String,
        signed_in_as: String,
    },

    #[error("{0}")]
    Config(String),

    #[error("{0}")]
    Crypto(String),

    #[error("{0}")]
    Invalid(String),

    /// A `SAVEPOINT`-guarded unique-violation recovery re-queried for the
    /// concurrent winner's row and found nothing — the winner's row vanished
    /// between the violation and the re-query (e.g. a concurrent delete). This
    /// is a transient server-side race, not a problem with the caller's
    /// request: unlike `Invalid`, retrying the identical call can plausibly
    /// succeed once the row settles. See `create_from_ticket`, `link_ticket`,
    /// `Tx::add_job`, and `Tx::send_message` for the four sites that raise it.
    #[error("{0}")]
    RaceLost(String),

    #[error(
        "idempotency_key {key:?} was already used for a different {tool} call in this \
         organization. Use a new key for a different request, or omit idempotency_key to \
         always create a new one."
    )]
    IdempotencyKeyConflict { key: String, tool: &'static str },

    /// The database cannot enforce tenant isolation as configured. Raised only
    /// by `Db::verify_tenant_isolation` at startup, never by a request: by the
    /// time a tool call is in flight it is far too late to discover that one
    /// org can read another's rows. Carries the specific findings because
    /// "isolation is broken" without naming the table or the role is not
    /// something an operator can act on at 3am.
    #[error(
        "the database cannot enforce tenant isolation, so one org could read \
         another's data: {problems}"
    )]
    IsolationNotEnforced { problems: String },

    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl Error {
    /// A stable, machine-readable code for the MCP error envelope. Agents
    /// branch on this; humans read the message.
    pub fn code(&self) -> &'static str {
        match self {
            Error::JobNotFound(_) => "job_not_found",
            Error::RepoNotFound(_) => "repo_not_found",
            Error::RepoUnresolved { .. } => "repo_unresolved",
            Error::RepoSlugTaken(_) => "repo_slug_taken",
            Error::RemoteTaken(..) => "remote_taken",
            Error::WrongStatus { .. } => "wrong_status",
            Error::AlreadyClaimed { .. } => "already_claimed",
            Error::TicketAlreadyLinked { .. } => "ticket_already_linked",
            Error::DependencyCycle(..) => "dependency_cycle",
            Error::LeaseHeld { .. } => "lease_held",
            Error::LeaseNotHeld(_) => "lease_not_held",
            Error::OrgNotFound(_) => "org_not_found",
            Error::TeamNotFound { .. } => "team_not_found",
            Error::TeamSlugTaken(_) => "team_slug_taken",
            Error::TeamInUse { .. } => "team_in_use",
            Error::NotAMember(_) => "not_a_member",
            Error::AlreadyAMember { .. } => "already_a_member",
            Error::InviteInvalid => "invite_invalid",
            Error::InviteWrongAccount { .. } => "invite_wrong_account",
            Error::Config(_) => "internal_error",
            Error::Crypto(_) => "internal_error",
            Error::Invalid(_) => "invalid_argument",
            Error::RaceLost(_) => "race_lost",
            Error::IdempotencyKeyConflict { .. } => "idempotency_key_conflict",
            Error::IsolationNotEnforced { .. } => "isolation_not_enforced",
            Error::Db(_) => "internal_error",
        }
    }

    /// Whether retrying the identical call could plausibly succeed. `LeaseHeld`
    /// is retriable (the lease expires); `DependencyCycle` is not (the request
    /// is wrong). Agents use this to decide between backing off and rethinking.
    ///
    /// `AlreadyClaimed` is **not** retriable, unlike `LeaseHeld`, even though
    /// both describe "someone else has this right now": a lease's holder can
    /// let it lapse without acting, so waiting and retrying the identical
    /// `acquire_lease` call can succeed on its own. `AlreadyClaimed` today is
    /// raised only by `ensure_claim_held` — a caller that is not (or is no
    /// longer) a job's claim holder — and retrying `complete_job`/`fail_job`/
    /// `cancel_job`/`renew_claim` with the same arguments can never succeed;
    /// the only way forward is a different call (`claim_jobs`, or `get_job`
    /// to see who holds it now). Telling an agent this is retriable would
    /// have it busy-loop a call that is structurally doomed.
    ///
    /// `RaceLost` is retriable for the same reason `Db` is: both describe a
    /// condition of the server's transaction, not the caller's arguments, and
    /// an identical retry can land in a different, successful outcome once the
    /// concurrent write that caused it has finished settling.
    pub fn retriable(&self) -> bool {
        matches!(
            self,
            Error::LeaseHeld { .. } | Error::Db(_) | Error::RaceLost(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Retrying the identical call with the identical key will fail
    /// identically every time — the caller must change the key or the
    /// payload, not back off and try again.
    #[test]
    fn idempotency_key_conflict_is_not_retriable() {
        let e = Error::IdempotencyKeyConflict {
            key: "k".into(),
            tool: "add_job",
        };
        assert!(!e.retriable());
        assert_eq!(e.code(), "idempotency_key_conflict");
    }

    #[test]
    fn race_lost_is_retriable() {
        let e = Error::RaceLost("x".into());
        assert!(e.retriable());
        assert_eq!(e.code(), "race_lost");
    }
}
