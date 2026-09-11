# Agent skills directory — ship a community-maintained client-skills/ directory

## Goal

Add a `client-skills/` directory to the otto-factory repo: a behavior contract and
contribution checklist, one fully working Claude Code reference template (a `SessionStart`
hook that resolves the repo and registers/closes a session-marker job), and four honest
stub READMEs (Copilot CLI, Cursor, Codex CLI, Otto CLI) inviting community contributions
against the same checklist. Link it from the repo's root `README.md`. No change to any Rust
crate or to `web/`.

## Status — 2026-09-11

Not started. This plan has not yet been reviewed or approved for implementation.

**Spec:** `docs/specs/2026-09-11-agent-skills-directory-design.md` — read it first. This plan
implements it exactly.

## Global Constraints

- **No server-side change of any kind.** Nothing in `crates/*` moves. Constraint 3
  (coding-agent agnostic) requires the server stay ignorant of whether, or how, any client
  automates queue registration.
- **No AI self-attribution anywhere** — not in commit messages, the PR body, code comments,
  or any file this plan creates.
- **Every marker job's repo identifier is the resolved `slug`, used verbatim in both
  `add_job`'s `repo` argument and the `idempotencyKey` string** — per the spec's Architecture
  section, these must never be derived two different ways.
- **`complete_job` is only ever called immediately after a successful `claim_jobs` on that
  exact job id** — never after a bare `add_job`. This is the correctness fix the spec's own
  review history exists to explain; do not regress to the simpler-looking two-call version.
- **The hook script itself makes no otto-factory MCP calls at all** — it only reads local git
  state and emits an `additionalContext` instruction (see spec Assumptions on why). Its own
  failure mode is limited to local detection: exit `0` with no stdout on anything but a clean
  git remote + branch (not a repo, no `origin`, detached HEAD).
- **Every otto-factory MCP call the *model* makes, following that injected instruction, fails
  silently from the developer's point of view** — no narration in the reply, no retry, no
  surfacing to the developer, on `resolve_repo` failing (unregistered repo), no MCP connection
  configured, or any other tool-call error. This is compliance-based, not a script-enforced exit
  code (see spec Risks) — the instruction text must say this explicitly, not just document it
  here.
- **N/A for this change, stated explicitly rather than silently skipped:** cross-org negative
  tests (no tenant table touched), `of-billing::classify` (no MCP tool added), breaking-change
  flagging (no public interface changed — `client-skills/` is new, additive, and not consumed
  by any existing interface). This is not the same as "no gate applies" — see the next point.
- **The repo's existing Rust and `web/` gates are not N/A; they're deferred to one end-of-plan
  run instead of gated per task**, since no task in this plan touches a Rust or TypeScript
  source file for them to check incrementally. `cargo test`, `cargo clippy --all-targets -- -D
  warnings`, `cargo fmt --all --check`, and the `web/` gate set (`npm run check && npm run lint
  && npm test && npm run build`) are all expected to pass unchanged, and Task 5 is where they
  actually run — see that task, not a substitute for it.
- **PR title:** `docs(client-skills): add a community-maintained per-client skills directory`
  — `docs` is the closest existing scope; noted in the spec's Risks as an imperfect but
  accepted fit.
