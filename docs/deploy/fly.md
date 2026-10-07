# Hosting: Fly.io

otto-factory runs as a single persistent machine on Fly.io — no scale-to-zero, because
`Watcher::spawn` (of-core) holds a detached `LISTEN` connection that a request-scoped
serverless model would fight. See `CLAUDE.md` for why the process must stay warm.

## Org and infra (provisioned)

- **Org**: `savvagent`.
- **App**: `otto-factory-mcp`, freshly created rather than renamed in place — Fly does
  not support renaming an app's slug, and the previous `dark-factory-mcp` app predates
  this rename.
- **Database**: database `otto_factory` on `otto-db` (region `iad`), a standalone
  (unmanaged) Fly Postgres app **shared with otto-platform**, which keeps its identity
  data in database `otto_platform` on the same instance.
  - Unmanaged because migrations need `CREATEROLE`, which the managed `savvagent-pg`
    cluster (used by `nels-api`) does not grant to a tenant app's own role.
  - Shared for cost. Each app's attach role is a **superuser on the whole instance**, so
    either app's credential can reach the other's database. Split onto dedicated
    instances before real customers.
  - Connecting role: the one `fly postgres attach` created for this app (a
    **superuser**), which is what makes the role migrations succeed (see below).
  - `DATABASE_URL` is staged as an app secret.
- **Migrations are otto-factory's own.** `of_core::migrate` applies
  `crates/of-core/migrations` (0001 onward) to `otto_factory`. otto-factory runs on the
  otto-platform crates (`otto-tenant`, `otto-core`, `otto-auth`, `otto-billing`), but it
  must never call `otto_tenant::Db::migrate`: that applies the platform's own,
  differently numbered history. The platform's tables are already here, brought in by
  otto-factory's migrations (`0033` resource registry, `0034` per-org domain claims).

- **Secrets staged**: `DATABASE_URL` and `OF_ENCRYPTION_KEY` (generated with
  `openssl rand -base64 32`, per `.env.example`; it is fed unchanged to
  `otto_tenant::crypto::Cipher`, whose AES-256-GCM format is byte-compatible with
  the data already encrypted). `OF_SIGNING_KEY` was removed; nothing reads it.

## Resource-server registration

At every startup, after migrations and before binding a port, `of-server` registers
its resource (`OF_RESOURCE_URI`) in the `resource_servers` table with
`otto_auth::resources::register`: name `otto-factory`, scopes `jobs:read jobs:write
repos:read repos:write messages trackers org:admin`, defaults `jobs:read repos:read`
(`of_core::scopes`). The authorization server validates every authorize, token,
refresh, and PAT request against that row. The upsert never re-enables a resource an
operator disabled, so a resource disabled for an incident stays disabled across restarts.

## Tenant isolation on the shared Postgres instance

