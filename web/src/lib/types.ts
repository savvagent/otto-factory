/**
 * The console API's wire types.
 *
 * Hand-written, and mirroring `crates/of-web/src/openapi.rs` rather than
 * generated from it. Generation would be the reflex; it is the wrong trade at
 * this size. A generator has to run in CI to be worth anything, and until
 * `of-server` binds a port there is no document to fetch — so the "generated"
 * file would in practice be a checked-in artifact nobody regenerates, which is
 * the same hand-written file with a comment claiming otherwise.
 * `GET /api/openapi.json` is the authority either way; this is a transcription
 * of it, and `npm run check` fails when a page reads a field not declared here.
 *
 * Every name is camelCase because every response body is: `of-web`'s structs
 * carry `#[serde(rename_all = "camelCase")]`.
 */

export type Role = 'owner' | 'admin' | 'member';
export type JobStatus = 'pending' | 'in-progress' | 'active' | 'completed' | 'failed';
export type Provider = 'github' | 'gitlab' | 'bitbucket' | 'other';
export type TokenKind = 'oauth' | 'pat';

export interface User {
  id: string;
  /** Absent until the account sets one — a passkey creates the account. */
  email: string | null;
  name: string | null;
  /**
   * Generated words — `brisk-harbor-42` — that name this account in a
   * credential vault's picker. Never an identifier and never unique; it exists
   * so a key belonging to an account with no address is still nameable.
   */
  label: string;
  /**
   * The console language this account chose, or `null` for "never chose".
   *
   * `null` is not English — it is the state where the browser's own preference
   * is still in charge. See `$lib/locale`.
   */
  locale: string | null;
  createdAt: string;
  disabledAt: string | null;
}

export interface Org {
  id: string;
  slug: string;
  name: string;
  plan: string;
  enforceSso: boolean;
  createdAt: string;
}

/** One org this account belongs to, and the role it holds there. */
export interface Membership {
  orgId: string;
  userId: string;
  role: Role;
  orgSlug: string;
  orgName: string;
  plan: string;
}

export interface Me {
  user: User;
  orgs: Membership[];
  shouldAddPasskey: boolean;
  passkeyCount: number;
  /**
   * What a fresh registration would file this account's credential under, as
   * `of_auth::passkeys::credential_names` composed it.
   *
   * Handed over rather than derived here, and forwarded to
   * `signalCurrentUserDetails` **verbatim**. A second copy of the
   * email-then-name-then-label precedence in TypeScript would drift from the
   * server's, and the drift would be silent: the signal is accepted either way
   * and writes words subtly unlike what registering again writes, so "repair a
   * stale label by signing in once" would half-work and look like it worked.
   */
  credentialName: string;
  /** The row a human reads in a vault's picker. Same rule: never composed here. */
  credentialDisplayName: string;
}

export interface Joined {
  org: Org;
  role: Role;
}

export interface SessionOpened {
  user: User;
  shouldAddPasskey: boolean;
}

export interface OrgMember {
  id: string;
  email: string | null;
  name: string | null;
  /** See `User.label` — what to render where an address is missing. */
  label: string;
  role: Role;
  joinedAt: string;
  disabledAt: string | null;
}

export interface Invite {
  id: string;
  orgId: string;
  email: string;
  role: Role;
  invitedBy: string | null;
  expiresAt: string;
  acceptedAt: string | null;
  createdAt: string;
}

/**
 * The response from minting an invitation: the invite, plus the one-time code.
 *
 * `code` and `link` are the same secret twice and are returned **only** here —
 * nothing is emailed, and only the hash is stored, so an admin who loses the
 * code re-invites rather than looking it up.
 */
export interface CreatedInvite extends Invite {
  code: string;
  link: string;
}

/** A registered authenticator, as the console lists it. */
export interface Passkey {
  id: string;
  /**
   * The credential's own id, base64url without padding — the same encoding the
   * ceremony speaks, so it can be compared with what an authenticator reports
   * without re-encoding either side.
   *
   * **A public handle, not a secret.** The authenticator hands it to any origin
   * it is asked to sign for; withholding it protects nothing. It is here
   * because `signalAllAcceptedCredentials` matches the surviving credentials by
   * it, and a list that omitted it would leave a deleted passkey in the
   * picker forever.
   */
  credentialId: string;
  nickname: string | null;
  createdAt: string;
  lastUsedAt: string | null;
}

/** A WebAuthn challenge plus the id that lets the server find its own state. */
export interface RegistrationChallenge {
  ceremonyId: string;
  challenge: unknown;
}

export interface AuthenticationChallenge {
  ceremonyId: string;
  challenge: unknown;
}

/**
 * A one-time code letting an account register a passkey again, returned once
 * to the admin who cleared them. Nothing is emailed.
 */
export interface ClaimCode {
  code: string;
  link: string;
}

export interface Team {
  id: string;
  orgId: string;
  slug: string;
  name: string;
  createdAt: string;
}

export interface TeamMember {
  userId: string;
  email: string;
  name: string | null;
  /** See `User.label` — what to render where an address is missing. */
  label: string;
  joinedAt: string;
}

