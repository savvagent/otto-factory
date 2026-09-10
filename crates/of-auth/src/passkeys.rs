//! Passkeys — WebAuthn registration and authentication.
//!
//! Replaces TOTP entirely. What that buys, in the order it matters:
//!
//! - **Phishing resistance.** A passkey signs over the origin it was registered
//!   to. A user can be talked into typing a six-digit code into a lookalike
//!   site; they cannot be talked into producing a signature their authenticator
//!   will only make for `OF_PUBLIC_URL`.
//! - **No shared secret at rest.** [`passkeys`] holds public keys. Losing the
//!   whole table to an attacker lets them sign in as nobody. The TOTP table it
//!   replaced held encrypted seeds, which is a much worse thing to hold.
//! - **No identifier at sign-in.** Credentials are *discoverable*, so the
//!   browser resolves who you are from the key you pick. Nothing is submitted
//!   before the ceremony, so [`start_authentication`] has no address to leak —
//!   the account-enumeration oracle that email removal forced onto signup does
//!   not exist here at all.
//!
//! ## Two round trips, and the state between them
//!
//! Every ceremony is start-then-finish, and the challenge issued by the first
//! is what makes the second's signature meaningful. That state lives in
//! `webauthn_ceremonies` — **server side**, single-use, and expiring. Handing it
//! to the client to give back would let an attacker keep one and replay it,
//! which webauthn-rs warns about in capitals; holding it in process memory
//! would break the moment a second machine answered the second request, which
//! is the normal case on Fly.
//!
//! ## Two places this deliberately overrides webauthn-rs
//!
//! Both are on the challenge, never on the verification state, so neither
//! weakens what the server checks when the signature comes back.
//!
//! 1. **Resident keys are required.** `start_passkey_registration` sets
//!    `require_resident_key(false)`. A non-discoverable credential cannot be
//!    found without being told the user first, which would put the identifier
//!    back into sign-in and undo the reason for choosing passkeys here.
//! 2. **Conditional mediation is cleared.** `start_discoverable_authentication`
//!    forces `mediation: conditional`, which is the autofill flow and shows no
//!    prompt of its own. The console signs in from a button, which needs the
//!    modal.

