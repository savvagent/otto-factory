//! The console API, end to end against a real Postgres.
//!
//! These drive the assembled router, so a test failing here means a real client
//! would have failed the same way. The properties under test are the ones a
//! unit test cannot reach: that the onboarding sequence actually completes, that
//! an org you are not in is indistinguishable from one that does not exist, that
//! a role boundary holds at the HTTP edge, and that removing someone actually
//! disconnects their agents.

mod common;

use base64::Engine;
use common::{
    add_member, harness, harness_behind_proxy, harness_with_trackers, onboard, org_with_owner,
    present_credential, sign_in, unregistered_credential, Call, CLIENT_IP_HEADER,
};
use http::StatusCode;
use of_core::orgs::Role;
use sqlx::PgPool;

// ------------------------------------------------------------- onboarding

/// The whole front door, in the order a person meets it. If this breaks, the
/// product has no first five minutes.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_new_user_signs_up_enrols_and_registers_a_repo(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;

    let me = Call::get("/api/me")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    me.expect(StatusCode::OK);
    assert_eq!(me.body["user"]["email"], "rob@acme.test");
    assert_eq!(me.body["passkeyCount"], 1);
    assert_eq!(
        me.body["shouldAddPasskey"], true,
        "one passkey is one device — the console has to ask for a second"
    );
    assert!(me.body["orgs"].as_array().unwrap().is_empty());

    org_with_owner(&h, "acme", &rob).await;

    let registered = Call::post("/api/orgs/acme/repos")
        .with_session(&rob.session)
        .json(serde_json::json!({
            "slug": "api",
            "name": "Acme API",
            "remotes": ["git@github.com:acme/api.git"],
        }))
        .send(&h.router)
        .await;
    registered.expect(StatusCode::CREATED);
    assert_eq!(
        registered.body["provider"], "github",
        "inferred from the remote"
    );

    let repos = Call::get("/api/orgs/acme/repos")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    repos.expect(StatusCode::OK);
    assert_eq!(repos.body.as_array().unwrap().len(), 1);
}
#[sqlx::test(migrations = "../of-core/migrations")]
async fn signing_in_and_out_works_and_a_dead_cookie_is_refused(pool: PgPool) {
    let h = harness(pool);
    let mut rob = onboard(&h, "rob@acme.test").await;

    let signed_in = sign_in(&h, &mut rob).await;
    signed_in.expect(StatusCode::OK);
    let session = signed_in.session_cookie().unwrap();

    Call::get("/api/me")
        .with_session(&session)
        .send(&h.router)
        .await
        .expect(StatusCode::OK);

    let logged_out = Call::post("/api/auth/logout")
        .with_session(&session)
        .send(&h.router)
        .await;
    logged_out.expect(StatusCode::NO_CONTENT);
    assert!(
        logged_out
            .headers
            .get_all(http::header::SET_COOKIE)
            .iter()
            .any(|v| v.to_str().unwrap().contains("Max-Age=0")),
        "logout must clear the cookie"
    );

    let after = Call::get("/api/me")
        .with_session(&session)
        .send(&h.router)
        .await;
    after.expect(StatusCode::UNAUTHORIZED);
    assert_eq!(after.error_code(), Some("unauthenticated"));
}
#[sqlx::test(migrations = "../of-core/migrations")]
async fn signing_out_everywhere_ends_every_session(pool: PgPool) {
    let h = harness(pool);
    let mut rob = onboard(&h, "rob@acme.test").await;

    let second = sign_in(&h, &mut rob).await.session_cookie().unwrap();

    let listed = Call::get("/api/me/sessions")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    listed.expect(StatusCode::OK);
    assert_eq!(listed.body.as_array().unwrap().len(), 2);

    Call::delete("/api/me/sessions")
        .with_session(&rob.session)
        .send(&h.router)
        .await
        .expect(StatusCode::OK);

    for session in [&rob.session, &second] {
        Call::get("/api/me")
            .with_session(session)
            .send(&h.router)
            .await
            .expect(StatusCode::UNAUTHORIZED);
    }
}

// ---------------------------------------------------- login/finish throttle

/// The whole point of #75: a source address shared by many honest sign-ins
/// must be throttled, never locked out. `LOGIN_IP_CAP.hard_cap` is generous
/// (50, against the lockout's `MAX_FAILURES` of 5) specifically so a realistic
/// shared-address failure burst never gets near it; this drives failures from
/// several distinct unregistered credentials — staying under each one's own,
/// much tighter, credential cap — so the address bucket alone is what is under
/// test.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_shared_address_is_throttled_not_locked_out(pool: PgPool) {
    let h = harness_behind_proxy(pool);
    let office = "198.51.100.4";

    let per_credential = (of_auth::ratelimit::LOGIN_CRED_CAP.hard_cap - 1) as usize;
    let needed = of_auth::ratelimit::LOGIN_IP_CAP.hard_cap as usize;
    let mut sent = 0usize;

    'outer: loop {
        let (mut stranger, unknown) = unregistered_credential(&h).await;
        for attempt in 0..per_credential {
            if sent >= needed {
                break 'outer;
            }
            let reply = present_credential(&h, &mut stranger, &unknown, Some(office)).await;
            assert_eq!(
                reply.error_code(),
                Some("unknown_credential"),
                "attempt {sent} (credential attempt {attempt}) should still be answered — \
                 far more than the old lockout's 5-failure threshold has landed"
            );
            sent += 1;
        }
    }

    // Past the address cap, a brand-new credential from the same office is
    // refused before any credential work runs — the address itself is what
    // is throttled, independent of which credential is being tried.
    let (mut stranger, unknown) = unregistered_credential(&h).await;
    let refused = present_credential(&h, &mut stranger, &unknown, Some(office)).await;
    refused.expect(StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.error_code(), Some("rate_limited"));
    let retry_after: i64 = refused
        .headers
        .get(http::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .expect("a rate-limited reply must carry Retry-After");

    // Keep flooding and confirm the wait never grows — a lockout would double
    // it on every further failure; a cap holds it flat at the window length.
    for round in 1..=5 {
        let (mut stranger, unknown) = unregistered_credential(&h).await;
        let refused = present_credential(&h, &mut stranger, &unknown, Some(office)).await;
        refused.expect(StatusCode::TOO_MANY_REQUESTS);
        let again: i64 = refused
            .headers
            .get(http::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap();
        assert_eq!(
            again, retry_after,
            "round {round}: retry-after must stay flat, never escalate like the lockout does"
        );
    }

    // A different address is unaffected: the bucket is keyed on the source,
    // not shared globally.
    let elsewhere = "203.0.113.44";
    let (mut stranger, unknown) = unregistered_credential(&h).await;
    let reply = present_credential(&h, &mut stranger, &unknown, Some(elsewhere)).await;
    assert_eq!(reply.error_code(), Some("unknown_credential"));
}

/// The credential-keyed bucket (#75): probing repeats against *one specific*
/// credential id is capped far tighter than the address bucket, and
/// independently of it — a brand new credential from the very same address
/// that just tripped the credential cap is still answered.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn probing_one_credential_is_capped_independently_of_the_address(pool: PgPool) {
    let h = harness_behind_proxy(pool);
    let (mut stranger, credential_id) = unregistered_credential(&h).await;
    let prober = "203.0.113.7";

    for attempt in 1..=of_auth::ratelimit::LOGIN_CRED_CAP.hard_cap {
        let reply = present_credential(&h, &mut stranger, &credential_id, Some(prober)).await;
        assert_eq!(
            reply.error_code(),
            Some("unknown_credential"),
            "attempt {attempt} against this id should still be answered"
        );
    }

    let refused = present_credential(&h, &mut stranger, &credential_id, Some(prober)).await;
    refused.expect(StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.error_code(), Some("rate_limited"));

    // A different credential id, same address: unaffected. The credential cap
    // bounds repetition against one id, not every id one address tries.
    let (mut other, other_id) = unregistered_credential(&h).await;
    let reply = present_credential(&h, &mut other, &other_id, Some(prober)).await;
    assert_eq!(reply.error_code(), Some("unknown_credential"));

    // Signing up from the same address is also unaffected: a different
    // surface, a different bucket.
    Call::post("/api/auth/signup/start")
        .header(CLIENT_IP_HEADER, prober)
        .send(&h.router)
        .await
        .expect(StatusCode::OK);
}

/// `CeremonyExpired` must never count against either bucket (#75): replaying a
/// ceremony that has already been consumed — exactly what an abandoned tab
/// looks like from the server's side — must keep answering `ceremony_expired`
/// no matter how many times it is retried, never `rate_limited`.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn ceremony_expired_is_never_charged_against_any_bucket(pool: PgPool) {
    let h = harness_behind_proxy(pool);
    let mut rob = onboard(&h, "rob@acme.test").await;
    let prober = "203.0.113.9";

    let started = Call::post("/api/auth/login/start").send(&h.router).await;
    started.expect(StatusCode::OK);
    let ceremony_id = started.body["ceremonyId"].as_str().unwrap().to_string();

    let mut challenge_value = started.body["challenge"].clone();
    challenge_value["publicKey"]["allowCredentials"] = serde_json::json!([
        { "type": "public-key", "id": rob.credential_id }
    ]);
    let challenge: webauthn_rs::prelude::RequestChallengeResponse =
        serde_json::from_value(challenge_value).unwrap();

    let credential = rob
        .auth
        .do_authentication(
            webauthn_rs::prelude::Url::parse(common::PUBLIC_URL).unwrap(),
            challenge,
        )
        .expect("the authenticator refused the sign-in challenge");

    let body = serde_json::json!({ "ceremonyId": ceremony_id, "credential": credential });

    // The first presentation succeeds and consumes the ceremony.
    Call::post("/api/auth/login/finish")
        .header(CLIENT_IP_HEADER, prober)
        .json(body.clone())
        .send(&h.router)
        .await
        .expect(StatusCode::OK);

    // Every further presentation of that same, now-consumed ceremony id must
    // report it as expired — well past the credential cap's hard limit —
    // and never once degrade into a rate-limit refusal.
    let attempts = of_auth::ratelimit::LOGIN_CRED_CAP.hard_cap + 5;
    for attempt in 1..=attempts {
        let reply = Call::post("/api/auth/login/finish")
            .header(CLIENT_IP_HEADER, prober)
            .json(body.clone())
            .send(&h.router)
            .await;
        assert_eq!(
            reply.error_code(),
            Some("ceremony_expired"),
            "attempt {attempt} should still report the expired ceremony, not a rate limit"
        );
    }
}

// -------------------------------------------------------- passkey signals

