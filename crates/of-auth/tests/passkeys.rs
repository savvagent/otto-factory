//! Passkey registration and sign-in, driven through a software authenticator.
//!
//! `SoftPasskey` performs the real ceremony — it produces genuine COSE
//! signatures over the challenge and client data this server issued, and
//! `webauthn-rs` verifies them the same way it verifies a YubiKey's. A test
//! that stubbed the signature would assert nothing about the part most likely
//! to be wrong.
//!
//! It is constructed with `falsify_uv: true` because the server requires user
//! verification and a software token has no biometric to perform. That is the
//! one thing here that is not what a real authenticator does, and it is the
//! difference between testing this flow and not testing it.
//!
//! ## What these tests do and do not cover
//!
//! Neither software authenticator in `webauthn-authenticator-rs` supports
//! resident keys — `SoftToken` says so in as many words ("These will be
//! supported in future :)") — so the challenges handed to it here are softened
//! by [`for_soft_token`] and [`offer`]: the resident-key requirement is dropped,
//! and the credential is named in `allowCredentials` because a token that does
//! not hold discoverable credentials cannot find one unprompted.
//!
//! **Only what is handed to the fake authenticator is softened. The server path
//! is the production one**, byte for byte: the same challenge issuance, the same
//! ceremony storage, the same account resolution by credential ID, and the same
//! signature verification. Neither field is part of the verification state.
//!
//! What is therefore *not* covered here is the browser finding a credential
//! without being told which one — that is browser behaviour rather than this
//! server's, and it is what `residentKey: required` asks for in production.

use of_auth::error::AuthError;
use of_auth::{login, passkeys};
use of_core::ids::UserId;
use of_core::Db;
use sqlx::PgPool;
use webauthn_authenticator_rs::softtoken::SoftToken;
use webauthn_authenticator_rs::WebauthnAuthenticator;
use webauthn_rs::prelude::Url;

const RP_ID: &str = "console.otto-factory.test";
const ORIGIN: &str = "https://console.otto-factory.test";

fn rp() -> webauthn_rs::Webauthn {
    passkeys::relying_party(RP_ID, ORIGIN).unwrap()
}

fn authenticator() -> WebauthnAuthenticator<SoftToken> {
    WebauthnAuthenticator::new(SoftToken::new(true).unwrap().0)
}

/// Drop the resident-key requirement from a challenge before handing it to the
/// software token. See the module docs: the server's state is unaffected.
fn for_soft_token(
    mut challenge: webauthn_rs::prelude::CreationChallengeResponse,
) -> webauthn_rs::prelude::CreationChallengeResponse {
    if let Some(sel) = challenge.public_key.authenticator_selection.as_mut() {
        sel.require_resident_key = false;
        sel.resident_key = None;
    }
    challenge
}

/// Name a credential in `allowCredentials`, so a token holding no discoverable
/// credentials can still find the right key. Production sends this list empty.
fn offer(
    mut challenge: webauthn_rs::prelude::RequestChallengeResponse,
    credential_id: &[u8],
) -> webauthn_rs::prelude::RequestChallengeResponse {
    use webauthn_rs::prelude::Base64UrlSafeData;
    use webauthn_rs_proto::AllowCredentials;
    challenge.public_key.allow_credentials = vec![AllowCredentials {
        type_: "public-key".to_string(),
        id: Base64UrlSafeData::from(credential_id.to_vec()),
        transports: None,
    }];
    challenge
}

/// Register a brand-new account, the way signup does.
async fn register_new(db: &Db, auth: &mut WebauthnAuthenticator<SoftToken>) -> UserId {
    let webauthn = rp();
    let ceremony = passkeys::start_registration(db, &webauthn, None)
        .await
        .unwrap();
    let credential = auth
        .do_registration(
            Url::parse(ORIGIN).unwrap(),
            for_soft_token(ceremony.challenge),
        )
        .expect("the authenticator refused the registration challenge");
    passkeys::finish_registration(
        db,
        &webauthn,
        ceremony.id,
        &credential,
        Some("laptop"),
        passkeys::RegistrationVia::Signup,
        None,
    )
    .await
    .unwrap()
}