use crate::error::{AuthError, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use of_core::audit::{action, Entry};
use of_core::ids::UserId;
use of_core::orgs::User;
use of_core::Db;
use uuid::Uuid;
use webauthn_rs::prelude::*;

/// The wire types a caller needs, re-exported so `of-web` talks to this module
/// rather than to webauthn-rs directly. The HTTP layer should not have an
/// opinion about which crate implements the ceremony.
pub use webauthn_rs::prelude::{
    CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse, Webauthn,
};
// Not in the prelude; needed to require discoverable credentials.
use webauthn_rs_proto::ResidentKeyRequirement;

/// How long a half-finished ceremony stays redeemable.
///
/// Short on purpose: the window only has to cover a human picking a key and
/// touching a sensor, and a challenge that outlives its ceremony is a challenge
/// somebody can come back to.
const CEREMONY_TTL_SECONDS: i64 = 300;

/// The relying party's name, as both the challenge and the relying party itself
/// report it. One constant so the two cannot disagree — an authenticator that
/// was told one name and shown another has no way to reconcile them.
const RP_NAME: &str = "otto-factory";

/// A registered authenticator, as the console lists it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredKey {
    pub id: Uuid,
    /// The credential's own id, base64url **unpadded** — the same alphabet the
    /// ceremony speaks, so the console can compare it to what an authenticator
    /// reports without re-encoding either side.
    ///
    /// This is not a secret. It is a public handle the authenticator already
    /// holds and hands to any origin it is asked to sign for; withholding it
    /// protects nothing. It is here because `signalAllAcceptedCredentials`
    /// cannot work without it: a browser matches the surviving credentials by
    /// this id, so a list that omitted it would leave a deleted passkey being
    /// offered in the picker forever. Do not remove it as a tightening.
    pub credential_id: String,
    pub nickname: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// A ceremony handed to the browser: the challenge, and the id that lets the
/// server find its own state again.
pub struct Ceremony<T> {
    pub id: Uuid,
    pub challenge: T,
}

/// Build the relying party.
///
/// `rp_id` is a *hostname*, and it is the thing a passkey is bound to. Changing
/// it invalidates every credential ever registered, so it is derived from
/// `OF_PUBLIC_URL` and asserted at startup rather than typed twice.
pub fn relying_party(rp_id: &str, rp_origin: &str) -> Result<Webauthn> {
    let origin = Url::parse(rp_origin).map_err(|_| {
        AuthError::Config(format!(
            "{rp_origin:?} is not a URL WebAuthn can use as an origin"
        ))
    })?;

    WebauthnBuilder::new(rp_id, &origin)
        .and_then(|b| b.rp_name(RP_NAME).build())
        .map_err(|e| {
            AuthError::Config(format!(
                "could not build the WebAuthn relying party for rp_id {rp_id:?} \
                 and origin {rp_origin}: {e}. The rp_id must be the origin's host, \
                 or a registrable parent domain of it."
            ))
        })
}

// ---------------------------------------------------------------- registration

/// The pair an authenticator files a credential under: `name` is what a
/// credential manager sorts and searches by, `display_name` is the row a human
/// reads when they are asked to choose.
#[derive(Debug, Clone)]
pub struct CredentialNames {
    pub name: String,
    pub display_name: String,
}

/// Name a credential after the account rather than after the site.
///
/// **One function, and not two `format!`s at the call site**, because the
/// console sends this same pair back through `signalCurrentUserDetails` to
/// repair credentials that were registered before there was anything to name
/// them with. A browser that composed the pair itself would hold a second copy
/// of the prefix and of the email → name → label precedence, in TypeScript,
/// with nothing keeping the two in step — and the drift would be silent: the
/// signal is accepted either way, and it would write a label subtly unlike what
/// a fresh registration writes, so "sign in once to fix the picker" would
/// half-work and look like it worked. The console is handed this pair on
/// `/api/me` instead.
pub fn credential_names(user: &User) -> CredentialNames {
    CredentialNames {
        // The address when there is one: it is what a manager sorts and
        // searches by, and the label is only ever a stand-in for it.
        name: user
            .email
            .clone()
            .or_else(|| user.name.clone())
            .unwrap_or_else(|| user.label.clone()),
        // The generated words, address or not. Someone who has learned which
        // key is `otto-factory · brisk-harbor-42` does not lose that the day
        // they fill in a profile — and the vault entry keeps the old words
        // regardless, so a rename here would only make the two disagree.
        display_name: format!("{RP_NAME} · {}", user.label),
    }
}

/// Begin registering a passkey.
///
/// `user` is `None` for a brand-new account: the row is created here, with no
/// address, because the passkey is what brings the account into existence. Pass
/// `Some` to add a second key to an account that already exists — which is the
/// recovery story, and what the console asks for straight after signup.
pub async fn start_registration(
    db: &Db,
    webauthn: &Webauthn,
    user: Option<UserId>,
) -> Result<Ceremony<CreationChallengeResponse>> {
    // Both arms end at the account's own row, because the names below are a
    // function of it and nothing else — a brand-new account is named by the
    // label its insert just generated, not by a stand-in.
    let account = match user {
        Some(id) => db.get_user(id).await?.ok_or(AuthError::UnknownUser)?,
        None => db.create_unclaimed_user().await?,
    };
    let user_id = account.id;
    let names = credential_names(&account);

    let existing_credentials = credential_ids_for(db, user_id).await?;

    let (mut challenge, state) = webauthn
        .start_passkey_registration(
            user_id.as_uuid(),
            &names.name,
            &names.display_name,
            Some(existing_credentials),
        )
        .map_err(webauthn_failed)?;

    // See the module docs: webauthn-rs leaves resident keys optional, and a
    // non-discoverable credential cannot be used to sign in without naming the
    // account first.
    let selection = challenge
        .public_key
        .authenticator_selection
        .get_or_insert_with(Default::default);
    selection.resident_key = Some(ResidentKeyRequirement::Required);
    selection.require_resident_key = true;
    // Left unset so a security key is as welcome as a platform authenticator —
    // pinning this to Platform is how someone with a YubiKey and no biometrics
    // discovers they cannot register at all.
    selection.authenticator_attachment = None;

    let id = store_ceremony(db, "register", Some(user_id), &state).await?;
    Ok(Ceremony { id, challenge })
}

/// Finish registering, and return the account the key now belongs to.
pub async fn finish_registration(
    db: &Db,
    webauthn: &Webauthn,
    ceremony: Uuid,
    credential: &RegisterPublicKeyCredential,
    nickname: Option<&str>,
) -> Result<UserId> {
    let (user_id, state): (Option<UserId>, PasskeyRegistration) =
        take_ceremony(db, ceremony, "register").await?;
    let user_id = user_id.ok_or(AuthError::CeremonyExpired)?;

    let passkey = webauthn
        .finish_passkey_registration(credential, &state)
        .map_err(webauthn_failed)?;

    let credential_id = passkey.cred_id().as_ref().to_vec();
    let encoded = serde_json::to_value(&passkey)
        .map_err(|e| AuthError::Config(format!("could not store a passkey: {e}")))?;

    // The unique index on credential_id is the real guard: an authenticator
    // must not be registrable twice, to two accounts, which is what the
    // exclude-credentials list asks for politely and this enforces.
    sqlx::query(
        "INSERT INTO passkeys (user_id, credential_id, credential, nickname) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(&credential_id)
    .bind(&encoded)
    .bind(nickname)
    .execute(db.pool())
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
            AuthError::CredentialAlreadyRegistered
        }
        _ => AuthError::from(e),
    })?;

    if let Err(e) = db
        .audit_global(Entry::new(action::PASSKEY_REGISTERED).actor(user_id))
        .await
    {
        tracing::error!(
            error = %e,
            user_id = %user_id,
            "failed to write audit event for passkey registration"
        );
    }

    Ok(user_id)
}

