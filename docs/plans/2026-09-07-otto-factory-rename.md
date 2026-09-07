# otto-factory rename — implementation plan

## Goal

Rename the project from **otto-factory** to **otto-factory** everywhere: prose, console UI,
crates (`df-*` → `of-*`), environment namespace (`DF_*` → `OF_*`), the `df_app` Postgres
role, the `df_*_` token prefixes, the `__Host-df_session` cookie, and every deployment name.
No backward-compatibility affordance is added anywhere.

## Status — 2026-09-07

✅ Implemented. Merged in `savvagent/otto-factory#51`, closing `savvagent/otto-factory#50`.

**Spec:** `docs/specs/2026-09-07-otto-factory-rename-design.md` — read it first. This plan
implements it exactly.

**Already done before this plan** (by hand, not by these tasks): the GitHub repo is renamed
to `savvagent/otto-factory`, the local checkout directory is `~/dev/otto-factory`, and
`origin` points at the new URL.

## Global Constraints

- No AI self-attribution anywhere (commits, comments, docs, PR body).
- **No compatibility shims.** No `DF_*` fallback in `Config::from_env`, no dual-read of the
  session cookie, no `df_app` fallback in `Db::begin`, no acceptance of `of_pat_` tokens.
  Per the spec's decision 1, a legacy alias is a defect here, not a courtesy.
- Use `git mv` for every directory and file rename so history follows the file.
- `savvagent/otto-factory#NN` references in `docs/plans/*` and `docs/specs/*` are history and
  are **left alone** (spec decision 5). Only the product name in their prose changes.
- Every task ends green and gets its own commit. `cargo test --workspace`,
  `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all` for Rust tasks;
  `npm run check && npm run lint && npm test` for `web/` tasks.
- Migrations are forward-only. `0007_rls.sql` and `0008_audit.sql` are **not edited**.

## File Structure

| File | Responsibility |
| --- | --- |
| `crates/df-*/` → `crates/of-*/` | **Rename (7 dirs).** Task 1. |
| `Cargo.toml` (workspace + 7 crate manifests) | **Modify.** Members list, `[workspace.dependencies]` paths, package names. Task 1. |
| `crates/of-server/src/config.rs` | **Modify.** Every `DF_*` → `OF_*`, no fallback. Task 2. |
| `crates/of-core/migrations/0018_rename_tenant_role.sql` | **Create.** `df_app` → `of_app`. Task 3. |
| `crates/of-core/src/db.rs` | **Modify.** `TENANT_ROLE` constant + doc comments. Task 3. |
| `crates/of-core/src/isolation.rs` | **Modify.** Role name in error strings and unit-test fixtures. Task 3. |
| `crates/of-auth/src/crypto.rs` | **Modify.** `ACCESS`/`SESSION`/`PAT`/`INVITE` prefixes. Task 4. |
| `crates/of-web/src/session.rs` | **Modify.** `COOKIE_NAME`, clear-cookie literal, 8 negative tests. Task 4. |
| `web/src/lib/clients.ts` | **Modify.** Every agent connect snippet. Task 5. |
| `web/src/lib/openapi.ts`, `openapi.fixtures.ts`, `of-web/src/openapi.rs` | **Modify.** `x-otto-factory-auth` → `x-otto-factory-auth`. Task 5. |
| `docs/specs/2026-09-01-otto-factory-design.md` | **Rename** → `…-otto-factory-design.md`, fix inbound links. Task 5. |
| `.github/skills/otto-factory-development/` | **Rename** → `otto-factory-development/`. Task 6. |
| `fly.toml`, `web/wrangler.jsonc`, `web/package.json`, `Dockerfile`, `.github/workflows/ci.yml` | **Modify.** Task 6. |

## Task Order & Rationale

Innermost identifiers first, outermost names last, so each task leaves a tree that builds and
a suite that passes. Crates before env vars because the env-var edits live in a crate whose
path changes; role and cookie before prose because they are the two tasks with real failure
modes and deserve a clean tree around them. The hostname move is last because it is the only
task that is not find-and-replace and the only one that can be dropped without incoherence.

## Task 1 — Crates `df-*` → `of-*` ✅

