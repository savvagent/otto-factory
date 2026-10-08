# Milestone-1 plan status — record CI and the live deploy

Goal: bring `docs/plans/2026-09-01-milestone-1.md` in line with the repository
(savvagent/otto-factory#203). Its `## Status` block, Task 1, and Task 13 still said there was
no CI and no deploy, and the status block predated the platform cutover.

## Status — 2026-10-08

Task 1 ✅.

## Task 1 — Refresh the milestone-1 status ✅

Docs-only, one file; no code, schema, or interface change, so no test applies.

- [x] Verify each claim against `.github/workflows/ci.yml`, `fly.toml`,
      `docs/deploy/fly.md`, and `git log` (PRs #15, #39, #52, #55, #114, #194, #197, #199;
      deploy run 37796471843 for 0.10.1).
- [x] Rewrite `## Status` as of 2026-10-08: CI jobs, release-gated deploy to
      `otto-factory-mcp`, and the move of identity to otto-platform.
- [x] Task 1 🚧 → ✅ (CI exists); Task 13 🚧 → ✅ (deployed, hostname confirmed); and
      the Sequencing note.
- [x] Commit: `docs: refresh milestone-1 plan status for CI and the live deploy`.
