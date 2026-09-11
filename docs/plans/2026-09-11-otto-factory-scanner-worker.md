# otto-factory-scanner and otto-factory-worker skills — implementation plan

**Fast-path: no design spec per otto-factory-development trivial-task criteria** — two new
Markdown skill files (docs, not source), no new public interface, no breaking change, no
auth/tenant/metering/migration/crate-boundary/deploy-shape surface touched, no tested behavior
changed, acceptance criterion fits one sentence: *this repo can self-host its own
issue-to-job pipeline the same way `savvagent/otto` self-hosts via `otto-scanner`/`otto-worker`,
using `otto-factory-development` in place of `otto-development`.*

**Spec:** none (fast-path). Source material: `~/dev/otto/.claude/skills/otto-scanner/SKILL.md`
and `~/dev/otto/.claude/skills/otto-worker/SKILL.md`, read in full before drafting. Adaptation
requirements are recorded in savvagent/otto-factory#149.

## Goal

`otto-scanner` and `otto-worker` are Claude-Code-native skills in `savvagent/otto` that bridge
GitHub Issues and the `otto-factory` job queue for that repo: one finds uncompliant/unqueued
open issues and queues them, the other claims and works exactly one queued job end-to-end via
`otto-development`. otto-factory has no equivalent for its own repo. Copy and adapt both,
retargeted at `savvagent/otto-factory` and dispatching this repo's own
`otto-factory-development` skill instead of `otto-development`, with the compliance check in
the scanner simplified to match this repo's actual (looser) tracker conventions — no
`creating-github-issues`-style body-shape requirement exists here. Implements
savvagent/otto-factory#149.

## Status — 2026-09-11

🚧 In progress — implementation complete, addressing mandatory review trio findings on PR #150.
Round 2 re-review found structural issues in the worker's Step 4/4.5/5 split (a self-report
merge bypass, a lease-id/resource-name mismatch, and double ownership of review findings) and
in the scanner's per-issue trust ordering; both `SKILL.md` files were revised accordingly. The
worker's Step 5 verification gate is now a branch-resolution query
(`gh pr list --head <branch> --state merged --json
number,mergedAt,headRefName,closingIssuesReferences,body,comments,statusCheckRollup`) requiring
merge timing after the claim, an issue cross-reference, green CI by run id, and a
per-job `trio-cleared` marker comment — not the earlier two-field `gh pr view` check.

## Global Constraints

- No AI self-attribution anywhere — commits, PR body, skill content.
- Both files live at `.github/skills/<name>/SKILL.md` — this repo's single source of truth for
  skill content per `CLAUDE.md`'s "Development skills" section — and are visible to Claude Code
  at `.claude/skills/<name>/SKILL.md` only via the repo's existing directory symlink. Unlike
  `otto`, there is no CI port-verification/diff-checking system to maintain a second copy against.
- No SQL, no crate, no MCP tool, no console route, no migration, no `OF_*` config key is
  touched by this change — it is skill-content only.
- Every reference to `savvagent/otto`, `otto-development`, or `creating-github-issues` in the
  copied source must be replaced with this repo's own equivalents before commit.

## File Structure