/// The credential IDs an account holds, for `offer`.
async fn credential_ids(db: &Db, user: UserId) -> Vec<Vec<u8>> {
    sqlx::query_scalar("SELECT credential_id FROM passkeys WHERE user_id = $1 ORDER BY created_at")
        .bind(user)
        .fetch_all(db.pool())
        .await
        .unwrap()
}

/// Sign in, usernameless — nothing but the credential identifies the account.
async fn sign_in(
    db: &Db,
    auth: &mut WebauthnAuthenticator<SoftToken>,
    credential_id: &[u8],
) -> of_auth::error::Result<UserId> {
    let webauthn = rp();
    let ceremony = passkeys::start_authentication(db, &webauthn).await.unwrap();
    let credential = auth
        .do_authentication(
            Url::parse(ORIGIN).unwrap(),
            offer(ceremony.challenge, credential_id),
        )
        .expect("the authenticator refused the authentication challenge");
    passkeys::finish_authentication(db, &webauthn, ceremony.id, &credential, None).await
}

/// The whole point, in one test: an account is created by registering a key,
/// and signing back in names nobody.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_passkey_creates_an_account_and_signs_back_into_it(pool: PgPool) {
    let db = Db::from_pool(pool);
    let mut auth = authenticator();

    let user = register_new(&db, &mut auth).await;

    // The account exists and has no address — the passkey brought it into
    // being, and the profile comes later.
    let account = db.get_user(user).await.unwrap().unwrap();
    assert_eq!(account.email, None);
    assert_eq!(passkeys::count(&db, user).await.unwrap(), 1);

    let ids = credential_ids(&db, user).await;
    let signed_in = sign_in(&db, &mut auth, &ids[0]).await.unwrap();
    assert_eq!(
        signed_in, user,
        "the credential must resolve to the account that registered it"
    );

    let opened = login::with_passkey(&db, signed_in, None).await.unwrap();
    assert_eq!(opened.method, login::Method::Passkey);
    assert!(
        opened.should_add_passkey,
        "one key means one device, and the console has to say so"
    );
}

/// A second key on the same account is the recovery story, and either one must
/// open the account on its own.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_second_passkey_also_opens_the_account(pool: PgPool) {
    let db = Db::from_pool(pool);
    let mut first = authenticator();
    let user = register_new(&db, &mut first).await;

    let webauthn = rp();
    let mut second = authenticator();
    let ceremony = passkeys::start_registration(&db, &webauthn, Some(user))
        .await
        .unwrap();
    let credential = second
        .do_registration(
            Url::parse(ORIGIN).unwrap(),
            for_soft_token(ceremony.challenge),
        )
        .unwrap();
    let same = passkeys::finish_registration(
        &db,
        &webauthn,
        ceremony.id,
        &credential,
        Some("phone"),
        passkeys::RegistrationVia::Add,
        None,
    )
    .await
    .unwrap();
    assert_eq!(same, user);
    assert_eq!(passkeys::count(&db, user).await.unwrap(), 2);

    let ids = credential_ids(&db, user).await;
    assert_eq!(sign_in(&db, &mut first, &ids[0]).await.unwrap(), user);
    assert_eq!(sign_in(&db, &mut second, &ids[1]).await.unwrap(), user);

    let opened = login::with_passkey(&db, user, None).await.unwrap();
    assert!(
        !opened.should_add_passkey,
        "two keys is the state the console stops nagging about"
    );
}

/// A ceremony is single-use. Replaying a captured one must not authenticate
/// anybody — this is what the `DELETE … RETURNING` in `take_ceremony` buys.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_ceremony_cannot_be_replayed(pool: PgPool) {
    let db = Db::from_pool(pool);
    let mut auth = authenticator();
    let user = register_new(&db, &mut auth).await;
    let ids = credential_ids(&db, user).await;

    let webauthn = rp();
    let ceremony = passkeys::start_authentication(&db, &webauthn)
        .await
        .unwrap();
    let credential = auth
        .do_authentication(
            Url::parse(ORIGIN).unwrap(),
            offer(ceremony.challenge, &ids[0]),
        )
        .unwrap();

    passkeys::finish_authentication(&db, &webauthn, ceremony.id, &credential, None)
        .await
        .expect("the first use must work");

    let replayed =
        passkeys::finish_authentication(&db, &webauthn, ceremony.id, &credential, None).await;
    assert!(
        replayed.is_err(),
        "a spent ceremony authenticated a second time"
    );
}

