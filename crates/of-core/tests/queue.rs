//! Queue behaviour: job lifecycle, atomic claiming, dependencies, leases, and
//! the message channel.

mod common;

use common::{db, job, tenant, Tenant};
use of_core::ids::JobId;
use of_core::jobs::{JobFilter, Status, DEFAULT_CLAIM_TTL_SECS, MAX_CLAIM_TTL_SECS};
use of_core::messages::{InboxQuery, NewMessage};
use of_core::orgs::Role;
use of_core::repos::{NewRepo, RepoPatch, RepoRef};
use sqlx::PgPool;

#[sqlx::test]
async fn job_ids_are_dense_and_per_org(pool: PgPool) {
    let db = db(pool);
    let a = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let b = tenant(&db, "globex", "git@github.com:globex/api.git").await;

    let mut tx = db.begin(a.org).await.unwrap();
    let a1 = tx.add_job(job(&a, "first")).await.unwrap();
    let a2 = tx.add_job(job(&a, "second")).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin(b.org).await.unwrap();
    let b1 = tx.add_job(job(&b, "first")).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(a1.id.as_str(), "job-1");
    assert_eq!(a2.id.as_str(), "job-2");
    // Each org starts its own count — this is what stops one customer inferring
    // another's volume from an id.
    assert_eq!(b1.id.as_str(), "job-1");
}

#[sqlx::test]
async fn lifecycle_pending_to_completed(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "ship it")).await.unwrap();
    assert_eq!(j.status, Status::Pending);
    assert_eq!(j.attempts, 0);

    let claimed = tx
        .claim_jobs(
            std::slice::from_ref(&j.id),
            t.user,
            Some("claude-code@laptop"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(claimed[0].status, Status::InProgress);
    assert_eq!(claimed[0].attempts, 1);
    assert_eq!(
        claimed[0].claimed_by_label.as_deref(),
        Some("claude-code@laptop")
    );

    let done = tx
        .complete_job(&j.id, t.user, Some("merged in #12"), None)
        .await
        .unwrap();
    assert_eq!(done.status, Status::Completed);
    assert_eq!(done.result.as_deref(), Some("merged in #12"));
    assert!(done.completed_at.is_some());
    tx.commit().await.unwrap();
}

#[sqlx::test]
async fn completing_an_unclaimed_job_is_refused(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "not started")).await.unwrap();
    let err = tx
        .complete_job(&j.id, t.user, Some("lying"), None)
        .await
        .unwrap_err();
    tx.rollback().await.unwrap();
    assert_eq!(err.code(), "wrong_status");
}

#[sqlx::test]
async fn repend_preserves_attempts(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "flaky")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();
    tx.fail_job(&j.id, t.user, Some("CI red"), None)
        .await
        .unwrap();

    let again = tx.repend_job(&j.id).await.unwrap();
    assert_eq!(again.status, Status::Pending);
    assert_eq!(again.attempts, 1, "attempts must survive a repend");
    assert!(again.error.is_none());
    assert!(again.claimed_by.is_none());
    assert!(
        again.claim_expires_at.is_none(),
        "a repended job must not carry its previous claim's expiry forward"
    );

    // The second claim increments to 2, so a job that keeps coming back is
    // visible as such rather than looking fresh every time.
    let reclaimed = tx
        .claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();
    assert_eq!(reclaimed[0].attempts, 2);
    tx.commit().await.unwrap();
}

/// A batch claim is all-or-nothing. A partial claim would leave an agent
/// believing it owns work it does not — the exact race this queue prevents.
#[sqlx::test]
async fn claim_is_all_or_nothing(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let a = tx.add_job(job(&t, "a")).await.unwrap();
    let b = tx.add_job(job(&t, "b")).await.unwrap();
    tx.commit().await.unwrap();

    // Someone else takes `b` first.
    let mut tx = db.begin(t.org).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&b.id), t.user, Some("other"), None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // The batch containing it must fail entirely, leaving `a` claimable.
    let mut tx = db.begin(t.org).await.unwrap();
    let err = tx
        .claim_jobs(&[a.id.clone(), b.id.clone()], t.user, Some("me"), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "wrong_status");
    tx.rollback().await.unwrap();

    let mut tx = db.begin(t.org).await.unwrap();
    let still = tx.get_job(&a.id).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        still.status,
        Status::Pending,
        "a partial claim leaked through"
    );
}

#[sqlx::test]
async fn claiming_a_missing_job_names_it(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let a = tx.add_job(job(&t, "a")).await.unwrap();
    let err = tx
        .claim_jobs(&[a.id.clone(), JobId::from("job-999")], t.user, None, None)
        .await
        .unwrap_err();
    tx.rollback().await.unwrap();

    assert_eq!(err.code(), "job_not_found");
    assert!(
        err.to_string().contains("job-999"),
        "the error must name the missing job so the agent can fix its request: {err}"
    );
}

#[sqlx::test]
async fn dependencies_gate_ready_and_claim(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let first = tx.add_job(job(&t, "migration")).await.unwrap();
    let second = tx.add_job(job(&t, "use the new column")).await.unwrap();
    tx.set_dependencies(&second.id, std::slice::from_ref(&first.id), &[])
        .await
        .unwrap();

    let ready: Vec<String> = tx
        .ready(None, None)
        .await
        .unwrap()
        .into_iter()
        .map(|j| j.id.0)
        .collect();
    let blocked: Vec<String> = tx
        .blocked(None)
        .await
        .unwrap()
        .into_iter()
        .map(|j| j.id.0)
        .collect();
    assert_eq!(ready, vec![first.id.0.clone()]);
    assert_eq!(blocked, vec![second.id.0.clone()]);
    tx.commit().await.unwrap();

    // Claiming a blocked job is refused even when asked for directly. Rolled
    // back on its own so the refusal cannot be confused with the setup being
    // discarded.
    let mut tx = db.begin(t.org).await.unwrap();
    let err = tx
        .claim_jobs(std::slice::from_ref(&second.id), t.user, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "wrong_status");
    tx.rollback().await.unwrap();

    // Once the dependency completes, the dependent becomes ready.
    let mut tx = db.begin(t.org).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&first.id), t.user, None, None)
        .await
        .unwrap();
    tx.complete_job(&first.id, t.user, Some("done"), None)
        .await
        .unwrap();
    let ready: Vec<String> = tx
        .ready(None, None)
        .await
        .unwrap()
        .into_iter()
        .map(|j| j.id.0)
        .collect();
    assert_eq!(ready, vec![second.id.0.clone()]);
    tx.commit().await.unwrap();
}

#[sqlx::test]
async fn dependency_cycles_are_rejected(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let a = tx.add_job(job(&t, "a")).await.unwrap();
    let b = tx.add_job(job(&t, "b")).await.unwrap();
    let c = tx.add_job(job(&t, "c")).await.unwrap();

    // Direct self-dependency.
    assert_eq!(
        tx.set_dependencies(&a.id, std::slice::from_ref(&a.id), &[])
            .await
            .unwrap_err()
            .code(),
        "dependency_cycle"
    );

    // Indirect: a → b → c → a.
    tx.set_dependencies(&a.id, std::slice::from_ref(&b.id), &[])
        .await
        .unwrap();
    tx.set_dependencies(&b.id, std::slice::from_ref(&c.id), &[])
        .await
        .unwrap();
    let err = tx
        .set_dependencies(&c.id, std::slice::from_ref(&a.id), &[])
        .await
        .unwrap_err();
    assert_eq!(err.code(), "dependency_cycle");
    tx.rollback().await.unwrap();
}

#[sqlx::test]
async fn stats_counts_blocked_separately(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let a = tx.add_job(job(&t, "a")).await.unwrap();
    let b = tx.add_job(job(&t, "b")).await.unwrap();
    tx.set_dependencies(&b.id, std::slice::from_ref(&a.id), &[])
        .await
        .unwrap();
    let c = tx.add_job(job(&t, "c")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&c.id), t.user, None, None)
        .await
        .unwrap();

    let s = tx.stats(None).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(s.total, 3);
    assert_eq!(s.pending, 2);
    assert_eq!(s.in_progress, 1);
    // `blocked` is a subset of `pending`, not a separate status.
    assert_eq!(s.blocked, 1);
}

#[sqlx::test]
async fn stats_counts_active_separately_from_in_progress(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let claimed_only = tx.add_job(job(&t, "claimed")).await.unwrap();
    let claimed_and_active = tx.add_job(job(&t, "active")).await.unwrap();
    tx.claim_jobs(
        &[claimed_only.id.clone(), claimed_and_active.id.clone()],
        t.user,
        None,
        None,
    )
    .await
    .unwrap();
    tx.activate_job(&claimed_and_active.id).await.unwrap();

    let s = tx.stats(None).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(s.total, 2);
    assert_eq!(s.in_progress, 1);
    assert_eq!(s.active, 1);
}

