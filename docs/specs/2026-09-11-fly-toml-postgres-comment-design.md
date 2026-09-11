# Fix fly.toml's header comment on the Postgres deployment shape

> **Status:** IMPLEMENTED — PR #156 (merged, `b6781333368007374ffe97c4c4cfa332089edde3`) —
> corrects `fly.toml`'s header comment to match the actual, documented Postgres deployment
> shape.

## Premise corrections

`fly.toml`'s header comment (lines 1-9) currently reads:

```
# Database: a dedicated `otto_factory` database and `otto-factory` schema_admin
# role on the shared `savvagent-pg` managed Postgres cluster (kyzl60xmdjxopj9g),
# isolated from the `light_factory` / nels-api databases on the same cluster.
# DATABASE_URL is already staged as a secret via `fly mpg attach`.
```

This describes a shared-managed-Postgres-cluster deployment that was considered and then
**deliberately abandoned**. `docs/deploy/fly.md` (`## Org and infra (provisioned)`, `##
Tenant isolation on a dedicated Postgres instance`) records what was actually built and why:

- The database is a **dedicated, standalone (unmanaged) Fly Postgres app**,
  `otto-factory-mcp-db` (region `iad`) — not attached to the shared `savvagent-pg` managed
  Postgres cluster that `nels-api` uses.
- The reason for the change: `mpg`'s per-cluster role model does not grant `CREATEROLE` to a
  tenant app's own role, and `0007_rls.sql` needs `CREATE ROLE of_app NOLOGIN` /
  `GRANT of_app TO CURRENT_USER`, both of which require `CREATEROLE`.
- The connecting role, `otto_factory_mcp` (created by `fly postgres attach`), **is a
  superuser** on this dedicated instance — which is what makes `CREATE ROLE of_app NOLOGIN`
  succeed at migration time, and is also what makes `CLAUDE.md`'s guard 2
  (`SET LOCAL ROLE of_app` / `FORCE ROW LEVEL SECURITY`) apply in the `of_app`-assumable
  shape rather than the `FORCE ROW LEVEL SECURITY` fallback shape.
- `DATABASE_URL` was staged via `fly postgres attach otto-factory-mcp-db -a otto-factory-mcp
  --database-name otto_factory`, not `fly mpg attach`.
- There is no `schema_admin` role in the actual deployment; `0007_rls.sql` creates and uses
  `of_app` (`NOLOGIN`), assumed via `SET LOCAL ROLE`.

`docs/deploy/fly.md`'s account was itself confirmed against the live deployment
(2026-09-10): `otto-factory-mcp-db` is a standalone app, not part of the `savvagent-pg` MPG
cluster, and its connecting role is a superuser. That confirmation is the basis for this
spec — `docs/deploy/fly.md` is treated as the accurate account and `fly.toml`'s comment is
brought into line with it, not the other way around.

## Scope

**In:**

- Rewrite `fly.toml`'s header `# Database: …` block (and the `DATABASE_URL` staging line
  immediately below it) to describe the dedicated `otto-factory-mcp-db` app, the
  `otto_factory_mcp` superuser connecting role, and `fly postgres attach` as the staging
  mechanism — matching `docs/deploy/fly.md`'s account.

**Out:**

- No change to `docs/deploy/fly.md` itself — it is already accurate; this spec brings
  `fly.toml` into agreement with it, not the reverse.
- No change to any running infrastructure, Fly app, Postgres role, or secret. This is a
  comment-only edit with zero functional or deployment impact — confirmed in the issue body
  and independently by reading `fly.toml` (the comment block is `#`-prefixed lines above the
  `app = "otto-factory-mcp"` TOML content; no key or value in the file changes).
- No change to `[env]`, `[build]`, or any other `fly.toml` section.
- Against the three constraints in `CLAUDE.md`: this change touches no repo-coordination
  logic, adds no workflow opinion, and is not agent-specific — it is a documentation
  accuracy fix in a deploy config file's comment, orthogonal to all three.

## Assumptions

- `docs/deploy/fly.md` is the accurate account, per the issue's own live-deployment
  confirmation (2026-09-10) and per this spec's independent reading of both documents (the
  live-deployment confirmation is not something this workflow can re-verify from a
  read-only repo checkout, so it is taken as given from the issue and cross-checked instead
  against `docs/deploy/fly.md`'s internal detail and consistency, which is thorough and
  specific — role name, app name, region, attach command, and the `CREATEROLE` reasoning all
  agree with each other and with `CLAUDE.md`'s tenant-isolation guard 2 discussion).
- The replacement comment should be a compact summary (matching the header's existing
  register — a handful of lines, not a restatement of all of `docs/deploy/fly.md`), with a
  pointer to `docs/deploy/fly.md` for the full account, so the two documents cannot drift
  out of agreement as silently next time — a reader who needs the detail is sent to the
  document that carries it.
- The provisioning identifier `kyzl60xmdjxopj9g` (the shared cluster's ID) is dropped rather
  than replaced with anything — the dedicated app has no analogous cluster ID to cite, and
  inventing one would be worse than omitting it.

## Goal & Success Criteria

Bring `fly.toml`'s header comment describing the Postgres deployment shape into agreement
with `docs/deploy/fly.md`'s (accurate, live-deployment-confirmed) account, so a future reader
of `fly.toml` alone is not misled about which Postgres shape is in use.

- `fly.toml`'s header comment no longer mentions the shared `savvagent-pg` cluster, the
  `kyzl60xmdjxopj9g` cluster ID, `fly mpg attach`, or a `schema_admin` role.
- `fly.toml`'s header comment names the dedicated `otto-factory-mcp-db` app, the
  `otto_factory_mcp` connecting role (and that it is a superuser, and why that matters —
  `CREATE ROLE of_app NOLOGIN` at migration time), and `fly postgres attach` as the staging
  mechanism.
- No other line in `fly.toml` changes.
- `cargo test --workspace` and the existing CI gates are unaffected (this file is not
  parsed by any test — confirmed by grepping the workspace for `fly.toml` references below).

## Error Handling & Edge Cases

None — this is a static comment in a TOML file, read by Fly's CLI/deploy tooling only as
`#`-prefixed content it ignores. There is no parse path, no runtime behavior, and no test
that reads these specific lines (confirmed: `rg -n "fly\.toml" --type rust` returns no
matches under `crates/`).

## Risks & Open Questions

- None identified. This is a documentation-accuracy fix with no functional surface; the
  only risk is introducing a *new* inaccuracy, which the plan's implementation step
  mitigates by quoting `docs/deploy/fly.md` verbatim for the facts it asserts (app name,
  role name, superuser status, attach command) rather than re-deriving them from memory.

## Tenant isolation / Metering

Not applicable — no tenant table, RLS policy, MCP tool, or billing classification is added
or touched by this change.