/// The console repairs credentials registered before there was anything to name
/// them with, and it needs the relying-party id to do it. That id is not the
/// page's hostname: an rp_id may be a registrable parent domain of the origin,
/// so a console reading `location.hostname` would signal against the wrong
/// relying party on the day this deployment moves to a subdomain — silently,
/// because a signal for an unknown rp_id is simply ignored.
///
/// Public because it must be: the same string is in every creation challenge an
/// unauthenticated caller can ask for.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn the_relying_party_id_is_published_and_matches_the_challenge(pool: PgPool) {
    let h = harness(pool);

    let config = Call::get("/api/auth/webauthn").send(&h.router).await;
    config.expect(StatusCode::OK);

    let host = common::PUBLIC_URL.split("://").nth(1).unwrap();
    assert_eq!(
        config.body["rpId"], host,
        "the published rp_id must be the one relying_party built the challenge from"
    );

    let started = Call::post("/api/auth/signup/start").send(&h.router).await;
    started.expect(StatusCode::OK);
    assert_eq!(
        started.body["challenge"]["publicKey"]["rp"]["id"], config.body["rpId"],
        "a console that signalled a different rp_id than the challenge carries \
         would be writing to nothing"
    );
}

/// `signalCurrentUserDetails` has to write byte-for-byte what a fresh
/// registration would write, so the pair the console sends is the pair the
/// server composes — never one TypeScript composed from the same rule.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn console_signal_matches_the_challenge(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;

    let started = Call::post("/api/me/passkeys/start")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    started.expect(StatusCode::OK);

    let me = Call::get("/api/me")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    me.expect(StatusCode::OK);

    let user = &started.body["challenge"]["publicKey"]["user"];
    assert_eq!(user["name"], me.body["credentialName"]);
    assert_eq!(user["displayName"], me.body["credentialDisplayName"]);

    assert_eq!(
        me.body["credentialName"], "rob@acme.test",
        "the address is what a credential manager sorts and searches by"
    );

    let label = me.body["user"]["label"].as_str().unwrap();
    assert!(!label.is_empty(), "every account carries generated words");
    assert_eq!(
        me.body["credentialDisplayName"],
        format!("otto-factory · {label}"),
    );
}

/// `add_passkey_finish` (`POST /api/me/passkeys/finish`) is the one call site
/// #88 gave genuinely new logic: a fresh `Parts` extractor and a `client_ip`
/// call of its own. Drive a real second-key ceremony through it and check the
/// audit row it writes carries both fields `finish_registration`'s `via`/`ip`
/// parameters exist to record — `via: "add"`, and an IP that is not null.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn add_passkey_finish_records_the_add_flow_and_its_ip(pool: PgPool) {
    let db = of_core::Db::from_pool(pool);
    let mut config = of_web::Config::new(common::PUBLIC_URL, common::RESOURCE);
    config.client_ip_header = Some("x-forwarded-for".into());
    let webauthn = of_web::relying_party(&config).expect("relying party");
    let state = of_web::AppState::new(db.clone(), common::cipher(), webauthn, config);
    let h = common::Harness {
        db,
        router: of_web::router(state),
        cipher: common::cipher(),
    };

    let rob = onboard(&h, "rob@acme.test").await;

    let started = Call::post("/api/me/passkeys/start")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    started.expect(StatusCode::OK);

    // `SoftToken` cannot hold discoverable credentials — see `of-auth`'s
    // `tests/passkeys.rs` for the full note — so the resident-key requirement
    // is dropped before it is handed the challenge. Only what the fake
    // authenticator sees is softened; the server path is the production one.
    let mut challenge: webauthn_rs::prelude::CreationChallengeResponse =
        serde_json::from_value(started.body["challenge"].clone()).unwrap();
    if let Some(selection) = challenge.public_key.authenticator_selection.as_mut() {
        selection.require_resident_key = false;
        selection.resident_key = None;
    }

    let mut second_device = common::authenticator();
    let credential = second_device
        .do_registration(
            webauthn_rs::prelude::Url::parse(common::PUBLIC_URL).unwrap(),
            challenge,
        )
        .expect("the authenticator refused the registration challenge");

    let finished = Call::post("/api/me/passkeys/finish")
        .with_session(&rob.session)
        .header("x-forwarded-for", "203.0.113.7")
        .json(serde_json::json!({
            "ceremonyId": started.body["ceremonyId"].as_str().unwrap(),
            "credential": credential,
        }))
        .send(&h.router)
        .await;
    finished.expect(StatusCode::NO_CONTENT);

    // One query for both columns, ordered by `id` rather than `created_at` —
    // `created_at` defaults to the transaction's start time and can tie.
    let row: (serde_json::Value, Option<String>) = sqlx::query_as(
        "SELECT detail->'via', ip FROM audit_events \
         WHERE action = $1 AND actor_user_id = $2 \
         ORDER BY id DESC LIMIT 1",
    )
    .bind(of_core::audit::action::PASSKEY_REGISTERED)
    .bind(rob.user)
    .fetch_one(h.db.pool())
    .await
    .unwrap();
    assert_eq!(row.0, serde_json::json!("add"));
    assert!(
        row.1.is_some(),
        "add_passkey_finish must record the caller's IP, not just signup's"
    );
    assert_eq!(row.1.as_deref(), Some("203.0.113.7"));
}

/// `signalAllAcceptedCredentials` names the credentials that still exist, and a
/// browser matches them by credential id — so a list that carries only a row's
/// UUID leaves a deleted passkey being offered in the picker forever. The
/// encoding is as load-bearing as the value: base64url **unpadded**, the same
/// alphabet the ceremony speaks, so the console compares what the server sent
/// against what the authenticator holds without re-encoding either.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn the_passkey_list_carries_the_credential_id_a_browser_matches_on(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;

    let keys = Call::get("/api/me/passkeys")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    keys.expect(StatusCode::OK);

    let listed = keys.body.as_array().unwrap();
    assert_eq!(listed.len(), 1, "onboarding registers exactly one passkey");

    for key in listed {
        let id: uuid::Uuid = key["id"].as_str().unwrap().parse().unwrap();
        let encoded = key["credentialId"]
            .as_str()
            .expect("every listed passkey carries its credential id");

        let stored: Vec<u8> =
            sqlx::query_scalar("SELECT credential_id FROM passkeys WHERE id = $1")
                .bind(id)
                .fetch_one(h.db.pool())
                .await
                .unwrap();

        assert_eq!(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(encoded)
                .expect("the credential id must be unpadded base64url"),
            stored,
            "the console signals these bytes verbatim"
        );
    }

    assert_eq!(
        listed[0]["credentialId"], rob.credential_id,
        "the id the list reports is the one the ceremony produced"
    );
}

// ------------------------------------------------------------------- orgs

/// The isolation property, at the HTTP edge: an org you are not in must be
/// indistinguishable from one that does not exist. A `403` here and a `404`
/// there turns any account into a customer directory.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn another_orgs_data_is_not_merely_forbidden_it_is_invisible(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let mallory = onboard(&h, "mallory@evil.test").await;

    org_with_owner(&h, "acme", &rob).await;
    org_with_owner(&h, "evil", &mallory).await;

    Call::post("/api/orgs/acme/repos")
        .with_session(&rob.session)
        .json(serde_json::json!({ "slug": "api", "remotes": ["git@github.com:acme/api.git"] }))
        .send(&h.router)
        .await
        .expect(StatusCode::CREATED);

    let real_org = Call::get("/api/orgs/acme/repos")
        .with_session(&mallory.session)
        .send(&h.router)
        .await;
    let imaginary_org = Call::get("/api/orgs/no-such-org/repos")
        .with_session(&mallory.session)
        .send(&h.router)
        .await;

    real_org.expect(StatusCode::NOT_FOUND);
    assert_eq!(
        real_org.status, imaginary_org.status,
        "an org you are not in must not be distinguishable from one that does not exist"
    );
    assert_eq!(real_org.error_code(), imaginary_org.error_code());
    assert!(
        !real_org.text.contains("api"),
        "the refusal leaked the other org's contents"
    );
}
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_member_cannot_do_what_an_admin_can(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let bob = onboard(&h, "bob@acme.test").await;

    let org = org_with_owner(&h, "acme", &rob).await;
    add_member(&h, org, bob.user, Role::Member).await;

    // A member reads.
    Call::get("/api/orgs/acme/members")
        .with_session(&bob.session)
        .send(&h.router)
        .await
        .expect(StatusCode::OK);
    Call::get("/api/orgs/acme/teams")
        .with_session(&bob.session)
        .send(&h.router)
        .await
        .expect(StatusCode::OK);

    // And does not write.
    for call in [
        Call::post("/api/orgs/acme/teams").json(serde_json::json!({ "slug": "platform" })),
        Call::post("/api/orgs/acme/repos").json(serde_json::json!({ "slug": "api" })),
        Call::post("/api/orgs/acme/invites").json(serde_json::json!({ "email": "eve@acme.test" })),
    ] {
        let refused = call.with_session(&bob.session).send(&h.router).await;
        refused.expect(StatusCode::FORBIDDEN);
        assert!(
            refused.text.contains("you are a member"),
            "the refusal should say what role you actually hold: {}",
            refused.text
        );
    }

    // The audit log is admin-only, unlike the rest of the reads.
    Call::get("/api/orgs/acme/audit")
        .with_session(&bob.session)
        .send(&h.router)
        .await
        .expect(StatusCode::FORBIDDEN);
}

/// An org with no owner cannot be administered by anyone, and only someone with
/// database access could repair it.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn the_last_owner_cannot_be_removed_or_demoted(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let bob = onboard(&h, "bob@acme.test").await;
    let org = org_with_owner(&h, "acme", &rob).await;
    add_member(&h, org, bob.user, Role::Admin).await;

    let demote = Call::patch(format!("/api/orgs/acme/members/{}", rob.user))
        .with_session(&rob.session)
        .json(serde_json::json!({ "role": "member" }))
        .send(&h.router)
        .await;
    demote.expect(StatusCode::CONFLICT);
    assert_eq!(demote.error_code(), Some("last_owner"));

    let leave = Call::delete(format!("/api/orgs/acme/members/{}", rob.user))
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    leave.expect(StatusCode::CONFLICT);

    // With a second owner in place, both become possible.
    Call::patch(format!("/api/orgs/acme/members/{}", bob.user))
        .with_session(&rob.session)
        .json(serde_json::json!({ "role": "owner" }))
        .send(&h.router)
        .await
        .expect(StatusCode::NO_CONTENT);

    Call::delete(format!("/api/orgs/acme/members/{}", rob.user))
        .with_session(&rob.session)
        .send(&h.router)
        .await
        .expect(StatusCode::NO_CONTENT);
}

/// An admin who could promote themselves to owner is an admin with owner
/// powers one request away.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn only_an_owner_may_create_another_owner(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let bob = onboard(&h, "bob@acme.test").await;
    let org = org_with_owner(&h, "acme", &rob).await;
    add_member(&h, org, bob.user, Role::Admin).await;

    let self_promotion = Call::patch(format!("/api/orgs/acme/members/{}", bob.user))
        .with_session(&bob.session)
        .json(serde_json::json!({ "role": "owner" }))
        .send(&h.router)
        .await;
    self_promotion.expect(StatusCode::FORBIDDEN);

    let demote_the_owner = Call::patch(format!("/api/orgs/acme/members/{}", rob.user))
        .with_session(&bob.session)
        .json(serde_json::json!({ "role": "member" }))
        .send(&h.router)
        .await;
    demote_the_owner.expect(StatusCode::FORBIDDEN);
}