/// A registration state must not be finishable as an authentication, or the
/// two ceremonies' guarantees leak into each other.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_registration_ceremony_is_not_an_authentication(pool: PgPool) {
    let db = Db::from_pool(pool);
    let mut auth = authenticator();
    let user = register_new(&db, &mut auth).await;
    let ids = credential_ids(&db, user).await;

    let webauthn = rp();
    let registration = passkeys::start_registration(&db, &webauthn, None)
        .await
        .unwrap();

    // Drive a real sign-in, then try to finish it against the registration's
    // ceremony id.
    let signin = passkeys::start_authentication(&db, &webauthn)
        .await
        .unwrap();
    let assertion = auth
        .do_authentication(
            Url::parse(ORIGIN).unwrap(),
            offer(signin.challenge, &ids[0]),
        )
        .unwrap();

    let crossed =
        passkeys::finish_authentication(&db, &webauthn, registration.id, &assertion, None).await;
    assert!(
        crossed.is_err(),
        "a registration ceremony was spent as an authentication"
    );
}

/// A signature made for a different relying party must not be accepted. This is
/// the phishing resistance the whole change is for: a lookalike origin cannot
/// produce something this server will take.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_credential_from_another_origin_is_refused(pool: PgPool) {
    let db = Db::from_pool(pool);
    let mut auth = authenticator();
    register_new(&db, &mut auth).await;

    // The attacker's site issues its own challenge, and the authenticator
    // happily signs for *them* — that is correct behaviour, and it is why the
    // server checks the origin rather than trusting the signature alone.
    let evil = passkeys::relying_party("evil.test", "https://evil.test").unwrap();
    let ours = rp();

    let ceremony = passkeys::start_authentication(&db, &ours).await.unwrap();
    let stolen = auth.do_authentication(Url::parse("https://evil.test").unwrap(), {
        let (challenge, _) = evil.start_discoverable_authentication().unwrap();
        challenge
    });

    // Either the authenticator refuses outright (no credential for that RP), or
    // it signs something our server must reject. Both are acceptable; silently
    // accepting is not.
    if let Ok(credential) = stolen {
        let accepted =
            passkeys::finish_authentication(&db, &ours, ceremony.id, &credential, None).await;
        assert!(
            accepted.is_err(),
            "a credential signed for another origin was accepted"
        );
    }
}

/// Removing the last passkey would lock the account out permanently, with no
/// email to recover through. The refusal is the feature.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn the_last_passkey_cannot_be_removed(pool: PgPool) {
    let db = Db::from_pool(pool);
    let mut auth = authenticator();
    let user = register_new(&db, &mut auth).await;

    let keys = passkeys::list(&db, user).await.unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].nickname.as_deref(), Some("laptop"));

    let refused = passkeys::remove(&db, user, keys[0].id, None).await;
    assert!(refused.is_err(), "the only passkey was removed");
    assert_eq!(passkeys::count(&db, user).await.unwrap(), 1);
}

/// A disabled account's keys still produce valid signatures. The refusal has to
/// happen at the account level, after the ceremony.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_disabled_account_cannot_sign_in_with_a_valid_passkey(pool: PgPool) {
    let db = Db::from_pool(pool);
    let mut auth = authenticator();
    let user = register_new(&db, &mut auth).await;

    sqlx::query("UPDATE users SET disabled_at = now() WHERE id = $1")
        .bind(user)
        .execute(db.pool())
        .await
        .unwrap();

    // The ceremony itself still succeeds — the credential is genuine.
    let ids = credential_ids(&db, user).await;
    let identified = sign_in(&db, &mut auth, &ids[0]).await.unwrap();
    assert_eq!(identified, user);

    // The account check is what refuses.
    match login::with_passkey(&db, user, None).await {
        Ok(_) => panic!("a disabled account was signed in"),
        Err(e) => assert_eq!(e.public(), "invalid credentials"),
    }
}

