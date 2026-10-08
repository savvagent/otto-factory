# Milestone-1 plan Postgres shape — point Task 13 at `otto-db`

Goal: `docs/plans/2026-09-01-milestone-1.md` Task 13 stops describing the abandoned shared
`savvagent-pg` managed cluster (`fly mpg attach`) and instead names the deployment production
actually runs on — database `otto_factory` on `otto-db`, a standalone Fly Postgres app shared
with otto-platform — deferring to `docs/deploy/fly.md` as the record. Closes
`savvagent/otto-factory#158`.

Fast-path: no design spec per otto-factory-development trivial-task criteria — a one-paragraph
docs correction with no code, schema, interface, or deploy-shape change.

## Status — 2026-10-08

Done in the PR that adds this plan.

## Task 1 — Correct the Task 13 Postgres sentence ✅

**Files:** Modify `docs/plans/2026-09-01-milestone-1.md` (Task 13, "The target itself already
exists" paragraph).

- [x] Confirm the current shape in `docs/deploy/fly.md` §"Org and infra": `otto_factory` on
      `otto-db` (`iad`), unmanaged, shared with otto-platform's `otto_platform` database,
      attached with `fly postgres attach`; `savvagent-pg` rejected for lacking `CREATEROLE`.
- [x] Replace the `savvagent-pg` / `fly mpg attach` sentence with that shape and a link to
      `docs/deploy/fly.md`.
- [x] `grep -rn "savvagent-pg\|mpg attach" docs/plans` — what remains is the corrected
      sentence itself (naming `savvagent-pg` as the rejected option) and historical plans that
      describe the earlier `fly.toml` fix.
- [x] Commit: `docs: point milestone-1 plan at the otto-db Postgres deployment`.