/// Deletion is strictly stronger than demotion: an admin blocked from
/// demoting an owner (above) must not reach the same outcome by removing
/// them outright. A second owner keeps the last-owner guard from being the
/// thing that blocks the request, isolating the privilege check.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn an_admin_cannot_remove_an_owner(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let bob = onboard(&h, "bob@acme.test").await;
    let carol = onboard(&h, "carol@acme.test").await;
    let org = org_with_owner(&h, "acme", &rob).await;
    add_member(&h, org, bob.user, Role::Owner).await;
    add_member(&h, org, carol.user, Role::Admin).await;

    let remove_owner = Call::delete(format!("/api/orgs/acme/members/{}", bob.user))
        .with_session(&carol.session)
        .send(&h.router)
        .await;
    remove_owner.expect(StatusCode::FORBIDDEN);

    // An owner may still remove another owner.
    Call::delete(format!("/api/orgs/acme/members/{}", bob.user))
        .with_session(&rob.session)
        .send(&h.router)
        .await
        .expect(StatusCode::NO_CONTENT);
}

/// Removing someone has to disconnect their agents too. A token that outlives
/// the membership it was granted under is the interesting failure here — the
/// console would show them gone while their agent kept working the queue.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn removing_a_member_revokes_their_tokens_for_that_org(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let bob = onboard(&h, "bob@acme.test").await;

    let acme = org_with_owner(&h, "acme", &rob).await;
    add_member(&h, acme, bob.user, Role::Member).await;
    // Bob is also in an unrelated org, which must be untouched.
    org_with_owner(&h, "globex", &bob).await;

    let minted = Call::post("/api/orgs/acme/tokens")
        .with_session(&bob.session)
        .json(serde_json::json!({ "name": "bob's laptop" }))
        .send(&h.router)
        .await;
    minted.expect(StatusCode::CREATED);
    let token = minted.body["token"].as_str().unwrap().to_string();

    let elsewhere = Call::post("/api/orgs/globex/tokens")
        .with_session(&bob.session)
        .json(serde_json::json!({ "name": "same laptop, other org" }))
        .send(&h.router)
        .await;
    elsewhere.expect(StatusCode::CREATED);
    let other_token = elsewhere.body["token"].as_str().unwrap().to_string();

    // The token works before removal.
    of_auth::tokens::introspect(&h.db, &token, common::RESOURCE)
        .await
        .expect("a freshly minted token should introspect");

    Call::delete(format!("/api/orgs/acme/members/{}", bob.user))
        .with_session(&rob.session)
        .send(&h.router)
        .await
        .expect(StatusCode::NO_CONTENT);

    assert!(
        of_auth::tokens::introspect(&h.db, &token, common::RESOURCE)
            .await
            .is_err(),
        "a removed member's agent kept a working token"
    );
    of_auth::tokens::introspect(&h.db, &other_token, common::RESOURCE)
        .await
        .expect("their token for an unrelated org must survive");

    // And their session is untouched: it is how they reach that other org.
    Call::get("/api/me")
        .with_session(&bob.session)
        .send(&h.router)
        .await
        .expect(StatusCode::OK);
}
// --------------------------------------------------------------- language

/// The console language, over the wire, in all three of its states.
///
/// It lives on the account rather than in one browser's `localStorage` so the
/// choice follows the person to their next device. `null` is the state that
/// needs the most care: it is how somebody goes back to "match my browser",
/// and serde would collapse it into "leave alone" without the `double_option`
/// on `ProfileRequest`.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn the_console_language_is_set_cleared_and_left_alone(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;

    let fresh = Call::get("/api/me")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    fresh.expect(StatusCode::OK);
    assert!(
        fresh.body["user"]["locale"].is_null(),
        "a new account has chosen nothing — which is not the same as choosing English"
    );

    let set = Call::patch("/api/me")
        .with_session(&rob.session)
        .json(serde_json::json!({ "locale": "de" }))
        .send(&h.router)
        .await;
    set.expect(StatusCode::OK);
    assert_eq!(set.body["locale"], "de");

    // Absent leaves it alone, which is what every other PATCH field does.
    let renamed = Call::patch("/api/me")
        .with_session(&rob.session)
        .json(serde_json::json!({ "name": "Rob" }))
        .send(&h.router)
        .await;
    renamed.expect(StatusCode::OK);
    assert_eq!(
        renamed.body["locale"], "de",
        "omitting the field must not clear the language"
    );

    // An explicit null is the only way back to following the browser.
    let cleared = Call::patch("/api/me")
        .with_session(&rob.session)
        .json(serde_json::json!({ "locale": null }))
        .send(&h.router)
        .await;
    cleared.expect(StatusCode::OK);
    assert!(
        cleared.body["locale"].is_null(),
        "an explicit null must clear the language, not read as 'leave alone'"
    );
}

#[sqlx::test(migrations = "../of-core/migrations")]
async fn an_unsupported_language_is_refused_by_name(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;

    let refused = Call::patch("/api/me")
        .with_session(&rob.session)
        .json(serde_json::json!({ "locale": "klingon" }))
        .send(&h.router)
        .await;
    refused.expect(StatusCode::BAD_REQUEST);
    assert_eq!(refused.error_code(), Some("invalid_argument"));

    let message = refused.body["error"]["message"].as_str().unwrap();
    for locale in of_core::i18n::SUPPORTED_LOCALES {
        assert!(
            message.contains(locale),
            "{message:?} should name the supported locale {locale}"
        );
    }

    // An empty string is refused too, rather than quietly reading as
    // "leave alone" — a broken picker should look broken.
    Call::patch("/api/me")
        .with_session(&rob.session)
        .json(serde_json::json!({ "locale": "" }))
        .send(&h.router)
        .await
        .expect(StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------- invites

#[sqlx::test(migrations = "../of-core/migrations")]
async fn an_invitation_code_is_handed_back_accepted_once_and_grants_its_role(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    org_with_owner(&h, "acme", &rob).await;
    let bob = onboard(&h, "bob@acme.test").await;

    let invited = Call::post("/api/orgs/acme/invites")
        .with_session(&rob.session)
        .json(serde_json::json!({ "email": "bob@acme.test", "role": "admin" }))
        .send(&h.router)
        .await;
    invited.expect(StatusCode::CREATED);

    // The code comes back to the admin — there is no mailbox it went to
    // instead — and the link is the same secret wrapped in a console URL.
    let token = invited.body["code"].as_str().expect("no code").to_string();
    assert!(token.starts_with("of_inv_"), "unexpected code: {token}");
    assert_eq!(
        invited.body["link"],
        format!("https://console.otto-factory.test/invite/acme?token={token}")
    );

    let pending = Call::get("/api/orgs/acme/invites")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    pending.expect(StatusCode::OK);
    assert_eq!(pending.body.as_array().unwrap().len(), 1);

    let joined = Call::post("/api/orgs/acme/invites/accept")
        .with_session(&bob.session)
        .json(serde_json::json!({ "token": token }))
        .send(&h.router)
        .await;
    joined.expect(StatusCode::OK);
    assert_eq!(joined.body["role"], "admin");

    // Bob can now do admin things, and the invitation is spent.
    Call::post("/api/orgs/acme/teams")
        .with_session(&bob.session)
        .json(serde_json::json!({ "slug": "platform" }))
        .send(&h.router)
        .await
        .expect(StatusCode::CREATED);

    let replayed = Call::post("/api/orgs/acme/invites/accept")
        .with_session(&bob.session)
        .json(serde_json::json!({ "token": token }))
        .send(&h.router)
        .await;
    replayed.expect(StatusCode::GONE);
}

/// A forwarded invitation mail must not be a way into someone else's org.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn an_invitation_cannot_be_accepted_by_the_wrong_account(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    org_with_owner(&h, "acme", &rob).await;
    let mallory = onboard(&h, "mallory@evil.test").await;

    let invited = Call::post("/api/orgs/acme/invites")
        .with_session(&rob.session)
        .json(serde_json::json!({ "email": "bob@acme.test" }))
        .send(&h.router)
        .await;
    invited.expect(StatusCode::CREATED);

    let token = invited.body["code"].as_str().unwrap().to_string();
    let refused = Call::post("/api/orgs/acme/invites/accept")
        .with_session(&mallory.session)
        .json(serde_json::json!({ "token": token }))
        .send(&h.router)
        .await;
    refused.expect(StatusCode::FORBIDDEN);
    assert_eq!(refused.error_code(), Some("invite_wrong_account"));
    assert!(
        h.db.member_role(
            h.db.get_org_by_slug("acme").await.unwrap().unwrap().id,
            mallory.user
        )
        .await
        .unwrap()
        .is_none(),
        "the wrong account was admitted"
    );

    // And the invitation is still there for the right person.
    let bob = onboard(&h, "bob@acme.test").await;
    Call::post("/api/orgs/acme/invites/accept")
        .with_session(&bob.session)
        .json(serde_json::json!({ "token": token }))
        .send(&h.router)
        .await
        .expect(StatusCode::OK);
}

#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_withdrawn_invitation_stops_working(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    org_with_owner(&h, "acme", &rob).await;
    let bob = onboard(&h, "bob@acme.test").await;

    let invited = Call::post("/api/orgs/acme/invites")
        .with_session(&rob.session)
        .json(serde_json::json!({ "email": "bob@acme.test" }))
        .send(&h.router)
        .await;
    invited.expect(StatusCode::CREATED);
    let id = invited.body["id"].as_str().unwrap().to_string();
    let token = invited.body["code"].as_str().unwrap().to_string();

    Call::delete(format!("/api/orgs/acme/invites/{id}"))
        .with_session(&rob.session)
        .send(&h.router)
        .await
        .expect(StatusCode::NO_CONTENT);

    Call::post("/api/orgs/acme/invites/accept")
        .with_session(&bob.session)
        .json(serde_json::json!({ "token": token }))
        .send(&h.router)
        .await
        .expect(StatusCode::GONE);
}
/// Only an owner may hand out ownership, whether directly or by invitation —
/// otherwise the invite endpoint is a way around the role check on members.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn an_admin_cannot_invite_an_owner(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let bob = onboard(&h, "bob@acme.test").await;
    let org = org_with_owner(&h, "acme", &rob).await;
    add_member(&h, org, bob.user, Role::Admin).await;

    let refused = Call::post("/api/orgs/acme/invites")
        .with_session(&bob.session)
        .json(serde_json::json!({ "email": "eve@acme.test", "role": "owner" }))
        .send(&h.router)
        .await;
    refused.expect(StatusCode::FORBIDDEN);

    // The same admin may invite an ordinary member.
    Call::post("/api/orgs/acme/invites")
        .with_session(&bob.session)
        .json(serde_json::json!({ "email": "eve@acme.test", "role": "member" }))
        .send(&h.router)
        .await
        .expect(StatusCode::CREATED);
}

// ------------------------------------------------------------------ teams