/// Clearing an account's keys is the admin half of recovery, and it must leave
/// the account genuinely unusable rather than merely inconvenient.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn clearing_passkeys_leaves_no_way_in(pool: PgPool) {
    let db = Db::from_pool(pool);
    let mut auth = authenticator();
    let user = register_new(&db, &mut auth).await;
    let ids = credential_ids(&db, user).await;

    let removed = passkeys::clear(&db, user, user, None).await.unwrap();
    assert_eq!(removed, 1);
    assert!(!passkeys::has_credential(&db, user).await.unwrap());

    let refused = sign_in(&db, &mut auth, &ids[0]).await;
    assert!(
        refused.is_err(),
        "a cleared account still accepted its old passkey"
    );
}

// -------------------------- an unknown credential vs. a bad signature (#60)

/// How many failed-sign-in rows this account has. The distinction #60 needs is
/// only safe because the unknown-credential branch attributes nothing, so the
/// count is part of the assertion rather than a detail.
async fn login_failures(db: &Db, user: UserId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = $1 AND actor_user_id = $2")
        .bind(of_core::audit::action::LOGIN_FAILED)
        .bind(user)
        .fetch_one(db.pool())
        .await
        .unwrap()
}

/// How many rows this account has under a given action. Parameterized so both
/// the registration and clear paths can share one helper (#76).
async fn action_count(db: &Db, action: &str, user: UserId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = $1 AND actor_user_id = $2")
        .bind(action)
        .bind(user)
        .fetch_one(db.pool())
        .await
        .unwrap()
}

// --------------------------------------- passkey.* audit actions, not totp.* (#76)

/// Registration writes the new `auth.passkey.registered` action, never the old
/// `auth.totp.enrolled` one TOTP left behind.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn registration_writes_the_passkey_registered_action(pool: PgPool) {
    let db = Db::from_pool(pool);
    let mut auth = authenticator();
    let user = register_new(&db, &mut auth).await;

    assert_eq!(
        action_count(&db, of_core::audit::action::PASSKEY_REGISTERED, user).await,
        1,
        "registration must write auth.passkey.registered"
    );
    assert_eq!(
        action_count(&db, of_core::audit::action::TOTP_ENROLLED, user).await,
        0,
        "registration must not write the historical TOTP action"
    );
}

/// The claim path's row is the one that matters most: it is what proves who
/// actually walked through the door after an admin-assisted reset (#88).
#[sqlx::test(migrations = "../of-core/migrations")]
async fn registration_records_which_flow_wrote_it(pool: PgPool) {
    let db = Db::from_pool(pool);
    let webauthn = rp();
    let mut auth = authenticator();
    let user = register_new(&db, &mut auth).await;

    let ceremony = passkeys::start_registration(&db, &webauthn, Some(user))
        .await
        .unwrap();
    let credential = auth
        .do_registration(
            Url::parse(ORIGIN).unwrap(),
            for_soft_token(ceremony.challenge),
        )
        .expect("the authenticator refused the registration challenge");
    passkeys::finish_registration(
        &db,
        &webauthn,
        ceremony.id,
        &credential,
        None,
        passkeys::RegistrationVia::Claim,
        Some("203.0.113.7"),
    )
    .await
    .unwrap();

    // One query for both columns, ordered by `id` rather than `created_at` —
    // `created_at` defaults to the transaction's start time and can tie, and
    // `of_core::audit`'s own reader already orders by `created_at DESC, id
    // DESC` for exactly that reason. Two separate queries could in principle
    // disagree about which row is "latest"; one query cannot.
    let row: (serde_json::Value, Option<String>) = sqlx::query_as(
        "SELECT detail->'via', ip FROM audit_events \
         WHERE action = $1 AND actor_user_id = $2 \
         ORDER BY id DESC LIMIT 1",
    )
    .bind(of_core::audit::action::PASSKEY_REGISTERED)
    .bind(user)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(row.0, serde_json::json!("claim"));
    assert_eq!(row.1.as_deref(), Some("203.0.113.7"));
}

/// Clearing writes the new `auth.passkey.cleared` action, never the old
/// `auth.totp.reset` one TOTP left behind.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn clearing_writes_the_passkey_cleared_action(pool: PgPool) {
    let db = Db::from_pool(pool);
    let mut auth = authenticator();
    let user = register_new(&db, &mut auth).await;

    passkeys::clear(&db, user, user, None).await.unwrap();

    assert_eq!(
        action_count(&db, of_core::audit::action::PASSKEY_CLEARED, user).await,
        1,
        "clearing must write auth.passkey.cleared"
    );
    assert_eq!(
        action_count(&db, of_core::audit::action::TOTP_RESET, user).await,
        0,
        "clearing must not write the historical TOTP action"
    );
}

