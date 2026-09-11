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

✅ Shipped in savvagent/otto-factory#150 (squash-merged as `8233d6a`), closing #149. Five rounds
of mandatory rust-pro/architect-reviewer/security-auditor review ran against the PR before merge
(round 1 also had a comment-quality pass); each round's findings were fixed in the commit that
followed it, summarized as PR comments referencing the commit SHA.

- **Round 1** (initial trio + comment-analyzer): a self-location doc error in both skills' opening
  paragraphs, and the design gap the rest of the rounds build on — a dispatched subagent has no
  `Agent` tool and so cannot itself satisfy the mandatory review trio. Also introduced, in this
  round, the properties every later round hardens: the scanner's `authorAssociation` gate (only
  `OWNER`/`MEMBER`/`COLLABORATOR`-authored issues are ever labeled or queued — this repo is public
  with issues enabled), the worker's `<<<BEGIN...END>>>` fencing of job content as untrusted data,
  the `whoami` org confirmation, and `register_repo` demoted from auto-remediate to report-and-stop.
- **Round 2** (re-review): a trust-boundary gap in how fenced job content reached the dispatched
  subagent, a self-attested merge-verification gate, and the unchecked `whoami` comparison
  tightened further.
- **Round 3** (re-review of the Step 4/4.5 rework): a documented bypass around the trio gate (a
  subagent could report "shipped and merged" without ever going through Step 4.5), a
  lease-id/resource-name mismatch, and several missing failure branches.
- **Round 4** (re-review): the trio-cleared marker itself was guessable/forgeable in this public
  repo (sequential job ids, a publicly committed marker format), plus unvalidated
  branch/PR-number/lease-id interpolation into orchestrator shell commands. Fixed with an
  orchestrator-generated nonce, format validation, and moving the record-as-shipped PR under its
  own trio-review cycle instead of letting a subagent with no `Agent` tool merge it directly.
- **Round 5** (final re-review): round 4's SHA half of the marker still didn't verify anything —
  it was captured by the orchestrator at the *start* of Step 4.5 (before the review-response loop
  ran) and compared against that same already-known constant, never against what actually merged.
  Both reviewers converged on this independently. Fixed by having the follow-up subagent capture
  its own `headRefOid` immediately before merging and embedding *that* in the marker, with Step 5
  checking it against the PR's actual post-merge `headRefOid` — the comparison that genuinely
  enforces "no commit lands between the marker being posted and the merge." Also fixed: the
  record-as-shipped review cycle's own termination (an explicit `Cycle` field, capped at one
  round, its own lease), the reviewer-report fence check's false-positive on review content that
  legitimately quotes the fence syntax by name, and several smaller citation/validation gaps.

One round-1 architect finding was deferred rather than fixed in PR #150: the fast-path
(no-design-spec) criteria in `otto-factory-development` have no category for normative process
documentation, which is what let a change this design-heavy fast-path in the first place. Filed
as savvagent/otto-factory#152, not fixed here — it's a change to a different skill's own criteria,
out of scope for this PR.

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
| **Create.** `.github/skills/otto-factory-scanner/SKILL.md` (visible at `.claude/skills/otto-factory-scanner/SKILL.md` via the repo's existing symlink) | Adapted from `otto-scanner`: finds open `savvagent/otto-factory` issues with no job yet, gates queueing on the issue author's GitHub `authorAssociation` (`OWNER`/`MEMBER`/`COLLABORATOR` only — this repo is public with issues enabled, so anything else is reported for human triage rather than labeled or queued), checks/fixes a type label, queues via `add_job`/`link_ticket`. |
| **Create.** `.github/skills/otto-factory-worker/SKILL.md` (visible at `.claude/skills/otto-factory-worker/SKILL.md` via the repo's existing symlink) | Adapted from `otto-worker`, then hardened across five review rounds into a Step 4 → Step 4.5 → Step 5 split: a Step 4 implementer subagent claims the job and opens the PR but never merges; the orchestrator itself dispatches the mandatory rust-pro/architect-reviewer/security-auditor trio in Step 4.5 and hands their raw (fenced) reports to a follow-up subagent that addresses findings, captures its own head SHA immediately before merging, posts a nonce+that-SHA-bound `trio-cleared` marker, and merges — recursing on `NEEDS_SECURITY_REEVIEW` (capped at 3 rounds) and on the record-as-shipped PR's own `NEEDS_REVIEWERS_FOR_RECORD_PR` cycle (capped at 1 round, its own lease); Step 5 resolves the job only after independently verifying that marker against the PR's actual post-merge `headRefOid` (not an echoed constant), merge timing, and the post-merge CI run before `complete_job`/`fail_job`. |

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
