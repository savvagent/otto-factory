---
name: otto-factory-worker
description: Use when picking up and working a single job from the otto-factory job queue for savvagent/otto-factory — claiming exactly one job, dispatching a subagent to run it end-to-end via otto-factory-development, and reporting the outcome back to otto-factory (complete_job/fail_job) plus a concise summary to the user. Trigger on "pick up a job", "work the next job", "process an otto-factory job", "claim a job and work it", "run otto-factory-worker". Not for scanning/filing GitHub issues into the queue (otto-factory-scanner) or for the per-job spec → plan → implement → PR → review → merge mechanics themselves (otto-factory-development, which the dispatched subagent invokes).
---

# Otto Factory Worker

This skill is the bridge between the otto-factory job queue and actually getting
a job done: it claims one job for `savvagent/otto-factory`, hands it to a subagent that
works it end-to-end via `otto-factory-development`, and reports the result back to
otto-factory. It is Claude-Code-native — it orchestrates via the `Agent` tool
and calls `otto-factory` MCP tools directly. Unlike `otto-factory-development` (a
`.claude/skills/` symlink into `.github/skills/`, this repo's single source of truth
for the development workflow), this skill has no `.github/skills/` counterpart at all:
otto-factory has no port-verification system to keep a second copy in sync with, so
like `otto-factory-scanner` it lives directly and only at
`.claude/skills/otto-factory-worker/SKILL.md`, with no canonical/port split.

This skill only **claims, dispatches, and resolves** a job. It never scans or
files GitHub issues (that's `otto-factory-scanner`), and it never writes the spec,
plan, code, or PR itself — that is entirely `otto-factory-development`'s job, run by a
subagent this skill dispatches.

## The Iron Law

**Every job this skill claims ends up resolved — `complete_job`'d,
`fail_job`'d, or (only when the dispatched subagent complied with an
external cancellation request) `cancel_job`'d by that subagent itself —
before this skill finishes, and exactly one job is claimed per run.** A job
left claimed with no resolution blocks it from ever being retried or reported
on; claiming a second job "while you're at it" doubles the blast radius of a
single run going wrong. If this skill's own run is interrupted after claiming
but before resolving, don't hunt for and guess at reclaiming that orphaned
claim on the next invocation — a claim's TTL (kept alive by the subagent's
own `renew_claim` calls, Step 3, for as long as real work is happening) is
what protects against a truly abandoned claim: once it lapses, `ready`
surfaces the job as claimable again on its own, with no need for this skill
to guess which in-flight claim is actually dead.

## Context discipline: the real work happens in one subagent

The orchestrating session (you, reading this skill) must stay small: it reads
job *metadata* (id, title, ticket ref) and the subagent's one-line final
result, never full job descriptions rendered as prose, full diffs, or PR
review transcripts. The entire spec → plan → implement → PR → review →
merge lifecycle happens inside a single `Agent` tool call to a subagent that
was handed everything it needs up front — the orchestrator does not watch it
work, does not re-read its intermediate output, and does not do any of that
work itself. If you find yourself about to open a file, run `cargo`, or call
`gh pr` to watch progress in the orchestrator, stop — that belongs in the
subagent. (Step 5's one-time `gh pr view` call, made after the subagent has
already finished to verify its report, is the sole exception — it is not
progress-watching.)

## Step 1 — Identify the repo

1. Call `whoami` to confirm which organization this token opens.
2. Call `resolve_repo` with `remote` set to the output of
   `git remote get-url origin` (`https://github.com/savvagent/otto-factory.git`).
3. If that fails to resolve, call `list_repos`; if `savvagent/otto-factory` is truly
   unregistered, `register_repo` it before continuing.

Keep the resolved repo slug — every following otto-factory call needs it.

## Step 2 — Claim exactly one job

1. Call `ready` for the resolved repo slug to see claimable work. If nothing
   is ready, report that (Step 6) and stop — there is nothing to do.
2. Call `claim_jobs` for **one** job. Never claim more than one in a single
   run: a second claimed-but-unworked job is exactly the orphaned-job failure
   the Iron Law exists to prevent.
3. Note the job's id, title, description, `ticketRef` (if any — this is
   normally `savvagent/otto-factory#<n>` when the job came from
   `otto-factory-scanner`), and any `metadata`. This is the only job content the
   orchestrator holds onto; it gets handed to the subagent whole in Step 4
   (including `metadata`, if any was set), not re-fetched or re-summarized later.