/// Removing a key (never the last) writes `auth.passkey.removed`. Evicting the
/// legitimate owner's key is the standard persistence step after a session
/// takeover, so this must be visible in the trail (#89).
#[sqlx::test(migrations = "../of-core/migrations")]
async fn removing_a_passkey_writes_the_passkey_removed_action(pool: PgPool) {
    let db = Db::from_pool(pool);
    let mut first = authenticator();
    let user = register_new(&db, &mut first).await;

    let webauthn = rp();
    let mut second = authenticator();
    let ceremony = passkeys::start_registration(&db, &webauthn, Some(user))
        .await
        .unwrap();
    let credential = second
        .do_registration(
            Url::parse(ORIGIN).unwrap(),
            for_soft_token(ceremony.challenge),
        )
        .unwrap();
    passkeys::finish_registration(
        &db,
        &webauthn,
        ceremony.id,
        &credential,
        Some("phone"),
        passkeys::RegistrationVia::Add,
        None,
    )
    .await
    .unwrap();

    let keys = passkeys::list(&db, user).await.unwrap();
    assert_eq!(keys.len(), 2);
    passkeys::remove(&db, user, keys[0].id, Some("203.0.113.9"))
        .await
        .unwrap();

    assert_eq!(
        action_count(&db, of_core::audit::action::PASSKEY_REMOVED, user).await,
        1,
        "removing a key must write auth.passkey.removed"
    );
}

/// Renaming a key writes `auth.passkey.renamed`.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn renaming_a_passkey_writes_the_passkey_renamed_action(pool: PgPool) {
    let db = Db::from_pool(pool);
    let mut auth = authenticator();
    let user = register_new(&db, &mut auth).await;
    let keys = passkeys::list(&db, user).await.unwrap();

    passkeys::rename(&db, user, keys[0].id, "renamed laptop", None)
        .await
        .unwrap();

    assert_eq!(
        action_count(&db, of_core::audit::action::PASSKEY_RENAMED, user).await,
        1,
        "renaming a key must write auth.passkey.renamed"
    );
}

/// A corrupted stored credential must not change `finish_authentication`'s
/// outcome — it is dropped and logged, and the account still falls through to
/// `InvalidCredentials` via the existing `keys.is_empty()` branch rather than a
/// panic or a different error. This is the regression the deserialize-logging
/// rewrite is most likely to introduce if the match arms are transposed.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_corrupted_stored_credential_is_dropped_not_fatal(pool: PgPool) {
    let db = Db::from_pool(pool);
    let mut auth = authenticator();
    let user = register_new(&db, &mut auth).await;
    let ids = credential_ids(&db, user).await;

    sqlx::query("UPDATE passkeys SET credential = $1 WHERE user_id = $2")
        .bind(serde_json::json!({"garbage": true}))
        .bind(user)
        .execute(db.pool())
        .await
        .unwrap();

    match sign_in(&db, &mut auth, &ids[0]).await {
        Err(AuthError::InvalidCredentials) => {}
        other => panic!("a corrupted stored credential answered {other:?}"),
    }
}

/// A credential this server has never stored answers `unknown_credential`, and
/// the assertion is a genuine one — the signature verifies, so the refusal can
/// only be the lookup. That is what lets the console tell a vault to forget a
/// key without evicting a good one after a cancelled prompt.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_credential_this_server_never_stored_says_so(pool: PgPool) {
    let db = Db::from_pool(pool);
    let webauthn = rp();
    let mut stranger = authenticator();

    // A real ceremony, deliberately never finished: the authenticator holds the
    // key and will sign with it, and no `passkeys` row exists for it.
    let ceremony = passkeys::start_registration(&db, &webauthn, None)
        .await
        .unwrap();
    let credential = stranger
        .do_registration(
            Url::parse(ORIGIN).unwrap(),
            for_soft_token(ceremony.challenge),
        )
        .unwrap();
    let credential_id = credential.raw_id.as_ref().to_vec();

    match sign_in(&db, &mut stranger, &credential_id).await {
        Err(AuthError::UnknownCredential) => {}
        other => panic!("a credential with no row answered {other:?}"),
    }
}