export interface Repo {
  id: string;
  orgId: string;
  slug: string;
  name: string;
  provider: Provider;
  defaultBranch: string;
  teamId: string | null;
  defaultAgentType: string | null;
  active: boolean;
  createdAt: string;
  createdBy: string | null;
}

export type TrackerProvider = 'github' | 'jira';

/**
 * One org's connection to a tracker.
 *
 * There is no field for the stored credential and there is not meant to be:
 * the server returns `hasCredentials` and never the ciphertext itself.
 */
export interface TrackerConnection {
  id: string;
  provider: TrackerProvider;
  /** GitHub: the App installation id. JIRA: the cloud site id. */
  externalId: string;
  hasCredentials: boolean;
  createdAt: string;
  updatedAt: string;
}

/**
 * What this deployment can take an admin through, for one provider.
 *
 * Read at runtime rather than baked into the bundle: `startUrl` carries the
 * App slug or OAuth client id of whichever deployment served the page, and a
 * hard-coded one is how a staging console sends an admin to install the
 * production App.
 */
export interface ProviderSetup {
  configured: boolean;
  /** Where to send the browser to begin, minus its `state`. */
  startUrl: string | null;
}

export interface TrackerConnections {
  connections: TrackerConnection[];
  github: ProviderSetup;
  jira: ProviderSetup;
}

export interface TrackerBinding {
  id: string;
  repoId: string;
  provider: TrackerProvider;
  /** GitHub: `owner/repo`. JIRA: a project key. */
  externalRef: string;
  triggerLabel: string;
  /** False while the org has no connection for this provider. */
  live: boolean;
  createdAt: string;
  updatedAt: string;
}

/**
 * An advisory, time-bounded claim on one branch. The server cannot enforce it
 * against a git operation it cannot see; it makes collisions visible rather
 * than impossible.
 */
export interface Lease {
  id: string;
  repoId: string;
  branch: string;
  holderUserId: string;
  holderLabel: string | null;
  jobId: string | null;
  acquiredAt: string;
  renewedAt: string;
  expiresAt: string;
}

export interface Job {
  id: string;
  orgId: string;
  repoId: string;
  teamId: string | null;
  title: string;
  description: string | null;
  status: JobStatus;
  ticketRef: string | null;
  tracker: 'jira' | 'github' | null;
  agentType: string | null;
  /** Opaque to otto-factory. A customer's own skill owns the shape. */
  metadata: Record<string, unknown>;
  createdAt: string;
  startedAt: string | null;
  completedAt: string | null;
  attempts: number;
  result: string | null;
  error: string | null;
  createdBy: string | null;
  claimedBy: string | null;
  claimedByLabel: string | null;
}

export interface JobDetail extends Job {
  dependsOn: string[];
}

/**
 * `blocked` overlaps `pending` rather than partitioning it: it counts the
 * pending jobs still waiting on a dependency. Two pending jobs where one cannot
 * start is not the same queue as two that can.
 */
export interface QueueStats {
  pending: number;
  inProgress: number;
  active: number;
  completed: number;
  failed: number;
  blocked: number;
  total: number;
}

export interface UsageStatus {
  plan: string;
  includedOps: number;
  billableUsed: number;
  remaining: number;
  totalCalls: number;
  periodStart: string;
  warning: boolean;
  hardStop: boolean;
  /** Whether the server is currently refusing billable calls over the bucket. */
  enforced: boolean;
}

/** A live credential. Never includes the token itself. */
export interface TokenSummary {
  id: string;
  name: string | null;
  kind: TokenKind;
  clientId: string | null;
  scopes: string[];
  createdAt: string;
  lastUsedAt: string | null;
  expiresAt: string;
}

/** Shown once. Only a SHA-256 hash of `token` is stored. */
export interface MintedToken {
  token: string;
  id: string;
  name: string;
  scopes: string[];
  /** The MCP endpoint this token is audienced for. */
  resource: string;
}

export interface BrowserSession {
  id: string;
  userId: string;
  expiresAt: string;
  createdAt: string;
}

export interface AuditEvent {
  id: number;
  orgId: string | null;
  actorUserId: string | null;
  actorLabel: string | null;
  action: string;
  targetType: string | null;
  targetId: string | null;
  ip: string | null;
  userAgent: string | null;
  detail: Record<string, unknown>;
  createdAt: string;
}

/**
 * The relying party every passkey on this deployment is bound to.
 *
 * Read from the server rather than taken from `location.hostname`: an rp_id may
 * be a registrable *parent* of the origin, and a browser discards a signal that
 * names the wrong one without an error — so a guess no-ops on exactly the
 * deployments where it differs, and nobody finds out.
 */
export interface WebauthnConfig {
  rpId: string;
}

/** RFC 9728, as `/.well-known/oauth-protected-resource` serves it. */
export interface ProtectedResourceMetadata {
  resource: string;
  authorization_servers: string[];
  scopes_supported: string[];
  bearer_methods_supported: string[];
}