**Files:** `crates/df-{auth,billing,core,mcp,server,trackers,web}/` (rename), root `Cargo.toml`,
7 crate `Cargo.toml`s, every `use df_*` in the workspace, `Dockerfile`, `.github/workflows/ci.yml`,
`CLAUDE.md`.

Purely mechanical — ~1,460 identifier occurrences — but it must be done as one commit or the
workspace does not build.

- [x] `git mv` each of the seven crate directories: `crates/of-core` → `crates/of-core`, and
      the same for `of-auth`, `of-mcp`, `of-billing`, `of-trackers`, `of-web`, `of-server`.
- [x] Root `Cargo.toml`: update the `members` list and every `[workspace.dependencies]` entry
      (`of-core = { path = "crates/of-core" }` → `of-core = { path = "crates/of-core" }`).
- [x] Each crate `Cargo.toml`: `name = "df-x"` → `name = "of-x"`, and every intra-workspace
      dependency key.
- [x] Sweep the Rust sources: `of_core::` → `of_core::` and the same for the other six
      (hyphens in manifests and prose, underscores in `use` paths).
- [x] `Dockerfile`: `cargo build --release -p of-server`, both `cp`/`COPY` paths, and the
      `ENTRYPOINT` — four lines, all naming the binary.
- [x] `.github/workflows/ci.yml` and `CLAUDE.md`: `cargo test -p of-core --test isolation` and
      the crate-responsibility table.
- [x] `cargo build --workspace` — first proof the manifests are coherent.
- [x] `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all`.
- [x] Commit: `rename: df-* crates to of-*`.

## Task 2 — Environment namespace `DF_*` → `OF_*` ✅

**Files:** `crates/of-server/src/config.rs`, `.env.example`, `fly.toml`, `web/wrangler.jsonc`,
`web/worker/index.ts`, `.github/workflows/ci.yml`, `docs/deploy/*`.

Full list: `OF_ALLOWED_HOSTS`, `OF_ALLOWED_ORIGINS`, `OF_ALLOW_LOG_MAILER`, `OF_API_ORIGIN`,
`OF_BIND`, `OF_CLIENT_IP_HEADER`, `OF_ENCRYPTION_KEY`, `OF_ENFORCE_QUOTAS`, `OF_GITHUB_APP_*`
(6), `OF_JIRA_*` (2), `OF_LOG_FORMAT`, `OF_ORIGIN`, `OF_PUBLIC_URL`, `OF_RESOURCE_URI`,
`OF_RUN_MIGRATIONS`, `OF_SIGNING_KEY`, `OF_STATIC_DIR`, `OF_TOTP_ISSUER`, `OF_UPGRADE_URL`.

- [x] Rename every variable at its read site in `config.rs`. **Add no fallback** — per spec
      decision 1, a deployment still setting `OF_PUBLIC_URL` must fail to boot naming
      `OF_PUBLIC_URL`, which `Config::from_env`'s existing "required, no default" path already
      does correctly once the name changes.
- [x] Rename the `OF_TEST_ABSENT_VAR_XYZ` / `OF_TEST_ABSENT_LIST_XYZ` fixtures in the
      `config.rs` tests to match.
- [x] `.env.example` — including the commented `#OF_TOTP_ISSUER=otto-factory` line, whose
      *value* is also the old product name.
- [x] `web/worker/index.ts` — the `OF_ORIGIN` binding, its interface field, its two error
      strings, and the `--var OF_ORIGIN:` guidance in the thrown message.
- [x] `web/wrangler.jsonc` `vars` block; `fly.toml` `[env]`; `.github/workflows/ci.yml`.
- [x] `docs/deploy/fly.md` and `docs/deploy/cloudflare.md` — including the config table whose
      `OF_ALLOWED_HOSTS` row documents the trap that breaks every authenticated MCP call.
- [x] **Rename every key in the local, developer `.env`** (the file `.env.example` is copied to,
      gitignored) from `DF_*` to `OF_*`, keeping each value as-is. With no compatibility fallback
      (spec decision 1), `cargo run -p of-server` will refuse to boot on the old names — this step
      is what makes Task 3's later boot check pass rather than fail on an unrelated env-var error.
- [x] `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`.
- [x] `cd web && npm run check && npm run lint && npm test`.
- [x] Commit: `rename: DF_ environment namespace to OF_`.

