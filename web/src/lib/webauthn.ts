/**
 * The browser half of a WebAuthn ceremony.
 *
 * `navigator.credentials` speaks `ArrayBuffer`; JSON does not. Everything here
 * exists to cross that boundary in both directions, and to do it in exactly the
 * shape the server's `webauthn-rs` types deserialize from — `rawId`,
 * `clientDataJSON`, `attestationObject`, `authenticatorData`, `userHandle`,
 * `type`, all base64url with no padding.
 *
 * **Written out by hand rather than using `PublicKeyCredential.toJSON()`.**
 * That method produces very nearly this shape and is not available everywhere
 * yet; the failure when it is missing, or when a field is named slightly
 * differently, is a server-side "invalid credentials" that looks like a broken
 * authenticator rather than a broken serializer. Thirty lines of explicit
 * conversion is worth not debugging that.
 */

/** base64url → bytes. Tolerates padding and the standard alphabet. */
function fromBase64Url(value: string): Uint8Array {
  const padded = value.replace(/-/g, '+').replace(/_/g, '/');
  const binary = atob(padded + '='.repeat((4 - (padded.length % 4)) % 4));
  return Uint8Array.from(binary, (c) => c.charCodeAt(0));
}

/** bytes → base64url, unpadded, which is what the server's decoder expects. */
function toBase64Url(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  let binary = '';
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/** Whether this browser can do WebAuthn at all. */
export function isSupported(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof window.PublicKeyCredential !== 'undefined' &&
    typeof navigator?.credentials?.create === 'function'
  );
}

/**
 * Whether this device can *store* a passkey itself — a fingerprint reader, Face
 * ID, Windows Hello.
 *
 * Used only to word the prompt. A false here does not mean the user cannot
 * register: a security key or a phone over Bluetooth works fine, and telling
 * someone they cannot sign up because their laptop has no sensor would be
 * wrong.
 */
export async function hasPlatformAuthenticator(): Promise<boolean> {
  try {
    return await window.PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable();
  } catch {
    return false;
  }
}

/** The server's challenge, as JSON, before the buffers are decoded. */
type CreationChallenge = {
  publicKey: {
    challenge: string;
    user: { id: string; name: string; displayName: string };
    excludeCredentials?: { id: string; type: string; transports?: string[] }[];
    [k: string]: unknown;
  };
};

type RequestChallenge = {
  publicKey: {
    challenge: string;
    allowCredentials?: { id: string; type: string; transports?: string[] }[];
    [k: string]: unknown;
  };
  mediation?: string | null;
};

/**
 * Run a registration ceremony and return what the server needs back.
 *
 * Throws `WebauthnError` with a message written for a person: the browser's own
 * exceptions say things like "The operation either timed out or was not
 * allowed", which is what a user sees when they simply changed their mind.
 */
export async function register(challenge: CreationChallenge): Promise<unknown> {
  const publicKey = {
    ...challenge.publicKey,
    challenge: fromBase64Url(challenge.publicKey.challenge),
    user: {
      ...challenge.publicKey.user,
      id: fromBase64Url(challenge.publicKey.user.id)
    },
    excludeCredentials: (challenge.publicKey.excludeCredentials ?? []).map((c) => ({
      ...c,
      id: fromBase64Url(c.id)
    }))
  } as unknown as PublicKeyCredentialCreationOptions;

  const credential = (await navigator.credentials
    .create({ publicKey })
    .catch(rethrow)) as PublicKeyCredential | null;

  if (!credential) throw new WebauthnError('no_passkey_created', 'No passkey was created.');
  const response = credential.response as AuthenticatorAttestationResponse;

  return {
    id: credential.id,
    rawId: toBase64Url(credential.rawId),
    type: credential.type,
    response: {
      attestationObject: toBase64Url(response.attestationObject),
      clientDataJSON: toBase64Url(response.clientDataJSON),
      // Present on most browsers, and worth sending: it is how the server can
      // later tell a phone from a security key in the list of keys.
      transports:
        typeof response.getTransports === 'function' ? response.getTransports() : undefined
    },
    clientExtensionResults: credential.getClientExtensionResults()
  };
}