The connecting role **is a superuser** (`fly postgres attach` creates it that way on an
unmanaged instance). That matters because `0032_rename_tenant_role_otto_app.sql`
(following `0018_rename_tenant_role.sql` and `0007_rls.sql`, which established the same
requirement for the role's earlier names, `df_app` and `of_app`) renames the tenant role
to `otto_app`, the name every otto-* service shares, and issues `GRANT otto_app TO
CURRENT_USER`. Both need `CREATEROLE`, and a superuser has it unconditionally. Expect at
startup:

```
INFO of_server: tenant isolation enforced as role "otto_app"
                (assumed via SET LOCAL ROLE); 16 tenant tables, 16 forced
```

So this deployment runs in the same configuration as local development and
`#[sqlx::test]`: `otto_app` exists, `Db::begin` issues `SET LOCAL ROLE otto_app` for
every tenant transaction, and the connecting superuser is never the role a request
actually runs as. `Db::verify_tenant_isolation` re-derives this from the catalog at
startup and `of-server` refuses to bind a port otherwise.

**Roles are cluster-scoped, and `otto-db` is one cluster.** `otto_app` is a single role
shared by `otto_factory` and `otto_platform`; its grants are per database. Whichever
app migrates first creates it, and the other finds it present and only re-grants. Neither
`0032` nor otto-platform's `0004_rls.sql` is disturbed by that.

**Rollout caveat for `0032`.** On `otto-db`, `otto_app` already exists (otto-platform
created it), so `0032` takes the grant-only branch: `of_app` is **not** renamed and a
still-running old release is unaffected.

On a cluster where `otto_app` does not exist yet, `0032` renames `of_app` when the new
release boots. An old release that has already served traffic cached "role assumable" at
startup (`Db`'s `OnceCell`) and keeps issuing `SET LOCAL ROLE of_app`, which now fails, so
its tenant transactions **error** (fail closed) rather than running as the superuser.
Expect 500s from the old machine until Fly stops it once the new one passes `/readyz`;
this app runs a single machine, so the window is the migration itself.

## The Phase 3 deploy is forward-only

Once the release that runs on the otto-platform crates has booted, do not redeploy the
previous image. Two of its migrations break the old binary:

- `0034_claimed_domains_per_org` drops the `claimed_domains` primary key on `(domain)`.
  The old code's `ON CONFLICT (domain)` has no matching constraint and fails every
  domain claim.
- On a cluster where `0032_rename_tenant_role_otto_app` renamed `of_app` (not `otto-db`,
  see the rollout caveat above), an old release can no longer assume that role. A
  freshly started old process finds no `of_app`, skips `SET LOCAL ROLE`, and
  `verify_tenant_isolation` refuses to start it on a superuser connection.

**Rollback means restoring the database** from a snapshot taken before the deploy, then
deploying the old image. Take that snapshot first.

> An earlier version of this section described a managed-Postgres deployment where
> `CREATEROLE` was unavailable and isolation instead relied on `FORCE ROW LEVEL
> SECURITY` applying to a non-superuser table owner. That is not this deployment, but it
> remains the fallback path `of-server` supports if this ever moves to managed Postgres.

## What's scaffolded

- `Dockerfile` — a console stage that builds `web/` into `/srv/console`, a Rust
  stage that builds `of-server`, and a slim non-root runtime holding both. The
  migrations are not copied in: `of_core::migrate` uses the `sqlx::migrate!` macro,
  which embeds them in the binary at compile time.
- `fly.toml` — `otto-factory-mcp` app, region `iad` (co-located with its Postgres
  instance), `min_machines_running = 1` with `auto_stop_machines = "suspend"` so a
  `watch` long poll never pays a cold start, health check on `GET /readyz`.

## What's assembled (Task 13 in the milestone plan)

`of-server/src/main.rs` now assembles the real server:

1. The Axum router merges of-mcp (`/mcp`, bearer-authenticated) and of-web
   (console API, OAuth AS, `/.well-known/…`), bound to `OF_BIND`. of-mcp's own
   copy of `/.well-known/oauth-protected-resource` is left out of the merge —
   see `of_mcp::mcp_endpoint` — since of-web already serves that path and
   Axum panics on two handlers for one path.
2. `/healthz` (liveness, no DB check) and `/readyz` (readiness — `SELECT 1`
   against the pool) are mounted directly in `of-server`.
3. Migrations run at startup via `of_core::migrate`, which already takes a Postgres
   advisory lock for the duration (`sqlx::migrate!`'s built-in behavior), so
   several machines starting concurrently on a fresh database wait rather than
   racing through the same DDL.
4. Structured (JSON) logging via `tracing-subscriber`, filtered by `RUST_LOG`.
5. Graceful shutdown on `SIGTERM`/Ctrl+C: stops accepting new connections,
   waits for in-flight requests, then calls `Watcher::shutdown()` to release
   its detached `LISTEN` connection before the process exits.
6. A background sweep loop for the auth tables that would otherwise grow
   without bound (`auth_attempts` is the hot-path one — see
   `of_server::spawn_sweeper`'s doc comment).
7. `Db::verify_tenant_isolation` runs after the migrations and **before the port
   is bound**, so a database that cannot enforce tenant isolation stops the
   process instead of serving. See the section above for the two configurations
   that pass.

## What a deploy can and cannot do

Everything. `/readyz` passes, the API works end to end for an MCP client, and the
console is served — `web/` is built into `/srv/console` by the Dockerfile's console
stage, which `OF_STATIC_DIR` points at.

Onboarding is self-contained: **the product sends no email**. Signing up creates a passkey
in the browser, so an account is created and signed in during a single visit to `/signup`,
with no mail provider in the loop and nothing to configure. Recovery is a second passkey,
or an org admin clearing a member's keys and handing over the one-time code that comes
back. Invitations are codes the admin copies out of the console.

There is consequently no `OF_ALLOW_LOG_MAILER`, no `Mailer`, and no deployment state in
which links go to a log instead of a mailbox.

### `OF_PUBLIC_URL`'s host is now load-bearing in a way it was not before

A passkey is cryptographically bound to the WebAuthn **relying party id**, which
`of-server` derives from `OF_PUBLIC_URL`'s host and asserts at startup (`of_web::relying_party`
refuses to boot on a mismatch rather than failing at somebody's first sign-in).

**Changing that host invalidates every passkey ever registered.** Nothing can soften it;
that is what binding a credential to an origin means. This is exactly why the hostname
below was settled before the first account existed, and why moving the console behind the
Cloudflare Worker in [issue #2](https://github.com/savvagent/otto-factory/issues/2) is
designed to keep `OF_PUBLIC_URL` unchanged rather than to swap it for a new one.

Note also that account creation now requires a browser: there is no scripted signup, so
`docs/clients/matrix.md`'s conformance sequence needs a real browser or a virtual
authenticator for its first step.

## The hostname, and why it was settled early

The public hostname is **`otto-factory.savvagent.com`**, and the app answers on
`otto-factory-mcp.fly.dev` without advertising it. DNS is at Namecheap:

```
A     otto-factory.savvagent.com  →  66.241.124.65             (Fly shared IPv4)
AAAA  otto-factory.savvagent.com  →  2a09:8280:1::186:67e:0     (this app's dedicated IPv6)
```

`fly certs add otto-factory.savvagent.com -a otto-factory-mcp` issues and renews the
certificate; nothing else in DNS is needed, because the dedicated IPv6 is what Fly
validates ownership against.

**This was decided before the first account existed, on purpose.** `OF_PUBLIC_URL`
is the OAuth issuer, the token audience, and both discovery documents — and, as the
section above lays out, it is also the WebAuthn relying party id that every passkey is
cryptographically bound to. Doing it while the database held zero users cost nothing.

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