If claiming fails (another agent took it first), go back to `ready` and try
the next candidate rather than giving up immediately — but if that retry
`ready` call itself now comes back empty, that's the same "nothing is ready"
case as Step 2.1: report it (Step 6) and stop, don't keep polling.

## Step 3 — The subagent keeps both the claim and the branch lease alive

The orchestrator dispatches the subagent in Step 4 with a single `Agent`
call, which blocks until the subagent's entire run is finished and returns
only once, at the end — there is no channel for the orchestrator to
interleave `renew_claim` or `renew_lease` calls, or learn the branch name,
while that one call is still outstanding. Two things need keeping alive
during that long single call, and the subagent is the only one that can do
either:

- **The job claim itself.** `claim_jobs` (Step 2) expires after its TTL
  (900s by default) if never renewed, and a real spec → plan → implement →
  review → merge run routinely runs longer than that — an
  unrenewed claim expiring mid-run would let a second invocation of this
  skill claim and dispatch a duplicate subagent for the same job. So the
  subagent calls `renew_claim` on `<job-id>` periodically throughout its own
  work, not the orchestrator — comfortably inside the 900s TTL, e.g. after
  each `otto-factory-development` phase transition (Phase 0 done, spec or
  fast-path plan committed, plan committed, each task implemented, PR opened,
  each review round) — those are natural checkpoints that recur far more
  often than every 15 minutes on any job worth running.
- **The branch lease.** Once the subagent knows its branch name
  (`otto-factory-development`'s Phase 0), it acquires `branch:<name>`
  (`acquire_lease`, for this repo), renews it (`renew_lease`) on the same
  phase-transition cadence as the claim renewal, and releases it
  (`release_lease`) right before it reports back to the orchestrator. Leases
  are advisory (see the otto-factory server instructions) — this makes a
  collision with another agent visible, it doesn't prevent one.

Both are self-contained steps in the subagent's own prompt (Step 4 spells
them out) — the orchestrator does neither. Both also assume the subagent can
act as the same otto-factory identity that claimed the job: it shares this
session's `otto-factory` MCP server connection, the same assumption
`otto-factory-scanner`'s own per-issue subagents already make when they call
`add_job`/`link_ticket` directly.

## Step 4 — Dispatch one subagent to do the actual work

Launch exactly one subagent (a fresh `general-purpose` agent via the `Agent`
tool — it needs no prior context from this session) with a fully
self-contained prompt. It will not see anything above this point, so the
prompt must carry everything it needs:

