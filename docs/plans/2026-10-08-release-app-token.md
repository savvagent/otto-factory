# release-please on a GitHub App token

Goal: close the last item of savvagent/otto-factory#117. The repo setting
`can_approve_pull_request_reviews` ("Allow GitHub Actions to create and approve pull requests")
is now off, so a compromised workflow token cannot approve a PR — but the same switch is what let
release-please open its release PR with `GITHUB_TOKEN`. Give release-please its own credential,
and keep that credential away from every branch but `master`.

## Status — 2026-10-09

Task 1 ✅, exercised live. After the key was corrected, run 37860359372 updated release PR #207
as the app (and CI ran on a release PR for the first time); merging #207 then cut `v0.11.0`,
authored by `otto-factory-release[bot]`, applied `autorelease: tagged` without `issues: write`,
and deployed.

## Task 1 — Mint a release-app token for release-please ✅

CI configuration only; no code, schema, or interface change.

- [x] Operator: create the `savvagent`-owned GitHub App (Contents and Pull requests read and
      write; no webhook), install it on `savvagent/otto-factory` only, and store
      `RELEASE_APP_CLIENT_ID` as a repository variable.
- [x] Operator: turn off `can_approve_pull_request_reviews`.
- [x] Create the `release` environment, admitting only the `master` branch.
- [x] Operator: store `RELEASE_APP_PRIVATE_KEY` as a `release` environment secret and delete the
      repository-level copy (a repository secret is readable by any same-repo branch's run).
- [x] `release-please` job: `environment: release`; mint the token with
      `actions/create-github-app-token` (SHA-pinned, v3.2.0) scoped to contents/pull-requests
      write; pass it as `token:`; the job's own `GITHUB_TOKEN` gets `permissions: {}`.
- [x] Note the change in the semver-release spec (§1 and §2).
- [x] First `master` push after merge: release PR updated by the app, CI runs on it.
