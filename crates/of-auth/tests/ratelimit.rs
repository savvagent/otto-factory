//! Rate limiting: the cap policy, and the atomicity fix both policies share
//! (savvagent/otto-factory#75).
//!
//! `of-web`'s `tests/console.rs` covers the endpoint-level behaviour — the
//! bucket keys `login/finish` charges, the pre-check, and the
//! `CeremonyExpired` exclusion. This file is about the module's own
//! guarantees against a real Postgres, with no HTTP in between: that a cap
//! never escalates the way the lockout does, that a refusal does not grow the
//! table forever, and that concurrent callers racing one bucket cannot push
//! it past its limit.

use of_auth::ratelimit::{self, CapPolicy};
use of_auth::AuthError;
use of_core::Db;
use sqlx::PgPool;

/// A small policy so these tests do not need hundreds of requests to reach
/// the interesting boundary.
const TEST_CAP: CapPolicy = CapPolicy {
    window_secs: 900,
    soft_cap: 3,
    hard_cap: 5,
};

/// The cap, not the lockout: every attempt up through `hard_cap` is still
/// charged and answered, and a refusal past it carries a **flat**
/// `retry_after_secs` that does not grow as more failures keep landing —
/// unlike `lockout_secs`, which doubles per additional failure.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn cap_charge_refuses_past_hard_cap_with_a_flat_retry_after(pool: PgPool) {
    let db = Db::from_pool(pool);
    let bucket = "test:cap-flat";

    for n in 1..=TEST_CAP.hard_cap {
        ratelimit::cap_charge(&db, bucket, &TEST_CAP)
            .await
            .unwrap_or_else(|e| panic!("attempt {n} should still be charged, got {e}"));
    }

    for refusal in 1..=5 {
        let err = ratelimit::cap_charge(&db, bucket, &TEST_CAP)
            .await
            .expect_err("past hard_cap, a charge must refuse");
        match err {
            AuthError::RateLimited { retry_after_secs } => assert_eq!(
                retry_after_secs, TEST_CAP.window_secs,
                "retry-after on refusal {refusal} must stay flat at the window length, not escalate"
            ),
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }
}

/// A refused charge is not written — the bucket stops growing once it is
/// already over the line, so a sustained flood past `hard_cap` costs one row
/// per window rather than one per request forever.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_refused_charge_is_not_written(pool: PgPool) {
    let db = Db::from_pool(pool);
    let bucket = "test:cap-no-write-past-cap";

    for _ in 0..TEST_CAP.hard_cap {
        ratelimit::cap_charge(&db, bucket, &TEST_CAP).await.unwrap();
    }
    for _ in 0..10 {
        let _ = ratelimit::cap_charge(&db, bucket, &TEST_CAP).await;
    }

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM auth_attempts WHERE bucket = $1")
        .bind(bucket)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        rows, TEST_CAP.hard_cap,
        "ten further floods past the cap must not have added rows"
    );
}

/// The race #75 names: a `check()` read followed by a separate write let N
/// concurrent callers all read a count under the threshold before any of
/// their writes landed. `cap_charge` closes it with `pg_advisory_xact_lock`,
/// so launching far more concurrent callers than `hard_cap` against one
/// bucket must let through **exactly** `hard_cap` of them — not
/// approximately, not all of them, and never more.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn concurrent_cap_charges_against_one_bucket_never_exceed_the_cap(pool: PgPool) {
    let db = Db::from_pool(pool);
    let bucket = "test:cap-race";
    const CONCURRENT: usize = 40;

    let calls = (0..CONCURRENT).map(|_| {
        let db = db.clone();
        async move { ratelimit::cap_charge(&db, bucket, &TEST_CAP).await }
    });
    let results = futures::future::join_all(calls).await;

    let ok = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(
        ok, TEST_CAP.hard_cap as usize,
        "exactly hard_cap charges should succeed out of {CONCURRENT} concurrent callers"
    );

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM auth_attempts WHERE bucket = $1")
        .bind(bucket)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        rows, TEST_CAP.hard_cap,
        "the stored row count must exactly match what the calls above actually won, \
         never more than the cap — a race would overshoot this"
    );
}

/// The same fix, for the lockout's `check_and_charge`. The race predates the
/// cap and was already true of every bucket that called `check()` then
/// `charge()` as two separate round trips — signup, the claim-code path,
/// dynamic client registration — so it has to hold for the combined call too.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn concurrent_check_and_charge_never_exceeds_the_lockout_threshold(pool: PgPool) {
    let db = Db::from_pool(pool);
    let bucket = "test:lockout-race";
    const CONCURRENT: usize = 40;

    let calls = (0..CONCURRENT).map(|_| {
        let db = db.clone();
        async move { ratelimit::check_and_charge(&db, bucket).await }
    });
    let results = futures::future::join_all(calls).await;

    let ok = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(
        ok,
        ratelimit::MAX_FAILURES as usize,
        "exactly MAX_FAILURES charges should land before the lockout engages, \
         even with {CONCURRENT} callers racing the same bucket"
    );
}

/// [`ratelimit::cap_peek`] is a read with no lock, by design — it is the
/// early exit `login/finish` uses to skip real work, not the enforcement
/// itself. It must still report a number a caller can act on: zero for a
/// bucket nothing has charged, and the true count once something has.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn cap_peek_reports_the_current_count_without_charging(pool: PgPool) {
    let db = Db::from_pool(pool);
    let bucket = "test:cap-peek";

    assert_eq!(
        ratelimit::cap_peek(&db, bucket, &TEST_CAP).await.unwrap(),
        0
    );

    ratelimit::cap_charge(&db, bucket, &TEST_CAP).await.unwrap();
    ratelimit::cap_charge(&db, bucket, &TEST_CAP).await.unwrap();

    assert_eq!(
        ratelimit::cap_peek(&db, bucket, &TEST_CAP).await.unwrap(),
        2,
        "peeking must not itself add a charge"
    );
}

/// `login:cred:{sha256(id)}` must not leak the credential id itself into the
/// bucket string — that would defeat hashing it in the first place, storing
/// the very id this is meant to avoid writing down unencoded.
#[test]
fn credential_bucket_does_not_contain_the_raw_id() {
    let id = b"a very specific credential id";
    let bucket = ratelimit::credential_bucket(id);
    assert!(bucket.starts_with("login:cred:"));
    assert!(!bucket.contains("a very specific credential id"));
}