/// The org-wide branch of `stats` reads `completed`/`failed` off
/// `orgs.jobs_completed_total`/`jobs_failed_total` instead of scanning every
/// job the org has ever run — this pins those counters to every way a job
/// can enter or leave a terminal status, so they never drift from what a
/// full rescan would say.
#[sqlx::test]
async fn org_wide_stats_terminal_counters_track_every_transition(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let completed = tx.add_job(job(&t, "will complete")).await.unwrap();
    let failed = tx.add_job(job(&t, "will fail")).await.unwrap();
    let repended = tx.add_job(job(&t, "will be repended")).await.unwrap();
    let deleted = tx.add_job(job(&t, "will be deleted")).await.unwrap();
    tx.claim_jobs(
        &[
            completed.id.clone(),
            failed.id.clone(),
            repended.id.clone(),
            deleted.id.clone(),
        ],
        t.user,
        None,
        None,
    )
    .await
    .unwrap();
    tx.complete_job(&completed.id, t.user, None, None)
        .await
        .unwrap();
    tx.fail_job(&failed.id, t.user, Some("boom"), None)
        .await
        .unwrap();
    tx.complete_job(&repended.id, t.user, None, None)
        .await
        .unwrap();
    tx.complete_job(&deleted.id, t.user, None, None)
        .await
        .unwrap();

    let s = tx.stats(None).await.unwrap();
    assert_eq!(s.completed, 3, "completed, repended, and deleted");
    assert_eq!(s.failed, 1);

    // Repending a completed job takes it out of `completed` again.
    tx.repend_job(&repended.id).await.unwrap();
    // Deleting a completed job removes it from the count entirely, not just
    // from the table — a stale row would leave the counter overstated
    // forever.
    tx.delete_job(&deleted.id).await.unwrap();

    let s = tx.stats(None).await.unwrap();
    assert_eq!(s.completed, 1, "only the untouched one is left");
    assert_eq!(s.failed, 1);
    assert_eq!(s.pending, 1, "the repended job");
    assert_eq!(s.total, 3, "completed + failed + the repended job");

    // Repending an in-progress (never-terminal) job must not touch either
    // counter — there was nothing to take back out.
    let never_finished = tx
        .add_job(job(&t, "claimed, never finished"))
        .await
        .unwrap();
    tx.claim_jobs(std::slice::from_ref(&never_finished.id), t.user, None, None)
        .await
        .unwrap();
    tx.repend_job(&never_finished.id).await.unwrap();
    let s = tx.stats(None).await.unwrap();
    assert_eq!(s.completed, 1);
    assert_eq!(s.failed, 1);

    tx.commit().await.unwrap();
}

/// A repo-scoped read is the exact, full-scan query, unaffected by the
/// org-wide counters above — and it and the org-wide read must still agree.
#[sqlx::test]
async fn repo_scoped_stats_match_the_org_wide_counters(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let a = tx.add_job(job(&t, "a")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&a.id), t.user, None, None)
        .await
        .unwrap();
    tx.complete_job(&a.id, t.user, None, None).await.unwrap();

    let org_wide = tx.stats(None).await.unwrap();
    let repo_scoped = tx.stats(Some(t.repo)).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(org_wide.completed, repo_scoped.completed);
    assert_eq!(org_wide.total, repo_scoped.total);
}

#[sqlx::test]
async fn activating_a_claimed_job_moves_it_to_active(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "ship it")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();

    let activated = tx.activate_job(&j.id).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(activated.status, Status::Active);
    assert!(!activated.status.is_terminal());
}

#[sqlx::test]
async fn activating_a_job_that_was_never_claimed_is_refused(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "still pending")).await.unwrap();
    let err = tx.activate_job(&j.id).await.unwrap_err();
    tx.rollback().await.unwrap();

    assert_eq!(err.code(), "wrong_status");
}

#[sqlx::test]
async fn completing_or_failing_an_active_job_still_works(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();

    let will_complete = tx.add_job(job(&t, "will complete")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&will_complete.id), t.user, None, None)
        .await
        .unwrap();
    tx.activate_job(&will_complete.id).await.unwrap();
    let completed = tx
        .complete_job(&will_complete.id, t.user, Some("done"), None)
        .await
        .unwrap();
    assert_eq!(completed.status, Status::Completed);

    let will_fail = tx.add_job(job(&t, "will fail")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&will_fail.id), t.user, None, None)
        .await
        .unwrap();
    tx.activate_job(&will_fail.id).await.unwrap();
    let failed = tx
        .fail_job(&will_fail.id, t.user, Some("nope"), None)
        .await
        .unwrap();
    assert_eq!(failed.status, Status::Failed);

    // The direct in-progress -> completed path (no activate_job call) is
    // unchanged — activation is optional, never a mandatory gate.
    let direct = tx.add_job(job(&t, "direct")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&direct.id), t.user, None, None)
        .await
        .unwrap();
    let direct_done = tx
        .complete_job(&direct.id, t.user, Some("done"), None)
        .await
        .unwrap();
    assert_eq!(direct_done.status, Status::Completed);

    tx.commit().await.unwrap();
}

// ------------------------------------------------------------------- cancel

/// Cancelling a job that has not started yet has nobody to notify, so
/// `request_cancel` finalizes it immediately instead of leaving it in limbo
/// waiting for a holder that will never call `cancel_job`.
#[sqlx::test]
async fn request_cancel_on_pending_finalizes_immediately(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "never started")).await.unwrap();
    let cancelled = tx
        .request_cancel(&j.id, t.user, Some("no longer needed"))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(cancelled.status, Status::Cancelled);
    assert!(cancelled.status.is_terminal());
    assert!(cancelled.completed_at.is_some());
    assert!(cancelled.cancel_requested_at.is_some());
    assert_eq!(cancelled.cancel_requested_by, Some(t.user));
    assert_eq!(cancelled.cancel_reason.as_deref(), Some("no longer needed"));
}

/// Cancelling a claimed job only raises the flag — the holder is the only one
/// who can say when it has actually stopped, via `cancel_job`. A second call
/// re-stamps the request rather than being refused, so an agent that lost
/// track of an earlier request can simply ask again.
#[sqlx::test]
async fn request_cancel_on_in_progress_only_sets_the_flag(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "long running")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();

    let flagged = tx
        .request_cancel(&j.id, t.user, Some("first reason"))
        .await
        .unwrap();
    assert_eq!(flagged.status, Status::InProgress);
    assert!(flagged.cancel_requested_at.is_some());
    assert_eq!(flagged.cancel_requested_by, Some(t.user));
    assert_eq!(flagged.cancel_reason.as_deref(), Some("first reason"));

    let restamped = tx
        .request_cancel(&j.id, t.user, Some("second reason"))
        .await
        .unwrap();
    assert_eq!(restamped.status, Status::InProgress);
    assert_eq!(restamped.cancel_reason.as_deref(), Some("second reason"));
    assert!(restamped.cancel_requested_at >= flagged.cancel_requested_at);

    tx.commit().await.unwrap();
}

/// `active` is a refinement of `in-progress`, and a cancellation request must
/// reach it the same way.
#[sqlx::test]
async fn request_cancel_on_active_only_sets_the_flag(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "actively working")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();
    tx.activate_job(&j.id).await.unwrap();

    let flagged = tx.request_cancel(&j.id, t.user, None).await.unwrap();
    assert_eq!(flagged.status, Status::Active);
    assert!(flagged.cancel_requested_at.is_some());
    tx.commit().await.unwrap();
}

/// A job already at a terminal status has nothing left to cancel.
#[sqlx::test]
async fn request_cancel_on_a_terminal_job_is_refused(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();

    let completed = tx.add_job(job(&t, "will complete")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&completed.id), t.user, None, None)
        .await
        .unwrap();
    tx.complete_job(&completed.id, t.user, Some("done"), None)
        .await
        .unwrap();
    let err = tx
        .request_cancel(&completed.id, t.user, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "wrong_status");
    assert!(
        err.to_string().contains("pending, in-progress, or active"),
        "must name the valid starting states: {err}"
    );

    let failed = tx.add_job(job(&t, "will fail")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&failed.id), t.user, None, None)
        .await
        .unwrap();
    tx.fail_job(&failed.id, t.user, Some("nope"), None)
        .await
        .unwrap();
    let err = tx
        .request_cancel(&failed.id, t.user, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "wrong_status");

    let cancelled = tx.add_job(job(&t, "will be cancelled")).await.unwrap();
    tx.request_cancel(&cancelled.id, t.user, None)
        .await
        .unwrap();
    let err = tx
        .request_cancel(&cancelled.id, t.user, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "wrong_status");

    tx.rollback().await.unwrap();
}

