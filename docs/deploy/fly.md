# Hosting: Fly.io

otto-factory runs as a single persistent machine on Fly.io — no scale-to-zero, because
`Watcher::spawn` (of-core) holds a detached `LISTEN` connection that a request-scoped
serverless model would fight. See `CLAUDE.md` for why the process must stay warm.

## Topology

otto-factory is a **resource server** of the otto platform (savvagent/otto-factory#192).
Two Fly apps, two databases on one Postgres instance, one direction of dependency:

```
  otto-platform   (https://otto.savvagent.com)        otto-factory-mcp  (https://otto-factory.savvagent.com)
  OAuth AS, login, passkeys, orgs, teams, billing     POST /mcp       bearer tokens, introspected at the platform
  db: otto_platform  ◄────────── same instance ─────► db: otto_factory  repos, jobs, leases, messages, trackers
        ▲   POST /oauth/introspect, /internal/*  ◄────────────────  per request (cached 60 s)
        │   POST /internal/usage                 ◄────────────────  background shipper (usage_outbox)
        └─► POST /platform/webhooks  (org.deleted, team.deleted, member.removed) ──────────────►
```

The factory's database holds **no identity**: no users, orgs, teams, memberships, tokens,
sessions, plans, or usage totals, and no foreign key to any of them. Every `org_id`,
`team_id`, and user id in it is a uuid the platform issued.

## Org and infra (provisioned)

- **Org**: `savvagent`.
- **App**: `otto-factory-mcp`, freshly created rather than renamed in place — Fly does
  not support renaming an app's slug, and the previous `dark-factory-mcp` app predates
  this rename.
- **Database**: database `otto_factory` on `otto-db` (region `iad`), a standalone
  (unmanaged) Fly Postgres app **shared with the platform**, which keeps its identity
  data in database `otto_platform` on the same instance.
  - Unmanaged because the migration creates the `otto_app` role, which needs `CREATEROLE`
    that the managed `savvagent-pg` cluster (used by `nels-api`) does not grant to a
    tenant app's own role.
  - Shared for cost. Each app's attach role is a **superuser on the whole instance**, so
    either app's credential can reach the other's database. Split onto dedicated
    instances before real customers.
  - Connecting role: the one `fly postgres attach` created for this app (a
    **superuser**).
  - `DATABASE_URL` is staged as an app secret.
- **Migrations are otto-factory's own.** `of_core::migrate` applies
  `crates/of-core/migrations` to `otto_factory`. It must never call
  `otto_tenant::Db::migrate`, which applies the platform's own, differently numbered
  history (`tests/guards.rs` fails the build if anything does).

### Secrets

`fly secrets set` (never in `fly.toml`):

| Secret | What it is | Where it comes from |
|---|---|---|
| `DATABASE_URL` | Postgres URL for `otto_factory` | `fly postgres attach` |
| `OF_ENCRYPTION_KEY` | 32 bytes base64; seals tracker credentials at rest | `openssl rand -base64 32`. Fed unchanged to `otto_tenant::crypto::Cipher`. |
| `OF_INTROSPECTION_SECRET` | `otto_rs_…`; authenticates every call this service makes to the platform | Printed once by `resource register` / `resource rotate-secret` |
| `OF_PLATFORM_WEBHOOK_SECRET` | `otto_whsec_…`; verifies the platform's lifecycle webhooks | Printed once by `resource set-webhook` |

Non-secret settings (`OF_PUBLIC_URL`, `OF_RESOURCE_URI`, `OF_PLATFORM_URL`,
`OF_CONSOLE_CLIENT_ID`, …) live in `fly.toml`'s `[env]`.

## Registering with the platform

The platform's authorization server validates every authorize, token, refresh, and PAT
request against a registered resource, and authenticates this service's calls with the
credential issued at registration. Registration is an **operator step on the platform**
(it replaces the startup upsert the factory used to do against its own database):

```bash
# On the platform (a machine with otto-platform-server and its OTTO_* environment):
otto-platform-server resource register https://otto-factory.savvagent.com/mcp \
  --name otto-factory \
  --scopes jobs:read,jobs:write,repos:read,repos:write,messages,trackers,org:admin \
  --default-scopes jobs:read,repos:read
#   -> prints the introspection secret ONCE: set it as OF_INTROSPECTION_SECRET

otto-platform-server resource set-webhook https://otto-factory.savvagent.com/mcp \
  https://otto-factory.savvagent.com/platform/webhooks
#   -> prints the webhook signing secret ONCE: set it as OF_PLATFORM_WEBHOOK_SECRET
```

The scope list is `of_core::scopes::KNOWN` and the defaults are `of_core::scopes::DEFAULT`;
keep them in step when a release adds a scope (the factory's own check and its
`/.well-known/oauth-protected-resource` document read from the same constants, but the
platform's registered list is what it will actually issue). `OF_RESOURCE_URI` must be
**exactly** the URI registered here: it is the HTTP Basic user on every platform call and
the audience every token must carry.

At startup `of-server` makes one harmless platform call and logs whether the platform
accepted its credential (`REJECTED this service's credential` means a wrong secret or
URI). It does not refuse to start on failure: a platform outage must not stop a restart
that would otherwise serve cached tokens.

## Registering the console's sign-in client

The console signs people in with an OAuth authorization-code + PKCE flow against the
platform, and then keeps its own session cookie (`__Host-of_session`) over a platform token
pair it holds server-side. For that the platform needs to know the console as an OAuth
client: a **public, first-party** client (no secret, no dynamic registration) whose only
redirect URI is `{OF_PUBLIC_URL}/auth/callback`. Like the resource registration above, this
is an operator step on the platform:

```bash
# Run on the platform's machine (it needs the platform's OTTO_* environment):
fly ssh console -a otto-platform -C \
  'otto-platform-server client register --name "otto-factory console" \
     --redirect-uri https://otto-factory.savvagent.com/auth/callback --first-party'
#   -> prints the client_id: set it as OF_CONSOLE_CLIENT_ID
```

The `client_id` is not a secret (a public client has nothing else to authenticate with, and
PKCE is what binds a code to the browser that asked for it), so it goes in `fly.toml`'s
`[env]`, beside `OF_PUBLIC_URL`. The redirect URI is registered **exactly**: a deployment
that moves to another hostname registers a new one.

`OF_CONSOLE_CLIENT_ID` is optional. While it is unset the server boots and serves everything
else, and `GET /auth/login` answers `503 console_login_disabled`; the console then shows
its sign-in failing with that message rather than redirecting somewhere that cannot work.
Bearer tokens (scripts, CI) and the MCP surface do not depend on it.

Sessions live in the `console_sessions` table (access and refresh tokens sealed with
`OF_ENCRYPTION_KEY`; the cookie itself is stored only as a SHA-256). Because the key seals
them, **rotating `OF_ENCRYPTION_KEY` signs every console user out**. The platform rotates a
refresh token on every use and revokes the whole login if a spent one is replayed, so the
factory refreshes under a row lock and each refresh happens exactly once, even across
machines.

## Usage metering

Every tool call writes a row to `usage_outbox` in the tool's own transaction; a background
task in `of-server` ships rows to the platform's `/internal/usage` in batches of up to 500,
deleting what the platform accepted (it dedupes on the row's `event_id`, so retries are
safe). While the platform is unreachable rows accumulate and nothing user-visible breaks;
they drain, oldest first, when it returns. To watch the backlog:

```sql
SELECT count(*), min(occurred_at), max(attempts), max(last_error) FROM usage_outbox;
```

A backlog that keeps growing with `last_error` mentioning `credential` means
`OF_INTROSPECTION_SECRET` is wrong. Quota checks (`OF_ENFORCE_QUOTAS=1`) read the platform's
cached usage status, so a hard-stop org can overrun its bucket by one minute of usage plus
whatever is still in the outbox; if the platform cannot answer, the call is allowed (and
still recorded).

## Tenant isolation on the shared Postgres instance

The connecting role **is a superuser** (`fly postgres attach` creates it that way on an
unmanaged instance). The baseline migration creates the `otto_app` role if it is missing
and `GRANT otto_app TO CURRENT_USER`; both need `CREATEROLE`, which a superuser has
unconditionally. Expect at startup:

```
INFO of_server: tenant isolation enforced as role "otto_app"
                (assumed via SET LOCAL ROLE); 12 tenant tables, 12 forced
```

So this deployment runs in the same configuration as local development and
`#[sqlx::test]`: `otto_app` exists, `Db::begin` issues `SET LOCAL ROLE otto_app` for
every tenant transaction, and the connecting superuser is never the role a request
actually runs as. `Db::verify_tenant_isolation` re-derives this from the catalog at
startup and `of-server` refuses to bind a port otherwise.

**Roles are cluster-scoped, and `otto-db` is one cluster.** `otto_app` is a single role
shared by `otto_factory` and `otto_platform`; its grants are per database. Whichever
app migrates first creates it, and the other finds it present and only re-grants.

> An earlier version of this section described a managed-Postgres deployment where
> `CREATEROLE` was unavailable and isolation instead relied on `FORCE ROW LEVEL
> SECURITY` applying to a non-superuser table owner. That is not this deployment, but it
> remains the fallback path `of-server` supports if this ever moves to managed Postgres.

## Cutover checklist (the one-time reset)

The platform split replaces the factory's 34 migrations with a single domain-only baseline
(`0001_baseline.sql`) and moves identity to the platform. **Production data was disposable,
so nothing is migrated: the factory database is recreated empty, accounts are created fresh
at the platform, and passkeys and OAuth clients are re-registered.** This is the only time
the migration history has been rewritten; it is append-only again from the baseline.

**There is no rollback to the previous image.** The old binary expects identity tables in
its own database and a different `_sqlx_migrations` history. Rolling back means
re-creating the old database from a snapshot taken first.

Order matters; the platform goes first.

1. **Platform up.** `otto-platform` is deployed at `https://otto.savvagent.com`, its
   `/readyz` is green, and you can sign in and create your org there. (Its `rp_id` is the
   platform host, so the factory's old passkeys are gone by design.)
2. **Snapshot** `otto_factory` (and take note of anything you need from it; it is about to
   be dropped).
3. **Register the resource** at the platform (see above) and keep the two secrets it prints.
4. **Recreate the factory database empty.** On `otto-db`:
   ```sql
   DROP DATABASE otto_factory;   -- after stopping otto-factory-mcp, or with FORCE
   CREATE DATABASE otto_factory;
   ```
   (`fly postgres connect -a otto-db`). Do not touch `otto_platform`.
5. **Stage the secrets** on `otto-factory-mcp`:
   ```bash
   fly secrets set -a otto-factory-mcp \
     OF_INTROSPECTION_SECRET='otto_rs_…' \
     OF_PLATFORM_WEBHOOK_SECRET='otto_whsec_…'
   ```
   `DATABASE_URL` and `OF_ENCRYPTION_KEY` are unchanged. (Tracker credentials in the old
   database are gone with it; the key only has to stay stable from here on.)
6. **Deploy the factory** (merge the release PR; or `fly deploy -a otto-factory-mcp`). It
   applies the baseline to the empty database, verifies tenant isolation, checks its
   credential against the platform, and binds a port. Look for
   `the otto platform accepted this service's credential`.
7. **Verify**: `GET /.well-known/oauth-protected-resource` names the platform in
   `authorization_servers`; an MCP client pointed at `https://otto-factory.savvagent.com/mcp`
   is sent to the platform to sign in, and `whoami` answers with the platform's records.
   `usage_outbox` drains (`SELECT count(*) FROM usage_outbox` goes to 0).
8. **Webhooks**: delete a throwaway team at the platform and check `audit_events` for a
   `platform.team.deleted` row (`GET /api/orgs/{org}/audit?actionPrefix=platform.`).
9. **Update client configuration**: MCP clients re-authorize against the platform (their old
   tokens and registrations were the factory's and are gone). `docs/clients/matrix.md` and
   `client-skills/` need only the MCP URL, which is unchanged.

10. **Register the console's sign-in client** at the platform and set
    `OF_CONSOLE_CLIENT_ID` (see *Registering the console's sign-in client* above), then
    redeploy. The console is not usable until this is done: `/auth/login` answers
    `503 console_login_disabled` and the UI shows its sign-in failing. Verify by opening
    `https://otto-factory.savvagent.com/` in a browser: **Sign in** goes to the platform,
    comes back to `/o/<your org>`, and the header shows your name and a **Manage** menu
    linking into the platform's console. Agents (`/mcp`) never depended on this step.

## What's scaffolded

- `Dockerfile` — a console stage that builds `web/` into `/srv/console`, a Rust
  stage that builds `of-server`, and a slim non-root runtime holding both. The
  migrations are not copied in: `of_core::migrate` uses the `sqlx::migrate!` macro,
  which embeds them in the binary at compile time.
- `fly.toml` — `otto-factory-mcp` app, region `iad` (co-located with its Postgres
  instance), `min_machines_running = 1` with `auto_stop_machines = "suspend"` so a
  `watch` long poll never pays a cold start, health check on `GET /readyz`.

## What's assembled

`of-server/src/main.rs` assembles the real server:

1. The Axum router merges of-mcp (`/mcp`, bearer-authenticated, plus
   `/.well-known/oauth-protected-resource` naming the platform as the authorization
   server) and of-web (the console API, tracker webhooks, and `/platform/webhooks`), bound
   to `OF_BIND`.
2. `/healthz` (liveness, no DB check) and `/readyz` (readiness — `SELECT 1`
   against the pool) are mounted directly in `of-server`. Neither depends on the platform:
   a platform outage must not take a replica out of rotation.
3. Migrations run at startup via `of_core::migrate`, which already takes a Postgres
   advisory lock for the duration (`sqlx::migrate!`'s built-in behavior), so
   several machines starting concurrently on a fresh database wait rather than
   racing through the same DDL.
4. Structured (JSON) logging via `tracing-subscriber`, filtered by `RUST_LOG`.
5. The usage shipper (`of_billing::outbox::run`) drains `usage_outbox` to the platform.
6. Graceful shutdown on `SIGTERM`/Ctrl+C: stops accepting new connections,
   waits for in-flight requests, makes a final usage flush, then calls
   `Watcher::shutdown()` to release its detached `LISTEN` connection before the
   process exits.
7. `Db::verify_tenant_isolation` runs after the migrations and **before the port
   is bound**, so a database that cannot enforce tenant isolation stops the
   process instead of serving.

## What a deploy can and cannot do

`/readyz` passes and the API works end to end for an MCP client once the resource is
registered at the platform and its two secrets are set; the console additionally needs
`OF_CONSOLE_CLIENT_ID`. The product sends no email and holds no accounts: signing in,
passkeys, recovery, invitations, SSO, members, and teams are the platform's, and the
console links there. See the cutover checklist for the order.

## The hostname

The public hostname is **`otto-factory.savvagent.com`**, and the app answers on
`otto-factory-mcp.fly.dev` without advertising it. DNS is at Namecheap:

```
A     otto-factory.savvagent.com  →  66.241.124.65             (Fly shared IPv4)
AAAA  otto-factory.savvagent.com  →  2a09:8280:1::186:67e:0     (this app's dedicated IPv6)
```

`fly certs add otto-factory.savvagent.com -a otto-factory-mcp` issues and renews the
certificate; nothing else in DNS is needed, because the dedicated IPv6 is what Fly
validates ownership against.

`OF_PUBLIC_URL` is the origin of the discovery pointer in a `401` and of the tracker OAuth
callbacks; `OF_RESOURCE_URI` is the token audience. **Passkeys are no longer bound to this
hostname** — they belong to the platform's host — so, unlike before the split, moving it
does not invalidate anyone's credentials (it does require re-registering the resource at the
platform under the new URI, and re-authorizing clients).

A later move behind a Cloudflare Worker keeps this same hostname: a Worker custom
domain serves `otto-factory.savvagent.com` directly and proxies to the Fly app, so
`OF_ORIGIN` becomes `otto-factory-mcp.fly.dev` and `OF_PUBLIC_URL` does not
change. See `cloudflare.md`, whose `OF_ALLOWED_HOSTS` trap is exactly about that
split.

## Deploying

Deploys are automatic: the `deploy` job in `.github/workflows/ci.yml` runs
`flyctl deploy --remote-only -a otto-factory-mcp` when the release-please-maintained
"chore: release X.Y.Z" PR is merged to `master` — i.e., when release-please has just cut a
release — authenticated via the `FLY_API_TOKEN` repository secret (an app-scoped Fly deploy
token, minted with `fly tokens create deploy -a otto-factory-mcp` and never valid for any
other app on the `savvagent` org). A failed `flyctl deploy` leaves the previously-running
machine serving traffic, since Fly's own rolling-deploy health check (`/readyz`) never cuts
traffic to a machine that hasn't passed it.

A push that fails `rust`, `web`, or `docker-build` never reaches `deploy` — `release-please`
(which cuts the tag/Release `deploy` gates on) itself needs all three to pass first, so a red
check anywhere upstream skips the whole chain rather than attempting a deploy against broken CI.

For an out-of-band deploy — re-deploying without a new commit, or deploying a specific
historical SHA — the manual command still works exactly as before:

```bash
fly deploy -a otto-factory-mcp
```

which will build the Dockerfile, run migrations on startup, and pass the `/readyz`
check before routing traffic to the machine.

## Operational notes

- `usage_outbox_rejected` holds usage events the platform refused (with its reason); a row there
  is billing the platform was never told about, and each one is also logged at error level.
- `platform_events` markers older than 30 days and expired `removed_members` tombstones are swept
  hourly. `deleted_orgs` is permanent (the platform never reuses an org id).
