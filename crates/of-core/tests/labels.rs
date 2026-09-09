//! Every account carries a generated `label` — the words a password manager
//! shows beside a passkey so two accounts on the same site are told apart.
//!
//! **No cross-org test here, deliberately.** `users` has no `org_id` and is
//! absent from the `tenant_tables` array in `0007_rls.sql`, so the column is
//! not tenant data and the two-guard rule does not reach it. The two org-scoped
//! reads that carry a label — `list_org_members` and `list_team_members` — keep
//! the predicates they already had; `crates/of-core/tests/membership.rs` and the
//! `rls_scopes_*` cases in `crates/of-core/tests/isolation.rs` are what prove
//! that. Saying so explicitly, because "a tenant-scoped function without a
//! cross-org negative test is not done" and a reviewer should be able to see
//! this was decided rather than forgotten.

mod common;

use common::{db, tenant};
use sqlx::PgPool;

#[sqlx::test]
async fn a_new_passkey_account_gets_a_label(pool: PgPool) {
    let db = db(pool);

    let one = db.create_unclaimed_user().await.unwrap();
    let two = db.create_unclaimed_user().await.unwrap();

    assert!(!one.label.is_empty());
    assert!(!two.label.is_empty());
    // Signup mints a label per account rather than reusing one constant, which
    // is the whole failure this change exists to fix.
    assert_ne!(one.label, two.label);
}

#[sqlx::test]
async fn a_federated_account_gets_a_label(pool: PgPool) {
    let db = db(pool);

    // The enterprise-OIDC insert path is a second writer, and a label only the
    // passkey path minted would leave federated accounts with none.
    let user = db
        .upsert_user("someone@example.test", Some("Someone"))
        .await
        .unwrap();

    assert!(!user.label.is_empty());
}

#[sqlx::test]
async fn a_label_is_stored_not_recomputed(pool: PgPool) {
    let db = db(pool);
    let created = db.create_unclaimed_user().await.unwrap();

    let read_back = db.get_user(created.id).await.unwrap().expect("user exists");

    assert_eq!(read_back.label, created.label);
}

#[sqlx::test]
async fn setting_a_profile_leaves_the_label_alone(pool: PgPool) {
    let db = db(pool);
    let user = db.create_unclaimed_user().await.unwrap();

    let updated = db
        .set_profile(user.id, Some("someone@example.test"), Some("Someone"), None)
        .await
        .unwrap();

    // The label names the account in a credential vault, and a vault entry that
    // renamed itself when somebody filled in their profile would stop matching
    // the key already stored under the old words.
    assert_eq!(updated.label, user.label);
}

#[sqlx::test]
async fn org_members_carry_their_labels(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let members = db.list_org_members(t.org).await.unwrap();

    assert_eq!(members.len(), 1);
    assert!(!members[0].label.is_empty());
}

#[sqlx::test]
async fn team_members_carry_their_labels(pool: PgPool) {
    let db = db(pool);
    let t = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    let mut tx = db.begin(t.org).await.unwrap();
    let team = tx.create_team("platform", "Platform").await.unwrap();
    tx.add_team_member(team.id, t.user).await.unwrap();
    let members = tx.list_team_members(team.id).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(members.len(), 1);
    // The console renders one `person()` across the org-members and teams
    // pages, so a label on one shape and not the other would not compile there.
    assert!(!members[0].label.is_empty());
}
