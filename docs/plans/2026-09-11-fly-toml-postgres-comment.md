# Fix fly.toml's header comment — implementation plan

**Spec:** `docs/specs/2026-09-11-fly-toml-postgres-comment-design.md` — read it first. This
plan implements it exactly.

## Goal

Closes `savvagent/otto-factory#106`: rewrite `fly.toml`'s header comment describing the
Postgres deployment shape so it matches `docs/deploy/fly.md`'s accurate, live-deployment-
confirmed account (a dedicated, standalone `otto-factory-mcp-db` app with a superuser
connecting role `otto_factory_mcp`) instead of the abandoned shared-`savvagent-pg`-cluster
plan it currently describes.

## Status — 2026-09-11

Not yet started.

## Global Constraints

- No AI self-attribution anywhere (commits, PR body, comments, docs).
- This is a comment-only edit inside `fly.toml`; no TOML key or value changes, no
  functional or deployment impact.
- No tenant table, RLS policy, MCP tool, or `of-billing::classify` entry is added or
  touched — no cross-org negative test or billing classification needed.
- No migration, no schema change, no out-of-band artifact (container image, console
  bundle, Cloudflare Worker) is touched.
- Quote `docs/deploy/fly.md` for the facts the new comment asserts (app name, role name,
  superuser status, attach command) rather than re-deriving them from memory, per the
  spec's Risks section.

## File Structure

| File | Responsibility |
|---|---|
| `fly.toml` | **Modify.** Header comment block (lines 1-9) rewritten to describe the dedicated `otto-factory-mcp-db` app and superuser `otto_factory_mcp` role, matching `docs/deploy/fly.md`. |

## Task Order & Rationale

Single task — one file, one contiguous comment block.

## Task 1 — Rewrite fly.toml's header comment ✅

**Files:** `fly.toml`
**Interfaces:** none — comment-only, no TOML structure change.

- [ ] Confirm the current header text with `sed -n '1,9p' fly.toml` and confirm
      `docs/deploy/fly.md` still says what the spec quotes (`sed -n '7,30p' docs/deploy/fly.md`) —
      guards against either file having moved since the spec was written.
- [ ] Replace lines 1-9 of `fly.toml` (currently the `# otto-factory on Fly.io...` through
      `# DATABASE_URL is already staged as a secret via \`fly mpg attach\`.` block) with:
      ```
      # otto-factory on Fly.io, org "savvagent".
      #
      # Database: a dedicated, standalone (unmanaged) Fly Postgres app,
      # `otto-factory-mcp-db` (region iad), created specifically for this app rather
      # than attached to the shared `savvagent-pg` managed Postgres cluster that
      # nels-api uses — mpg's per-cluster role model does not grant CREATEROLE to a
      # tenant app's own role, which `0007_rls.sql` needs. The connecting role,
      # `otto_factory_mcp` (created by `fly postgres attach`), is a superuser on this
      # dedicated instance, which is what makes `CREATE ROLE of_app NOLOGIN` succeed
      # at migration time. DATABASE_URL is already staged as a secret via
      # `fly postgres attach otto-factory-mcp-db -a otto-factory-mcp`. See
      # docs/deploy/fly.md for the full account, including why the shared-cluster
      # plan was abandoned.
      #
      # Secrets are NOT in this file — `fly secrets set` puts DATABASE_URL and
      # OF_ENCRYPTION_KEY in the machine's environment. Everything below is
      # configuration a reader is meant to see.
      ```
      (the trailing "Secrets are NOT in this file..." paragraph is unchanged from today's
      file — reproduced above only so the whole header block replacement is unambiguous;
      do not duplicate it).
- [ ] Confirm no other line in `fly.toml` changed: `git diff fly.toml` shows only the
      header comment block touched, `app = "otto-factory-mcp"` and everything below it
      byte-identical.
- [ ] Confirm the file still parses as valid TOML (comments are the only edit, but verify
      mechanically): `cargo run -p of-server -- --help 2>&1 | head -5` is not applicable
      (fly.toml isn't read at runtime); instead confirm with a TOML-aware check —
      `python3 -c "import tomllib; tomllib.load(open('fly.toml','rb'))"` (or `tomli` if
      `tomllib` unavailable) — confirm no parse error.
- [ ] Confirm nothing in the workspace parses this comment programmatically:
      `rg -n "fly\.toml" --type rust` — confirm no matches (already confirmed in the spec;
      re-confirm here since the plan is the last checkpoint before commit).
- [ ] Format and commit: `git commit -m "deploy: correct fly.toml's header comment on the Postgres deployment shape"`
      (no `cargo fmt` needed — no Rust file touched).

## Final Verification (after Task 1)

- [ ] `git log --oneline` on the branch shows the two doc commits (spec, plan) plus this
      task's commit, none carrying AI attribution.
- [ ] `cargo test --workspace` — vacuously unaffected (no Rust source touched), run once
      to confirm the workspace is otherwise green on this branch: full green expected.
- [ ] No `web/` change — `npm run check`/`lint`/`test`/`build` vacuously satisfied.
- [ ] No migration, no container image, no Cloudflare Worker touched — vacuously satisfied.
- [ ] `docs/deploy/fly.md` itself unchanged — `git diff --stat` shows only `fly.toml` plus
      the two doc-plan/spec files added.