## Task 3 — Postgres role `df_app` → `of_app`, database `otto_factory` → `otto_factory` ✅

**Files:** `crates/of-core/migrations/0018_rename_tenant_role.sql` (new),
`crates/of-core/src/db.rs`, `crates/of-core/src/isolation.rs`, `crates/of-core/src/lib.rs`,
`crates/of-core/tests/isolation.rs`, `compose.yaml`, `.env.example`,
`.github/workflows/ci.yml`, `docs/deploy/fly.md`.

**This is the one task that can silently disable a security control.** Guard 2 of the
two-guard tenant isolation rule is the role; `#[sqlx::test]` connects as a superuser and
bypasses RLS, so a green suite is not evidence.

- [x] Write `0018_rename_tenant_role.sql`. `ALTER ROLE df_app RENAME TO of_app` when `df_app`
      exists; otherwise create `of_app` under the same guarded, CREATEROLE-tolerant shape as
      `0007_rls.sql` (a deployment where the role cannot exist is supported, not a failure);
      `GRANT of_app TO CURRENT_USER` to mirror `0007`. Do **not** edit `0007` or `0008`.
- [x] `crates/of-core/src/db.rs`: `const TENANT_ROLE: &str = "of_app";` plus the doc comments
      on `Db::begin` and the `can_set_role` field.
- [x] `crates/of-core/src/isolation.rs`: the two remediation strings
      (`"revoke SUPERUSER/BYPASSRLS from of_app"`, `"CREATE ROLE of_app NOLOGIN; GRANT of_app
      TO CURRENT_USER"`), the module docs, the `effective_role: "df_app"` unit fixture, and the
      `assert!(problems[0].contains("CREATE ROLE df_app"))` assertion.
- [x] `crates/of-core/src/lib.rs` module docs.
- [x] **Re-read every `rls_scopes_*` test in `crates/of-core/tests/isolation.rs`** and confirm
      each still issues `SET LOCAL ROLE of_app` explicitly. These are the only tests that
      exercise guard 2 rather than guard 1; one that lost the statement passes against no
      policy at all.
- [x] `.github/workflows/ci.yml`: the `Pre-create df_app role` step name, its
      `CREATE ROLE df_app NOLOGIN;` statement, and the comment explaining why the step exists.
- [x] Database name `otto_factory` → `otto_factory` in `compose.yaml` (both `POSTGRES_DB` and
      the `pg_isready` healthcheck), `.env.example`'s `DATABASE_URL`, and the four places in
      `.github/workflows/ci.yml`.
- [x] Recreate the local database: `podman compose down -v && podman compose up -d`, then
      update the local `.env` to the new `DATABASE_URL`.
- [x] `cargo test -p of-core --test isolation` — then `cargo test --workspace`.
- [x] **`cargo run -p of-server` and confirm the startup log reads
      `tenant isolation enforced as role "of_app"`** (or, on a managed-Postgres shape, the
      owner-role equivalent). Per the spec, this line is the proof; the test run is not.
- [x] `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all`.
- [x] Commit: `rename: tenant role to of_app and database to otto_factory`.

## Task 4 — Token prefixes and the session cookie ✅

**Files:** `crates/of-auth/src/crypto.rs`, `crates/of-auth/src/sessions.rs`,
`crates/of-web/src/session.rs`, `crates/of-web/src/openapi.rs`, `crates/of-mcp/src/auth.rs`,
plus test fixtures across `of-auth`, `of-web`, `of-mcp`.

- [x] `crypto.rs`: `ACCESS` `df_at_` → `of_at_`, `SESSION` `df_ss_` → `of_ss_`, `PAT`
      `of_pat_` → `of_pat_`, `INVITE` `of_inv_` → `of_inv_`, and the two in-module assertions.
- [x] `session.rs`: `COOKIE_NAME` → `"__Host-of_session"`, **and** the independent literal in
      the clear-cookie header (`"__Host-df_session=; Path=/; HttpOnly; …"`) — two separate
      strings, and the second is not covered by changing the constant.
- [x] **Re-read the eight negative cookie tests one at a time.** They construct near-miss
      names — `evil__Host-df_session`, `x__Host-df_session`, `__Host-df_session_other` — to
      prove the matcher is exact. A find-and-replace updates them in lockstep with the matcher
      and they keep passing while proving nothing. Each must still name something that
      *differs* from `__Host-of_session`.