#[sqlx::test(migrations = "../of-core/migrations")]
async fn teams_scope_repos_and_refuse_to_widen_them_on_delete(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    org_with_owner(&h, "acme", &rob).await;

    let team = Call::post("/api/orgs/acme/teams")
        .with_session(&rob.session)
        .json(serde_json::json!({ "slug": "Platform", "name": "Platform Engineering" }))
        .send(&h.router)
        .await;
    team.expect(StatusCode::CREATED);
    assert_eq!(team.body["slug"], "platform", "slugs are lowercased");
    let team_id = team.body["id"].as_str().unwrap().to_string();

    Call::put(format!(
        "/api/orgs/acme/teams/platform/members/{}",
        rob.user
    ))
    .with_session(&rob.session)
    .send(&h.router)
    .await
    .expect(StatusCode::NO_CONTENT);

    let roster = Call::get("/api/orgs/acme/teams/platform/members")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    roster.expect(StatusCode::OK);
    assert_eq!(roster.body.as_array().unwrap().len(), 1);

    Call::post("/api/orgs/acme/repos")
        .with_session(&rob.session)
        .json(serde_json::json!({ "slug": "api", "teamId": team_id }))
        .send(&h.router)
        .await
        .expect(StatusCode::CREATED);

    let refused = Call::delete("/api/orgs/acme/teams/platform")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    refused.expect(StatusCode::CONFLICT);
    assert_eq!(refused.error_code(), Some("team_in_use"));
    assert!(
        refused.text.contains("api"),
        "the refusal should name the repo"
    );

    // Unassign the repo, and the delete goes through.
    Call::patch("/api/orgs/acme/repos/api")
        .with_session(&rob.session)
        .json(serde_json::json!({ "teamId": null }))
        .send(&h.router)
        .await
        .expect(StatusCode::OK);

    Call::delete("/api/orgs/acme/teams/platform")
        .with_session(&rob.session)
        .send(&h.router)
        .await
        .expect(StatusCode::NO_CONTENT);
}

#[sqlx::test(migrations = "../of-core/migrations")]
async fn an_unknown_team_slug_names_the_alternatives(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    org_with_owner(&h, "acme", &rob).await;

    Call::post("/api/orgs/acme/teams")
        .with_session(&rob.session)
        .json(serde_json::json!({ "slug": "platform" }))
        .send(&h.router)
        .await
        .expect(StatusCode::CREATED);

    let missed = Call::get("/api/orgs/acme/teams/platfrom")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    missed.expect(StatusCode::NOT_FOUND);
    assert!(
        missed.text.contains("platform"),
        "an error that does not name the alternatives makes a caller guess: {}",
        missed.text
    );
}

// ------------------------------------------------------------------ queue