/**
 * Run an authentication ceremony.
 *
 * `allowCredentials` arrives empty and stays empty: the point of a discoverable
 * credential is that the browser resolves who is signing in, so nothing here
 * tells it which account to look for. The server's `mediation` field is ignored
 * — it asks for the autofill flow, and this is a button.
 */
export async function authenticate(challenge: RequestChallenge): Promise<unknown> {
  const publicKey = {
    ...challenge.publicKey,
    challenge: fromBase64Url(challenge.publicKey.challenge),
    allowCredentials: (challenge.publicKey.allowCredentials ?? []).map((c) => ({
      ...c,
      id: fromBase64Url(c.id)
    }))
  } as unknown as PublicKeyCredentialRequestOptions;

  const credential = (await navigator.credentials
    .get({ publicKey })
    .catch(rethrow)) as PublicKeyCredential | null;

  if (!credential) throw new WebauthnError('no_passkey_offered', 'No passkey was offered.');
  const response = credential.response as AuthenticatorAssertionResponse;

  return {
    id: credential.id,
    rawId: toBase64Url(credential.rawId),
    type: credential.type,
    response: {
      authenticatorData: toBase64Url(response.authenticatorData),
      clientDataJSON: toBase64Url(response.clientDataJSON),
      signature: toBase64Url(response.signature),
      userHandle: response.userHandle ? toBase64Url(response.userHandle) : null
    },
    clientExtensionResults: credential.getClientExtensionResults()
  };
}

/**
 * A ceremony that did not produce a credential.
 *
 * **Carries a `code` for the same reason `ApiError` does**: an error has a code
 * before it has a message. The code is the stable branch point a UI switches on
 * — here, what `$lib/errors` looks up to render the message in the reader's
 * language — and the `message` is the English fallback for a code nothing has
 * been taught yet. This type was the one error surface in the console exempt
 * from that rule, which meant the login page — the first page anyone sees —
 * could not be translated at all.
 */
export class WebauthnError extends Error {
  readonly code: WebauthnErrorCode;
  /**
   * The `DOMException` name, when the browser refused for a reason we have no
   * specific code for.
   *
   * Carried as data rather than baked into the message, because it is not a
   * word in anybody's language and the sentence around it has to be
   * translatable without losing it.
   */
  readonly detail?: string;

  constructor(code: WebauthnErrorCode, message: string, detail?: string) {
    super(message);
    this.name = 'WebauthnError';
    this.code = code;
    this.detail = detail;
  }
}

export type WebauthnErrorCode =
  /** The ceremony resolved, but with nothing in it. */
  | 'no_passkey_created'
  | 'no_passkey_offered'
  /** Dismissed, or timed out — by far the most common outcome. */
  | 'passkey_cancelled'
  /** This authenticator already holds a credential for this account. */
  | 'passkey_already_registered'
  /** An `rp_id` that does not match the origin: a deployment error. */
  | 'passkey_misconfigured'
  /** Anything else the browser refused, named by its `DOMException`. */
  | 'passkey_refused';

/**
 * Turn the browser's exception into something worth showing.
 *
 * `NotAllowedError` covers both "the user cancelled" and "the operation timed
 * out", and it is by far the most common outcome — it is what a dismissed
 * prompt produces. Rendering the raw DOMException text there tells someone
 * their device failed when they simply pressed Escape.
 */
function rethrow(e: unknown): never {
  if (e instanceof DOMException) {
    if (e.name === 'NotAllowedError') {
      throw new WebauthnError(
        'passkey_cancelled',
        'No passkey was used. Try again when you are ready.'
      );
    }
    if (e.name === 'InvalidStateError') {
      throw new WebauthnError(
        'passkey_already_registered',
        'That authenticator is already registered to this account.'
      );
    }
    if (e.name === 'SecurityError') {
      // Almost always an rp_id that does not match the page's origin, which is
      // a deployment error rather than anything the user did.
      throw new WebauthnError(
        'passkey_misconfigured',
        'This site is not configured correctly for passkeys. Tell whoever runs it that the ' +
          'relying party ID does not match this origin.'
      );
    }
    throw new WebauthnError(
      'passkey_refused',
      `Your browser refused the passkey: ${e.name}.`,
      e.name
    );
  }
  throw e;
}