/// `cancel_job` is how a holder that received a `request_cancel` reports that
/// it actually stopped.
#[sqlx::test]
async fn cancel_job_after_request_cancel_succeeds(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "long running")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();
    tx.request_cancel(&j.id, t.user, Some("stop please"))
        .await
        .unwrap();

    let cancelled = tx
        .cancel_job(&j.id, t.user, Some("stopped as requested"), None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(cancelled.status, Status::Cancelled);
    assert!(cancelled.completed_at.is_some());
    assert_eq!(cancelled.error.as_deref(), Some("stopped as requested"));
}

/// Stopping unilaterally, with no `request_cancel` on file, is `fail_job`'s
/// job, not `cancel_job`'s — the distinction is what tells an observer whether
/// the stop was in response to a request or the holder's own decision.
#[sqlx::test]
async fn cancel_job_with_no_request_on_file_is_invalid(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "long running")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();

    let err = tx
        .cancel_job(&j.id, t.user, Some("giving up"), None)
        .await
        .unwrap_err();
    tx.rollback().await.unwrap();

    assert_eq!(err.code(), "invalid_argument");
}

/// `cancel_job` only makes sense once a job has been claimed — a pending job
/// with a cancellation request already finalized inside `request_cancel`
/// itself.
#[sqlx::test]
async fn cancel_job_on_a_pending_job_is_refused(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "still pending")).await.unwrap();
    let err = tx.cancel_job(&j.id, t.user, None, None).await.unwrap_err();
    tx.rollback().await.unwrap();

    assert_eq!(err.code(), "wrong_status");
    assert!(
        err.to_string().contains("in-progress or active"),
        "must name the valid starting states: {err}"
    );
}

/// `cancel_job` accepts no note at all — an agent complying with a stop
/// request has nothing more to say. `error` must end up `None`, not some
/// invented default string.
#[sqlx::test]
async fn cancel_job_with_no_note_leaves_error_unset(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "long running")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();
    tx.request_cancel(&j.id, t.user, Some("stop please"))
        .await
        .unwrap();

    let cancelled = tx.cancel_job(&j.id, t.user, None, None).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(cancelled.status, Status::Cancelled);
    assert!(cancelled.error.is_none());
}

/// Cancelling a pending job leaves anything that depends on it permanently
/// blocked, the same as `fail_job` — `ready`/`blocked` only treat a dependency
/// as satisfied once it is `completed`. Unblocking it is a job for
/// `repend_job` or `set_dependencies`, not something `request_cancel` does on
/// its own.
#[sqlx::test]
async fn request_cancel_on_pending_leaves_dependents_blocked(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let dependency = tx.add_job(job(&t, "migration")).await.unwrap();
    let dependent = tx.add_job(job(&t, "use the new column")).await.unwrap();
    tx.set_dependencies(&dependent.id, std::slice::from_ref(&dependency.id), &[])
        .await
        .unwrap();

    let cancelled = tx
        .request_cancel(&dependency.id, t.user, Some("no longer needed"))
        .await
        .unwrap();
    assert_eq!(cancelled.status, Status::Cancelled);

    let blocked: Vec<String> = tx
        .blocked(None)
        .await
        .unwrap()
        .into_iter()
        .map(|j| j.id.0)
        .collect();
    let ready: Vec<String> = tx
        .ready(None, None)
        .await
        .unwrap()
        .into_iter()
        .map(|j| j.id.0)
        .collect();
    tx.commit().await.unwrap();

    assert_eq!(blocked, vec![dependent.id.0.clone()]);
    assert!(!ready.contains(&dependent.id.0));
}

/// `cancel_job` finalizes a claim exactly like `complete_job`/`fail_job` do,
/// so it gets the identical claimer fence: a stale holder confirming a stop
/// on a job someone else has since reclaimed must not be able to terminate
/// the reclaimer's live attempt out from under it.
#[sqlx::test]
async fn a_stale_holder_cannot_cancel_after_someone_else_reclaims(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let user_b = second_user(&db, &t, "second@acme.test").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "stranded")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, Some("agent-a"), None)
        .await
        .unwrap();
    expire_claim(&mut tx, t.org, &j.id).await;
    tx.claim_jobs(std::slice::from_ref(&j.id), user_b, Some("agent-b"), None)
        .await
        .unwrap();
    tx.request_cancel(&j.id, user_b, Some("stop"))
        .await
        .unwrap();

    let err = tx
        .cancel_job(&j.id, t.user, Some("stale confirmation"), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "already_claimed");

    let cancelled = tx
        .cancel_job(&j.id, user_b, Some("stopped as requested"), None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(cancelled.status, Status::Cancelled);
}

/// A repended cancelled job gets a clean slate: no stale cancellation request
/// should follow it into its next attempt.
#[sqlx::test]
async fn repend_clears_cancellation_fields(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx
        .add_job(job(&t, "will be cancelled and retried"))
        .await
        .unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();
    tx.request_cancel(&j.id, t.user, Some("stop"))
        .await
        .unwrap();
    tx.cancel_job(&j.id, t.user, Some("stopped"), None)
        .await
        .unwrap();

    let repended = tx.repend_job(&j.id).await.unwrap();
    assert_eq!(repended.status, Status::Pending);
    assert!(repended.cancel_requested_at.is_none());
    assert!(repended.cancel_requested_by.is_none());
    assert!(repended.cancel_reason.is_none());

    // The fresh attempt has no request on file, so a stop now must go
    // through fail_job, not cancel_job.
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();
    let err = tx.cancel_job(&j.id, t.user, None, None).await.unwrap_err();
    assert_eq!(err.code(), "invalid_argument");
    tx.commit().await.unwrap();
}

#[sqlx::test]
async fn stats_reports_cancelled_and_the_total_reconciles(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();

    let pending = tx.add_job(job(&t, "pending")).await.unwrap();
    let _ = pending;

    let in_progress = tx.add_job(job(&t, "in progress")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&in_progress.id), t.user, None, None)
        .await
        .unwrap();

    let active = tx.add_job(job(&t, "active")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&active.id), t.user, None, None)
        .await
        .unwrap();
    tx.activate_job(&active.id).await.unwrap();

    let completed = tx.add_job(job(&t, "completed")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&completed.id), t.user, None, None)
        .await
        .unwrap();
    tx.complete_job(&completed.id, t.user, Some("done"), None)
        .await
        .unwrap();

    let failed = tx.add_job(job(&t, "failed")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&failed.id), t.user, None, None)
        .await
        .unwrap();
    tx.fail_job(&failed.id, t.user, Some("nope"), None)
        .await
        .unwrap();

    let cancelled = tx.add_job(job(&t, "cancelled")).await.unwrap();
    tx.request_cancel(&cancelled.id, t.user, None)
        .await
        .unwrap();

    let s = tx.stats(None).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(s.cancelled, 1);
    assert_eq!(s.total, 6);
    assert_eq!(
        s.pending + s.in_progress + s.active + s.completed + s.failed + s.cancelled,
        s.total
    );
}

// ------------------------------------------------------------- claim expiry
//
// savvagent/otto-factory#65: a claimed job never expired, and complete_job
// never checked the claimer. These tests exercise the fix — reaping a
// stranded claim, TTL clamping, and the fencing that stops a stale holder
// from finalizing work someone else has since reclaimed.

/// Add a second member to `t`'s org, for the fencing tests below that need two
/// distinct claimants.
async fn second_user(db: &of_core::Db, t: &Tenant, email: &str) -> of_core::ids::UserId {
    let user = db.upsert_user(email, Some("Second")).await.unwrap();
    db.add_member(t.org, user.id, Role::Member).await.unwrap();
    user.id
}

/// Force a job's claim into the past, bypassing the public API — there is no
/// legitimate way to create this state quickly, so the test reaches past `Tx`
/// with a raw statement on the same connection/transaction.
async fn expire_claim(tx: &mut of_core::db::Tx<'_>, org: of_core::ids::OrgId, id: &JobId) {
    sqlx::query(
        "UPDATE jobs SET claim_expires_at = now() - interval '1 second' \
         WHERE org_id = $1 AND id = $2",
    )
    .bind(org)
    .bind(id)
    .execute(tx.conn())
    .await
    .unwrap();
}

#[sqlx::test]
async fn claim_jobs_defaults_ttl_to_the_default_constant(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "default ttl")).await.unwrap();
    let before = chrono::Utc::now();
    let claimed = tx
        .claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let expires_at = claimed[0]
        .claim_expires_at
        .expect("a freshly claimed job must carry a claim expiry");
    let expected = before + chrono::Duration::seconds(DEFAULT_CLAIM_TTL_SECS);
    let delta = (expires_at - expected).num_seconds().abs();
    assert!(
        delta < 5,
        "expected claim_expires_at near {expected}, got {expires_at}"
    );
}