| File                                                    | Responsibility                                                                                                   |
| --------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| **Create.** `.github/skills/otto-factory-scanner/SKILL.md` (visible at `.claude/skills/otto-factory-scanner/SKILL.md` via the repo's existing symlink) | Adapted from `otto-scanner`: finds open `savvagent/otto-factory` issues with no job yet, checks/fixes a type label, queues via `add_job`/`link_ticket`. |
| **Create.** `.github/skills/otto-factory-worker/SKILL.md` (visible at `.claude/skills/otto-factory-worker/SKILL.md` via the repo's existing symlink) | Adapted from `otto-worker`: claims exactly one queued job for `savvagent/otto-factory`, dispatches a subagent to run it via `otto-factory-development`, resolves it (`complete_job`/`fail_job`). |

## Task Order & Rationale

Single task — both files are one adaptation pass over the same two source documents, and
neither is usable without the other (the worker's cross-reference names the scanner and vice
versa), so they are written and reviewed together.

## Task 1 — Add `otto-factory-scanner` and `otto-factory-worker` ✅

**Files:** `.github/skills/otto-factory-scanner/SKILL.md`, `.github/skills/otto-factory-worker/SKILL.md`

**Interfaces:** consumes the `otto-factory` MCP server's own tools (`whoami`, `resolve_repo`,
`list_repos`, `register_repo`, `list_jobs`, `add_job`, `link_ticket`, `ready`, `claim_jobs`,
`renew_claim`, `acquire_lease`, `renew_lease`, `release_lease`, `get_job`, `complete_job`,
`fail_job`, `cancel_job`) and `gh issue`/`gh label` against `savvagent/otto-factory`; produces
no new interface of its own (skill content only, no code).

- [x] Read `~/dev/otto/.claude/skills/otto-scanner/SKILL.md` and
      `~/dev/otto/.claude/skills/otto-worker/SKILL.md` in full.
- [x] Write `.github/skills/otto-factory-scanner/SKILL.md`:
  - Retarget every `savvagent/otto` reference to `savvagent/otto-factory`.
  - Replace the `creating-github-issues`-based compliance check (Step 2) with: exactly one type
    label (`bug`/`enhancement`/`documentation`) present, confirmed against
    `gh label list --repo savvagent/otto-factory`, added via `gh issue edit` if missing. Drop
    the body-shape (Steps-to-reproduce/Acceptance-criteria-section) requirement — no skill in
    this repo mandates that shape, and `otto-factory-development`'s own tracker table treats
    the issue body as the AC verbatim with no required structure.
  - Keep: the "roster" subagent step (open issues minus housekeeping labels minus
    already-queued jobs via `list_jobs`/`ticketRef`), the per-issue parallel-subagent fan-out
    (capped batch of 15), the `idempotencyKey` scheme (`otto-factory-scanner-issue-<n>`), the
    `add_job`/`link_ticket` sequence, and the final report shape — these are otto-factory MCP
    mechanics, not `otto`-repo-specific.
  - Update frontmatter `name:`/`description:` and every cross-reference
    (`Common Rationalizations`, `Red Flags`, `Cross-references`) to point at
    `otto-factory-worker` and drop the `creating-github-issues`/`otto-development` mentions.
- [x] Write `.github/skills/otto-factory-worker/SKILL.md`:
  - Retarget every `savvagent/otto` reference to `savvagent/otto-factory`.
  - Replace every `otto-development` dispatch with `otto-factory-development` (including the
    Step 4 subagent prompt template, its cutting-a-release rule reference, and its worktree
    convention reference — confirm the worktree path matches `otto-factory-development`'s own
    `.worktrees/<branch>` convention, not `otto`'s `.claude/worktrees/<branch>`).
  - Keep: the claim/lease keep-alive split between orchestrator and subagent, an
    independent verification gate before `complete_job` (superseded post-review — see
    Status below), the `fail_job`/`cancel_job` resolution rules, and the final report
    shape.
  - Update frontmatter `name:`/`description:` and every cross-reference to point at
    `otto-factory-scanner` and `otto-factory-development`.
- [x] Cross-check both files against each other: `otto-factory-scanner`'s Cross-references
      section names `otto-factory-worker` and vice versa; neither file mentions
      `savvagent/otto`, `otto-development`, or `creating-github-issues` anywhere
      (`grep -rn 'savvagent/otto"\|otto-development\|creating-github-issues' .github/skills/otto-factory-scanner .github/skills/otto-factory-worker` must return nothing).
- [x] Format and commit: `git add .github/skills/otto-factory-scanner .github/skills/otto-factory-worker && git commit -m "docs: add otto-factory-scanner and otto-factory-worker skills"`.

## Final gate

- [x] `cargo test --workspace` — vacuously satisfied, no Rust source changed; state this
      explicitly in the PR body rather than running it pointlessly (no `.rs` file in this diff).
- [x] `cargo clippy --all-targets -- -D warnings` / `cargo fmt --all --check` — same, vacuous.
- [x] `cd web && npm run check && npm run lint && npm test` — vacuous, no `web/` file changed.
- [x] No container image, console bundle, Cloudflare Worker, or migration touched — Phase 5
      out-of-band verification is vacuously satisfied; state so explicitly.