/// Enqueue through `of-core`, because the console cannot.
///
/// That asymmetry is the point of the queue routes: a job is created and
/// completed by the agent doing the work, over MCP. Reaching past the API to
/// set up this fixture is not a shortcut around a route that exists — it is the
/// only way to reach the state, and a test that could enqueue over HTTP would
/// be evidence of a route that should not be there.
async fn enqueue(
    h: &common::Harness,
    org: of_core::ids::OrgId,
    repo: of_core::ids::RepoId,
    title: &str,
    created_by: of_core::ids::UserId,
) -> of_core::jobs::Job {
    let mut tx = h.db.begin(org).await.unwrap();
    let job = tx
        .add_job(of_core::jobs::NewJob {
            repo_id: repo,
            title: title.into(),
            created_by: Some(created_by),
            ..Default::default()
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    job
}

#[sqlx::test(migrations = "../of-core/migrations")]
async fn the_queue_view_lists_filters_and_counts(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let sam = onboard(&h, "sam@acme.test").await;
    let acme = org_with_owner(&h, "acme", &rob).await;
    add_member(&h, acme, sam.user, Role::Member).await;

    let api: of_core::ids::RepoId = {
        let created = Call::post("/api/orgs/acme/repos")
            .with_session(&rob.session)
            .json(serde_json::json!({ "slug": "api" }))
            .send(&h.router)
            .await;
        created.expect(StatusCode::CREATED);
        created.body["id"].as_str().unwrap().parse().unwrap()
    };
    let web: of_core::ids::RepoId = {
        let created = Call::post("/api/orgs/acme/repos")
            .with_session(&rob.session)
            .json(serde_json::json!({ "slug": "web" }))
            .send(&h.router)
            .await;
        created.expect(StatusCode::CREATED);
        created.body["id"].as_str().unwrap().parse().unwrap()
    };

    let first = enqueue(&h, acme, api, "rewrite the resolver", rob.user).await;
    enqueue(&h, acme, api, "add a lease test", rob.user).await;
    enqueue(&h, acme, web, "fix the meter", sam.user).await;

    // Claiming one moves it off `pending`, which is what makes the status
    // filter and the counters worth asserting separately.
    {
        let mut tx = h.db.begin(acme).await.unwrap();
        tx.claim_jobs(
            std::slice::from_ref(&first.id),
            rob.user,
            Some("claude-code"),
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    let all = Call::get("/api/orgs/acme/jobs")
        .with_session(&sam.session)
        .send(&h.router)
        .await;
    all.expect(StatusCode::OK);
    assert_eq!(
        all.body.as_array().unwrap().len(),
        3,
        "any member sees the whole org's queue"
    );

    let pending = Call::get("/api/orgs/acme/jobs?status=pending")
        .with_session(&sam.session)
        .send(&h.router)
        .await;
    pending.expect(StatusCode::OK);
    assert_eq!(pending.body.as_array().unwrap().len(), 2);

    let in_repo = Call::get("/api/orgs/acme/jobs?repo=web")
        .with_session(&sam.session)
        .send(&h.router)
        .await;
    in_repo.expect(StatusCode::OK);
    assert_eq!(in_repo.body.as_array().unwrap().len(), 1);
    assert_eq!(in_repo.body[0]["title"], "fix the meter");

    let mine = Call::get("/api/orgs/acme/jobs?mine=true")
        .with_session(&sam.session)
        .send(&h.router)
        .await;
    mine.expect(StatusCode::OK);
    assert_eq!(
        mine.body.as_array().unwrap().len(),
        1,
        "`mine` is the caller, not the org"
    );

    let stats = Call::get("/api/orgs/acme/jobs/stats")
        .with_session(&sam.session)
        .send(&h.router)
        .await;
    stats.expect(StatusCode::OK);
    assert_eq!(stats.body["total"], 3);
    assert_eq!(stats.body["pending"], 2);
    assert_eq!(stats.body["inProgress"], 1);
    assert_eq!(stats.body["blocked"], 0);

    // `/jobs/stats` and `/jobs/{job}` share a prefix; a router that resolved
    // the literal to the parameter would answer this with a 404 for a job
    // called "stats".
    let one = Call::get(format!("/api/orgs/acme/jobs/{}", first.id))
        .with_session(&sam.session)
        .send(&h.router)
        .await;
    one.expect(StatusCode::OK);
    assert_eq!(one.body["title"], "rewrite the resolver");
    assert_eq!(one.body["status"], "in-progress");
    assert_eq!(one.body["claimedByLabel"], "claude-code");
    assert_eq!(one.body["dependsOn"].as_array().unwrap().len(), 0);

    let repo_stats = Call::get("/api/orgs/acme/jobs/stats?repo=api")
        .with_session(&sam.session)
        .send(&h.router)
        .await;
    repo_stats.expect(StatusCode::OK);
    assert_eq!(repo_stats.body["total"], 2);
}

/// `active` is a `of-core` state the console never sets — it only has to
/// read it back correctly through the same read-only routes every other
/// status already goes through.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn an_active_job_is_visible_in_the_queue_and_its_stats(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let acme = org_with_owner(&h, "acme", &rob).await;

    let api: of_core::ids::RepoId = {
        let created = Call::post("/api/orgs/acme/repos")
            .with_session(&rob.session)
            .json(serde_json::json!({ "slug": "api" }))
            .send(&h.router)
            .await;
        created.expect(StatusCode::CREATED);
        created.body["id"].as_str().unwrap().parse().unwrap()
    };

    let job = enqueue(&h, acme, api, "activate then check", rob.user).await;
    {
        let mut tx = h.db.begin(acme).await.unwrap();
        tx.claim_jobs(
            std::slice::from_ref(&job.id),
            rob.user,
            Some("agent-one"),
            None,
        )
        .await
        .unwrap();
        tx.activate_job(&job.id).await.unwrap();
        tx.commit().await.unwrap();
    }

    let active = Call::get("/api/orgs/acme/jobs?status=active")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    active.expect(StatusCode::OK);
    let active_jobs = active.body.as_array().unwrap();
    assert_eq!(active_jobs.len(), 1);
    assert_eq!(active_jobs[0]["id"], job.id.to_string());

    let stats = Call::get("/api/orgs/acme/jobs/stats")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    stats.expect(StatusCode::OK);
    assert_eq!(stats.body["active"], 1);
    assert_eq!(stats.body["inProgress"], 0);
}

#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_job_detail_names_what_it_is_waiting_for(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let acme = org_with_owner(&h, "acme", &rob).await;

    let created = Call::post("/api/orgs/acme/repos")
        .with_session(&rob.session)
        .json(serde_json::json!({ "slug": "api" }))
        .send(&h.router)
        .await;
    created.expect(StatusCode::CREATED);
    let api: of_core::ids::RepoId = created.body["id"].as_str().unwrap().parse().unwrap();

    let blocker = enqueue(&h, acme, api, "land the migration", rob.user).await;
    let blocked = {
        let mut tx = h.db.begin(acme).await.unwrap();
        let job = tx
            .add_job(of_core::jobs::NewJob {
                repo_id: api,
                title: "use the new column".into(),
                depends_on: vec![blocker.id.clone()],
                created_by: Some(rob.user),
                ..Default::default()
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
        job
    };

    let detail = Call::get(format!("/api/orgs/acme/jobs/{}", blocked.id))
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    detail.expect(StatusCode::OK);
    assert_eq!(detail.body["dependsOn"][0], blocker.id.as_str());

    // A blocked job is still pending. The overview counts it in both, and the
    // difference is the whole reason `blocked` is reported at all: two pending
    // jobs where one cannot start is not the same queue as two that can.
    let stats = Call::get("/api/orgs/acme/jobs/stats")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    stats.expect(StatusCode::OK);
    assert_eq!(stats.body["pending"], 2);
    assert_eq!(stats.body["blocked"], 1);
}

#[sqlx::test(migrations = "../of-core/migrations")]
async fn an_unknown_repo_filter_names_the_registered_slugs(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    org_with_owner(&h, "acme", &rob).await;

    Call::post("/api/orgs/acme/repos")
        .with_session(&rob.session)
        .json(serde_json::json!({ "slug": "api" }))
        .send(&h.router)
        .await
        .expect(StatusCode::CREATED);

    // Not an empty list. A filter that silently matched nothing would render a
    // queue that looks quiet rather than one that was never asked about.
    let missed = Call::get("/api/orgs/acme/jobs?repo=apo")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    missed.expect(StatusCode::NOT_FOUND);
    assert!(
        missed.text.contains("api"),
        "the error should name what is registered: {}",
        missed.text
    );

    let bad_status = Call::get("/api/orgs/acme/jobs?status=done")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    bad_status.expect(StatusCode::BAD_REQUEST);
    assert!(
        bad_status.text.contains("completed"),
        "an unknown status should list the valid ones: {}",
        bad_status.text
    );
}

#[sqlx::test(migrations = "../of-core/migrations")]
async fn one_orgs_queue_is_invisible_to_another(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let mal = onboard(&h, "mal@other.test").await;
    let acme = org_with_owner(&h, "acme", &rob).await;
    org_with_owner(&h, "other", &mal).await;

    let created = Call::post("/api/orgs/acme/repos")
        .with_session(&rob.session)
        .json(serde_json::json!({ "slug": "api" }))
        .send(&h.router)
        .await;
    created.expect(StatusCode::CREATED);
    let api: of_core::ids::RepoId = created.body["id"].as_str().unwrap().parse().unwrap();
    let job = enqueue(&h, acme, api, "acme's secret roadmap", rob.user).await;

    // 404, not 403: a 403 on a real slug and a 404 on a fake one turns any
    // signed-in account into a directory of who uses the product.
    for uri in [
        "/api/orgs/acme/jobs".to_string(),
        "/api/orgs/acme/jobs/stats".to_string(),
        format!("/api/orgs/acme/jobs/{}", job.id),
    ] {
        let refused = Call::get(&uri)
            .with_session(&mal.session)
            .send(&h.router)
            .await;
        refused.expect(StatusCode::NOT_FOUND);
        assert!(
            !refused.text.contains("secret roadmap"),
            "{uri} leaked another org's job"
        );
    }

    // And the same job id, asked for from an org that has one of its own, is
    // that org's job or nothing — ids are per-org counters, so `job-1` exists
    // in both and must not cross.
    let elsewhere = Call::get("/api/orgs/other/jobs/job-1")
        .with_session(&mal.session)
        .send(&h.router)
        .await;
    elsewhere.expect(StatusCode::NOT_FOUND);
}

// ----------------------------------------------------------------- leases

/// Leases, like jobs, are written by an agent over MCP, never by the console
/// — there is no route to create one here, so seeding one for this fixture
/// means reaching past the API into `of-core`, the same way `enqueue` does
/// for jobs above.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn the_lease_route_reports_the_resource_field(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let acme = org_with_owner(&h, "acme", &rob).await;

    let api: of_core::ids::RepoId = {
        let created = Call::post("/api/orgs/acme/repos")
            .with_session(&rob.session)
            .json(serde_json::json!({ "slug": "api" }))
            .send(&h.router)
            .await;
        created.expect(StatusCode::CREATED);
        created.body["id"].as_str().unwrap().parse().unwrap()
    };

    {
        let mut tx = h.db.begin(acme).await.unwrap();
        tx.acquire_lease(
            api,
            "src/main.rs",
            rob.user,
            Some("claude-code"),
            None,
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    let leases = Call::get("/api/orgs/acme/repos/api/leases")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    leases.expect(StatusCode::OK);
    let all = leases.body.as_array().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(
        all[0]["resource"], "src/main.rs",
        "the console reads of_core::leases::Lease verbatim, so its wire shape \
         must carry `resource`, not `branch`: {}",
        leases.body
    );
}

// --------------------------------------------------------- tokens & usage

#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_pat_is_shown_once_audienced_for_mcp_and_revocable(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    org_with_owner(&h, "acme", &rob).await;

    let minted = Call::post("/api/orgs/acme/tokens")
        .with_session(&rob.session)
        .json(serde_json::json!({ "name": "laptop", "scopes": ["jobs:read", "jobs:write"] }))
        .send(&h.router)
        .await;
    minted.expect(StatusCode::CREATED);

    let token = minted.body["token"].as_str().unwrap().to_string();
    let id = minted.body["id"].as_str().unwrap().to_string();
    assert!(token.starts_with("of_pat_"));
    assert_eq!(minted.body["resource"], common::RESOURCE);

    let principal = of_auth::tokens::introspect(&h.db, &token, common::RESOURCE)
        .await
        .expect("a minted PAT must introspect against the MCP resource");
    assert!(principal.has_scope("jobs:write"));

    // Audienced: a token for this resource is refused by any other.
    assert!(
        of_auth::tokens::introspect(&h.db, &token, "https://someone-else.test/mcp")
            .await
            .is_err(),
        "the audience check is what stops a confused deputy"
    );

    // Listed without the secret.
    let listed = Call::get("/api/orgs/acme/tokens")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    listed.expect(StatusCode::OK);
    assert_eq!(listed.body.as_array().unwrap().len(), 1);
    assert!(
        !listed.text.contains(&token),
        "the token itself must never be listed back"
    );

    Call::delete(format!("/api/orgs/acme/tokens/{id}"))
        .with_session(&rob.session)
        .send(&h.router)
        .await
        .expect(StatusCode::NO_CONTENT);

    assert!(
        of_auth::tokens::introspect(&h.db, &token, common::RESOURCE)
            .await
            .is_err(),
        "revocation must take effect immediately"
    );
}

/// A PAT must not be a way to obtain a scope the console would not grant.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_member_cannot_mint_an_admin_scoped_token(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let bob = onboard(&h, "bob@acme.test").await;
    let org = org_with_owner(&h, "acme", &rob).await;
    add_member(&h, org, bob.user, Role::Member).await;

    let refused = Call::post("/api/orgs/acme/tokens")
        .with_session(&bob.session)
        .json(serde_json::json!({ "name": "sneaky", "scopes": ["org:admin"] }))
        .send(&h.router)
        .await;
    refused.expect(StatusCode::FORBIDDEN);

    let unknown_scope = Call::post("/api/orgs/acme/tokens")
        .with_session(&bob.session)
        .json(serde_json::json!({ "name": "typo", "scopes": ["jobs:destroy"] }))
        .send(&h.router)
        .await;
    unknown_scope.expect(StatusCode::BAD_REQUEST);
    assert!(
        unknown_scope.text.contains("jobs:read"),
        "an unknown scope should list the supported ones: {}",
        unknown_scope.text
    );
}

/// You cannot revoke another member's token even holding its id.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn one_member_cannot_revoke_anothers_token(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let bob = onboard(&h, "bob@acme.test").await;
    let org = org_with_owner(&h, "acme", &rob).await;
    add_member(&h, org, bob.user, Role::Member).await;

    let minted = Call::post("/api/orgs/acme/tokens")
        .with_session(&bob.session)
        .json(serde_json::json!({ "name": "bob's" }))
        .send(&h.router)
        .await;
    minted.expect(StatusCode::CREATED);
    let id = minted.body["id"].as_str().unwrap().to_string();
    let token = minted.body["token"].as_str().unwrap().to_string();

    // Even the owner cannot reach into someone else's credential from here.
    Call::delete(format!("/api/orgs/acme/tokens/{id}"))
        .with_session(&rob.session)
        .send(&h.router)
        .await
        .expect(StatusCode::NOT_FOUND);

    of_auth::tokens::introspect(&h.db, &token, common::RESOURCE)
        .await
        .expect("the token should still be live");
}

#[sqlx::test(migrations = "../of-core/migrations")]
async fn usage_reads_the_same_numbers_the_agent_sees(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let org = org_with_owner(&h, "acme", &rob).await;

    // Meter something the way a tool call would.
    let mut tx = h.db.begin(org).await.unwrap();
    tx.record_usage(Some(rob.user), "add_job", true)
        .await
        .unwrap();
    tx.record_usage(Some(rob.user), "watch", false)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let usage = Call::get("/api/orgs/acme/usage")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    usage.expect(StatusCode::OK);

    assert_eq!(usage.body["plan"], "Free");
    assert_eq!(usage.body["billableUsed"], 1, "only billable calls count");
    assert_eq!(usage.body["totalCalls"], 2, "every call is recorded");
    assert_eq!(
        usage.body["enforced"], false,
        "enforcement is off by default"
    );
}

// ---------------------------------------------------------------- openapi

/// The document is served, describes the routes that exist, and needs no
/// credential — a client generator cannot log in.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn the_openapi_document_is_public_and_describes_the_surface(pool: PgPool) {
    let h = harness(pool);
    let doc = Call::get("/api/openapi.json").send(&h.router).await;
    doc.expect(StatusCode::OK);

    assert_eq!(doc.body["openapi"], "3.1.0");
    assert!(doc.body["paths"]["/api/orgs/{org}/repos"]["post"].is_object());
    assert_eq!(
        doc.body["components"]["securitySchemes"]["sessionCookie"]["name"],
        "__Host-of_session"
    );
}

/// The OpenAPI document's version matches the workspace version. This pins
/// existing-correct behavior: `openapi.rs` uses `env!("CARGO_PKG_VERSION")`,
/// which in turn resolves to the `of-web` Cargo.toml's version field — set to
/// `version.workspace = true`, so it reads from the workspace root.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn the_openapi_document_version_matches_workspace_version(pool: PgPool) {
    let h = harness(pool);
    let doc = Call::get("/api/openapi.json").send(&h.router).await;
    doc.expect(StatusCode::OK);

    assert_eq!(
        doc.body["info"]["version"],
        env!("CARGO_PKG_VERSION"),
        "OpenAPI document version must match the workspace version"
    );
}

/// The console displays `web/package.json`'s `version` in its footer
/// (`web/src/lib/version.ts`); this proves that value is actually the
/// workspace version and not something that has drifted from it.
/// `include_str!` rather than `std::fs::read_to_string` so a missing or
/// unparseable file fails the build, not a passing test that never ran.
#[test]
fn the_console_and_the_server_agree_on_the_version() {
    const PACKAGE_JSON: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../web/package.json"
    ));
    let package: serde_json::Value = serde_json::from_str(PACKAGE_JSON).expect("web/package.json");
    assert_eq!(
        package["version"].as_str(),
        Some(env!("CARGO_PKG_VERSION")),
        "web/package.json and the workspace version disagree — the console footer would \
         display a version the server was not built from"
    );
}

/// Routes are mounted from the same list the document is rendered from, so
/// anything described has to actually answer. This catches the failure the
/// catalog exists to prevent — a documented endpoint that is not mounted.
///
/// A handler's own `404` ("no such repo") is a pass; the router's is not. They
/// are told apart by the body: every failure this crate produces carries the
/// `{"error": {...}}` envelope, and a route that does not exist produces an
/// empty one.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn every_documented_get_is_actually_mounted(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    org_with_owner(&h, "acme", &rob).await;

    let doc = Call::get("/api/openapi.json").send(&h.router).await;
    let paths = doc.body["paths"].as_object().unwrap().clone();
    let mut checked = 0;

    for (path, operations) in paths {
        if operations.get("get").is_none() {
            continue;
        }
        let concrete = path
            .replace("{org}", "acme")
            .replace("{team}", "platform")
            .replace("{repo}", "api")
            .replace("{user}", &rob.user.to_string())
            .replace("{id}", &uuid::Uuid::nil().to_string());

        let reply = Call::get(&concrete)
            .with_session(&rob.session)
            .send(&h.router)
            .await;
        checked += 1;

        assert_ne!(
            reply.status,
            StatusCode::METHOD_NOT_ALLOWED,
            "GET {concrete} is documented but the path serves other methods only"
        );
        if reply.status == StatusCode::NOT_FOUND {
            assert!(
                reply.error_code().is_some(),
                "GET {concrete} is documented but not mounted — the 404 came from \
                 the router, not from a handler (body was {:?})",
                reply.text
            );
        }
    }

    assert!(
        checked > 10,
        "only {checked} GETs were checked; the document looks empty"
    );
}