/// Everything downstream of the lookup stays collapsed. A key this server does
/// hold, presented with a signature that does not verify, is still
/// `invalid_credentials` — telling a forger which part of the forgery failed is
/// the leak the module exists to avoid.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_known_credential_with_a_bad_signature_is_still_invalid_credentials(pool: PgPool) {
    use webauthn_rs::prelude::Base64UrlSafeData;

    let db = Db::from_pool(pool);
    let webauthn = rp();
    let mut auth = authenticator();
    let user = register_new(&db, &mut auth).await;
    let ids = credential_ids(&db, user).await;

    let ceremony = passkeys::start_authentication(&db, &webauthn)
        .await
        .unwrap();
    let mut credential = auth
        .do_authentication(
            Url::parse(ORIGIN).unwrap(),
            offer(ceremony.challenge, &ids[0]),
        )
        .unwrap();

    // Corrupt the signature and nothing else: the credential id still resolves,
    // so the lookup succeeds and only the verification can fail.
    let mut signature = credential.response.signature.as_ref().to_vec();
    let last = signature.len() - 1;
    signature[last] ^= 0xff;
    credential.response.signature = Base64UrlSafeData::from(signature);

    match passkeys::finish_authentication(&db, &webauthn, ceremony.id, &credential, None).await {
        Err(AuthError::InvalidCredentials) => {}
        other => panic!("a bad signature against a stored key answered {other:?}"),
    }

    assert_eq!(
        login_failures(&db, user).await,
        1,
        "a failure against a resolved account belongs in the audit trail"
    );
}

/// The assisted-recovery case #60 was filed for. After an admin clears the
/// keys, the old credential is gone from the table, so the answer is
/// `unknown_credential` — which is what finally lets the vault stop offering a
/// dead key to somebody who is already locked out. No audit row is written,
/// because the lookup resolves no account to attribute one to.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_cleared_accounts_old_credential_is_unknown_and_unattributed(pool: PgPool) {
    let db = Db::from_pool(pool);
    let mut auth = authenticator();
    let user = register_new(&db, &mut auth).await;
    let ids = credential_ids(&db, user).await;

    passkeys::clear(&db, user, user, None).await.unwrap();

    match sign_in(&db, &mut auth, &ids[0]).await {
        Err(AuthError::UnknownCredential) => {}
        other => panic!("a cleared account's old key answered {other:?}"),
    }

    assert_eq!(
        login_failures(&db, user).await,
        0,
        "a row was attributed to an account the lookup never resolved"
    );
}

// ------------------------------------------------- naming the credential (#59)

/// The words an authenticator will file the credential under, read off the
/// challenge exactly as the browser would.
fn names_in(challenge: &webauthn_rs::prelude::CreationChallengeResponse) -> (String, String) {
    (
        challenge.public_key.user.name.clone(),
        challenge.public_key.user.display_name.clone(),
    )
}

fn a_user(email: Option<&str>, name: Option<&str>, label: &str) -> of_core::orgs::User {
    of_core::orgs::User {
        id: UserId::new(),
        email: email.map(str::to_string),
        name: name.map(str::to_string),
        locale: None,
        label: label.to_string(),
        created_at: chrono::Utc::now(),
        disabled_at: None,
    }
}

/// The bug #59 reports, at its source: a challenge named after the product
/// leaves every account on this site as an identical row in a vault's picker.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_new_accounts_challenge_is_not_named_after_the_product(pool: PgPool) {
    let db = Db::from_pool(pool);
    let ceremony = passkeys::start_registration(&db, &rp(), None)
        .await
        .unwrap();
    let (name, display_name) = names_in(&ceremony.challenge);

    assert_ne!(
        display_name, "otto-factory",
        "the picker entry must name the account, not the site"
    );
    assert_ne!(
        name, "otto-factory",
        "the sortable name must name the account too"
    );
    assert!(
        display_name.contains("otto-factory"),
        "the site still belongs in the display name, beside the account: {display_name:?}"
    );
}

