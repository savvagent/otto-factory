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

✅ Shipped in PR #156 (merged, `b6781333368007374ffe97c4c4cfa332089edde3`). Task 1
implemented and committed; verification steps re-run and checked off during PR #156's
review-response pass, which also corrected two accuracy issues the mandatory review trio
found in the header text itself (see Task 1's steps below) and fixed the `git commit -m`
instruction's non-conventional-commit type.

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

- [x] Confirm the current header text with `sed -n '1,9p' fly.toml` and confirm
      `docs/deploy/fly.md` still says what the spec quotes (`sed -n '7,30p' docs/deploy/fly.md`) —
      guards against either file having moved since the spec was written.
- [x] Replace lines 1-9 of `fly.toml` (currently the `# otto-factory on Fly.io...` through
      `# DATABASE_URL is already staged as a secret via \`fly mpg attach\`.` block) with
      (final text, as corrected during PR #156's review-response pass — the mandatory
      review trio caught that the first version cited `0007_rls.sql` alone for
      `CREATE ROLE of_app NOLOGIN`, which actually happens in
      `0018_rename_tenant_role.sql`; dropped `--database-name otto_factory` from the
      quoted `fly postgres attach` command; and stated the connecting role's superuser
      status as a bare enabler without noting that it also means the role bypasses RLS
      entirely — all three are fixed below):
      ```
      # otto-factory on Fly.io, org "savvagent".
      #
      # Database: a dedicated, standalone (unmanaged) Fly Postgres app,
      # `otto-factory-mcp-db` (region iad), created specifically for this app rather
      # than attached to the shared `savvagent-pg` managed Postgres cluster that
      # nels-api uses — mpg's per-cluster role model does not grant CREATEROLE to a
      # tenant app's own role, which `0007_rls.sql` needs. The connecting role,
      # `otto_factory_mcp` (created by `fly postgres attach`), is a superuser on this
      # dedicated instance, which is what makes `CREATE ROLE of_app NOLOGIN`
      # (`0018_rename_tenant_role.sql`) succeed at migration time. The flip side:
      # this role BYPASSES RLS entirely, FORCE included — isolation rests wholly on
      # `Db::begin` issuing `SET LOCAL ROLE of_app`, and any statement that runs
      # directly on the pool (every migration, per `Db::migrate`) sees every org's
      # rows. DATABASE_URL is already staged as a secret via `fly postgres attach
      # otto-factory-mcp-db -a otto-factory-mcp --database-name otto_factory`. See
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
- [x] Confirm no other line in `fly.toml` changed: `git diff fly.toml` shows only the
      header comment block touched, `app = "otto-factory-mcp"` and everything below it
      byte-identical.
- [x] Confirm the file still parses as valid TOML (comments are the only edit; `fly.toml`
      isn't read by any Rust binary, so there's no `cargo run` check to run) — confirm
      with a TOML-aware check instead:
      `python3 -c "import tomllib; tomllib.load(open('fly.toml','rb'))"` (or `tomli` if
      `tomllib` unavailable) — confirmed no parse error.
- [x] Confirm nothing in the workspace parses this comment programmatically:
      `rg -n "fly\.toml" --type rust` — confirmed no matches (already confirmed in the
      spec; re-confirmed here since the plan is the last checkpoint before commit).
- [x] Format and commit: `git commit -m "docs: correct fly.toml's header comment on the Postgres deployment shape"`
      (no `cargo fmt` needed — no Rust file touched). `docs` is the correct
      Conventional-Commits type for the PR title `pr-title` CI checks against — `deploy`
      is not in `.github/workflows/ci.yml`'s allowed type list, which is why the PR
      needed retitling during review-response.

## Final Verification (after Task 1)

- [x] `git log --oneline` on the branch shows the doc commits (spec, plan, Task 1, and the
      review-response fix commit), none carrying AI attribution.
- [x] `cargo test --workspace` — vacuously unaffected (no Rust source touched); the
      branch's `rust` CI job already ran this against the equivalent no-Rust-change diff
      and passed — re-verified for the head commit that actually merges via the CI run
      captured at merge time (step (d) of the job's review-response procedure), by run ID.
- [x] No `web/` change — `npm run check`/`lint`/`test`/`build` vacuously satisfied (the
      `web` CI job also passed on this branch).
- [x] No migration, no container image, no Cloudflare Worker touched — vacuously satisfied.
- [ ] `docs/deploy/fly.md` itself unchanged — no longer holds: the review-response pass
      corrected a stale migration citation in `docs/deploy/fly.md` (`0007_rls.sql` →
      `0018_rename_tenant_role.sql` for `CREATE ROLE of_app NOLOGIN`) per the mandatory
      review trio's architect finding, since leaving the same inaccuracy in the document
      this PR cites as authoritative would defeat the PR's purpose. This is a deliberate,
      reasoned departure from the spec's original "Out: no change to `docs/deploy/fly.md`"
      scope line, not an oversight — see the PR's aggregated review-response comment.