/// The only assisted way back into an account, and the limits that make it
/// safe to have. Without email there is nothing else, so this endpoint carries
/// weight the mailed recovery link used to.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn an_admin_can_reset_a_members_authenticator_but_gains_nothing_by_it(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let acme = org_with_owner(&h, "acme", &rob).await;
    let mut bob = onboard(&h, "bob@acme.test").await;
    add_member(&h, acme, bob.user, of_core::orgs::Role::Member).await;

    let reset = Call::post(format!(
        "/api/orgs/acme/members/{}/reset-passkeys",
        bob.user
    ))
    .with_session(&rob.session)
    .send(&h.router)
    .await;
    reset.expect(StatusCode::CREATED);

    let audit = Call::get("/api/orgs/acme/audit?actionPrefix=org.member.passkeys_reset")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    audit.expect(StatusCode::OK);
    let rows = audit
        .body
        .as_array()
        .expect("audit response must be an array");
    assert_eq!(
        rows.len(),
        1,
        "the reset must write exactly one org.member.passkeys_reset row"
    );
    assert_eq!(
        rows[0]["actorUserId"].as_str().unwrap(),
        rob.user.to_string()
    );
    assert_eq!(rows[0]["targetId"].as_str().unwrap(), bob.user.to_string());

    // This path writes only org.member.passkeys_reset, not a second
    // auth.passkey.cleared row — that second write existed once, best-effort
    // and NULL-org, on the theory it mirrored a self-service clear (which
    // does not exist in production), and was dropped rather than merely
    // re-scoped when #134 made this call site atomic. See
    // `of_auth::passkeys::clear`'s doc comment.
    let cleared: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = $1 AND target_id = $2",
    )
    .bind(of_core::audit::action::PASSKEY_CLEARED)
    .bind(bob.user.to_string())
    .fetch_one(h.db.pool())
    .await
    .unwrap();
    assert_eq!(
        cleared, 0,
        "reset_member_passkeys must not write a redundant auth.passkey.cleared \
         row alongside org.member.passkeys_reset"
    );

    let stale_action = Call::get("/api/orgs/acme/audit?actionPrefix=auth.totp")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    stale_action.expect(StatusCode::OK);
    assert_eq!(
        stale_action.body.as_array().unwrap().len(),
        0,
        "the reset must not write the historical auth.totp.reset action"
    );

    // Bob's old passkey is gone...
    let stale = sign_in(&h, &mut bob).await;
    assert_ne!(stale.status, StatusCode::OK, "the old passkey still works");

    // ...and so is his session, so a reset actually interrupts whoever holds
    // the account rather than leaving them running.
    let dead = Call::get("/api/me")
        .with_session(&bob.session)
        .send(&h.router)
        .await;
    assert_eq!(dead.status, StatusCode::UNAUTHORIZED);

    // The admin gained nothing directly: no session was handed to Rob for Bob.
    // What Rob holds is the claim code, and it is the *only* way back — an
    // account with no passkeys is otherwise claimable by whoever reaches
    // registration first, which is the takeover this endpoint exists to close.
    let code = reset.body["code"]
        .as_str()
        .expect("no claim code")
        .to_string();

    let mut new_device = common::authenticator();
    let started = Call::post("/api/auth/claim/start")
        .json(serde_json::json!({ "code": code }))
        .send(&h.router)
        .await;
    started.expect(StatusCode::OK);

    let reclaimed = common::finish_registration(
        &h,
        &mut new_device,
        "/api/auth/claim/finish",
        &started.body,
        serde_json::json!({ "code": code }),
    )
    .await;
    reclaimed.expect(StatusCode::OK);
    assert_eq!(
        reclaimed.body["user"]["id"].as_str().unwrap(),
        bob.user.to_string(),
        "the claim must land on the account it was issued for"
    );

    // And the code is spent.
    let replayed = Call::post("/api/auth/claim/start")
        .json(serde_json::json!({ "code": code }))
        .send(&h.router)
        .await;
    assert_ne!(replayed.status, StatusCode::OK, "a claim code was reusable");
}

/// The whole `reset_member_passkeys` transaction — the passkey delete, the
/// session revoke, the claim-code insert, and the audit write — is one
/// atomic unit (#134): a `BEFORE INSERT` trigger forces the audit write to
/// fail deterministically, and nothing else in the transaction may commit
/// either.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_forced_audit_failure_rolls_back_an_admin_assisted_reset(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let acme = org_with_owner(&h, "acme", &rob).await;
    let mut bob = onboard(&h, "bob@acme.test").await;
    add_member(&h, acme, bob.user, of_core::orgs::Role::Member).await;

    sqlx::query(
        "CREATE FUNCTION reject_reset_audit() RETURNS trigger AS $$ \
         BEGIN RAISE EXCEPTION 'forced failure for test'; END; \
         $$ LANGUAGE plpgsql",
    )
    .execute(h.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_reset_audit \
         BEFORE INSERT ON audit_events \
         FOR EACH ROW WHEN (NEW.action = 'org.member.passkeys_reset') \
         EXECUTE FUNCTION reject_reset_audit()",
    )
    .execute(h.db.pool())
    .await
    .unwrap();

    let reset = Call::post(format!(
        "/api/orgs/acme/members/{}/reset-passkeys",
        bob.user
    ))
    .with_session(&rob.session)
    .send(&h.router)
    .await;
    assert_eq!(
        reset.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a forced audit-write failure must fail the whole request, not \
         silently succeed with a lost audit row"
    );

    // Nothing committed: Bob's original passkey still works...
    let still_works = sign_in(&h, &mut bob).await;
    still_works.expect(StatusCode::OK);

    // ...his session was never revoked...
    let still_alive = Call::get("/api/me")
        .with_session(&bob.session)
        .send(&h.router)
        .await;
    assert_eq!(still_alive.status, StatusCode::OK);

    // ...no claim code was minted...
    let claims: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_claims WHERE user_id = $1 AND consumed_at IS NULL",
    )
    .bind(bob.user)
    .fetch_one(h.db.pool())
    .await
    .unwrap();
    assert_eq!(
        claims, 0,
        "no claim row may exist when the transaction that would have \
         written it rolled back"
    );

    // ...and the org.member.passkeys_reset row itself was not written either
    // — the forced failure is on its own insert, and the assertions above
    // prove the delete, the session revoke, and the claim insert that
    // preceded it in the same transaction rolled back too, not just the
    // audit write.
    let reset_rows = Call::get("/api/orgs/acme/audit?actionPrefix=org.member.passkeys_reset")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    reset_rows.expect(StatusCode::OK);
    assert_eq!(
        reset_rows.body.as_array().unwrap().len(),
        0,
        "the forced audit-write failure must itself result in no \
         org.member.passkeys_reset row existing"
    );
}

/// Without a claim code, a reset account must not be claimable at all — that
/// race is the takeover the code exists to prevent.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_reset_account_cannot_be_claimed_without_the_code(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let org = org_with_owner(&h, "acme", &rob).await;
    let bob = onboard(&h, "bob@acme.test").await;
    add_member(&h, org, bob.user, of_core::orgs::Role::Member).await;

    Call::post(format!(
        "/api/orgs/acme/members/{}/reset-passkeys",
        bob.user
    ))
    .with_session(&rob.session)
    .send(&h.router)
    .await
    .expect(StatusCode::CREATED);

    // A stranger with a guessed code gets nowhere.
    let guessed = Call::post("/api/auth/claim/start")
        .json(serde_json::json!({ "code": "of_inv_not-a-real-code" }))
        .send(&h.router)
        .await;
    assert_ne!(guessed.status, StatusCode::OK);

    // And signing up creates a *new* account rather than claiming Bob's — there
    // is no identifier to aim at, which is the whole point of the passkey-first
    // signup. Bob's account keeps its membership and its address.
    let stranger = onboard(&h, "stranger@acme.test").await;
    assert_ne!(stranger.user, bob.user);
    assert!(
        h.db.member_role(org, stranger.user)
            .await
            .unwrap()
            .is_none(),
        "a stranger's new account inherited a reset member's org"
    );
}

/// The exact hazard `savvagent/otto-factory#132` reports: a failure on the
/// credential insert (a unique-violation, here forced by a colliding row —
/// the literal pre-`#131` failure mode the issue names) must not leave the
/// claim code burned with nothing to show for it. Before `#132`'s fix, the
/// claim was spent by an autocommitted statement before `claim_finish` ever
/// attempted the credential insert, so this exact sequence left an account
/// with no passkey and no usable claim — for an org's last owner, nobody
/// above them could issue a second one.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_credential_collision_during_claim_finish_leaves_the_claim_code_usable(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let org = org_with_owner(&h, "acme", &rob).await;
    let bob = onboard(&h, "bob@acme.test").await;
    add_member(&h, org, bob.user, of_core::orgs::Role::Member).await;

    let reset = Call::post(format!(
        "/api/orgs/acme/members/{}/reset-passkeys",
        bob.user
    ))
    .with_session(&rob.session)
    .send(&h.router)
    .await;
    reset.expect(StatusCode::CREATED);
    let code = reset.body["code"]
        .as_str()
        .expect("no claim code")
        .to_string();

    let started = Call::post("/api/auth/claim/start")
        .json(serde_json::json!({ "code": code }))
        .send(&h.router)
        .await;
    started.expect(StatusCode::OK);

    // Drive the ceremony to a real, signed credential — but do not submit it
    // yet. Its raw id is what the eventual `claim/finish` will try to insert.
    let mut new_device = common::authenticator();
    let (ceremony_id, credential) = common::register_credential(&mut new_device, &started.body);
    let colliding_id = credential.raw_id.as_ref().to_vec();

    // Plant a passkey already using that exact credential id, on a different,
    // unrelated account — this is what makes `claim_finish`'s own INSERT hit
    // the unique-violation → `CredentialAlreadyRegistered` path deterministically,
    // without needing to guess at a database-level fault to inject.
    sqlx::query(
        "INSERT INTO passkeys (user_id, credential_id, credential, nickname) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(rob.user)
    .bind(&colliding_id)
    .bind(serde_json::json!({}))
    .bind("decoy")
    .execute(h.db.pool())
    .await
    .unwrap();

    let finished = Call::post("/api/auth/claim/finish")
        .json(serde_json::json!({
            "ceremonyId": ceremony_id,
            "credential": credential,
            "code": code,
        }))
        .send(&h.router)
        .await;
    assert_ne!(
        finished.status,
        StatusCode::OK,
        "a colliding credential id must be refused, not silently accepted"
    );

    // The assertion this test exists for: the claim code must still be live.
    // Under the pre-#132 code this fails — the code was already spent by an
    // autocommitted `UPDATE` before the credential insert was even attempted.
    let retried = Call::post("/api/auth/claim/start")
        .json(serde_json::json!({ "code": code }))
        .send(&h.router)
        .await;
    retried.expect(StatusCode::OK);

    // And the account is actually recoverable end to end, not merely that the
    // code still "looks" valid. A fresh ceremony (via `retried` above) and a
    // fresh device: the failed attempt's own ceremony is restored by the same
    // rollback that restored the claim (see
    // `a_forced_audit_failure_also_restores_the_ceremony` in `of-auth`'s
    // suite), but its only credential was the colliding one already rejected
    // above, so nothing usable is left to retry it with — a clean reclaim
    // needs a new ceremony either way.
    let mut recovery_device = common::authenticator();
    let reclaimed = common::finish_registration(
        &h,
        &mut recovery_device,
        "/api/auth/claim/finish",
        &retried.body,
        serde_json::json!({ "code": code }),
    )
    .await;
    reclaimed.expect(StatusCode::OK);
    assert_eq!(
        reclaimed.body["user"]["id"].as_str().unwrap(),
        bob.user.to_string(),
        "the claim must still land on the account it was issued for"
    );
}