```
Work otto-factory job <job-id> for savvagent/otto-factory end-to-end using
this repo's `otto-factory-development` skill.

Job title: <title>
Job description: <full description, verbatim>
GitHub issue: <ticketRef, if present, e.g. "savvagent/otto-factory#123" — use
  this as the issue to work from in otto-factory-development's intake step.
  If no ticketRef is present, treat the title/description above as the plain
  task brief otto-factory-development also accepts (its ticketless path).>
Job metadata: <the job's metadata object, verbatim, if any was set — otherwise
  omit this line entirely>

Requirements:
- Work in your own isolated worktree per otto-factory-development's own
  Phase 0 worktree convention — `.worktrees/<branch>` at the repo root
  (`git worktree add .worktrees/<branch> -b <branch> origin/master`), never
  the shared main checkout. This is Non-Negotiable Rule 1.
- Use `otto-factory-development` for the full lifecycle: intake, spec
  (skippable only under its own fast-path trivial-task criteria — even then
  a minimal plan document is still written, never skipped), plan,
  implementation, PR, the mandatory review trio, fixing findings, and merge.
  `master` changes only ever land through a reviewed, merged PR — never a
  direct commit or push to `master` (Non-Negotiable Rule 2).
- Every PR needs a dedicated `rust-pro` review, a dedicated `architect-reviewer`
  review, AND an independent `security-auditor` review (which receives only
  the diff — never the spec, plan, task brief, or PR-body summary) on record
  before it merges. This is Non-Negotiable Rules 4-5, with no fast-path or
  size carve-out — do not merge, or report the PR as mergeable, until all
  three have cleared.
- Title the PR `<type>(<scope>): <subject>` (e.g. `fix(of-core): ...`,
  `feat(of-mcp): ...`) — this repo's `pr-title` CI check enforces it, and
  release-please computes the version bump and changelog from it
  automatically once its own periodic release PR merges. There is no
  separate release-cutting step for you to run; do not tag a version or edit
  `CHANGELOG.md` yourself. If your change is a **breaking** change to a
  public interface (the MCP tool surface, the console REST API, the
  OAuth/discovery endpoints, the config surface, or the schema), mark the PR
  title `<type>(<scope>)!:` or add a `BREAKING CHANGE:` footer, name it in
  the spec and plan, and flag it explicitly to the architect reviewer — this
  is Non-Negotiable Rule 6, and it is never a fast-path change.
- You are responsible for two otto-factory keep-alive calls throughout your
  run, since only you can interleave them with your own long-running work:
  call `renew_claim` on job `<job-id>` after every otto-factory-development
  phase transition (Phase 0 done, spec or fast-path plan committed, plan
  committed, each task implemented, PR opened, each review round) —
  comfortably inside the 900s default claim TTL — so the claim doesn't expire
  out from under you (resolve the repo slug yourself via `whoami`/
  `resolve_repo` first); and, as soon as your branch name is decided
  (otto-factory-development's Phase 0), `acquire_lease` on `branch:<name>`,
  renew it (`renew_lease`) on the same checkpoints, and `release_lease` as
  your very last action before you report back.
- Follow otto-factory-development's own git/GitHub mechanics directly
  (worktree add, commit, push, `gh pr create`/`gh pr merge`) — it already
  specifies these in full; do not route them through any other tool.
- Do not add any attribution anywhere — not in commit messages, PR bodies,
  code comments, or docs. No Co-Authored-By trailer, no "Generated with"
  footer, no bot marker. Omit it silently. This is Non-Negotiable Rule 3,
  with no exception for delegated or subagent work.
- If, at any point, you notice the job's cancellation was requested by
  someone else (checking `get_job`), stop your work and call `cancel_job`
  yourself before reporting back — you are the claim holder, only you can
  finalize it that way. If you decide to stop for your own reasons (e.g. the
  job turns out to be already done or a duplicate), do not call
  `cancel_job` or `request_cancel` — just say so plainly in your final
  report and let the orchestrator `fail_job` it.
- When you finish (claim renewed throughout, lease already released), report:
  outcome (shipped and merged / blocked / failed and why / stopped on request
  and already called cancel_job), the PR URL, and the branch name.
```

Wait for this subagent to finish. Do not poll it while it's running, do not
re-derive its progress from `gh` calls in the orchestrator mid-run — its
final report is what drives Step 5. Step 5 below does make exactly one
verification call against that report before trusting it, which is not
polling: it happens once, after the subagent has already finished.

## Step 5 — Resolve the job on otto-factory

Based on the subagent's final report:

- **Shipped and merged** → before calling `complete_job`, independently
  verify the claim with `gh pr view <PR-number> --repo savvagent/otto-factory
  --json state,mergedAt` and confirm `state` is `MERGED`. Do not take "shipped
  and merged" on the subagent's word alone — a subagent reporting success on a
  PR that was never actually merged is a known failure mode in this repo's own
  history, and `complete_job` on an unmerged job is exactly the wrong
  resolution to make irreversible. If verification fails (PR open, or
  doesn't exist), treat this the same as **Blocked or failed** below rather
  than completing it. Once verified, `complete_job` with a `result` string
  naming the PR URL. (Do not wait for or report a release version — this
  repo's release-please automation cuts that separately, on its own batching
  schedule, once its own periodic `chore: release` PR merges; it is not tied
  to any single job.)
- **Already stopped and finalized by the subagent itself** (it reported it
  called `cancel_job` after noticing an external cancellation request) →
  nothing to do here; the job is already resolved. Skip straight to Step 6.
- **Blocked, failed, or the subagent stopped for its own reasons** (including
  "already done" / "duplicate") → `fail_job` with the subagent's stated
  reason, worded so a human can tell a real failure from a redundant job.
  `request_cancel`/`cancel_job` are not the orchestrator's tools here: the
  orchestrator holds the claim, and per otto-factory's own tool
  descriptions, `request_cancel` on a job you already hold doesn't finalize
  anything (it only flags a cancellation for the holder to notice), and
  `cancel_job` requires that someone else requested the stop. A holder
  stopping for its own reasons calls `fail_job`, full stop. Do not silently
  retry it in the same run — a failed job needs a human look, the same
  convention `otto-factory-scanner` applies to failed/cancelled jobs it finds
  already on the queue.
- **The `Agent` call itself errored, or the subagent produced no parseable
  final report** (crashed mid-run) → `fail_job` with a reason noting the
  subagent did not report back. Never leave the job claimed with nothing
  called — that violates the Iron Law regardless of why the subagent didn't
  finish cleanly.

The subagent already released its own lease and renewed its own claim
throughout (Step 3/4), so there is nothing left to release or renew here.

## Step 6 — Report

If a job was claimed and worked, output:

```
Otto Factory Worker — savvagent/otto-factory

Job <job-id> — <title>
Outcome: shipped (PR #<n>, merged) | failed (<reason>) | cancelled on request (<reason>)
Branch: <name>
```

If Step 2 found nothing ready to claim, skip the job/branch lines entirely:

```
Otto Factory Worker — savvagent/otto-factory

Nothing ready to claim.
```

Then STOP. Do not claim another job in the same run — that's a separate
invocation of this skill.

## Common Rationalizations (all are violations)

| Excuse | Reality |
|---|---|
| "I'll claim two jobs since I'm already here" | Exactly one job per run. A second claimed job with no subagent working it is an orphaned claim. |
| "I'll just read the job description myself to see if it's worth doing" | The description goes straight into the subagent's prompt. Reading it to decide isn't the orchestrator's job — claiming already committed you to it. |
| "The subagent's taking a while, let me check `gh pr view` on its branch" | That's re-deriving progress *while it's still running*. Wait for its final report — the one `gh pr view` call Step 5 makes happens only after that report, to verify it, not to watch progress. |
| "It said 'shipped and merged', that's good enough for `complete_job`" | Verify it with `gh pr view --json state,mergedAt` first (Step 5). A subagent reporting success on a PR that was never actually merged is a known failure mode here — `complete_job` is not reversible. |
| "It failed, but I can see the fix, let me just patch it here" | The orchestrator does not touch code. Either dispatch a follow-up subagent or `fail_job` it for a human. |
| "I'll acquire the lease myself once the subagent tells me the branch name" | The `Agent` call blocks until the subagent's entire run finishes — there's no point where the orchestrator can act on a mid-run report. The subagent leases its own branch. |
| "I'll renew the job claim myself from the orchestrator too" | Same blocking-call problem as the lease. The subagent renews its own claim throughout its run — the orchestrator never gets a chance to. |
| "The job's redundant, I'll `request_cancel` it to close it out" | The orchestrator holds this claim; `request_cancel` on a job you already hold doesn't finalize anything. Use `fail_job` with a reason explaining it's redundant. |
| "No `ticketRef` on this job, I'll invent one so otto-factory-development has an issue to close" | Don't fabricate a tracker reference. Pass the job's title/description as a plain task brief — otto-factory-development's ticketless path accepts that too. |
| "The job's tiny, no need for the full rust-pro/architect-reviewer/security-auditor review trio" | Non-Negotiable Rules 4-5 in `otto-factory-development` have no fast-path or size carve-out. The dispatch prompt says so — don't soften it. |

## Red Flags — STOP

- About to claim a second job before the first is resolved
- About to run `cargo`, `gh`, or edit a file directly in the orchestrator
  instead of inside the dispatched subagent
- About to leave a claimed job without calling `complete_job` or `fail_job`
  (or confirming the subagent already called `cancel_job` itself)
- About to try acquiring/renewing a lease, or renewing the job claim, from
  the orchestrator instead of telling the subagent to own both lifecycles
- About to dispatch more than one subagent for a single job
- About to call `complete_job` on a "shipped and merged" report without
  first confirming it with `gh pr view --json state,mergedAt`
- About to call `request_cancel` or `cancel_job` from the orchestrator on a
  job it holds — that's `fail_job`'s job

Each = stop, do the step correctly, continue.

## Cross-references

- `otto-factory-development` — the actual spec → plan → implement → PR →
  review → merge lifecycle (including its own Non-Negotiable Rules on
  worktree-only edits, PR-only `master` changes, no attribution, the
  mandatory `rust-pro`/`architect-reviewer`/`security-auditor` review trio,
  and public-interface-change discipline), run by the subagent this skill
  dispatches. This skill never duplicates those mechanics. release-please
  cuts the version and changelog automatically afterward from the merged
  PR's conventional-commit title, on its own periodic schedule — not
  something this skill or its dispatched subagent orchestrates.
- `otto-factory-scanner` — the upstream counterpart that gets issues onto the queue
  this skill claims from; also the convention this skill mirrors for leaving
  a failed/cancelled job for a human rather than silently retrying it.