/// Two signups, two picker entries a human can tell apart. Probabilistic by
/// construction — the label space is ~576,000, so this fails about once in that
/// many runs rather than never.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn two_new_accounts_are_named_differently(pool: PgPool) {
    let db = Db::from_pool(pool);
    let webauthn = rp();
    let first = passkeys::start_registration(&db, &webauthn, None)
        .await
        .unwrap();
    let second = passkeys::start_registration(&db, &webauthn, None)
        .await
        .unwrap();

    assert_ne!(
        names_in(&first.challenge).1,
        names_in(&second.challenge).1,
        "two accounts were handed the same picker entry"
    );
}

/// The stability rule, and the exact confusion #59 is about: adding a second key
/// must not file it under different words from the first.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_second_key_on_one_account_is_named_like_the_first(pool: PgPool) {
    let db = Db::from_pool(pool);
    let webauthn = rp();
    let mut auth = authenticator();

    let first = passkeys::start_registration(&db, &webauthn, None)
        .await
        .unwrap();
    let (first_name, first_display) = names_in(&first.challenge);
    let credential = auth
        .do_registration(Url::parse(ORIGIN).unwrap(), for_soft_token(first.challenge))
        .unwrap();
    let user = passkeys::finish_registration(
        &db,
        &webauthn,
        first.id,
        &credential,
        None,
        passkeys::RegistrationVia::Signup,
        None,
    )
    .await
    .unwrap();

    let second = passkeys::start_registration(&db, &webauthn, Some(user))
        .await
        .unwrap();
    assert_eq!(names_in(&second.challenge), (first_name, first_display));
}

/// `credential_names` is a pure function of the account, so the precedence is
/// testable without a ceremony — and it is the one place either half of the
/// pair is composed.
#[test]
fn the_address_wins_then_the_name_then_the_label() {
    let with_address = a_user(Some("ada@example.test"), Some("Ada"), "brisk-harbor-42");
    let with_name = a_user(None, Some("Ada"), "brisk-harbor-42");
    let bare = a_user(None, None, "brisk-harbor-42");

    assert_eq!(
        passkeys::credential_names(&with_address).name,
        "ada@example.test"
    );
    assert_eq!(passkeys::credential_names(&with_name).name, "Ada");
    assert_eq!(passkeys::credential_names(&bare).name, "brisk-harbor-42");

    for user in [&with_address, &with_name, &bare] {
        assert!(
            passkeys::credential_names(user)
                .display_name
                .contains("brisk-harbor-42"),
            "the generated words belong in every display name"
        );
    }
}

/// An account that later sets an address gets the better sortable name without
/// losing the words its owner has already learned.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn setting_an_address_keeps_the_generated_words(pool: PgPool) {
    let db = Db::from_pool(pool);
    let webauthn = rp();
    let mut auth = authenticator();
    let user = register_new(&db, &mut auth).await;

    let before = passkeys::start_registration(&db, &webauthn, Some(user))
        .await
        .unwrap();
    let (_, display_before) = names_in(&before.challenge);

    db.set_profile(user, Some("ada@example.test"), None, None)
        .await
        .unwrap();

    let after = passkeys::start_registration(&db, &webauthn, Some(user))
        .await
        .unwrap();
    let (name_after, display_after) = names_in(&after.challenge);

    assert_eq!(name_after, "ada@example.test");
    assert_eq!(
        display_after, display_before,
        "filling in a profile renamed the key its owner already knows"
    );
}

/// The server half of an encoding pair: every WebAuthn signal method the console
/// calls takes this handle back, and a base64url of the UUID's *text* would be
/// accepted and match nothing.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn the_user_handle_is_base64url_of_the_uuids_sixteen_bytes(pool: PgPool) {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    let db = Db::from_pool(pool);
    let webauthn = rp();
    let ceremony = passkeys::start_registration(&db, &webauthn, None)
        .await
        .unwrap();

    // Off the serialized challenge, because what the console has to match is the
    // wire form and not the Rust type behind it.
    let wire = serde_json::to_value(&ceremony.challenge).unwrap();
    let handle = wire["publicKey"]["user"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let raw = URL_SAFE_NO_PAD
        .decode(&handle)
        .expect("base64url, unpadded");
    assert_eq!(raw.len(), 16, "the handle is the UUID's raw bytes");

    let user: UserId = sqlx::query_scalar("SELECT id FROM users")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        handle,
        URL_SAFE_NO_PAD.encode(user.as_uuid().as_bytes()),
        "the console cannot reconstruct this handle from the UUID's text form"
    );
}