// -------------------------------------------------------------- authentication

/// Begin signing in. Takes no identifier — that is the point.
pub async fn start_authentication(
    db: &Db,
    webauthn: &Webauthn,
) -> Result<Ceremony<RequestChallengeResponse>> {
    let (mut challenge, state) = webauthn
        .start_discoverable_authentication()
        .map_err(webauthn_failed)?;

    // See the module docs: the console signs in from a button, not from an
    // autofill hint on a text field.
    challenge.mediation = None;

    let id = store_ceremony(db, "authenticate", None, &state).await?;
    Ok(Ceremony { id, challenge })
}

/// Finish signing in.
///
/// The browser hands back a credential carrying the user handle it was
/// registered with, which is how an account is found without one being typed.
/// That handle is a claim, not proof — the signature checked below is the
/// proof, and it is checked against the key stored for that account.
pub async fn finish_authentication(
    db: &Db,
    webauthn: &Webauthn,
    ceremony: Uuid,
    credential: &PublicKeyCredential,
    ip: Option<&str>,
) -> Result<UserId> {
    let (_, state): (Option<UserId>, DiscoverableAuthentication) =
        take_ceremony(db, ceremony, "authenticate").await?;

    // Resolve the account from the **credential ID**, not the user handle.
    //
    // webauthn-rs offers `identify_discoverable_authentication`, which reads the
    // user handle the authenticator returns. That works, but it trusts the one
    // field an authenticator is allowed to omit — and several do, including
    // every software authenticator available to test with. The credential ID is
    // always present, it is what the unique index is on, and looking the account
    // up by it is strictly more robust.
    //
    // It is not a weaker check. Neither field is evidence of anything: both are
    // claims the client makes about *which* account to check against, and the
    // signature verified below is what makes the answer true.
    let credential_id = credential.raw_id.as_ref();

    let owner: Option<UserId> =
        sqlx::query_scalar("SELECT user_id FROM passkeys WHERE credential_id = $1")
            .bind(credential_id)
            .fetch_optional(db.pool())
            .await?;

    let Some(user_id) = owner else {
        // An unknown credential, and the one sign-in failure this server names.
        //
        // Nothing to attribute an audit row to, so `note_failure` is not called
        // here — otherwise any id a stranger posted would write a row.
        //
        // Naming it is not the enumeration leak the rest of this module avoids.
        // A credential ID is unguessable bytes minted by an authenticator and is
        // never disclosed cross-origin, so the only caller who can ask this
        // question is one already holding the answer's subject. And it resolves
        // no account: the lookup above asks `passkeys` alone, no `users` row is
        // read on this path, and there is no address or org anywhere in the
        // answer. That ordering is the load-bearing half of the argument — the
        // entropy is why the question is hard to ask, but the ordering is why
        // the answer says nothing. Everything downstream stays collapsed into
        // `InvalidCredentials`.
        //
        // The alternative is a console that cannot call
        // `signalUnknownCredential` without guessing: signal on every failure
        // and one cancelled prompt evicts a good passkey from somebody's vault;
        // signal on none and a deleted key haunts the picker forever — worst for
        // the person an admin has just reset, who is offered the dead key first
        // because it is the oldest.
        return Err(AuthError::UnknownCredential);
    };

    // Only this account's keys. Passing every key in the database would
    // authenticate whoever the signature happened to match, which is a
    // different and much worse function.
    let stored: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT credential FROM passkeys WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(db.pool())
            .await?;

    let keys: Vec<DiscoverableKey> = stored
        .into_iter()
        .filter_map(|raw| match serde_json::from_value::<Passkey>(raw) {
            Ok(passkey) => Some(passkey),
            Err(e) => {
                tracing::error!(
                    error = %e,
                    user_id = %user_id,
                    "stored passkey credential failed to deserialize; skipping it"
                );
                None
            }
        })
        .map(|p| DiscoverableKey::from(&p))
        .collect();

    if keys.is_empty() {
        note_failure(db, user_id, ip).await;
        return Err(AuthError::InvalidCredentials);
    }

    let result = match webauthn.finish_discoverable_authentication(credential, state, &keys) {
        Ok(result) => result,
        Err(_) => {
            note_failure(db, user_id, ip).await;
            return Err(AuthError::InvalidCredentials);
        }
    };

    // A counter that has not moved forward is the documented signal of a cloned
    // authenticator. Most passkeys report zero and never move, which is normal
    // and not what this is looking for — a *decrease* from a nonzero counter is.
    if result.needs_update() {
        update_stored_credential(db, user_id, credential_id, &result).await?;
    }

    sqlx::query(
        "UPDATE passkeys SET last_used_at = now() \
         WHERE user_id = $1 AND credential_id = $2",
    )
    .bind(user_id)
    .bind(credential_id)
    .execute(db.pool())
    .await?;

    Ok(user_id)
}

