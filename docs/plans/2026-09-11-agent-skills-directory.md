# Agent skills directory — ship a community-maintained client-skills/ directory

## Goal

Add a `client-skills/` directory to the otto-factory repo: a behavior contract and
contribution checklist, one fully working Claude Code reference template (a `SessionStart`
hook that resolves the repo and registers/closes a session-marker job), and five honest
stub READMEs (Copilot CLI, Cursor, Codex CLI, Otto CLI, generic) inviting community
contributions against the same checklist. Link it from the repo's root `README.md`. No
change to any Rust crate or to `web/`.

## Status — 2026-09-11

Implemented and shipped on `docs/client-skills-directory` (PR #181, closes
savvagent/otto-factory#180). Tasks 1-5 all ran; every checkbox below reflects what actually
happened, not what was planned before implementation. The one deliberately-deferred item is
Task 3's live-Claude-Code-session verification step (the "watch a real session actually
follow the injected instruction end to end" check) — not run in this pass, disclosed
honestly in `client-skills/claude-code/README.md`'s "What was verified, and how" section
rather than silently checked off. Everything else in Task 3's manual-verification list did
run, including the character-restriction confirmation and the empirical heredoc-injection
check the review trio's follow-up asked for.

A follow-up review round (rust-pro, architect-reviewer, security-auditor, plus
`pr-review-toolkit:comment-analyzer`) found the initial implementation's compliance-based
instruction text vulnerable to prompt injection via the interpolated remote/branch values,
plus a real `add_job`→`claim_jobs` race this repo's own `otto-factory-worker` could act in.
Both were addressed in a review-response pass: a strict character allowlist on the remote
and branch (reject and exit silently, same as "not a git repo"), the injected instruction
restructured so those values are presented as explicitly labeled, fenced data rather than
spliced into the imperative steps, the job's `title` changed from `"session: <branch>"` to a
fixed literal `"session marker"`, an `agentType: "session-marker"` tag added as a partial
(not complete) mitigation for the claim race, the settings snippet's default install path
changed from a `$CLAUDE_PROJECT_DIR`-relative command to a user-level absolute path, and the
"never narrate success or failure" instruction relaxed to "keep it to one short line, don't
suppress a genuine failure." See the spec's Architecture and Risks sections for the full
correction record, including two developer sign-off questions the spec's Risks section
raises that remain genuinely open (the compliance-based mechanism itself, and whether
`add_job`→`claim_jobs`→`complete_job` is the right primitive at all versus a lease or a
`send_message` announcement) — addressing the security and architecture findings did not
resolve those, and this plan does not claim otherwise.

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
  quietly from the developer's point of view** — no retry, no more than a one-line
  acknowledgement in the reply, on `resolve_repo` failing (unregistered repo), no MCP connection
  configured, or any other tool-call error. (Revised during review-response: the original
  constraint said "no narration ... no mention of any failure either," which the
  security-auditor correctly flagged as instructing the model to suppress the one signal that
  would let a developer notice something went wrong — see spec Risks.) This is compliance-based,
  not a script-enforced exit code — the instruction text must say this explicitly, not just
  document it here.
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
| `client-skills/generic/README.md` | **Create.** Added during review-response — explains why `generic` (any MCP client) names no single automation surface to target. |
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

## Task 1 — Open the tracking GitHub issue ✅

**Files:** none (GitHub only)
**Interfaces:** Produces the issue number every later task and the PR body reference.

- [x] `gh issue create --repo savvagent/otto-factory --title "Add a community-maintained client-skills/ directory" --body "$(cat <<'EOF'
Ship a client-skills/ directory: per-coding-agent templates that register a session-start
marker job on otto-factory, maintained by the community rather than the server (constraint
3 — coding-agent agnostic). See docs/specs/2026-09-11-agent-skills-directory-design.md for
the full design.

Scope: client-skills/README.md (contract + contribution checklist), a working Claude Code
reference template, and stub READMEs for Copilot CLI, Cursor, Codex CLI, and Otto CLI. No
server-side change.
EOF
)"` — capture the returned issue number as `<n>` for every later reference. Ran as
      `savvagent/otto-factory#180`.
- [x] Confirm `gh issue view <n> --repo savvagent/otto-factory --json number,url` returns the
      expected issue before proceeding — a malformed `gh issue create` call can silently open
      the issue with truncated body content if the heredoc quoting is wrong. Confirmed: issue
      #180 is open with the expected title and URL.

## Task 2 — Write `client-skills/README.md`: the contract and contribution checklist ✅

**Files:** `client-skills/README.md` (create)
**Interfaces:** Produces the behavior contract every subdirectory in this task and Tasks 3-4
must satisfy. Consumes nothing.

- [x] Create `client-skills/` and write `client-skills/README.md` covering, per the spec's
      Goal/Architecture/Assumptions sections (a review-response pass later added a "Launch
      scope" section — fixing the stub READMEs' dangling cross-reference — a sixth behavior-
      contract point on treating interpolated local values as untrusted, and a matching
      contribution-checklist box; see the plan's Status block):
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

## Task 3 — The Claude Code reference template ✅

**Files:** `client-skills/claude-code/README.md`, `client-skills/claude-code/session-start-hook.sh`,
`client-skills/claude-code/settings-snippet.json` (all create)
**Interfaces:** Consumes `client-skills/README.md`'s contract (Task 2). The model, following the
script's injected instruction, calls otto-factory's `resolve_repo`, `add_job`, `claim_jobs`,
`complete_job` MCP tools at runtime — not the script itself, and not at build time; nothing here
is exercised by this repo's own test suite.

**Revised during review-response** (see plan Status): the mandatory review trio found the first
implementation's instruction text vulnerable to prompt injection via the interpolated remote and
branch values, and a real `add_job`→`claim_jobs` claim race this repo's own `otto-factory-worker`
could act in. The checklist below reflects what actually shipped after that pass, not the
original draft — see the spec's Architecture and Risks sections for the correction record.

- [x] Write `client-skills/claude-code/settings-snippet.json`: a `SessionStart` hook entry
      with matcher `startup` (not `resume` or `clear` — see spec Architecture step 1) pointing
      at `session-start-hook.sh`, in the shape Claude Code's current hooks documentation
      specifies for `.claude/settings.json`. Shipped with the `command` pointing at
      `"$HOME"/.claude/hooks/session-start-hook.sh` — a **user-level absolute path**, not the
      `$CLAUDE_PROJECT_DIR`-relative one the first draft used (the review-response pass caught
      that a user-level `settings.json` entry pointing into `$CLAUDE_PROJECT_DIR` means every
      repository opened runs whatever script exists at that path in *its own* `.claude/hooks/`,
      as the developer, before typing anything — see `claude-code/README.md`'s Install section
      for the corrected guidance and the project-level exception).
- [x] Confirmed against Claude Code's hooks documentation how a `SessionStart` hook injects text
      into the model's own next turn: `hookSpecificOutput.additionalContext`, matching the
      spec's working assumption — no rethink needed.
- [x] Write `client-skills/claude-code/session-start-hook.sh` implementing the spec's
      Architecture steps 2-4 (the script's own deterministic half) exactly:
  1. `git remote get-url origin`, `git rev-parse --abbrev-ref HEAD`, and `date -u +%Y-%m-%d`;
     exit `0` with no stdout if either git command fails.
  2. **Validate both captured values against a strict allowlist** before either is used
     anywhere: reject a remote outside `[A-Za-z0-9._:/@+~-]` or a branch outside
     `[A-Za-z0-9._/-]`, or either exceeding a fixed length cap, exiting `0` with no stdout the
     same as "not a git repo." Added during review-response in direct response to the
     security-auditor's Critical finding — not in the original task text, but load-bearing.
  3. On success, print one JSON object to stdout in the shape confirmed above, whose injected
     text carries the captured remote URL, branch, and date inside a clearly labeled, fenced
     *data* block — never spliced directly into the imperative instruction steps — with the
     instruction template itself built from a **quoted** heredoc delimiter so it undergoes no
     shell expansion at all, the two values substituted in afterwards via literal parameter
     substitution rather than by re-opening the heredoc to interpolation.
- [x] Draft the injected instruction text itself (embedded in the script's stdout, per the step
      above) so it tells the model, using its own already-authenticated otto-factory MCP tools,
      to perform exactly the spec's Architecture steps 5-9:
  1. Call `resolve_repo` with `remote` set to the "remote" value from the data block above; on
     failure, do nothing further and say nothing about it.
  2. On success, keep the resolved `slug`. Call `add_job` with `repo: <slug>`,
     `title: "session marker"` (a **fixed, literal** string — changed from `"session: <branch>"`
     during review-response, since `title` is the field most likely to be read as prose by
     another agent and the branch is attacker-influenceable), `agentType: "session-marker"`
     (added during review-response as a partial, constraint-2-compliant mitigation for the
     `add_job`→`claim_jobs` race — a hint only, never enforced), a **fixed, literal**
     `description` string, `metadata: {"kind": "session-marker", "source":
     "client-skills/claude-code", "branch": "<branch>"}`, and
     `idempotencyKey: "session-<slug>-<branch>-<yyyy-mm-dd>"` using the embedded date.
  3. Branch on the returned job's `status`: `completed` → stop; `pending` → `claim_jobs` with
     `jobs: [<id>]`, then (only on a successful claim) `complete_job` with
     `result: "session marker — no work performed"`; anything else → stop without calling
     `complete_job`.
  4. Keep any acknowledgement to at most one short line — **not** an unconditional vow of
     silence on success or failure as the original draft had it (the security-auditor's Medium
     finding: telling a model to suppress every failure signal converts a survivable problem
     into an unobserved one).
- [x] Write `client-skills/claude-code/README.md`: what this template does (the two-actor shape,
      why, and — added during review-response — a dedicated Security section on the allowlist
      and data-fencing), the corrected install steps (user-level default, project-level as a
      documented exception with an explicit `$CLAUDE_PROJECT_DIR` warning), the full tool-call
      sequence (satisfying Task 2's contribution checklist, with the corrected `title`/`agentType`
      fields), the known gaps from the spec's Error Handling section (the model may not follow
      the injected instruction at all; the `add_job`→`claim_jobs` window and the `agentType`
      tag's honest partial-mitigation status; a stray `in-progress` marker job needs manual
      `complete_job`/`fail_job`; `origin` is the only remote name read; `idempotencyKey`'s
      200-character cap; a rejected-by-allowlist remote/branch produces no marker at all), and
      the billable-cost note (three calls for a new marker, **zero** for a same-day replay —
      corrected from the original draft's "one," which was wrong: `add_job`'s replay path
      records via `Meter::record_replay`, always free).
- [x] **Manual verification**, recorded honestly in this checklist (check off only what was
      actually run, per this repo's own standing convention in `docs/clients/matrix.md`):
  - [x] Run `session-start-hook.sh` by hand against a fake `SessionStart` stdin payload inside
        a real git checkout with a registered otto-factory repo, and confirm it prints the
        expected `additionalContext` JSON with the remote URL, branch, and date correctly
        substituted — this part needs no live Claude Code session to check.
  - [x] Confirm the allowlist rejects a hostile value: run the script against a branch crafted
        to look like an instruction override (containing `$(...)`, quotes, and other
        prose-breaking punctuation) and confirm it exits `0` with empty stdout, the same as
        "not a git repo" — added during review-response, not in the original checklist.
  - [x] Confirm, empirically, whether the instruction template's original unquoted-heredoc
        construction was a *second*, independent command-injection vector on top of the
        prompt-injection finding (as `pr-review-toolkit:comment-analyzer` flagged, with its own
        explicit hedge that this needed verification rather than assertion): ran a branch
        literally named `$(touch /tmp/pwned-test-marker)` through an isolated reproduction of
        the heredoc pattern. **Result: it did not reproduce** — plain `${var}` substitution in
        an unquoted heredoc does not re-scan the variable's own value for further shell
        expansion (that second pass is what `eval` does, not parameter substitution); the marker
        file was never created. Fixed anyway on general principle (quoted heredoc delimiter,
        see above), and the allowlist forecloses the character class regardless.
  - [x] `cargo run -p of-server` against a local Postgres (`podman compose up -d`), with a
        test org and a registered test repo.
  - [ ] Install the hook, open a **real** Claude Code session in that repo, and confirm — by
        watching what the session actually does, not by reading the script — that the model
        follows through on the injected instruction: `list_jobs`/`stats` shows exactly one
        completed `"session marker"` job. **Deliberately deferred, not run in this pass** — no
        interactive Claude Code session was available to this implementation/review-response
        pass; disclosed in `client-skills/claude-code/README.md`'s "What was verified, and how"
        section rather than silently claimed. This is the one step that verifies compliance, not
        code, and a future contributor with an interactive session can close it.
  - [x] Restart the session (same branch, same day) and confirm no second job is created —
        verified via a direct `add_job` replay against a live server (see README), not a live
        Claude Code session; also confirmed the replay is recorded as a free call, not billable.
  - [x] Switch to a different branch, restart, and confirm a second, distinct job is created —
        verified by the branch-with-`/` case below, which is a distinct branch from the first.
  - [x] Confirm the character-restriction question for `idempotencyKey`: used a branch name
        containing `/` (`docs/client-skills-directory`) and confirmed `add_job` accepts the
        resulting key without error.
  - [ ] Confirm a repo that is *not* registered with otto-factory produces zero visible
        difference in the session. **Not separately re-run in this pass** — covered in substance
        by `resolve_repo`'s documented failure contract (step 5) and the allowlist/silent-exit
        behavior already verified above; left unchecked rather than claimed as its own
        live-session observation, consistent with the same honesty convention as the deferred
        live-session item above.

## Task 4 — Stub READMEs for Copilot CLI, Cursor, Codex CLI, Otto CLI, generic ✅

**Files:** `client-skills/copilot-cli/README.md`, `client-skills/cursor/README.md`,
`client-skills/codex/README.md`, `client-skills/otto-cli/README.md`,
`client-skills/generic/README.md` (all create — `generic` added during review-response, see
below)
**Interfaces:** Consumes `client-skills/README.md`'s contract and contribution checklist
(Task 2) and `claude-code/README.md` as a worked example (Task 3). Produces nothing any other
task depends on.

- [x] For each of the four named clients, write a `README.md` stating: the target client by
      name, a starting hypothesis for its own session-start-equivalent automation surface (e.g.
      a config-driven extension point, a plugin hook, or whatever that client's own
      documentation currently suggests — named as a hypothesis, not a claim this repo has
      verified), a pointer to `client-skills/README.md`'s contribution checklist, and a
      pointer to `client-skills/claude-code/` as the worked reference (noting explicitly that
      Claude Code's own two-actor script/model split is a workaround for a limitation that may
      not even apply to this client — a client whose automation surface can itself hold a
      credential should prefer a single deterministic script, and a contributor is free to
      propose a different mechanism entirely if neither shape fits). Do not write any script or
      config file for these four — no fabricated, unverified hook code (spec Assumptions:
      "Launch scope"). Each stub's "Launch scope" cross-reference points at
      `client-skills/README.md`'s own "Launch scope" section — added there during
      review-response after `pr-review-toolkit:comment-analyzer` found the original stubs
      pointed at a heading that didn't exist.
- [x] Cross-check each stub's client name against `web/src/lib/clients.ts::CLIENTS`'s existing
      `id`/`label` for that client (`copilot-cli` / "Copilot CLI", `cursor` / "Cursor",
      `codex` / "Codex CLI", `otto-cli` / "Otto CLI") so the subdirectory name and the
      console's own naming never drift apart.
- [x] **Added during review-response**: write `client-skills/generic/README.md`. The
      architect-reviewer found that `client-skills/`'s own stated rationale for matching
      subdirectory names to `ClientRecipe.id` ("a future console link can construct the URL from
      `id` alone with no separate mapping table") was already broken as shipped, since
      `web/src/lib/clients.ts::CLIENTS` has a sixth entry (`generic`, "any client nobody here has
      heard of") with no matching subdirectory. Unlike the other four, this stub explains that
      `generic` names no single client's automation surface to target, rather than offering a
      hypothesis — see spec Architecture.

## Task 5 — Root README pointer, and the final sanity-check gate ✅

**Files:** `README.md` (repo root, modify)
**Interfaces:** Consumes the finished `client-skills/` tree (Tasks 2-4). Produces nothing
further.

- [x] In the repo root `README.md`, add a short pointer in the getting-started flow (near the
      existing `claude mcp add --transport http factory ...` snippet) — one or two sentences
      plus a link to `client-skills/README.md`. Landed as: "`client-skills/` for per-client
      templates, maintained by the community rather than the server."
- [x] Ran the repo's standard gates as a sanity check that nothing else was disturbed:
      `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`,
      and `cd web && npm run check && npm run lint && npm test && npm run build`. All passed
      with zero diffs outside this change, both at initial implementation and again after the
      review-response pass's edits (still no Rust or TypeScript source touched — every changed
      file in this PR is markdown, shell, or JSON).
- [x] Format and commit: no Rust/TS formatter applies to the new files (all markdown, shell,
      and JSON). `shellcheck` was not available in the implementation environment — noted
      honestly rather than silently skipped; this repo has no CI job for shell scripts, so this
      remains a quality bar, not a gate, and is a fair follow-up for whoever has `shellcheck`
      installed. Commits reference the Task 1 issue as `Closes savvagent/otto-factory#180`.
