# otto-factory rename — design

> **Status:** IMPLEMENTED — merged in savvagent/otto-factory#51, closing savvagent/otto-factory#50.
> Task 7 (public hostname move) is manual and out-of-band; see the plan's Task 7 checklist.

## Problem

The project is being renamed from **otto-factory** to **otto-factory**. The old name is
present 317 times across 90 files as prose, and again as every internal identifier: the
`df-*` crate prefix, the `DF_*` environment namespace, the `df_app` Postgres role, the
`df_*_` token prefixes, and the `__Host-df_session` cookie.

The rename is total — identifiers included — and there are no users, so nothing needs a
compatibility window. That last fact is what makes this a mechanical change rather than a
migration: **every "old name still accepted" affordance is explicitly out of scope**, and
adding one would be a defect, not a kindness.

## Decisions

### 1. No backward compatibility, anywhere

`Config::from_env` must not read `DF_*` as a fallback for a missing `OF_*`. `CLAUDE.md`'s
rule is that a variable set but unparseable is a startup error naming it, never a quiet
default; a silently-honoured legacy alias is the same failure wearing a friendlier face.
A deployment that still sets `OF_PUBLIC_URL` should fail to boot and say so, not come up
on a value the operator believes they removed.

Likewise: no dual-read of the session cookie, no acceptance of `df_pat_`-prefixed tokens,
no `df_app` fallback in `Db::begin`.

### 2. The tenant role is renamed by a new migration, not by editing `0007_rls.sql`

Migrations are forward-only and `0007_rls.sql` has been applied. A new migration renames
the role if it exists and creates it under the new name otherwise, so both a live database
and a fresh `#[sqlx::test]` cluster converge on `of_app`:

```sql
DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'df_app') THEN
    ALTER ROLE df_app RENAME TO of_app;
  ELSIF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'of_app') THEN
    -- Same guarded shape as 0007: managed Postgres withholds CREATEROLE, and a
    -- deployment without the role is a supported shape, not a failure.
    ...
  END IF;
END $$;
```

The role is guard 2 of the two-guard tenant isolation rule, so this migration is the one
part of the rename that can silently disable a security control. `Db::verify_tenant_isolation`
is what catches it: it reads the effective role back out of the catalog and `of-server`
refuses to bind a port unless the check passes. **The proof this task landed is the startup
log line naming `of_app`, not a green test run** — `#[sqlx::test]` connects as a superuser
and bypasses RLS, so the `rls_scopes_*` tests must be re-read to confirm they still issue
`SET LOCAL ROLE of_app` explicitly. A test that lost that line passes against no policy at
all.

`TENANT_ROLE` in `crates/of-core/src/db.rs` is a single constant, and the ambient
`GRANT df_app TO CURRENT_USER` in `0007` plus the guarded grant block in `0008_audit.sql`
both reference the old name in string literals that no compiler checks.

### 3. The session cookie is renamed, and that is a security-relevant edit

`__Host-df_session` → `__Host-of_session`. The `__Host-` prefix and the
`HttpOnly; Secure; Path=/; SameSite=Lax` attribute set are asserted by tests precisely
because losing one is a silent regression nothing else would notice. The rename touches the
literal in `COOKIE_NAME`, the independent literal in the clear-cookie header, the OpenAPI
description, and eight negative tests that construct near-miss names
(`evil__Host-df_session`, `x__Host-df_session`, `__Host-df_session_other`) to prove the
matcher is exact.

Those negative tests are the trap: a careless find-and-replace updates them in lockstep with
the matcher, and they keep passing while proving nothing. Each must be re-read after the
rename to confirm it still describes a name that *differs* from the real one.

Renaming the cookie signs out every existing session. Acceptable — there are no users.

### 4. The public hostname moves to `otto-factory.savvagent.com`

`df.savvagent.com` is the OAuth issuer, the token audience, both discovery documents, and
the **WebAuthn relying party id every passkey is cryptographically bound to**. Moving it
invalidates every registered passkey, and nothing can soften that — it is what binding a
credential to an origin means.

`docs/deploy/fly.md` records that this hostname was settled before the first account existed
specifically so the cost would be zero. That reasoning applies unchanged now: the user table
is effectively empty, so the move is cheapest today and strictly more expensive every day it
is deferred. It moves.

This is the only part of the rename that is not find-and-replace — it needs Namecheap DNS
records and a `fly certs add` — so it is the last task and separable. Dropping it leaves
tasks 1–6 coherent, at the cost of a console that says "otto-factory" on a host called `df`.

### 5. Historical references are not rewritten

`docs/plans/*` and `docs/specs/*` contain `savvagent/dark-factory#NN` references recording
work that shipped under that name. Those are accurate history and stay. What changes in
historical docs is the *product name in prose* and links that must still resolve.

### 6a. The design-of-record spec is renamed too, and every inbound link follows it

`docs/specs/2026-09-01-otto-factory-design.md` is `git mv`'d to
`docs/specs/2026-09-01-otto-factory-design.md`. `CLAUDE.md` links to it by path (twice), and
`web/worker/index.ts` and the renamed dev skill each reference it — every one of those links
is fixed in the same change, or the design of record 404s from its own linking documents.

### 6b. Every renamed identifier below is a deliberate, documented breaking change

Per `CLAUDE.md`'s rule on public-interface changes, non-additive renames must be named
explicitly rather than treated as incidental refactor side-effects. This rename touches
several: the `DF_*` config env-var namespace, the `df_*_` token prefixes, the
`__Host-df_session` cookie name, and the `x-otto-factory-auth` OpenAPI extension key (an
MCP/console-facing identifier, surfaced to every coding-agent client through
`web/src/lib/openapi.ts`). None of these renames is additive — each is named here, flagged to
the architect reviewer in the PR body, and recorded in `docs/clients/matrix.md` where it
changes what a client sees. Decision 1 (no compatibility shim) is what makes each of these an
explicit break rather than a silent one: there is no dual-read anywhere.

### 6. `.github/skills/otto-factory-development/` is operational, not documentation

The skill hard-codes `--repo savvagent/dark-factory` in ~25 commands it actually executes.
GitHub's redirect keeps them working, which is the problem: they will quietly keep the old
name alive in every `gh issue edit` and `gh run list` indefinitely. The directory, the
`name:` frontmatter field, and every flag are renamed.

## Out of scope

- Any `DF_*` → `OF_*` compatibility shim (see decision 1).
- Rewriting `savvagent/dark-factory#NN` history references (decision 5).
- The `<table>_tenant_isolation` policy naming convention — unaffected; `verify_tenant_isolation`
  discovers tenant tables by that suffix and no policy name contains the product name.
- Re-pointing the git remote and renaming the GitHub repo — **already done** before this spec.

## Verification

The gates in `CLAUDE.md`, plus the two that are specific to this change:

- `cargo run -p of-server` logs `tenant isolation enforced as role "of_app"` — the only
  direct evidence guard 2 survived the role rename.
- `grep -rIn 'dark.factory\|\bdf[-_]\|\bDF_' --exclude-dir=.git --exclude-dir=node_modules \
  --exclude-dir=target .` returns only the intentional `savvagent/dark-factory#NN` history.