// ------------------------------------------------------------------ management

/// One row of the key list, before it becomes a [`RegisteredKey`].
///
/// A named struct rather than a tuple, and matched by **column name**. This was
/// a four-element tuple decoded by position, which was survivable; adding
/// `credential_id` made it five, with a bare `Vec<u8>` sitting between an id and
/// two timestamps. At that width, inserting a column into the `SELECT` below or
/// swapping two type-compatible neighbours would compile cleanly and file every
/// key's data one field over — and nothing about that is visible until a browser
/// is comparing credential ids that never match.
#[derive(sqlx::FromRow)]
struct KeyRow {
    id: Uuid,
    credential_id: Vec<u8>,
    nickname: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn list(db: &Db, user: UserId) -> Result<Vec<RegisteredKey>> {
    let rows: Vec<KeyRow> = sqlx::query_as(
        "SELECT id, credential_id, nickname, created_at, last_used_at FROM passkeys \
             WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(user)
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| RegisteredKey {
            id: row.id,
            credential_id: URL_SAFE_NO_PAD.encode(row.credential_id),
            nickname: row.nickname,
            created_at: row.created_at,
            last_used_at: row.last_used_at,
        })
        .collect())
}

pub async fn count(db: &Db, user: UserId) -> Result<i64> {
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM passkeys WHERE user_id = $1")
        .bind(user)
        .fetch_one(db.pool())
        .await?;
    Ok(n)
}

