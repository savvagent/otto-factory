# Claude Code — session-start marker

Registers a session-start marker job on otto-factory automatically whenever a Claude Code
session starts fresh in a repo registered with otto-factory. This is the reference
implementation for `client-skills/` — read `../README.md` first for the behavior contract
this template follows.

## What this does — a two-actor design

A `SessionStart` hook in Claude Code is a detached OS subprocess: it has no access to the
session's own authenticated otto-factory MCP connection, and there is no supported way for
an external process to borrow it. So this template splits into two actors:

- **`session-start-hook.sh`** (deterministic, no otto-factory access): reads the repo's
  `origin` remote, the current branch, and today's UTC date, then prints one JSON object
  to stdout in the shape Claude Code's `SessionStart` hook uses to inject text into the
  model's own next turn (`hookSpecificOutput.additionalContext`). The injected text is a
  literal, self-contained instruction with the remote URL, branch, and date already
  substituted in — the model is never asked to compute or guess any of these three values.
- **The model**, processing that injected context on its own next turn (already
  otto-factory-authenticated through its own normal tool access), follows the instruction:
  `resolve_repo` → `add_job` → branch on status → `claim_jobs` → `complete_job`.

This is **compliance-based, not deterministic** — nothing mechanically guarantees the model
performs the tool-call sequence. A model that ignores, narrates, or only partially follows
the injected instruction produces no marker job, or a stuck `in-progress` one. See Known
Gaps below.

## Install

1. Copy `session-start-hook.sh` into your own repo, e.g. `.claude/hooks/session-start-hook.sh`
   (or install it once at the user level, e.g. `~/.claude/hooks/session-start-hook.sh`, to
   cover every repo you work in).
2. `chmod +x` it.
3. Merge `settings-snippet.json`'s `hooks.SessionStart` entry into your repo's
   `.claude/settings.json` (project-level) or `~/.claude/settings.json` (user-level),
   adjusting the `command` path to wherever you put the script in step 1.
4. Confirm your coding agent already has a working otto-factory MCP connection (OAuth or a
   personal access token) — this template assumes that connection exists; it does not
   configure one. See the root `docs/clients/matrix.md` and the console's connect page.

The `matcher` is `"startup"` deliberately — a `resume` or `clear` restart of an existing
session is not a new work session, and firing on every `resume` would defeat the
idempotency key's "once per branch per day" intent by looking like a fresh start each time.

## The full tool-call sequence

1. `resolve_repo` with `remote` set to the captured `origin` URL. On failure (repo not
   registered, no MCP connection, any error): stop, say nothing.
2. `add_job` with:
   - `repo`: the resolved canonical `slug` from step 1 (never the raw remote URL, and never
     re-derived a second way)
   - `title`: `"session: <branch>"`
   - `description`: a fixed, literal string — identical on every call, since `add_job`
     errors if a replayed idempotency key's other arguments differ
   - `metadata`: `{"kind": "session-marker", "source": "client-skills/claude-code", "branch": "<branch>"}`
   - `idempotencyKey`: `"session-<slug>-<branch>-<yyyy-mm-dd>"`
3. Branch on the returned job's `status`:
   - `completed` → stop; today's marker for this branch already exists and is closed out.
   - `pending` → `claim_jobs` with `jobs: [<that job id>]`. If the claim fails (a concurrent
     duplicate invocation won the race), stop.
   - anything else (in practice, `in-progress`) → stop without calling `complete_job` —
     see Known Gaps.
4. On a successful claim: `complete_job` on that same job id with
   `result: "session marker — no work performed"`.

None of this is narrated in the model's reply to the developer, on success or failure — the
tool calls still appear in the normal tool-call transcript; only the prose reply stays
silent about them.

## What was verified, and how

- **The script's own deterministic half** (no live session needed): run by hand against a
  fake `SessionStart` stdin payload (`{"cwd": "<repo path>", ...}`) inside this repo's own
  checkout. Confirmed it prints a well-formed `hookSpecificOutput.additionalContext` JSON
  object with the remote URL, branch (including one containing `/` —
  `docs/client-skills-directory`, this branch's own name), and date substituted in
  correctly. Confirmed it exits `0` with no stdout for: not a git repository, a git
  repository with no `origin` remote, and a detached `HEAD`.
- **The job-lifecycle contract** (`resolve_repo` → `add_job` → `claim_jobs` → `complete_job`,
  the idempotency key, and the branch-with-`/` question): exercised directly against a live
  otto-factory server, following the injected instruction's steps by hand exactly as
  written. `resolve_repo` resolved the remote to the `otto-factory` slug; `add_job` accepted
  an idempotency key containing `/` (`session-otto-factory-docs/client-skills-directory-<date>`)
  without error; the created job claimed and completed cleanly; a same-day replay with
  identical arguments returned the original job already `completed`, with no second job
  created and no second claim attempted — confirming both the idempotency behavior and that
  branch names containing `/` need no special-casing.
- **Not verified in this pass: a genuinely fresh, separately-observed live Claude Code
  session actually firing its own `SessionStart` hook and the model autonomously following
  the injected instruction with no operator driving it by hand.** That is the one part of
  this template that verifies compliance rather than code (see the spec's Risks section),
  and doing so honestly needs a real second interactive session watching itself, which this
  implementation pass did not have available to it. Recorded here rather than silently
  claimed, per this repo's own `docs/clients/matrix.md` convention: "not run" is an honest
  answer, a silent claim of "verified" when it wasn't is not. A future contributor with an
  interactive Claude Code session available can close this gap by: installing the hook,
  starting a real session, and confirming via `list_jobs`/`stats` that exactly one
  `session: <branch>` job appears, completed, with no prose narration in the reply.

## Known gaps

- **The model may not follow the injected instruction at all.** This is not just a
  network-failure edge case — a distracted or differently-tuned model could narrate it,
  skip it, or perform only part of the sequence. Nothing in this design mechanically
  catches or corrects that.
- **A stray `in-progress` marker job** (from `add_job`/`claim_jobs` succeeding but
  `complete_job` never being reached) is left claimed rather than completed. It is easy for
  a human to spot (title prefix `session:`, `metadata.kind: "session-marker"`) and resolve
  by hand (`complete_job` or `fail_job`).
- **Only the `origin` remote is read.** A repo using a different remote name, or a monorepo
  with multiple remotes, is out of scope for this reference implementation.
- **Every genuinely new marker job costs three billable otto-factory calls**
  (`add_job` + `claim_jobs` + `complete_job`); a same-day replay on the same branch costs
  one (`add_job` alone, short-circuited at step 3's `completed` branch). This is an ongoing
  cost every session start incurs once installed, not a one-time setup cost.
