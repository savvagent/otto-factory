/**
 * Turning a failure into a sentence the reader can act on, in their language.
 *
 * **This works at all because of a rule the server already keeps.** `api.ts`
 * states it: *an error has a code before it has a message.* `ApiError.code` is
 * the stable branch point — `not_found`, `rate_limited`, `last_passkey` — and
 * the `message` beside it is English prose written by the server. Pages have
 * always been forbidden from parsing the message; this module is what they
 * switch on the code with instead.
 *
 * `WebauthnError` was the one error surface in the console with no code, which
 * meant the login page — the first page anybody sees — had an error surface
 * that could not be translated. It has one now, and arrives here the same way.
 *
 * **An unknown code renders the thrower's English message rather than nothing.**
 * The server can add an error tomorrow and this bundle will not know its code;
 * an untranslated sentence is a far better outcome than a blank alert, and it
 * is the reason the code/message split is worth keeping on the wire. There is a
 * test for exactly that path.
 */

import { ApiError } from './api';
import { m } from './paraglide/messages';
import { WebauthnError } from './webauthn';

/**
 * Every code this bundle has a translation for.
 *
 * Enumerated from `of_core::Error::code()`, `of-web`'s `auth_code()`, the two
 * codes `api.ts` mints for itself, and `WebauthnErrorCode`. Deliberately not
 * exhaustive over what the server *could* send — see the fallback above. A code
 * missing from here is a sentence in English, not a missing sentence.
 */
const KNOWN: Record<string, () => string> = {
  // ---- transport, minted by api.ts rather than received ----
  network: () => m.error_network(),
  unknown: () => m.error_unknown(),

  // ---- of-core ----
  job_not_found: () => m.error_job_not_found(),
  repo_not_found: () => m.error_repo_not_found(),
  repo_unresolved: () => m.error_repo_unresolved(),
  repo_slug_taken: () => m.error_repo_slug_taken(),
  remote_taken: () => m.error_remote_taken(),
  wrong_status: () => m.error_wrong_status(),
  already_claimed: () => m.error_already_claimed(),
  ticket_already_linked: () => m.error_ticket_already_linked(),
  dependency_cycle: () => m.error_dependency_cycle(),
  lease_held: () => m.error_lease_held(),
  lease_not_held: () => m.error_lease_not_held(),
  org_not_found: () => m.error_org_not_found(),
  team_not_found: () => m.error_team_not_found(),
  team_slug_taken: () => m.error_team_slug_taken(),
  team_in_use: () => m.error_team_in_use(),
  not_a_member: () => m.error_not_a_member(),
  already_a_member: () => m.error_already_a_member(),
  invite_invalid: () => m.error_invite_invalid(),
  invite_wrong_account: () => m.error_invite_wrong_account(),
  invalid_argument: () => m.error_invalid_argument(),
  internal_error: () => m.error_internal(),

  // ---- of-web / of-auth ----
  unauthenticated: () => m.error_unauthenticated(),
  not_found: () => m.error_not_found(),
  forbidden: () => m.error_forbidden(),
  invalid_credentials: () => m.error_invalid_credentials(),
  ceremony_expired: () => m.error_ceremony_expired(),
  credential_already_registered: () => m.error_credential_already_registered(),
  unknown_credential: () => m.error_unknown_credential(),
  last_passkey: () => m.error_last_passkey(),
  last_owner: () => m.error_last_owner(),
  credential_expired: () => m.error_credential_expired(),
  wrong_audience: () => m.error_wrong_audience(),
  sso_required: () => m.error_sso_required(),
  rate_limited: () => m.error_rate_limited(),
  invalid_request: () => m.error_invalid_request(),
  invalid_client: () => m.error_invalid_client(),
  invalid_grant: () => m.error_invalid_grant(),
  unsupported_grant_type: () => m.error_unsupported_grant_type(),
  invalid_scope: () => m.error_invalid_scope(),
  tracker_unreachable: () => m.error_tracker_unreachable(),

  // ---- webauthn, thrown in this bundle ----
  no_passkey_created: () => m.error_no_passkey_created(),
  no_passkey_offered: () => m.error_no_passkey_offered(),
  passkey_cancelled: () => m.error_passkey_cancelled(),
  passkey_already_registered: () => m.error_passkey_already_registered(),
  passkey_misconfigured: () => m.error_passkey_misconfigured()
  // `passkey_refused` is handled below: it carries a `DOMException` name.
};

/**
 * The sentence to show for a failure.
 *
 * `fallback` is what to say for something that is neither an `ApiError` nor a
 * `WebauthnError` — a bug in this bundle, a `TypeError`, anything whose message
 * was written for a developer rather than a reader. Callers pass a keyed
 * message; never a raw `String(e)`.
 */
export function messageFor(error: unknown, fallback: string): string {
  if (error instanceof WebauthnError) {
    // The only code carrying data: the browser refused for a reason we have no
    // specific sentence for, and the `DOMException` name is the useful part.
    if (error.code === 'passkey_refused') {
      return m.error_passkey_refused({ reason: error.detail ?? 'unknown' });
    }
    return KNOWN[error.code]?.() ?? error.message;
  }

  if (error instanceof ApiError) {
    // The server's own English message, for a code this bundle predates.
    return KNOWN[error.code]?.() ?? error.message;
  }

  return fallback;
}