/// Whether this account can be signed into at all.
pub async fn has_credential(db: &Db, user: UserId) -> Result<bool> {
    Ok(count(db, user).await? > 0)
}

/// Remove a key, refusing to remove the last one.
///
/// **The refusal is the feature.** Deleting your only passkey locks you out of
/// your own account with no email to recover through, and the click that does
/// it looks exactly like tidying up a stale device. Someone who genuinely wants
/// out deletes the account.
pub async fn remove(db: &Db, user: UserId, key: Uuid, ip: Option<&str>) -> Result<()> {
    let remaining = count(db, user).await?;
    if remaining <= 1 {
        return Err(AuthError::LastPasskey);
    }

    let affected = sqlx::query("DELETE FROM passkeys WHERE user_id = $1 AND id = $2")
        .bind(user)
        .bind(key)
        .execute(db.pool())
        .await?
        .rows_affected();

    if affected == 0 {
        return Err(AuthError::UnknownCredential);
    }

    if let Err(e) = db
        .audit_global(
            Entry::new(action::PASSKEY_REMOVED)
                .actor(user)
                .target("passkey", key.to_string())
                .from_request(ip, None),
        )
        .await
    {
        tracing::error!(
            error = %e,
            user_id = %user,
            "failed to write audit event for passkey removal"
        );
    }

    Ok(())
}

pub async fn rename(
    db: &Db,
    user: UserId,
    key: Uuid,
    nickname: &str,
    ip: Option<&str>,
) -> Result<()> {
    let affected = sqlx::query("UPDATE passkeys SET nickname = $3 WHERE user_id = $1 AND id = $2")
        .bind(user)
        .bind(key)
        .bind(nickname.trim())
        .execute(db.pool())
        .await?
        .rows_affected();

    if affected == 0 {
        return Err(AuthError::UnknownCredential);
    }

    if let Err(e) = db
        .audit_global(
            Entry::new(action::PASSKEY_RENAMED)
                .actor(user)
                .target("passkey", key.to_string())
                .from_request(ip, None),
        )
        .await
    {
        tracing::error!(
            error = %e,
            user_id = %user,
            "failed to write audit event for passkey rename"
        );
    }

    Ok(())
}

/// Clear every passkey on an account. The admin-assisted half of recovery.
///
/// Leaves the account with no way in **by design** — the caller must issue a
/// claim code, or the account becomes claimable by whoever reaches registration
/// first. `of_web::routes::orgs::reset_member_passkeys` does both, but **not**
/// atomically — each of `clear`, `sessions::revoke_all`, and the claim-code
/// insert commits on its own connection before the next runs. A failure partway
/// through is a real, currently-unhandled lockout risk; see
/// `docs/specs/2026-09-10-passkey-audit-events-design.md` §1a for how this was
/// found and `savvagent/otto-factory#87` for the follow-up to fix it.
pub async fn clear(db: &Db, user: UserId, ip: Option<&str>) -> Result<u64> {
    let removed = sqlx::query("DELETE FROM passkeys WHERE user_id = $1")
        .bind(user)
        .execute(db.pool())
        .await?
        .rows_affected();

    if let Err(e) = db
        .audit_global(
            Entry::new(action::PASSKEY_CLEARED)
                .actor(user)
                .from_request(ip, None),
        )
        .await
    {
        tracing::error!(
            error = %e,
            user_id = %user,
            "failed to write audit event for passkey clear"
        );
    }

    Ok(removed)
}

/// Delete ceremonies nobody came back for.
pub async fn sweep(db: &Db) -> Result<u64> {
    let n = sqlx::query("DELETE FROM webauthn_ceremonies WHERE expires_at < now()")
        .execute(db.pool())
        .await?
        .rows_affected();
    Ok(n)
}

// ---------------------------------------------------------------------- internals