- [x] Confirm the attribute assertions (`HttpOnly`, `Secure`, `Path=/`, `SameSite=Lax`,
      `__Host-` prefix) are untouched — `SameSite=Lax` specifically, because `Strict` drops the
      cookie on the top-level navigation into `/oauth/authorize`.
- [x] `openapi.rs`: the `__Host-df_session` description and the `of_pat_…` example.
- [x] Sweep remaining fixtures: `df_ss_abc`, `df_at_abc`, `df_ss_supersecret`, `df_client_a`,
      `df_client_x`, `df_changes`, `of-webhook`.
- [x] `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all`.
- [x] Commit: `rename: token prefixes and session cookie to of_`.

## Task 5 — Product name in prose, docs, and the console UI ✅

**Files:** `README.md`, `CLAUDE.md`, `web/README.md`, `docs/specs/*`, `docs/plans/*`,
`docs/clients/matrix.md`, `docs/deploy/*`, and ~20 files under `web/src/`.

- [x] `git mv docs/specs/2026-09-01-otto-factory-design.md
      docs/specs/2026-09-01-otto-factory-design.md`, then fix every inbound link —
      `CLAUDE.md` (twice), `web/worker/index.ts`, and the dev skill both reference it by path.
- [x] Console page titles (`· otto-factory` → `· otto-factory`) in `login`, `signup`,
      `settings`, `settings/billing`, `orgs/new`, `invite/[org]`, `o/[org]/+layout`.
- [x] `+layout.svelte` header `aria-label`; `orgs/new` body copy; `o/[org]/trackers` GitHub
      App and JIRA copy; `o/[org]/queue/[job]` metadata description and comment;
      `o/[org]/repos` slug placeholder.
- [x] **`web/src/lib/clients.ts` — every connect snippet.** Claude Code
      (`claude mcp add --transport http otto-factory …` **and** the
      `claude mcp login otto-factory` follow-up line), the two JSON `mcpServers` keys, and
      Codex's `[mcp_servers.otto_factory]` TOML table (which appears twice, once with the
      `.http_headers` sub-table). These are what a customer pastes into their agent config.