- **The PR must reference a GitHub issue** (the developer's own standing instruction, separate
  from this repo's own ticketless-is-legitimate convention) — Task 1 opens one.

## File Structure

| File | Responsibility |
|---|---|
| `client-skills/README.md` | **Create.** Directory purpose, behavior contract, contribution checklist. |
| `client-skills/claude-code/README.md` | **Create.** What the template does, install steps, exact tool-call sequence, known gaps. |
| `client-skills/claude-code/session-start-hook.sh` | **Create.** The hook script. |
| `client-skills/claude-code/settings-snippet.json` | **Create.** The `.claude/settings.json` `SessionStart` entry to add. |
| `client-skills/copilot-cli/README.md` | **Create.** Stub: target client, automation-surface hypothesis, contribution checklist pointer. |
| `client-skills/cursor/README.md` | **Create.** Same shape. |
| `client-skills/codex/README.md` | **Create.** Same shape. |
| `client-skills/otto-cli/README.md` | **Create.** Same shape. |
| `README.md` (repo root) | **Modify.** Add a short "Client skills" pointer to the getting-started flow. |

## Task Order & Rationale

1. **Open the tracking issue first** — every later commit's PR needs one to reference, and
   filing it first means the issue number can be quoted directly in this plan's own commit
   messages and in the final PR body without a forward reference.
2. **`client-skills/README.md` before any per-client subdirectory** — it defines the behavior
   contract and contribution checklist every subdirectory (including the reference
   implementation) is written to satisfy; writing it first means the reference implementation
   is visibly conforming to a rule that already exists, not the other way around.
3. **The Claude Code reference implementation** — the one piece that must actually work
   end-to-end against a running server, so it runs before the stubs, which have no
   correctness to verify.
4. **The four stub READMEs** — mechanical once the contract (task 2) and one worked example
   (task 3) both exist to point at.
5. **The root `README.md` pointer, plus the final sanity-check gate run** — last, once there
   is something real to link to.

## Task 1 — Open the tracking GitHub issue ⬜

**Files:** none (GitHub only)
**Interfaces:** Produces the issue number every later task and the PR body reference.

- [ ] `gh issue create --repo savvagent/otto-factory --title "Add a community-maintained client-skills/ directory" --body "$(cat <<'EOF'
Ship a client-skills/ directory: per-coding-agent templates that register a session-start
marker job on otto-factory, maintained by the community rather than the server (constraint
3 — coding-agent agnostic). See docs/specs/2026-09-11-agent-skills-directory-design.md for
the full design.

Scope: client-skills/README.md (contract + contribution checklist), a working Claude Code
reference template, and stub READMEs for Copilot CLI, Cursor, Codex CLI, and Otto CLI. No
server-side change.
EOF
)"` — capture the returned issue number as `<n>` for every later reference.
- [ ] Confirm `gh issue view <n> --repo savvagent/otto-factory --json number,url` returns the
      expected issue before proceeding — a malformed `gh issue create` call can silently open
      the issue with truncated body content if the heredoc quoting is wrong.

## Task 2 — Write `client-skills/README.md`: the contract and contribution checklist ⬜

**Files:** `client-skills/README.md` (create)
**Interfaces:** Produces the behavior contract every subdirectory in this task and Tasks 3-4
must satisfy. Consumes nothing.

- [ ] Create `client-skills/` and write `client-skills/README.md` covering, per the spec's
      Goal/Architecture/Assumptions sections:
  - **Purpose**: why this directory exists (constraint 3 — the server can't depend on any
    client's hook system, so this automation lives here, community-maintained, instead of in
    otto-factory itself).
  - **The behavior contract** every template must follow (quote the spec's Architecture
    job-lifecycle shape in prose): resolve the repo via `resolve_repo`, exit/stop silently on
    any failure at any step, use the resolved canonical `slug` (never a second, independently
    derived repo string) for both the job's repo argument and its idempotency key, and close
    the marker job out (`claim_jobs` on the exact returned id, then `complete_job`) rather than
    leaving it claimable in `ready` — note for a contributor whose client *can* reach a valid
    otto-factory credential directly: this whole sequence may run in one deterministic script,
    same as the contract describes; only Claude Code's own template needs the two-actor
    script-plus-model-instruction split documented in `claude-code/README.md`, because only it
    lacks a way for the hook process itself to reach a credential (see that README once Task 3
    lands, and propose a different mechanism entirely if neither shape fits a given client).
  - **What this directory is not**: it doesn't configure OAuth/PAT auth (that's
    `docs/clients/matrix.md` / the console connect page's job), it doesn't claim or work real
    jobs, and it adds no otto-factory MCP tool.
  - **Contribution checklist** for a new (or the existing) client template, as a literal
    checklist a PR description can paste and tick:
    - [ ] Names the target client and the minimum version tested against.
    - [ ] Names the exact session-start (or equivalent) mechanism used, and links to that
          client's own documentation for it.
    - [ ] Lists, in order, every otto-factory MCP tool called and with what arguments.
    - [ ] States plainly whether this was verified against a real running `of-server`, or is
          an unverified starting hypothesis (see `docs/clients/matrix.md`'s own register for
          this — "not run" is an acceptable, honest answer, a silent claim of "verified" when
          it wasn't is not).
    - [ ] Follows the silent-failure contract above — no blocking, no error surfaced to the
          developer's normal session on any otto-factory-side failure.
    - [ ] Does not attempt to auto-register an unregistered repo.
    - [ ] Does not call `claim_jobs` against anything from the general `ready` pool — only
          against a job id this same template just created.
  - A short "see `claude-code/` for a worked example" pointer (added once Task 3 exists —
    fine to write this sentence now and have it become true by the end of Task 3, since both
    land in the same PR; if Task 3 hits its abort branch below, this forward reference will
    point at a file that was never created — the human picking this up should either finish
    Task 3 under a different mechanism or remove/reword this sentence before proceeding, since
    nothing here is committed yet at that point).

## Task 3 — The Claude Code reference template ⬜

**Files:** `client-skills/claude-code/README.md`, `client-skills/claude-code/session-start-hook.sh`,
`client-skills/claude-code/settings-snippet.json` (all create)
**Interfaces:** Consumes `client-skills/README.md`'s contract (Task 2). The model, following the
script's injected instruction, calls otto-factory's `resolve_repo`, `add_job`, `claim_jobs`,
`complete_job` MCP tools at runtime — not the script itself, and not at build time; nothing here
is exercised by this repo's own test suite.

- [ ] Write `client-skills/claude-code/settings-snippet.json`: a `SessionStart` hook entry
      with matcher `startup` (not `resume` or `clear` — see spec Architecture step 1) pointing
      at `session-start-hook.sh`, in the shape Claude Code's current hooks documentation
      specifies for `.claude/settings.json`. **Verify this shape against Claude Code's current
      hooks documentation before writing it** — the spec's own Risks section flags that the
      exact schema must be confirmed at implementation time, not assumed from the spec's
      prose.
- [ ] **Before writing any script code**, confirm against Claude Code's current hooks
      documentation exactly how a `SessionStart` hook injects text into the model's own next
      turn (the spec's working assumption is `hookSpecificOutput.additionalContext` — confirm
      the field name and the exact JSON envelope the hook process must print to stdout for
      Claude Code to pick it up). This is load-bearing: the script cannot call any otto-factory
      tool itself (it has no access to the session's authenticated MCP connection — spec
      Assumptions), so this injection mechanism is the *only* bridge between the script and the
      otto-factory calls below. **If no such mechanism exists in the currently-installed Claude
      Code version** (or its documented shape differs enough that this whole design needs
      rethinking, not just a field-name substitution): do not write `session-start-hook.sh` or
      `settings-snippet.json` at all. Instead, comment on the Task 1 tracking issue with exactly
      what was found (the Claude Code version checked, what the hooks documentation actually
      says, and why the `additionalContext`-based design doesn't fit it), leave Task 3's
      checklist items unchecked, and stop this plan's execution before Task 4 — Tasks 1 (the
      issue) and 2 (`client-skills/README.md`, the contract and contribution checklist, which
      does not depend on any single client's mechanism working) stand on their own and need no
      reverting; nothing has been committed yet at this point in the plan (Task 5 is the only
      commit step), so the working tree can simply be left with Task 2's file in place pending a
      human decision on how to proceed.
- [ ] Write `client-skills/claude-code/session-start-hook.sh` implementing the spec's
      Architecture steps 2-3 (the script's own deterministic half) exactly:
  1. `git remote get-url origin`, `git rev-parse --abbrev-ref HEAD`, and `date -u +%Y-%m-%d`;
     exit `0` with no stdout if either git command fails.
  2. On success, print one JSON object to stdout in the shape confirmed above, whose injected
     text is the full instruction block from step below — with the captured remote URL,
     branch, and date substituted in literally, never left for the model to compute.
- [ ] Draft the injected instruction text itself (embedded in the script's stdout, per the step
      above) so it tells the model, using its own already-authenticated otto-factory MCP tools
      and without narrating any of this in its reply — success or failure, no exceptions; see
      spec Architecture step 8 for why an unconditional rule beats one that asks the model to
      judge what's "worth" mentioning — to perform exactly the spec's Architecture steps 4-8:
  1. Call `resolve_repo` with `remote` set to the embedded URL; on failure, do nothing further
     and say nothing about it.
  2. On success, keep the resolved `slug`. Call `add_job` with `repo: <slug>`,
     `title: "session: <branch>"`, a **fixed, literal** `description` string (no timestamp, no
     per-call variation — `add_job` errors if a replayed idempotency key's other arguments
     differ, so this exact wording must be reproduced verbatim by every future session start on
     the same branch/day), `metadata: {"kind": "session-marker", "source":
     "client-skills/claude-code", "branch": "<branch>"}`, and
     `idempotencyKey: "session-<slug>-<branch>-<yyyy-mm-dd>"` using the embedded date.
  3. Branch on the returned job's `status`: `completed` → stop; `pending` → `claim_jobs` with
     `jobs: [<id>]`, then (only on a successful claim) `complete_job` with
     `result: "session marker — no work performed"`; anything else → stop without calling
     `complete_job`.
- [ ] Write `client-skills/claude-code/README.md`: what this template does (explicitly
      including the two-actor shape — a deterministic script handing an instruction to the
      model, not the script calling otto-factory itself, and why), the exact install steps
      (where the settings snippet goes — project-level `.claude/settings.json` or user-level,
      and how to point it at `session-start-hook.sh`'s actual path), the full tool-call
      sequence (satisfying Task 2's contribution checklist for this template itself), the known
      gaps from the spec's Error Handling section (the model may not follow the injected
      instruction at all — this is not just a network-failure edge case; a stray `in-progress`
      marker job from a partial follow-through needs manual `complete_job`/`fail_job`; `origin`
      is the only remote name read; branch names containing `/` were not specifically verified
      against `idempotencyKey`'s character rules until the manual verification step below
      confirms it), and the billable-cost note (three calls for a new marker, one for a
      same-day replay on the same branch).
- [ ] **Manual verification**, recorded honestly in this checklist (check off only what was
      actually run, per this repo's own standing convention in `docs/clients/matrix.md`):
  - [ ] Run `session-start-hook.sh` by hand against a fake `SessionStart` stdin payload inside
        a real git checkout with a registered otto-factory repo, and confirm it prints the
        expected `additionalContext` JSON with the remote URL, branch, and date correctly
        substituted — this part needs no live Claude Code session to check.
  - [ ] `cargo run -p of-server` against a local Postgres (`podman compose up -d`), with a
        test org and a registered test repo.
  - [ ] Install the hook (project-level `.claude/settings.json` pointing at the script), open a
        **real** Claude Code session in that repo, and confirm — by watching what the session
        actually does, not by reading the script — that the model follows through on the
        injected instruction: `list_jobs`/`stats` (call these MCP tools directly, or via the
        console) shows exactly one completed `session: <branch>` job. This is the one step in
        this whole template that verifies compliance, not code, and cannot be skipped or
        approximated by re-reading the instruction text.
  - [ ] Restart the session (same branch, same day) and confirm no second job is created.
  - [ ] Switch to a different branch, restart, and confirm a second, distinct job is created.
  - [ ] Confirm the character-restriction question for `idempotencyKey`: use a branch name
        containing `/` and confirm `add_job` accepts the resulting key without error, updating
        the README's "known gaps" note to state the confirmed answer rather than leaving it
        open.
  - [ ] Confirm a repo that is *not* registered with otto-factory produces zero visible
        difference in the session (silent-failure contract).

## Task 4 — Stub READMEs for Copilot CLI, Cursor, Codex CLI, Otto CLI ⬜

**Files:** `client-skills/copilot-cli/README.md`, `client-skills/cursor/README.md`,
`client-skills/codex/README.md`, `client-skills/otto-cli/README.md` (all create)
**Interfaces:** Consumes `client-skills/README.md`'s contract and contribution checklist
(Task 2) and `claude-code/README.md` as a worked example (Task 3). Produces nothing any other
task depends on.

- [ ] For each of the four clients, write a `README.md` stating: the target client by name,
      a starting hypothesis for its own session-start-equivalent automation surface (e.g. a
      config-driven extension point, a plugin hook, or whatever that client's own
      documentation currently suggests — named as a hypothesis, not a claim this repo has
      verified), a pointer to `client-skills/README.md`'s contribution checklist, and a
      pointer to `client-skills/claude-code/` as the worked reference (noting explicitly that
      Claude Code's own two-actor script/model split is a workaround for a limitation that may
      not even apply to this client — a client whose automation surface can itself hold a
      credential should prefer a single deterministic script, and a contributor is free to
      propose a different mechanism entirely if neither shape fits). Do not write any script or
      config file for these four — no fabricated, unverified hook code (spec Assumptions:
      "Launch scope").
- [ ] Cross-check each stub's client name against `web/src/lib/clients.ts::CLIENTS`'s existing
      `id`/`label` for that client (`copilot-cli` / "Copilot CLI", `cursor` / "Cursor",
      `codex` / "Codex CLI", `otto-cli` / "Otto CLI") so the subdirectory name and the
      console's own naming never drift apart.

## Task 5 — Root README pointer, and the final sanity-check gate ⬜

**Files:** `README.md` (repo root, modify)
**Interfaces:** Consumes the finished `client-skills/` tree (Tasks 2-4). Produces nothing
further.

- [ ] In the repo root `README.md`, add a short pointer in the getting-started flow (near the
      existing `claude mcp add --transport http factory ...` snippet) — one or two sentences
      plus a link to `client-skills/README.md`, e.g. "Want your agent to register its own
      sessions automatically? See `client-skills/` for per-client templates."
- [ ] Run the repo's standard gates as a sanity check that nothing else was disturbed:
      `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`,
      and `cd web && npm run check && npm run lint && npm test && npm run build`. All are
      expected to pass with zero diffs outside this change — this task adds no Rust or
      TypeScript source.
- [ ] Format and commit: no Rust/TS formatter applies to the new files (all markdown, shell,
      and JSON); run `shellcheck client-skills/claude-code/session-start-hook.sh` if
      `shellcheck` is available locally and address anything it flags (best-effort — this repo
      has no CI job for shell scripts, so this is a quality bar, not a gate). Commit with
      message `docs(client-skills): add a community-maintained per-client skills directory`,
      referencing the Task 1 issue number as `Closes savvagent/otto-factory#<n>` — this PR is
      the entire scope of that issue (Task 1 opened it for exactly this change, nothing else
      depends on it remaining open), so `Closes` is the correct default; use `Refs` only if,
      by the time this task runs, scope was deliberately split and part of the issue's ask is
      being left for a follow-up PR.