async fn credential_ids_for(db: &Db, user: UserId) -> Result<Vec<CredentialID>> {
    let rows: Vec<Vec<u8>> =
        sqlx::query_scalar("SELECT credential_id FROM passkeys WHERE user_id = $1")
            .bind(user)
            .fetch_all(db.pool())
            .await?;
    Ok(rows.into_iter().map(CredentialID::from).collect())
}

async fn store_ceremony<T: serde::Serialize>(
    db: &Db,
    kind: &str,
    user: Option<UserId>,
    state: &T,
) -> Result<Uuid> {
    let encoded = serde_json::to_value(state)
        .map_err(|e| AuthError::Config(format!("could not store a ceremony: {e}")))?;

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO webauthn_ceremonies (kind, user_id, state, expires_at) \
         VALUES ($1, $2, $3, now() + make_interval(secs => $4)) RETURNING id",
    )
    .bind(kind)
    .bind(user)
    .bind(&encoded)
    .bind(CEREMONY_TTL_SECONDS as f64)
    .fetch_one(db.pool())
    .await?;

    Ok(id)
}

/// Consume a ceremony: read it and delete it in one statement.
///
/// `DELETE … RETURNING` rather than select-then-delete, so two requests racing
/// the same challenge cannot both succeed. The `kind` predicate is part of the
/// same statement for the same reason — a registration state must never be
/// finishable as an authentication, and checking that after the fact would
/// leave a window where it could.
async fn take_ceremony<T: serde::de::DeserializeOwned>(
    db: &Db,
    id: Uuid,
    kind: &str,
) -> Result<(Option<UserId>, T)> {
    let row: Option<(Option<UserId>, serde_json::Value)> = sqlx::query_as(
        "DELETE FROM webauthn_ceremonies \
         WHERE id = $1 AND kind = $2 AND expires_at > now() \
         RETURNING user_id, state",
    )
    .bind(id)
    .bind(kind)
    .fetch_optional(db.pool())
    .await?;

    let (user, state) = row.ok_or(AuthError::CeremonyExpired)?;
    let state = serde_json::from_value(state)
        .map_err(|e| AuthError::Config(format!("could not read a ceremony: {e}")))?;
    Ok((user, state))
}

async fn update_stored_credential(
    db: &Db,
    user: UserId,
    credential_id: &[u8],
    result: &AuthenticationResult,
) -> Result<()> {
    let raw: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT credential FROM passkeys WHERE user_id = $1 AND credential_id = $2",
    )
    .bind(user)
    .bind(credential_id)
    .fetch_optional(db.pool())
    .await?;

    let Some(raw) = raw else { return Ok(()) };
    let mut passkey = match serde_json::from_value::<Passkey>(raw) {
        Ok(passkey) => passkey,
        Err(e) => {
            tracing::error!(
                error = %e,
                user_id = %user,
                "stored passkey credential failed to deserialize during a sign-counter update; skipping it"
            );
            return Ok(());
        }
    };

    if passkey.update_credential(result).is_some() {
        if let Ok(encoded) = serde_json::to_value(&passkey) {
            sqlx::query(
                "UPDATE passkeys SET credential = $3 \
                 WHERE user_id = $1 AND credential_id = $2",
            )
            .bind(user)
            .bind(credential_id)
            .bind(&encoded)
            .execute(db.pool())
            .await?;
        }
    }
    Ok(())
}

async fn note_failure(db: &Db, user: UserId, ip: Option<&str>) {
    let entry = Entry::new(action::LOGIN_FAILED)
        .actor(user)
        .from_request(ip, None)
        .detail(serde_json::json!({ "method": "passkey" }));
    if let Err(e) = db.audit_global(entry).await {
        tracing::error!(error = %e, "failed to write audit event for a sign-in attempt");
    }
}

/// Every WebAuthn failure becomes one opaque answer.
///
/// The library's errors are specific and useful in a log — "the origin did not
/// match", "user verification was not performed" — and handing that specificity
/// to the caller tells an attacker which part of their forgery to fix next.
fn webauthn_failed(e: WebauthnError) -> AuthError {
    tracing::warn!(error = %e, "webauthn ceremony failed");
    AuthError::InvalidCredentials
}