/// The one behavior change `#164`'s own doc comment flags for a close look:
/// the ceremony-ownership check (`registered != user`) now runs *before* the
/// transaction commits, so a mismatch rolls the claim and ceremony
/// consumption back rather than leaving them durably spent under a request
/// that gets rejected. Independently flagged with no existing coverage by
/// three reviewers on that PR (architect-reviewer, pr-test-analyzer,
/// type-design-analyzer) and by the automated Copilot reviewer.
///
/// Presents a claim code for one account (Bob) against a ceremony — and a
/// real, signed credential — that was started for a different account
/// (Carol), the substitution the check exists to catch.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_ceremony_ownership_mismatch_leaves_the_claim_and_ceremony_usable(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let org = org_with_owner(&h, "acme", &rob).await;
    let bob = onboard(&h, "bob@acme.test").await;
    let carol = onboard(&h, "carol@acme.test").await;
    add_member(&h, org, bob.user, of_core::orgs::Role::Member).await;
    add_member(&h, org, carol.user, of_core::orgs::Role::Member).await;

    let reset_bob = Call::post(format!(
        "/api/orgs/acme/members/{}/reset-passkeys",
        bob.user
    ))
    .with_session(&rob.session)
    .send(&h.router)
    .await;
    reset_bob.expect(StatusCode::CREATED);
    let code_bob = reset_bob.body["code"]
        .as_str()
        .expect("no claim code")
        .to_string();

    let reset_carol = Call::post(format!(
        "/api/orgs/acme/members/{}/reset-passkeys",
        carol.user
    ))
    .with_session(&rob.session)
    .send(&h.router)
    .await;
    reset_carol.expect(StatusCode::CREATED);
    let code_carol = reset_carol.body["code"]
        .as_str()
        .expect("no claim code")
        .to_string();

    // Carol's own ceremony, started against her own claim code.
    let started_carol = Call::post("/api/auth/claim/start")
        .json(serde_json::json!({ "code": code_carol }))
        .send(&h.router)
        .await;
    started_carol.expect(StatusCode::OK);

    let mut carol_device = common::authenticator();
    let (ceremony_id, credential) =
        common::register_credential(&mut carol_device, &started_carol.body);

    // Present Carol's ceremony and its real, signed credential — but Bob's
    // claim code.
    let mismatched = Call::post("/api/auth/claim/finish")
        .json(serde_json::json!({
            "ceremonyId": ceremony_id,
            "credential": credential,
            "code": code_bob,
        }))
        .send(&h.router)
        .await;
    assert_eq!(
        mismatched.status,
        StatusCode::FORBIDDEN,
        "a claim code for one account must not complete a ceremony started for another"
    );

    // The rejected attempt still left a trace: nothing commits on this path,
    // so `auth.passkey.registered` never lands, and `auth.claim.refused` is
    // what proves a substitution attempt against the admin-assisted-recovery
    // path was made at all.
    let refused: (Option<String>,) = sqlx::query_as(
        "SELECT actor_user_id::text FROM audit_events WHERE action = $1 AND org_id IS NULL",
    )
    .bind(of_core::audit::action::CLAIM_REFUSED)
    .fetch_one(h.db.pool())
    .await
    .unwrap();
    assert_eq!(
        refused.0.as_deref(),
        Some(carol.user.to_string().as_str()),
        "the refusal must be attributed to the ceremony's actual owner"
    );

    // Neither secret was spent. Bob's own code still opens a fresh ceremony...
    let bob_retry = Call::post("/api/auth/claim/start")
        .json(serde_json::json!({ "code": code_bob }))
        .send(&h.router)
        .await;
    bob_retry.expect(StatusCode::OK);

    // ...and Carol's own ceremony — the one the mismatched request presented
    // — can still complete a *correct* claim/finish with her own code,
    // proving the rejected attempt rolled the `DELETE FROM
    // webauthn_ceremonies` back rather than leaving the row gone for
    // nothing.
    let recovered = Call::post("/api/auth/claim/finish")
        .json(serde_json::json!({
            "ceremonyId": ceremony_id,
            "credential": credential,
            "code": code_carol,
        }))
        .send(&h.router)
        .await;
    recovered.expect(StatusCode::OK);
    assert_eq!(
        recovered.body["user"]["id"].as_str().unwrap(),
        carol.user.to_string(),
        "carol's own claim/ceremony pair must still complete correctly after the mismatch was refused"
    );

    // And the rejected attempt left no passkey behind for Bob, the account
    // the mismatched request's claim code named.
    let bob_passkeys: i64 = sqlx::query_scalar("SELECT count(*) FROM passkeys WHERE user_id = $1")
        .bind(bob.user)
        .fetch_one(h.db.pool())
        .await
        .unwrap();
    assert_eq!(
        bob_passkeys, 0,
        "a rejected ownership mismatch must not leave a passkey for the claim code's account"
    );
}

/// `of-auth`'s own `a_forced_audit_failure_also_restores_the_ceremony` proves
/// a forced audit-write failure rolls back at the `finish_registration_tx`
/// level. It cannot prove more than that: it calls `finish_registration`
/// directly and never touches claim consumption at all. This proves the same
/// failure rolls back through `claim_finish` itself, restoring the claim
/// code alongside the ceremony — flagged by pr-test-analyzer during `#164`'s
/// review as unproven end to end through the endpoint the fix actually
/// changed.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_forced_audit_failure_during_claim_finish_also_restores_the_claim(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let org = org_with_owner(&h, "acme", &rob).await;
    let bob = onboard(&h, "bob@acme.test").await;
    add_member(&h, org, bob.user, of_core::orgs::Role::Member).await;

    let reset = Call::post(format!(
        "/api/orgs/acme/members/{}/reset-passkeys",
        bob.user
    ))
    .with_session(&rob.session)
    .send(&h.router)
    .await;
    reset.expect(StatusCode::CREATED);
    let code = reset.body["code"]
        .as_str()
        .expect("no claim code")
        .to_string();

    // Same fault-injection technique as `of-auth`'s test: force the audit
    // write `finish_registration_tx` makes to fail, deterministically.
    sqlx::query(
        "CREATE FUNCTION reject_claim_registration_audit() RETURNS trigger AS $$ \
         BEGIN RAISE EXCEPTION 'forced failure for test'; END; \
         $$ LANGUAGE plpgsql",
    )
    .execute(h.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_claim_registration_audit \
         BEFORE INSERT ON audit_events \
         FOR EACH ROW WHEN (NEW.action = 'auth.passkey.registered') \
         EXECUTE FUNCTION reject_claim_registration_audit()",
    )
    .execute(h.db.pool())
    .await
    .unwrap();

    let started = Call::post("/api/auth/claim/start")
        .json(serde_json::json!({ "code": code }))
        .send(&h.router)
        .await;
    started.expect(StatusCode::OK);
    let ceremony_id: uuid::Uuid = started.body["ceremonyId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let mut device = common::authenticator();
    let finished = common::finish_registration(
        &h,
        &mut device,
        "/api/auth/claim/finish",
        &started.body,
        serde_json::json!({ "code": code }),
    )
    .await;
    assert_ne!(
        finished.status,
        StatusCode::OK,
        "a forced audit-write failure must abort the whole claim/finish request"
    );

    // The ceremony's own DELETE rolls back with the failed audit write, the
    // same fact `of-auth`'s test proves at the lower level...
    let ceremony_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM webauthn_ceremonies WHERE id = $1")
            .bind(ceremony_id)
            .fetch_one(h.db.pool())
            .await
            .unwrap();
    assert_eq!(
        ceremony_count, 1,
        "the ceremony must survive a rolled-back claim/finish, not just a \
         rolled-back finish_registration_tx"
    );

    // ...and so does the claim code, which only a test that goes through
    // claim_finish itself can show: the code was consumed by
    // consume_account_claim_tx on the same transaction, and must be restored
    // by the same rollback.
    let retried = Call::post("/api/auth/claim/start")
        .json(serde_json::json!({ "code": code }))
        .send(&h.router)
        .await;
    retried.expect(StatusCode::OK);
}

/// An admin must not reach through this endpoint what the role check refuses
/// everywhere else — resetting an owner is an owner's business.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn an_admin_cannot_reset_an_owners_authenticator(pool: PgPool) {
    let h = harness(pool);
    let mut owner = onboard(&h, "owner@acme.test").await;
    let org = org_with_owner(&h, "acme", &owner).await;
    let admin = onboard(&h, "admin@acme.test").await;
    add_member(&h, org, admin.user, of_core::orgs::Role::Admin).await;

    let refused = Call::post(format!(
        "/api/orgs/acme/members/{}/reset-passkeys",
        owner.user
    ))
    .with_session(&admin.session)
    .send(&h.router)
    .await;
    refused.expect(StatusCode::FORBIDDEN);

    // The owner's credential is untouched.
    sign_in(&h, &mut owner).await.expect(StatusCode::OK);
}

// --------------------------------------------------------------- trackers

