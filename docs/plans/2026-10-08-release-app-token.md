# release-please on a GitHub App token

Goal: close the last item of savvagent/otto-factory#117. The repo setting
`can_approve_pull_request_reviews` ("Allow GitHub Actions to create and approve pull requests")
is now off, so a compromised workflow token cannot approve a PR — but the same switch is what let
release-please open its release PR with `GITHUB_TOKEN`. Give release-please its own credential.

## Status — 2026-10-08

Task 1 ✅.

## Task 1 — Mint a release-app token for release-please ✅

CI configuration only; no code, schema, or interface change. The first push to `master` after
merge is the test: release-please must update the open release PR as the app.

- [x] Operator: create the `savvagent`-owned GitHub App (Contents, Pull requests, Issues: read
      and write; no webhook), install it on `savvagent/otto-factory` only, and store
      `RELEASE_APP_CLIENT_ID` (variable) and `RELEASE_APP_PRIVATE_KEY` (secret).
- [x] Operator: turn off `can_approve_pull_request_reviews`.
- [x] `release-please` job: mint the token with `actions/create-github-app-token` (SHA-pinned,
      v3.2.0) scoped to contents/pull-requests/issues write, pass it as `token:`, and drop the
      job's own `GITHUB_TOKEN` to `contents: read`.
- [x] Note in the semver-release spec that release PRs now run CI.
