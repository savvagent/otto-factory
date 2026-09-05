//! `set_profile`, and specifically the three states of `locale`.
//!
//! **No cross-org test here, deliberately.** `users` has no `org_id` and is
//! absent from the `tenant_tables` array in `0007_rls.sql`, so it is not a
//! tenant table and the two-guard rule does not reach it. A profile is
//! reachable only through the caller's own `CurrentUser`; there is no handler
//! that takes a user id from a request. Saying so explicitly, because "a
//! tenant-scoped function without a cross-org negative test is not done" and a
//! reviewer should be able to see this was decided rather than forgotten.

mod common;

use common::db;
use df_core::error::Error;
use df_core::i18n::SUPPORTED_LOCALES;
use sqlx::PgPool;

#[sqlx::test]
async fn a_new_account_has_chosen_no_locale(pool: PgPool) {
    let db = db(pool);
    let user = db.create_unclaimed_user().await.unwrap();

    // NULL is "never chose", not "chose English". The distinction is what lets
    // the console keep following the browser until someone says otherwise.
    assert_eq!(user.locale, None);
}

#[sqlx::test]
async fn a_locale_can_be_set_cleared_and_left_alone(pool: PgPool) {
    let db = db(pool);
    let user = db.create_unclaimed_user().await.unwrap();

    let set = db
        .set_profile(user.id, None, None, Some(Some("de")))
        .await
        .unwrap();
    assert_eq!(set.locale.as_deref(), Some("de"));

    // Absent leaves it alone — this is the shape every other PATCH field has,
    // and the reason `email`/`name` alone could not express a third state.
    let untouched = db
        .set_profile(user.id, None, Some("Renamed"), None)
        .await
        .unwrap();
    assert_eq!(untouched.locale.as_deref(), Some("de"));
    assert_eq!(untouched.name.as_deref(), Some("Renamed"));

    // Explicit null clears it: "match my browser" again.
    let cleared = db
        .set_profile(user.id, None, None, Some(None))
        .await
        .unwrap();
    assert_eq!(cleared.locale, None);
}

#[sqlx::test]
async fn a_region_subtag_is_stored_as_its_language(pool: PgPool) {
    let db = db(pool);
    let user = db.create_unclaimed_user().await.unwrap();

    // A browser that sends `es-419` means Spanish. Storing the region would
    // make the value fail to match any catalog the console actually ships.
    let updated = db
        .set_profile(user.id, None, None, Some(Some("es-419")))
        .await
        .unwrap();
    assert_eq!(updated.locale.as_deref(), Some("es"));
}

#[sqlx::test]
async fn every_supported_locale_round_trips(pool: PgPool) {
    let db = db(pool);
    let user = db.create_unclaimed_user().await.unwrap();

    for locale in SUPPORTED_LOCALES {
        let updated = db
            .set_profile(user.id, None, None, Some(Some(locale)))
            .await
            .unwrap();
        assert_eq!(updated.locale.as_deref(), Some(locale));
    }
}

#[sqlx::test]
async fn an_unsupported_locale_is_refused_and_names_the_options(pool: PgPool) {
    let db = db(pool);
    let user = db.create_unclaimed_user().await.unwrap();

    let err = db
        .set_profile(user.id, None, None, Some(Some("klingon")))
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Invalid(_)), "got {err:?}");
    assert_eq!(err.code(), "invalid_argument");

    let message = err.to_string();
    for locale in SUPPORTED_LOCALES {
        assert!(message.contains(locale), "{message:?} should name {locale}");
    }

    // And the refusal left nothing behind.
    let after = db.get_user(user.id).await.unwrap().unwrap();
    assert_eq!(after.locale, None);
}

#[sqlx::test]
async fn a_rejected_locale_does_not_apply_the_rest_of_the_patch(pool: PgPool) {
    let db = db(pool);
    let user = db.create_unclaimed_user().await.unwrap();

    // Validation happens before the statement, so a bad locale means the whole
    // patch is refused rather than half-applied — the caller retries one
    // request, not two.
    db.set_profile(user.id, None, Some("Kept out"), Some(Some("nope")))
        .await
        .unwrap_err();

    let after = db.get_user(user.id).await.unwrap().unwrap();
    assert_eq!(after.name, None);
    assert_eq!(after.locale, None);
}