/// Tracker setup grants a repo the ability to move a customer's tickets, so
/// every route that writes one is admin-only. The read is a member read for the
/// same reason `GET /repos` is: it describes a repo the member can already see.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn only_an_admin_can_connect_a_tracker_or_bind_a_repo(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let bob = onboard(&h, "bob@acme.test").await;

    let org = org_with_owner(&h, "acme", &rob).await;
    add_member(&h, org, bob.user, Role::Member).await;

    Call::post("/api/orgs/acme/repos")
        .with_session(&rob.session)
        .json(serde_json::json!({ "slug": "api" }))
        .send(&h.router)
        .await
        .expect(StatusCode::CREATED);

    // A member reads a repo's bindings.
    Call::get("/api/orgs/acme/repos/api/tracker-bindings")
        .with_session(&bob.session)
        .send(&h.router)
        .await
        .expect(StatusCode::OK);

    for call in [
        Call::get("/api/orgs/acme/tracker-connections"),
        Call::post("/api/orgs/acme/tracker-connections/github")
            .json(serde_json::json!({ "code": "x", "installationId": 17 })),
        Call::delete("/api/orgs/acme/tracker-connections/github"),
        Call::put("/api/orgs/acme/repos/api/tracker-bindings/github")
            .json(serde_json::json!({ "externalRef": "acme/api" })),
        Call::delete("/api/orgs/acme/repos/api/tracker-bindings/github"),
    ] {
        call.with_session(&bob.session)
            .send(&h.router)
            .await
            .expect(StatusCode::FORBIDDEN);
    }
}

/// The same "an org you are not in is a 404, never a 403" rule the rest of the
/// console keeps. A tracker route is a particularly bad place to break it: the
/// binding names the customer's JIRA project key.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn another_orgs_tracker_routes_are_invisible(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let mallory = onboard(&h, "mallory@evil.test").await;

    org_with_owner(&h, "acme", &rob).await;
    org_with_owner(&h, "evil", &mallory).await;

    Call::post("/api/orgs/acme/repos")
        .with_session(&rob.session)
        .json(serde_json::json!({ "slug": "api" }))
        .send(&h.router)
        .await
        .expect(StatusCode::CREATED);

    for (real, imaginary) in [
        (
            Call::get("/api/orgs/acme/tracker-connections"),
            Call::get("/api/orgs/no-such-org/tracker-connections"),
        ),
        (
            Call::delete("/api/orgs/acme/tracker-connections/github"),
            Call::delete("/api/orgs/no-such-org/tracker-connections/github"),
        ),
        (
            Call::get("/api/orgs/acme/repos/api/tracker-bindings"),
            Call::get("/api/orgs/no-such-org/repos/api/tracker-bindings"),
        ),
        (
            Call::put("/api/orgs/acme/repos/api/tracker-bindings/github")
                .json(serde_json::json!({ "externalRef": "acme/api" })),
            Call::put("/api/orgs/no-such-org/repos/api/tracker-bindings/github")
                .json(serde_json::json!({ "externalRef": "acme/api" })),
        ),
    ] {
        let real = real.with_session(&mallory.session).send(&h.router).await;
        let imaginary = imaginary
            .with_session(&mallory.session)
            .send(&h.router)
            .await;

        real.expect(StatusCode::NOT_FOUND);
        assert_eq!(
            real.status, imaginary.status,
            "a real org you are not in must answer like one that does not exist"
        );
        assert_eq!(real.error_code(), imaginary.error_code());
    }
}

/// A binding that can never match an inbound event is a configuration error
/// worth catching where it is typed, not at 3am when the label appears to do
/// nothing. `owner/repo` is what webhook ingest matches `repository.full_name`
/// against; a project key is what it matches `fields.project.key` against.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_binding_that_could_never_match_an_event_is_refused(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    org_with_owner(&h, "acme", &rob).await;

    Call::post("/api/orgs/acme/repos")
        .with_session(&rob.session)
        .json(serde_json::json!({ "slug": "api" }))
        .send(&h.router)
        .await
        .expect(StatusCode::CREATED);

    for (provider, bad) in [
        ("github", "acme"),
        ("github", "acme/api/extra"),
        ("github", "acme/"),
        ("jira", "not a key"),
        ("jira", "acme-123"),
    ] {
        let refused = Call::put(format!(
            "/api/orgs/acme/repos/api/tracker-bindings/{provider}"
        ))
        .with_session(&rob.session)
        .json(serde_json::json!({ "externalRef": bad }))
        .send(&h.router)
        .await;
        refused.expect(StatusCode::BAD_REQUEST);
        assert!(
            !refused.text.is_empty(),
            "{provider} binding {bad:?} was refused with no explanation"
        );
    }

    // And the shapes that can match are accepted, and round-trip.
    Call::put("/api/orgs/acme/repos/api/tracker-bindings/github")
        .with_session(&rob.session)
        .json(serde_json::json!({ "externalRef": "acme/api", "triggerLabel": "agent" }))
        .send(&h.router)
        .await
        .expect(StatusCode::OK);
    Call::put("/api/orgs/acme/repos/api/tracker-bindings/jira")
        .with_session(&rob.session)
        .json(serde_json::json!({ "externalRef": "ACME" }))
        .send(&h.router)
        .await
        .expect(StatusCode::OK);

    let listed = Call::get("/api/orgs/acme/repos/api/tracker-bindings")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    listed.expect(StatusCode::OK);
    assert!(listed.text.contains("acme/api"), "{}", listed.text);
    assert!(listed.text.contains("ACME"), "{}", listed.text);
    assert!(
        listed.text.contains("agent"),
        "the trigger label is what inbound sync watches for: {}",
        listed.text
    );

    Call::delete("/api/orgs/acme/repos/api/tracker-bindings/jira")
        .with_session(&rob.session)
        .send(&h.router)
        .await
        .expect(StatusCode::NO_CONTENT);

    let after = Call::get("/api/orgs/acme/repos/api/tracker-bindings")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    assert!(!after.text.contains("ACME"), "{}", after.text);
}

/// Ciphertext is not a secret in the sense that leaking it grants access, but a
/// console `GET` handing every admin's browser the sealed JIRA refresh token is
/// gratuitous exposure of exactly the material `OF_ENCRYPTION_KEY` exists to
/// protect. The listing is a view type for this reason, not the domain row.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn the_connection_listing_never_carries_stored_secrets(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    let org = org_with_owner(&h, "acme", &rob).await;

    let mut tx = h.db.begin(org).await.expect("tx");
    of_core::trackers::upsert_connection(
        &mut tx,
        of_core::trackers::Provider::Jira,
        "site-1",
        Some(&common::cipher().seal(b"jira-refresh-token").expect("seal")),
        None,
    )
    .await
    .expect("connection");
    tx.commit().await.expect("commit");

    let listed = Call::get("/api/orgs/acme/tracker-connections")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    listed.expect(StatusCode::OK);

    assert!(
        listed.text.contains("site-1"),
        "the external id is what the admin needs to see: {}",
        listed.text
    );
    assert!(
        !listed.text.contains("encrypted"),
        "the console listing carried stored ciphertext: {}",
        listed.text
    );
    assert!(
        listed.text.contains("hasCredentials"),
        "the page still needs to know a credential is stored: {}",
        listed.text
    );
}

/// A deployment with no GitHub OAuth client cannot finish a connect flow, and
/// says so instead of walking an admin through an install they then have to
/// undo by hand. The test harness configures no provider, which is exactly the
/// deployment shape being asserted.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_deployment_that_cannot_connect_a_provider_says_so(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    org_with_owner(&h, "acme", &rob).await;

    let listed = Call::get("/api/orgs/acme/tracker-connections")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    listed.expect(StatusCode::OK);
    assert!(
        listed.text.contains("\"configured\":false"),
        "an unconfigured deployment must not advertise a connect flow: {}",
        listed.text
    );

    let refused = Call::post("/api/orgs/acme/tracker-connections/github")
        .with_session(&rob.session)
        .json(serde_json::json!({ "code": "x", "installationId": 17 }))
        .send(&h.router)
        .await;
    refused.expect(StatusCode::BAD_REQUEST);
    assert!(
        refused.text.contains("not configured"),
        "the refusal should name the deployment's gap: {}",
        refused.text
    );
}

/// GitHub's connect flow needs an installation id and JIRA's does not have one.
/// A GitHub request without it is refused rather than defaulted, because the
/// value is what the whole verification is about — and refused *before* any
/// call to GitHub, which is what lets this test run without a network.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn connecting_github_without_an_installation_id_is_refused(pool: PgPool) {
    let h = harness_with_trackers(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    org_with_owner(&h, "acme", &rob).await;

    // A configured deployment advertises both flows, with the provider URLs
    // built server-side so no App slug or client id is baked into the bundle.
    let listed = Call::get("/api/orgs/acme/tracker-connections")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    listed.expect(StatusCode::OK);
    assert!(
        listed
            .text
            .contains("github.com/apps/otto-factory/installations/new"),
        "the install link is built from the configured slug: {}",
        listed.text
    );
    assert!(
        listed.text.contains("offline_access"),
        "without offline_access JIRA returns no refresh token and the connection dies \
         an hour after it is made: {}",
        listed.text
    );

    let refused = Call::post("/api/orgs/acme/tracker-connections/github")
        .with_session(&rob.session)
        .json(serde_json::json!({ "code": "x" }))
        .send(&h.router)
        .await;
    refused.expect(StatusCode::BAD_REQUEST);
    assert!(
        refused.text.contains("installationId"),
        "the refusal should name the missing field: {}",
        refused.text
    );
}

/// An unknown provider names the two that exist rather than 404ing into
/// silence — the same rule the MCP errors follow.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn an_unknown_provider_names_the_ones_that_exist(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    org_with_owner(&h, "acme", &rob).await;

    let refused = Call::delete("/api/orgs/acme/tracker-connections/linear")
        .with_session(&rob.session)
        .send(&h.router)
        .await;
    refused.expect(StatusCode::BAD_REQUEST);
    assert!(
        refused.text.contains("github") && refused.text.contains("jira"),
        "the refusal should list the valid providers: {}",
        refused.text
    );
}

/// The console API no longer writes the free-form `repos.tracker_binding` blob
/// — `tracker_bindings` rows replaced it, and they are what webhook ingest and
/// the sync engine actually read.
///
/// **Dropping the field must not turn it into a `400`.** An unknown field is
/// not an error in this API, and making it one for this field alone would be a
/// second breaking change on top of the first: a client still sending it would
/// go from "stored somewhere nothing reads" to "cannot register a repo at all".
/// It is ignored, and the repo is created.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn the_console_ignores_the_retired_tracker_binding_field(pool: PgPool) {
    let h = harness(pool);
    let rob = onboard(&h, "rob@acme.test").await;
    org_with_owner(&h, "acme", &rob).await;

    let created = Call::post("/api/orgs/acme/repos")
        .with_session(&rob.session)
        .json(serde_json::json!({
            "slug": "api",
            "trackerBinding": { "jira": "ACME" }
        }))
        .send(&h.router)
        .await;
    created.expect(StatusCode::CREATED);
    assert!(
        !created.text.contains("ACME"),
        "the retired field was stored rather than ignored: {}",
        created.text
    );

    let patched = Call::patch("/api/orgs/acme/repos/api")
        .with_session(&rob.session)
        .json(serde_json::json!({
            "name": "API",
            "trackerBinding": { "jira": "ACME" }
        }))
        .send(&h.router)
        .await;
    patched.expect(StatusCode::OK);
    assert!(
        patched.text.contains("API") && !patched.text.contains("ACME"),
        "the update applied the wrong half: {}",
        patched.text
    );
}