#[sqlx::test]
async fn claim_jobs_honors_an_explicit_ttl(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "explicit ttl")).await.unwrap();
    let before = chrono::Utc::now();
    let claimed = tx
        .claim_jobs(std::slice::from_ref(&j.id), t.user, None, Some(120))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let expires_at = claimed[0].claim_expires_at.expect("must set an expiry");
    let expected = before + chrono::Duration::seconds(120);
    let delta = (expires_at - expected).num_seconds().abs();
    assert!(
        delta < 5,
        "expected claim_expires_at near {expected}, got {expires_at}"
    );
}

#[sqlx::test]
async fn claim_jobs_clamps_an_out_of_range_ttl(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let too_low = tx.add_job(job(&t, "too low")).await.unwrap();
    let too_high = tx.add_job(job(&t, "too high")).await.unwrap();

    let before = chrono::Utc::now();
    let low = tx
        .claim_jobs(std::slice::from_ref(&too_low.id), t.user, None, Some(1))
        .await
        .unwrap();
    let high = tx
        .claim_jobs(
            std::slice::from_ref(&too_high.id),
            t.user,
            None,
            Some(MAX_CLAIM_TTL_SECS * 10),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Clamped, not rejected: a below-minimum or above-maximum TTL still
    // succeeds, just at the boundary value.
    let low_expected = before + chrono::Duration::seconds(60);
    let low_delta = (low[0].claim_expires_at.unwrap() - low_expected)
        .num_seconds()
        .abs();
    assert!(low_delta < 5, "TTL below 60s must clamp up to 60s");

    let high_expected = before + chrono::Duration::seconds(MAX_CLAIM_TTL_SECS);
    let high_delta = (high[0].claim_expires_at.unwrap() - high_expected)
        .num_seconds()
        .abs();
    assert!(
        high_delta < 5,
        "TTL above MAX_CLAIM_TTL_SECS must clamp down to it"
    );
}

/// The core of GH#65: an expired claim makes the job claimable again through
/// the normal `ready()` view, with no sweeper and no admin action needed.
#[sqlx::test]
async fn an_expired_claim_reappears_in_ready(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "stranded")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();

    let ready_before: Vec<String> = tx
        .ready(None, None)
        .await
        .unwrap()
        .into_iter()
        .map(|j| j.id.0)
        .collect();
    assert!(
        !ready_before.contains(&j.id.0),
        "a live claim must not be claimable"
    );

    expire_claim(&mut tx, t.org, &j.id).await;

    let ready_after: Vec<String> = tx
        .ready(None, None)
        .await
        .unwrap()
        .into_iter()
        .map(|j| j.id.0)
        .collect();
    assert!(
        ready_after.contains(&j.id.0),
        "an expired claim must reappear in ready()"
    );

    // §1 of the design spec's central claim: ready() *reinterprets*, it never
    // *mutates*. The row itself must still say in-progress, still under its
    // original holder, until something actually calls claim_jobs on it.
    let untouched = tx.get_job(&j.id).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(untouched.status, Status::InProgress);
    assert_eq!(untouched.claimed_by, Some(t.user));
}

/// The fencing half of GH#65 as it plays out through `claim_jobs` itself: once
/// a claim has lapsed, a *different* user can take it, and the reclaim is
/// visible as a new attempt, not a silent continuation of the old one.
#[sqlx::test]
async fn an_expired_claim_can_be_reclaimed_by_someone_else(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let user_b = second_user(&db, &t, "second@acme.test").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "stranded")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, Some("agent-a"), None)
        .await
        .unwrap();
    expire_claim(&mut tx, t.org, &j.id).await;

    let reclaimed = tx
        .claim_jobs(std::slice::from_ref(&j.id), user_b, Some("agent-b"), None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(reclaimed[0].claimed_by, Some(user_b));
    assert_eq!(reclaimed[0].claimed_by_label.as_deref(), Some("agent-b"));
    assert_eq!(
        reclaimed[0].attempts, 2,
        "one attempt from the original claim, one from the reclaim"
    );
}

/// Every test above claims a job and leaves it `in-progress`, but the reap
/// UPDATE, `ready()`'s widened predicate, and `ensure_claim_held` all
/// explicitly branch on `status IN ('in-progress', 'active')` — a long task
/// that got as far as `activate_job` before its agent died is exactly the
/// realistic case this covers, and it must reap, reappear, and fence
/// identically to the `in-progress` case.
#[sqlx::test]
async fn an_expired_active_claim_reaps_reappears_and_fences_like_in_progress(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let user_b = second_user(&db, &t, "second@acme.test").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx
        .add_job(job(&t, "long task, actually started"))
        .await
        .unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, Some("agent-a"), None)
        .await
        .unwrap();
    let active = tx.activate_job(&j.id).await.unwrap();
    assert_eq!(active.status, Status::Active);
    expire_claim(&mut tx, t.org, &j.id).await;

    let ready: Vec<String> = tx
        .ready(None, None)
        .await
        .unwrap()
        .into_iter()
        .map(|j| j.id.0)
        .collect();
    assert!(
        ready.contains(&j.id.0),
        "an expired active claim must reappear in ready() exactly like an expired in-progress one"
    );

    let reclaimed = tx
        .claim_jobs(std::slice::from_ref(&j.id), user_b, Some("agent-b"), None)
        .await
        .unwrap();
    assert_eq!(reclaimed[0].claimed_by, Some(user_b));
    assert_eq!(reclaimed[0].status, Status::InProgress);

    let err = tx
        .complete_job(&j.id, t.user, Some("stale"), None)
        .await
        .unwrap_err();
    tx.commit().await.unwrap();
    assert_eq!(err.code(), "already_claimed");
}

/// A reap is an involuntary repend, and must clear the same fields
/// `repend_job` does: a cancellation request aimed at the agent that died
/// must not follow the job to whoever reclaims it, or the new holder inherits
/// someone else's "please stop" and any org member can immediately
/// `cancel_job` an attempt nobody asked to stop.
#[sqlx::test]
async fn reaping_an_expired_claim_clears_its_cancellation_request(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let user_b = second_user(&db, &t, "second@acme.test").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx
        .add_job(job(&t, "stranded, cancel requested"))
        .await
        .unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();
    tx.request_cancel(&j.id, t.user, Some("no longer needed"))
        .await
        .unwrap();
    expire_claim(&mut tx, t.org, &j.id).await;

    let reclaimed = tx
        .claim_jobs(std::slice::from_ref(&j.id), user_b, None, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(
        reclaimed[0].cancel_requested_at.is_none(),
        "a stale cancellation request must not survive a reap onto a new holder"
    );
    assert!(reclaimed[0].cancel_requested_by.is_none());
    assert!(reclaimed[0].cancel_reason.is_none());
}

#[sqlx::test]
async fn renew_claim_by_the_holder_extends_the_expiry(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "long task")).await.unwrap();
    let claimed = tx
        .claim_jobs(std::slice::from_ref(&j.id), t.user, None, Some(60))
        .await
        .unwrap();
    let before = claimed[0].claim_expires_at.unwrap();

    let renewed = tx
        .renew_claim(&j.id, t.user, Some(3600), None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(
        renewed.claim_expires_at.unwrap() > before,
        "renew_claim must push the expiry forward"
    );
}

#[sqlx::test]
async fn renew_claim_by_someone_else_is_refused(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let user_b = second_user(&db, &t, "second@acme.test").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "long task")).await.unwrap();
    tx.claim_jobs(
        std::slice::from_ref(&j.id),
        t.user,
        Some("holder-label"),
        None,
    )
    .await
    .unwrap();

    let err = tx.renew_claim(&j.id, user_b, None, None).await.unwrap_err();
    tx.rollback().await.unwrap();

    assert_eq!(err.code(), "already_claimed");
    assert!(
        err.to_string().contains("holder-label"),
        "must name the actual holder: {err}"
    );
}

#[sqlx::test]
async fn renew_claim_on_a_pending_job_is_wrong_status(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "still pending")).await.unwrap();
    let err = tx.renew_claim(&j.id, t.user, None, None).await.unwrap_err();
    tx.rollback().await.unwrap();

    assert_eq!(err.code(), "wrong_status");
    assert!(
        err.to_string().contains("in-progress or active"),
        "must name the valid starting states: {err}"
    );
}