- [x] The `x-otto-factory-auth` OpenAPI extension → `x-otto-factory-auth`, in its producer
      (`crates/of-web/src/openapi.rs`) and its three consumers (`web/src/lib/openapi.ts`,
      `openapi.fixtures.ts`, and the `/docs/api` page's rendering).
- [x] `web/src/lib/types.ts` doc comment; `web/src/lib/clients.ts` header comment.
- [x] Prose sweep of `README.md`, `CLAUDE.md`, `web/README.md`, `docs/**`. Leave
      `savvagent/otto-factory#NN` history references intact (spec decision 5).
- [x] `cargo test --workspace` (the OpenAPI extension key is asserted server-side).
- [x] `cd web && npm run check && npm run lint && npm test && npm run build`.
- [x] Commit: `rename: product name in docs and console UI`.

## Task 6 — Deployment names and the dev skill ✅

**Files:** `fly.toml`, `web/wrangler.jsonc`, `web/package.json`, `web/package-lock.json`,
`.github/skills/otto-factory-development/`, `docs/deploy/*`.

- [x] `fly.toml`: `app = "otto-factory-mcp"` → `"otto-factory-mcp"`, plus the three header
      comments naming the app, database, and role.
- [x] `web/wrangler.jsonc`: `otto-factory-console-dev` and the production-env
      `otto-factory-console`. Keep the comment warning about the top-level/env name split —
      it is the reason a deploy once silently created `…-console-dev-production`.
- [x] `web/package.json` `name`, and regenerate `package-lock.json`.
- [x] `git mv .github/skills/otto-factory-development .github/skills/otto-factory-development`;
      update the `name:` frontmatter, the description, and **all ~25
      `--repo savvagent/otto-factory` flags** in `SKILL.md` and `agent-prompts.md`. These are
      executed commands, not prose — the GitHub redirect is what makes leaving them dangerous
      rather than broken (spec decision 6).
- [x] `docs/deploy/{fly,cloudflare}.md`: the `[issue #2](…/otto-factory/issues/2)` links, the
      `fly deploy -a` command, the Fly role/database names, and the `otto-factory-staging`
      example.
- [x] `cd web && npm run check && npm run lint && npm test && npm run build`.
- [x] `podman build -t otto-factory .` — confirms the Dockerfile's Task 1 binary rename.
- [x] Commit: `rename: deployment names and dev skill`.

**Out-of-band, not part of this task's commit** (no credentials/interactive browser access from
this environment): creating the new Fly app and Cloudflare Workers under the new names and
re-setting every secret under its `OF_*` name, with the old `DF_*` secrets removed rather than
left set (a stale `OF_ENCRYPTION_KEY` left on the machine is exactly the "operator believes they
removed it" case from spec decision 1). Tracked as a manual deploy step in Task 7, not blocking
this PR's merge — the code is deployment-name-correct once this task's diff lands; only the
actual infrastructure objects still need to be created by whoever holds the Fly/Cloudflare
credentials.

## Task 7 — Public hostname `df.savvagent.com` → `otto-factory.savvagent.com` (manual, out-of-band) ⬜

**Not a code task and not part of the rename PR.** Every step here is either an external
infrastructure change (Namecheap DNS, `fly certs`) or an interactive browser ceremony (passkey
re-registration) that cannot be executed from this environment — there is no DNS credential, no
Fly/Cloudflare session, and no browser to complete a WebAuthn ceremony in. This task is a
checklist for whoever holds those credentials to run **after** Task 6's PR is merged and
deployed, tracked separately from the code work so a missing DNS record or an unissued
certificate never blocks or is conflated with the code review. **Invalidates every registered
passkey** — the hostname is the WebAuthn relying party id. Re-registration is required
afterwards, including for the operator running this task.

- [ ] Add Namecheap records: `A otto-factory.savvagent.com → 66.241.124.226` (Fly shared IPv4) and
      `AAAA otto-factory.savvagent.com → 2a09:8280:1::181:a2ef:0` (this app's dedicated IPv6 — Fly
      validates ownership against it, so nothing else in DNS is needed).
- [ ] `fly certs add otto-factory.savvagent.com -a otto-factory-mcp`; wait for issuance.
- [ ] Set `OF_PUBLIC_URL=https://otto-factory.savvagent.com`. It is the OAuth issuer, the token
      audience, and both discovery documents — all three follow from this one value.
- [ ] Set `OF_ORIGIN=https://otto-factory-mcp.fly.dev` on the Worker and
      `OF_ALLOWED_HOSTS` to the origin hostname. Per `docs/deploy/cloudflare.md`'s trap:
      without `OF_ALLOWED_HOSTS` every authenticated MCP call fails, because `rmcp` validates
      a `Host` header that says the origin while `OF_PUBLIC_URL` says the Worker.
- [ ] Update `docs/deploy/fly.md`'s hostname section — including the note that the hostname
      was settled early to make exactly this move cheap, which is now the record of why it
      was affordable a second time.
- [ ] Re-register a passkey and sign in end-to-end in a real browser. Usernameless sign-in is
      discoverable-credential based, so this also confirms the credential was created as a
      resident key against the new RP id.
- [ ] `curl https://otto-factory.savvagent.com/.well-known/oauth-protected-resource` — confirm the
      issuer and resource URI both name the new host.
- [ ] This checklist has no code commit of its own — the `docs/deploy/fly.md` hostname-section
      edit lands as its own small follow-up PR once the DNS/cert/passkey steps above are
      actually done, so the doc never claims a hostname is live before it is.

## Final Verification

- [x] `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`,
      `cargo fmt --all --check`.
- [x] `cd web && npm run check && npm run lint && npm test && npm run build`.
- [x] `podman build -t otto-factory .`.
- [x] `cargo run -p of-server` → `tenant isolation enforced as role "of_app"`.
- [x] `grep -rIn '<old-brand-or-crate-pattern>' --exclude-dir=.git --exclude-dir=node_modules
      --exclude-dir=target .` returns only `savvagent/otto-factory#NN` history references.
- [x] Flip the spec's `> **Status:**` to IMPLEMENTED and this plan's markers to ✅ at
      record-as-shipped, after merge.
