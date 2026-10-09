# Deploy credential in a master-only environment

Goal: close savvagent/otto-factory#215. `FLY_API_TOKEN` is a repository secret, so any same-repo
branch can run its own edited copy of `ci.yml` and read it. Give `deploy` the same protection the
release app key got in #211.

## Status — 2026-10-09

Task 1 built; **not yet exercised live**. The next release's `deploy` job is the test.

## Task 1 — Run `deploy` in the `production` environment

CI configuration only; no code, schema, or interface change.

- [x] Create the `production` environment: deployment branches `master` only, no admin bypass.
- [x] `deploy` job: `environment: production` (with the service URL).
- [x] `docs/deploy/fly.md`: the token is an environment secret, and how to rotate it.
- [ ] Operator: mint a new deploy token, store it as a `production` environment secret, delete
      the repository-level `FLY_API_TOKEN`, and revoke the old token.
- [ ] Next release: `deploy` succeeds and records a `production` deployment.