#[sqlx::test]
async fn renew_claim_on_a_terminal_job_is_wrong_status(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();

    let completed = tx.add_job(job(&t, "will complete")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&completed.id), t.user, None, None)
        .await
        .unwrap();
    tx.complete_job(&completed.id, t.user, Some("done"), None)
        .await
        .unwrap();
    let err = tx
        .renew_claim(&completed.id, t.user, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "wrong_status");
    assert!(err.to_string().contains("in-progress or active"));

    let failed = tx.add_job(job(&t, "will fail")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&failed.id), t.user, None, None)
        .await
        .unwrap();
    tx.fail_job(&failed.id, t.user, Some("nope"), None)
        .await
        .unwrap();
    let err = tx
        .renew_claim(&failed.id, t.user, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "wrong_status");

    let cancelled = tx.add_job(job(&t, "will be cancelled")).await.unwrap();
    tx.request_cancel(&cancelled.id, t.user, None)
        .await
        .unwrap();
    let err = tx
        .renew_claim(&cancelled.id, t.user, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "wrong_status");

    tx.rollback().await.unwrap();
}

/// The key GH#65 scenario: A claims a job, its claim lapses, B reclaims it —
/// A's finalize call must now fail naming B, and B's own finalize call must
/// still succeed. Proving both halves in one test is what shows the
/// interleaving is actually closed, not just that each half works in
/// isolation.
#[sqlx::test]
async fn a_stale_holder_cannot_finalize_after_someone_else_reclaims(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let user_b = second_user(&db, &t, "second@acme.test").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "contested")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, Some("agent-a"), None)
        .await
        .unwrap();
    expire_claim(&mut tx, t.org, &j.id).await;
    tx.claim_jobs(std::slice::from_ref(&j.id), user_b, Some("agent-b"), None)
        .await
        .unwrap();

    let err = tx
        .complete_job(&j.id, t.user, Some("i finished, honest"), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "already_claimed");
    assert!(
        err.to_string().contains("agent-b"),
        "must name the actual current holder: {err}"
    );

    let done = tx
        .complete_job(&j.id, user_b, Some("actually finished"), None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(done.status, Status::Completed);
    assert_eq!(done.result.as_deref(), Some("actually finished"));
}

/// savvagent/otto-factory#103: `ensure_claim_held` fences by account
/// (`claimed_by: UserId`), which is not enough when two *instances* of the
/// same account race — one process crashes, its claim lapses, a second
/// process under the identical account reclaims and starts real work, and
/// the first process wakes up and tries to finalize/renew using the claim
/// generation it originally held. `expected_attempts` closes this: a caller
/// that supplies the `attempts` value it actually holds gets refused (naming
/// the reclaiming instance's label) instead of silently clobbering it; the
/// reclaiming instance's own calls, with the current `attempts` value,
/// succeed. One job per finalizer under test, since `complete_job`/
/// `fail_job`/`cancel_job` are each terminal.
#[sqlx::test]
async fn stale_generation_cannot_finalize_or_renew_after_same_account_reclaims(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    async fn claim_then_reclaim(
        tx: &mut of_core::db::Tx<'_>,
        org: of_core::ids::OrgId,
        user: of_core::ids::UserId,
        j: &JobId,
    ) -> (i32, i32) {
        let claimed = tx
            .claim_jobs(std::slice::from_ref(j), user, Some("agent-a"), None)
            .await
            .unwrap();
        let stale_attempts = claimed[0].attempts;
        expire_claim(tx, org, j).await;
        let reclaimed = tx
            .claim_jobs(std::slice::from_ref(j), user, Some("agent-a-prime"), None)
            .await
            .unwrap();
        (stale_attempts, reclaimed[0].attempts)
    }

    let mut tx = db.begin(t.org).await.unwrap();

    // complete_job
    let j = tx.add_job(job(&t, "contested completion")).await.unwrap();
    let (stale, current) = claim_then_reclaim(&mut tx, t.org, t.user, &j.id).await;
    let err = tx
        .complete_job(&j.id, t.user, Some("stale"), Some(stale))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "already_claimed");
    assert!(
        err.to_string().contains("agent-a-prime"),
        "must name the reclaiming instance: {err}"
    );
    let done = tx
        .complete_job(&j.id, t.user, Some("actual"), Some(current))
        .await
        .unwrap();
    assert_eq!(done.status, Status::Completed);

    // fail_job
    let j = tx.add_job(job(&t, "contested failure")).await.unwrap();
    let (stale, current) = claim_then_reclaim(&mut tx, t.org, t.user, &j.id).await;
    let err = tx
        .fail_job(&j.id, t.user, Some("stale"), Some(stale))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "already_claimed");
    let failed = tx
        .fail_job(&j.id, t.user, Some("actual"), Some(current))
        .await
        .unwrap();
    assert_eq!(failed.status, Status::Failed);

    // cancel_job
    let j = tx.add_job(job(&t, "contested cancellation")).await.unwrap();
    let (stale, current) = claim_then_reclaim(&mut tx, t.org, t.user, &j.id).await;
    tx.request_cancel(&j.id, t.user, Some("stop"))
        .await
        .unwrap();
    let err = tx
        .cancel_job(&j.id, t.user, Some("stale"), Some(stale))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "already_claimed");
    let cancelled = tx
        .cancel_job(&j.id, t.user, Some("actual"), Some(current))
        .await
        .unwrap();
    assert_eq!(cancelled.status, Status::Cancelled);

    // renew_claim
    let j = tx.add_job(job(&t, "contested renewal")).await.unwrap();
    let (stale, current) = claim_then_reclaim(&mut tx, t.org, t.user, &j.id).await;
    let err = tx
        .renew_claim(&j.id, t.user, None, Some(stale))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "already_claimed");
    let renewed = tx
        .renew_claim(&j.id, t.user, None, Some(current))
        .await
        .unwrap();
    assert!(renewed.claim_expires_at.is_some());

    tx.commit().await.unwrap();
}

/// The additive-compatibility half of the fix above: a caller that never
/// learns about `expected_attempts` (omits it, i.e. `None`) must see
/// bit-for-bit today's behavior — the account-only fence still lets a stale
/// same-account caller finalize over a reclaiming instance, exactly as
/// before this change. This is not a regression to fix later; it is the
/// explicit price of an optional, additive argument (Non-Negotiable Rule 6):
/// the guard is available to every caller, not forced on any of them.
#[sqlx::test]
async fn expected_attempts_omitted_preserves_todays_behavior(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "contested, unguarded")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, Some("agent-a"), None)
        .await
        .unwrap();
    expire_claim(&mut tx, t.org, &j.id).await;
    tx.claim_jobs(
        std::slice::from_ref(&j.id),
        t.user,
        Some("agent-a-prime"),
        None,
    )
    .await
    .unwrap();

    let done = tx
        .complete_job(&j.id, t.user, Some("clobbered, as before"), None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(done.status, Status::Completed);
}

/// The server-side half of the #103 security review: `ensure_claim_held`
/// refuses `complete_job`/`fail_job`/`cancel_job`/`renew_claim` once the
/// *caller's own* claim has expired and nobody else has reclaimed it yet —
/// closing GH#65's race for every caller, not just ones that learned to pass
/// `expected_attempts`. Before this check, an expired-but-unreclaimed claim
/// (`ready()` and `claim_jobs` already treat it as available; there is no
/// background reaper) would still finalize or renew successfully for the
/// original holder. One job per finalizer, since each is terminal, plus a
/// final `claim_jobs` proving the job is still genuinely reclaimable — the
/// fence closes finalize/renew against the stale holder, not availability
/// through the normal path.
#[sqlx::test]
async fn a_holder_cannot_act_on_their_own_expired_unreclaimed_claim(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();

    // complete_job
    let j = tx
        .add_job(job(&t, "expired, unreclaimed, complete"))
        .await
        .unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();
    expire_claim(&mut tx, t.org, &j.id).await;
    let err = tx
        .complete_job(&j.id, t.user, Some("late"), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "already_claimed");
    assert!(
        err.to_string().contains("expired"),
        "must say the claim expired, not just that someone else holds it: {err}"
    );

    // fail_job
    let j = tx
        .add_job(job(&t, "expired, unreclaimed, fail"))
        .await
        .unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();
    expire_claim(&mut tx, t.org, &j.id).await;
    let err = tx
        .fail_job(&j.id, t.user, Some("late"), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "already_claimed");

    // cancel_job — a cancellation request on file first, so this proves the
    // expiry check runs (and refuses) before cancel_job's own
    // no-request-on-file check ever gets a chance to fire.
    let j = tx
        .add_job(job(&t, "expired, unreclaimed, cancel"))
        .await
        .unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();
    tx.request_cancel(&j.id, t.user, Some("stop"))
        .await
        .unwrap();
    expire_claim(&mut tx, t.org, &j.id).await;
    let err = tx
        .cancel_job(&j.id, t.user, Some("late"), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "already_claimed");

    // renew_claim
    let j = tx
        .add_job(job(&t, "expired, unreclaimed, renew"))
        .await
        .unwrap();
    tx.claim_jobs(std::slice::from_ref(&j.id), t.user, None, None)
        .await
        .unwrap();
    expire_claim(&mut tx, t.org, &j.id).await;
    let err = tx.renew_claim(&j.id, t.user, None, None).await.unwrap_err();
    assert_eq!(err.code(), "already_claimed");

    // The job is still genuinely reclaimable through the normal path — the
    // fence above blocks the stale holder's finalize/renew, not availability.
    let reclaimed = tx
        .claim_jobs(
            std::slice::from_ref(&j.id),
            t.user,
            Some("agent-a-prime"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        reclaimed[0].attempts, 2,
        "one attempt from the original claim, one from the reclaim"
    );
    tx.commit().await.unwrap();
}

/// savvagent/otto-factory#103's premise correction: unlike jobs,
/// `repo_leases::acquire_lease` already mints a **new** lease `id` every
/// time a resource is reclaimed after its previous lease expired (the reap
/// step marks the old row `released_at`, then a fresh row is inserted) —
/// `renew_lease`/`release_lease` are keyed on that per-acquisition `id`, not
/// on `(org, repo, resource)` alone, so a stale holder's old `id` already
/// fails `lease_not_held` the instant a new acquisition has happened. This
/// locks that finding in as a permanent regression test: a future change to
/// `acquire_lease`'s reclaim path that accidentally reused the old lease
/// `id` (removing this implicit generation fence) would be caught here, not
/// rediscovered as a live incident. This is why `docs/specs/
/// 2026-09-11-claim-generation-fencing-design.md` §4 decides `repo_leases`
/// does not need the analogous `expected_attempts`-style fix jobs got above.
#[sqlx::test]
async fn lease_reclaim_after_expiry_mints_a_new_id_fencing_the_stale_holder(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let first = tx
        .acquire_lease(
            t.repo,
            "branch:main",
            t.user,
            Some("agent-a"),
            None,
            Some(60),
        )
        .await
        .unwrap();

    sqlx::query(
        "UPDATE repo_leases SET expires_at = now() - interval '1 second' \
         WHERE org_id = $1 AND id = $2",
    )
    .bind(t.org)
    .bind(first.id)
    .execute(tx.conn())
    .await
    .unwrap();

    let second = tx
        .acquire_lease(
            t.repo,
            "branch:main",
            t.user,
            Some("agent-a-prime"),
            None,
            Some(900),
        )
        .await
        .unwrap();

    assert_ne!(
        first.id, second.id,
        "reclaim-after-expiry must mint a new lease id, not reuse the old one"
    );

    let err = tx.renew_lease(first.id, t.user, None).await.unwrap_err();
    assert_eq!(err.code(), "lease_not_held");
    let err = tx.release_lease(first.id, t.user).await.unwrap_err();
    assert_eq!(err.code(), "lease_not_held");

    tx.renew_lease(second.id, t.user, None).await.unwrap();
    tx.commit().await.unwrap();
}

/// A caller with no claim at all on a job that was never claimed hits the
/// status check first — `Error::WrongStatus`, not `Error::AlreadyClaimed`.
/// `ensure_claim_held` checks status before ownership, and a pending job
/// fails on status regardless of who is asking.
#[sqlx::test]
async fn finalizing_a_never_claimed_job_is_wrong_status_not_already_claimed(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let j = tx.add_job(job(&t, "never touched")).await.unwrap();
    let err = tx
        .complete_job(&j.id, t.user, Some("nice try"), None)
        .await
        .unwrap_err();
    tx.rollback().await.unwrap();

    assert_eq!(err.code(), "wrong_status");
}

// NOTE: `ensure_claim_held`'s `unwrap_or_else(|| "nobody".into())` fallback —
// reached only if a row were `in-progress`/`active` with *both*
// `claimed_by` and `claimed_by_label` absent — is deliberately left
// uncovered. A job only ever reaches `in-progress`/`active` through
// `claim_jobs`, which always sets both columns in the same statement, so
// there is no call sequence that reaches that branch. This is a documented
// gap, not an oversight.

// --------------------------------------------------------------------- repos

#[sqlx::test]
async fn repo_resolves_from_any_remote_spelling(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    for spelling in [
        "git@github.com:acme/api.git",
        "https://github.com/acme/api",
        "https://github.com/acme/api.git",
        "ssh://git@github.com:22/acme/api.git",
    ] {
        let mut tx = db.begin(t.org).await.unwrap();
        let got = tx
            .resolve_repo(&RepoRef {
                remote: Some(spelling.into()),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("{spelling} failed to resolve: {e}"));
        tx.commit().await.unwrap();
        assert_eq!(got.id, t.repo, "{spelling}");
    }
}

/// An unresolvable repo must error with the registered slugs, never fall back to
/// some other repo — queueing work against the wrong repository is a silent,
/// expensive failure.
#[sqlx::test]
async fn unresolvable_repo_errors_helpfully(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let err = tx
        .resolve_repo(&RepoRef {
            remote: Some("git@github.com:acme/unknown.git".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    tx.commit().await.unwrap();

    assert_eq!(err.code(), "repo_unresolved");
    let msg = err.to_string();
    assert!(msg.contains("api"), "must list registered slugs: {msg}");
    assert!(
        msg.contains("register_repo"),
        "must say what to call next: {msg}"
    );

    // A mistyped slug is the commonest way to get here, and it has to be just
    // as informative as a remote that matched nothing. It is the one the caller
    // can actually fix from the answer.
    let mut tx = db.begin(t.org).await.unwrap();
    let err = tx
        .resolve_repo(&RepoRef {
            slug: Some("apo".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    tx.commit().await.unwrap();

    assert_eq!(err.code(), "repo_unresolved");
    let msg = err.to_string();
    assert!(msg.contains("apo"), "must repeat what was asked for: {msg}");
    assert!(msg.contains("api"), "must list registered slugs: {msg}");

    // And a slug that misses stops there. An agent typically passes its
    // checkout's remote alongside whatever repo it was told to use; falling
    // back to the remote on a typo would quietly queue the work against the
    // repository the agent happens to be sitting in.
    let mut tx = db.begin(t.org).await.unwrap();
    let err = tx
        .resolve_repo(&RepoRef {
            slug: Some("apo".into()),
            remote: Some("git@github.com:acme/api.git".into()),
        })
        .await
        .unwrap_err();
    tx.commit().await.unwrap();

    assert_eq!(err.code(), "repo_unresolved");
    assert!(
        err.to_string().contains("apo"),
        "an explicit slug wins over a remote, even when it misses: {err}"
    );
}

#[sqlx::test]
async fn a_remote_cannot_be_claimed_by_two_repos(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let err = tx
        .register_repo(NewRepo {
            slug: "api-mirror".into(),
            // Same repo, different spelling — must be caught after normalization.
            remotes: vec!["https://github.com/acme/api".into()],
            ..Default::default()
        })
        .await
        .unwrap_err();
    tx.rollback().await.unwrap();
    assert_eq!(err.code(), "remote_taken");
}

// -------------------------------------------------------------------- leases

#[sqlx::test]
async fn a_second_agent_cannot_take_a_held_lease(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let other = db.upsert_user("other@acme.test", None).await.unwrap();
    db.add_member(t.org, other.id, of_core::orgs::Role::Member)
        .await
        .unwrap();

    let mut tx = db.begin(t.org).await.unwrap();
    tx.acquire_lease(t.repo, "main", t.user, Some("agent-a"), None, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin(t.org).await.unwrap();
    let err = tx
        .acquire_lease(t.repo, "main", other.id, Some("agent-b"), None, None)
        .await
        .unwrap_err();
    tx.rollback().await.unwrap();

    assert_eq!(err.code(), "lease_held");
    assert!(
        err.to_string().contains("agent-a"),
        "the error must name the holder so the agent can go elsewhere or ask: {err}"
    );
}

/// Different branches of the same repo are independent — two agents on separate
/// worktrees is the normal case, not a collision.
#[sqlx::test]
async fn leases_are_per_branch(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let other = db.upsert_user("other@acme.test", None).await.unwrap();
    db.add_member(t.org, other.id, of_core::orgs::Role::Member)
        .await
        .unwrap();

    let mut tx = db.begin(t.org).await.unwrap();
    tx.acquire_lease(t.repo, "main", t.user, Some("a"), None, None)
        .await
        .unwrap();
    tx.acquire_lease(t.repo, "feature/x", other.id, Some("b"), None, None)
        .await
        .unwrap();
    let live = tx.list_leases(Some(t.repo)).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(live.len(), 2);
}

/// A lease is on a resource, not necessarily a branch — a staging slot or a
/// migration lock is as leasable as `branch:main`. Two different resources on
/// the same repo don't collide, and the "held" error names whatever resource
/// string was actually passed, not a hardcoded "branch".
#[sqlx::test]
async fn leases_are_per_resource_not_just_branch(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let other = db.upsert_user("other@acme.test", None).await.unwrap();
    db.add_member(t.org, other.id, of_core::orgs::Role::Member)
        .await
        .unwrap();

    let mut tx = db.begin(t.org).await.unwrap();
    tx.acquire_lease(t.repo, "branch:main", t.user, Some("a"), None, None)
        .await
        .unwrap();
    tx.acquire_lease(t.repo, "deploy:staging", other.id, Some("b"), None, None)
        .await
        .unwrap();
    let live = tx.list_leases(Some(t.repo)).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(live.len(), 2);

    let third = db.upsert_user("third@acme.test", None).await.unwrap();
    db.add_member(t.org, third.id, of_core::orgs::Role::Member)
        .await
        .unwrap();

    let mut tx = db.begin(t.org).await.unwrap();
    let err = tx
        .acquire_lease(t.repo, "deploy:staging", third.id, Some("c"), None, None)
        .await
        .unwrap_err();
    tx.rollback().await.unwrap();

    assert_eq!(err.code(), "lease_held");
    assert!(
        err.to_string().contains("deploy:staging"),
        "the error must name the resource, not a hardcoded \"branch\": {err}"
    );
}

/// A free-form resource has no natural length ceiling the way a branch name
/// used to imply, and a released lease row is kept for history rather than
/// deleted — without a cap, the column becomes unbounded free storage, and an
/// oversized value that only fails at the database's btree index limit would
/// surface to a caller as a retriable internal error instead of a clear,
/// non-retriable refusal.
#[sqlx::test]
async fn an_oversized_resource_is_refused_before_it_reaches_the_database(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let too_long = "x".repeat(of_core::leases::MAX_RESOURCE_LEN + 1);
    let mut tx = db.begin(t.org).await.unwrap();
    let err = tx
        .acquire_lease(t.repo, &too_long, t.user, None, None, None)
        .await
        .unwrap_err();
    tx.rollback().await.unwrap();

    assert_eq!(err.code(), "invalid_argument");
    assert!(
        !err.retriable(),
        "an oversized resource is never fixed by retrying"
    );
}

/// Re-acquiring your own lease renews it rather than failing, so an agent that
/// lost track of its own state converges instead of deadlocking against itself.
#[sqlx::test]
async fn reacquiring_your_own_lease_renews_it(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let first = tx
        .acquire_lease(t.repo, "main", t.user, Some("a"), None, Some(120))
        .await
        .unwrap();
    let second = tx
        .acquire_lease(t.repo, "main", t.user, Some("a"), None, Some(3600))
        .await
        .unwrap();
    let live = tx.list_leases(Some(t.repo)).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        first.id, second.id,
        "renewal must not create a second lease"
    );
    assert!(second.expires_at > first.expires_at);
    assert_eq!(live.len(), 1);
}

#[sqlx::test]
async fn only_the_holder_can_release(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let other = db.upsert_user("other@acme.test", None).await.unwrap();
    db.add_member(t.org, other.id, of_core::orgs::Role::Member)
        .await
        .unwrap();

    let mut tx = db.begin(t.org).await.unwrap();
    let lease = tx
        .acquire_lease(t.repo, "main", t.user, Some("a"), None, None)
        .await
        .unwrap();
    assert_eq!(
        tx.release_lease(lease.id, other.id)
            .await
            .unwrap_err()
            .code(),
        "lease_not_held"
    );
    tx.release_lease(lease.id, t.user).await.unwrap();
    assert!(tx.list_leases(None).await.unwrap().is_empty());
    tx.commit().await.unwrap();
}

// ------------------------------------------------------------------ messages

#[sqlx::test]
async fn directed_messages_are_private_within_the_org(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let bob = db.upsert_user("bob@acme.test", None).await.unwrap();
    let eve = db.upsert_user("eve@acme.test", None).await.unwrap();
    for u in [bob.id, eve.id] {
        db.add_member(t.org, u, of_core::orgs::Role::Member)
            .await
            .unwrap();
    }

    let mut tx = db.begin(t.org).await.unwrap();
    tx.send_message(
        t.user,
        NewMessage {
            body: "for bob only".into(),
            recipient_user_id: Some(bob.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    tx.send_message(
        t.user,
        NewMessage {
            body: "everyone".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin(t.org).await.unwrap();
    let bobs: Vec<String> = tx
        .inbox(bob.id, &InboxQuery::default())
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.body)
        .collect();
    let eves: Vec<String> = tx
        .inbox(eve.id, &InboxQuery::default())
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.body)
        .collect();
    tx.commit().await.unwrap();

    assert_eq!(bobs, vec!["for bob only", "everyone"]);
    assert_eq!(eves, vec!["everyone"], "eve read a directed message");
}

/// Your own messages are never unread to you, and the cursor cannot be advanced
/// past the newest message — otherwise a careless ack would suppress messages
/// that have not been written yet.
#[sqlx::test]
async fn unread_excludes_self_and_ack_is_clamped(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let bob = db.upsert_user("bob@acme.test", None).await.unwrap();
    db.add_member(t.org, bob.id, of_core::orgs::Role::Member)
        .await
        .unwrap();

    let mut tx = db.begin(t.org).await.unwrap();
    tx.send_message(
        t.user,
        NewMessage {
            body: "mine".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let theirs = tx
        .send_message(
            bob.id,
            NewMessage {
                body: "theirs".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(
        tx.unread_count(t.user).await.unwrap(),
        1,
        "own message counted as unread"
    );
    assert_eq!(tx.unread_count(bob.id).await.unwrap(), 1);

    let landed = tx.ack_messages(t.user, i64::MAX).await.unwrap();
    assert_eq!(
        landed, theirs.id,
        "ack was not clamped to the newest message"
    );
    assert_eq!(tx.unread_count(t.user).await.unwrap(), 0);

    // A later message is still unread despite the over-large ack.
    tx.send_message(
        bob.id,
        NewMessage {
            body: "later".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(tx.unread_count(t.user).await.unwrap(), 1);
    tx.commit().await.unwrap();
}

#[sqlx::test]
async fn oversized_and_empty_bodies_are_refused(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    assert_eq!(
        tx.send_message(
            t.user,
            NewMessage {
                body: "   ".into(),
                ..Default::default()
            }
        )
        .await
        .unwrap_err()
        .code(),
        "invalid_argument"
    );
    let huge = "x".repeat(of_core::messages::MAX_BODY_LEN + 1);
    assert_eq!(
        tx.send_message(
            t.user,
            NewMessage {
                body: huge,
                ..Default::default()
            }
        )
        .await
        .unwrap_err()
        .code(),
        "invalid_argument"
    );
    tx.rollback().await.unwrap();
}

#[sqlx::test]
async fn list_jobs_filters_by_repo(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let web = tx
        .register_repo(NewRepo {
            slug: "web".into(),
            remotes: vec!["git@github.com:acme/web.git".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    tx.add_job(job(&t, "api work")).await.unwrap();
    tx.add_job(of_core::jobs::NewJob {
        repo_id: web.id,
        title: "web work".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let only_web = tx
        .list_jobs(&JobFilter {
            repo_id: Some(web.id),
            ..Default::default()
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(only_web.len(), 1);
    assert_eq!(only_web[0].title, "web work");
}

/// `agent_type` is a routing hint, not access control: filtering by it must
/// still surface unrouted work, because that is anyone's to take.
#[sqlx::test]
async fn agent_type_filter_includes_matches_and_unrouted_jobs(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let claude = tx
        .add_job(of_core::jobs::NewJob {
            repo_id: t.repo,
            title: "claude work".into(),
            agent_type: Some("claude-code".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let copilot = tx
        .add_job(of_core::jobs::NewJob {
            repo_id: t.repo,
            title: "copilot work".into(),
            agent_type: Some("copilot-cli".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let unrouted = tx
        .add_job(of_core::jobs::NewJob {
            repo_id: t.repo,
            title: "unrouted work".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    // Omitting the filter sees everything, unchanged.
    let everything = tx.list_jobs(&JobFilter::default()).await.unwrap();
    assert_eq!(everything.len(), 3);

    // Filtering matches the exact agent_type plus every unrouted job.
    let for_claude = tx
        .list_jobs(&JobFilter {
            agent_type: Some("claude-code".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let for_claude_ids: Vec<_> = for_claude.iter().map(|j| j.id.as_str()).collect();
    assert_eq!(for_claude_ids.len(), 2);
    assert!(for_claude_ids.contains(&claude.id.as_str()));
    assert!(for_claude_ids.contains(&unrouted.id.as_str()));
    assert!(!for_claude_ids.contains(&copilot.id.as_str()));

    // `ready` applies the same rule.
    let ready_for_claude = tx.ready(None, Some("claude-code")).await.unwrap();
    let ready_ids: Vec<_> = ready_for_claude.iter().map(|j| j.id.as_str()).collect();
    assert_eq!(ready_ids.len(), 2);
    assert!(ready_ids.contains(&claude.id.as_str()));
    assert!(ready_ids.contains(&unrouted.id.as_str()));

    tx.commit().await.unwrap();
}

/// A patch touches only what it names. A caller written against three fields
/// must not blank the two it has never heard of.
#[sqlx::test]
async fn update_repo_leaves_unnamed_fields_alone(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let before = tx.get_repo(t.repo).await.unwrap().unwrap();
    assert_eq!(before.default_branch, "main");

    let after = tx
        .update_repo(
            t.repo,
            RepoPatch {
                default_branch: Some("trunk".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(after.default_branch, "trunk");
    assert_eq!(after.name, before.name, "name was not in the patch");
    assert_eq!(after.slug, before.slug);
    assert!(after.active);
}

/// Added remotes resolve afterwards, and a remote already claimed by a sibling
/// repo is refused by name rather than silently re-pointed.
#[sqlx::test]
async fn update_repo_adds_remotes_and_refuses_stolen_ones(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let web = tx
        .register_repo(NewRepo {
            slug: "web".into(),
            remotes: vec!["git@github.com:acme/web.git".into()],
            ..Default::default()
        })
        .await
        .unwrap();

    tx.update_repo(
        t.repo,
        RepoPatch {
            add_remotes: vec!["https://gitlab.com/acme/api-mirror".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let resolved = tx
        .resolve_repo(&RepoRef {
            remote: Some("git@gitlab.com:acme/api-mirror.git".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(resolved.id, t.repo, "the mirror must reach the same repo");

    let err = tx
        .update_repo(
            web.id,
            RepoPatch {
                add_remotes: vec!["git@github.com:acme/api.git".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("api"),
        "the error must name the repo already holding the remote: {err}"
    );
    let _ = tx.rollback().await;
}

// ------------------------------------------------------------ idempotency

#[sqlx::test]
async fn add_job_replays_with_the_same_idempotency_key(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let mut new = job(&t, "wire up health endpoint");
    new.idempotency_key = Some("k1".into());
    let first = tx.add_job(new.clone()).await.unwrap();
    let second = tx.add_job(new).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(first.id, second.id);

    let mut tx = db.begin(t.org).await.unwrap();
    let jobs = tx
        .list_jobs(&JobFilter {
            repo_id: Some(t.repo),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1, "a replay must not create a second row");
}

#[sqlx::test]
async fn add_job_with_a_reused_key_and_a_different_payload_conflicts(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let mut first = job(&t, "wire up health endpoint");
    first.idempotency_key = Some("k1".into());
    tx.add_job(first).await.unwrap();

    let mut second = job(&t, "a totally different job");
    second.idempotency_key = Some("k1".into());
    let err = tx.add_job(second).await.unwrap_err();
    tx.rollback().await.unwrap();
    assert_eq!(err.code(), "idempotency_key_conflict");
}

#[sqlx::test]
async fn add_job_replay_is_insensitive_to_depends_on_order(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let dep_a = tx.add_job(job(&t, "dep a")).await.unwrap();
    let dep_b = tx.add_job(job(&t, "dep b")).await.unwrap();

    let mut first = job(&t, "depends on both");
    first.idempotency_key = Some("k1".into());
    first.depends_on = vec![dep_a.id.clone(), dep_b.id.clone()];
    let first_job = tx.add_job(first).await.unwrap();

    let mut second = job(&t, "depends on both");
    second.idempotency_key = Some("k1".into());
    second.depends_on = vec![dep_b.id.clone(), dep_a.id.clone()];
    let second_job = tx.add_job(second).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        first_job.id, second_job.id,
        "dependency order carries no meaning and must not defeat a replay"
    );

    // Not just the same id — the dependency rows themselves must be exactly
    // the intended set, proving the second (converged, `created = false`)
    // call correctly skipped `set_dependencies` rather than silently
    // dropping or duplicating the relationship.
    let mut tx = db.begin(t.org).await.unwrap();
    let blocked: Vec<String> = tx
        .blocked(None)
        .await
        .unwrap()
        .into_iter()
        .map(|j| j.id.0)
        .collect();
    assert_eq!(
        blocked,
        vec![first_job.id.0.clone()],
        "the replayed job must still be gated on both dependencies"
    );
    tx.claim_jobs(std::slice::from_ref(&dep_a.id), t.user, None, None)
        .await
        .unwrap();
    tx.complete_job(&dep_a.id, t.user, None, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin(t.org).await.unwrap();
    let still_blocked = tx.blocked(None).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        still_blocked.len(),
        1,
        "one dependency completing must not free a job that needs both"
    );

    let mut tx = db.begin(t.org).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&dep_b.id), t.user, None, None)
        .await
        .unwrap();
    tx.complete_job(&dep_b.id, t.user, None, None)
        .await
        .unwrap();
    let ready: Vec<String> = tx
        .ready(None, None)
        .await
        .unwrap()
        .into_iter()
        .map(|j| j.id.0)
        .collect();
    tx.commit().await.unwrap();
    assert_eq!(
        ready,
        vec![first_job.id.0.clone()],
        "both dependencies completing must free the replayed job exactly once"
    );
}

#[sqlx::test]
async fn add_job_without_a_key_always_creates_a_new_job(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let a = tx.add_job(job(&t, "same title")).await.unwrap();
    let b = tx.add_job(job(&t, "same title")).await.unwrap();
    tx.commit().await.unwrap();

    assert_ne!(
        a.id, b.id,
        "omitting the key must reproduce today's behavior"
    );
}

#[sqlx::test]
async fn add_job_rejects_an_empty_or_over_length_idempotency_key(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let mut empty = job(&t, "x");
    empty.idempotency_key = Some("   ".into());
    let err = tx.add_job(empty).await.unwrap_err();
    assert_eq!(err.code(), "invalid_argument");
    tx.rollback().await.unwrap();

    let mut tx = db.begin(t.org).await.unwrap();
    let mut too_long = job(&t, "x");
    too_long.idempotency_key = Some("k".repeat(of_core::idempotency::MAX_KEY_LEN + 1));
    let err = tx.add_job(too_long).await.unwrap_err();
    assert_eq!(err.code(), "invalid_argument");
    tx.rollback().await.unwrap();
}

#[sqlx::test]
async fn send_message_replays_with_the_same_idempotency_key(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let new = NewMessage {
        body: "hand-off note".into(),
        idempotency_key: Some("k1".into()),
        ..Default::default()
    };
    let first = tx.send_message(t.user, new.clone()).await.unwrap();
    let second = tx.send_message(t.user, new).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(first.id, second.id);

    let mut tx = db.begin(t.org).await.unwrap();
    let msgs = tx
        .inbox(
            t.user,
            &InboxQuery {
                unread_only: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(msgs.len(), 1, "a replay must not create a second row");
}

#[sqlx::test]
async fn send_message_with_a_reused_key_and_a_different_payload_conflicts(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    tx.send_message(
        t.user,
        NewMessage {
            body: "first note".into(),
            idempotency_key: Some("k1".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let err = tx
        .send_message(
            t.user,
            NewMessage {
                body: "a totally different note".into(),
                idempotency_key: Some("k1".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    tx.rollback().await.unwrap();
    assert_eq!(err.code(), "idempotency_key_conflict");
}

#[sqlx::test]
async fn send_message_without_a_key_always_creates_a_new_message(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let a = tx
        .send_message(
            t.user,
            NewMessage {
                body: "same body".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let b = tx
        .send_message(
            t.user,
            NewMessage {
                body: "same body".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_ne!(
        a.id, b.id,
        "omitting the key must reproduce today's behavior"
    );
}

/// The same literal key string used once for `add_job` and once for
/// `send_message` does not conflict: `jobs_org_idempotency_key_idx` and
/// `messages_org_idempotency_key_idx` are independent indexes on
/// independent tables, so there is nothing to collide with.
#[sqlx::test]
async fn the_same_key_reused_across_add_job_and_send_message_does_not_conflict(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let mut new_job = job(&t, "wire up the health endpoint");
    new_job.idempotency_key = Some("shared-key".into());
    tx.add_job(new_job).await.unwrap();

    tx.send_message(
        t.user,
        NewMessage {
            body: "hand-off note".into(),
            idempotency_key: Some("shared-key".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}
