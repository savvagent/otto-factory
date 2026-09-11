# Agent skills directory design

## Brief

> Design and plan a `skills/` directory for the otto-factory repo: a community-maintained
> collection of per-coding-agent skill/hook templates that register a job in the otto-factory
> queue when a developer starts working on a repo (e.g., resolve_repo + add_job on session
> start). This must respect constraint 3 (coding-agent agnostic) — the server itself must not
> depend on or special-case any client; the directory is an optional, symmetric convenience for
> customers, distributed from this open-source repo so the community can help maintain
> per-agent templates as each agent's hook/skill format evolves. Also decide: contribution
> bar/checklist for a new agent template (what it must declare: target client, session-start
> mechanism, which MCP tools it calls), where it's linked from (README getting-started section,
> and possibly web/src/lib/clients.ts's per-client table in the console), and what ships at
> launch (e.g., a working Claude Code template as the reference implementation) vs. left as
> stubs/contribution invitations for other clients (Copilot CLI, Cursor, Codex).

Preceding conversation (not itself the brief, but the reasoning that produced it): the
developer wants every coding-agent session that starts work in a repo to become visible in
otto-factory automatically, rather than relying on the developer or their agent to remember to
call `add_job` by hand. Because the *server* cannot depend on any one client's hook system
(constraint 3), the automation has to live client-side, as a template the customer installs
into their own agent. Rather than the project maintaining N such templates itself, ship them
from an open-source directory so the community carries that maintenance burden per client.

## Assumptions

- **Output routing.** This repo's own convention (evidenced by every existing file in
  `docs/specs/` and `docs/plans/`) is a plain, git-tracked markdown file per change — not the
  JIRA/GitHub-issue-comment routing a generic planning pipeline defaults to. This spec and its
  plan are written directly to `docs/specs/2026-09-11-agent-skills-directory-design.md` and
  `docs/plans/2026-09-11-agent-skills-directory.md`, matching every prior spec/plan pair, and
  are left uncommitted in the working tree for the developer's review — consistent with how
  this repo's own `otto-factory-development` skill commits a spec/plan only once the developer
  (or, in its autonomous form, its own critique loop) has accepted it.
- **Directory name: `client-skills/`, not `skills/`.** This repo already has a `.github/skills/`
  directory (mirrored at `.claude/skills/` via symlink) holding otto-factory's own *development*
  skills — `otto-factory-development`, `otto-factory-scanner`, `otto-factory-worker` — which
  automate building otto-factory itself, not using it. A second, differently-purposed directory
  literally named `skills/` at the repo root would collide in name with that existing concept and
  mislead a reader into thinking it's more of the same. `client-skills/` reuses this codebase's
  existing "client" vocabulary (`web/src/lib/clients.ts`, `docs/clients/matrix.md`) for exactly
  the audience this directory serves: a customer's coding agent, not otto-factory's own
  contributors. Flagged in Risks as a judgment call the developer may want to override.
- **"Skill" is the right word for this whole distribution, not the right word inside every
  subdirectory.** Claude Code's own automatic-behavior mechanism for "run something on session
  start" is a **hook** (configured in `.claude/settings.json`), not a Skill/`SKILL.md` — those are
  invoked by name or by description match, never fired automatically on a lifecycle event. The
  Claude Code entry in `client-skills/` therefore ships a hook (a script plus a settings.json
  snippet), and each other client's entry uses whatever *that* client actually calls its own
  automation surface, documented in its own `README.md`. The directory's name and this spec's
  prose still say "skills" as the general product concept (something a customer drops in to
  teach their agent this behavior), matching how the developer described the idea.
- **The hook script cannot call otto-factory tools itself — it has no access to the session's
  authenticated MCP connection.** A `SessionStart` hook is a detached OS subprocess Claude Code
  launches and reads stdout from; it is not a party to the session's own MCP client, which holds
  whatever OAuth token or PAT the developer configured, and there is no supported way for an
  external process to borrow that connection. Two ways to bridge that gap were considered: (1)
  have the script speak MCP's Streamable HTTP JSON-RPC directly over `curl`, sourcing a bearer
  token from wherever Claude Code stores its own MCP client credentials — rejected, because that
  storage location is undocumented for external use, may live behind an OS keychain rather than a
  plain readable file, and building against it would be a fragile, unsupported integration into
  another program's private credential store, likely to break silently on a Claude Code update;
  or (2) have the script emit Claude Code's own supported `SessionStart` mechanism for injecting
  text into the model's next turn (`additionalContext`), so the *model* — which already has an
  authenticated otto-factory MCP connection through its own normal tool access — performs the
  actual tool calls, following precise, literal instructions the script hands it (remote URL,
  branch, and today's date all embedded, nothing for the model to compute or guess). This design
  adopts (2): it trades a shell script's guaranteed control flow for something that only relies on
  documented Claude Code hook behavior. See the new Architecture data flow below, and Risks for
  what this trade costs (compliance-based execution, not deterministic).
- **What "the otto-factory queue" means for a session-start marker: `add_job`, then `claim_jobs`
  on that exact job id, then `complete_job` — never a job left sitting `pending` in `ready`'s
  claimable list.** `complete_job`'s own contract is explicit that it "fails if you are not the
  job's current claim holder," and `add_job` alone never establishes one — a fresh job is
  `pending`, not claimed, so a two-call `add_job` → `complete_job` sequence (this spec's first
  draft) errors on every invocation and leaves the marker stuck `pending` and genuinely claimable
  by any real agent polling `ready`, which is exactly the orphaned/confusable-with-real-work state
  this bullet exists to avoid. The corrected three-call sequence narrows that gap: `claim_jobs`
  takes explicit job ids (`jobs: [<id>]`), so once the model reaches that step it claims precisely
  the job it just created — never anything from the general `ready` pool.

  **This does *not* mean the three calls happen with no gap another agent could act in — they do
  not run inside one synchronous script invocation at all.** An earlier draft of this bullet
  claimed that; it was true only of a script-driven design this spec no longer adopts. In the
  two-actor design actually shipped (see the mechanism-bridging bullet above), `add_job` and
  `claim_jobs` are two separate MCP round-trips the *model* issues on its own next turn, not one
  atomic step a script performs — so the marker job genuinely sits `pending`, visible in the
  general `ready` pool, for at least the gap between those two tool calls (plausibly a full model
  turn). This repo's own `.github/skills/otto-factory-worker` claims jobs out of `ready` with no
  `agentType` filter today and can win that race, claiming and dispatching real development work
  against what is meant to be an inert marker. The mitigation actually shipped is tagging the
  marker job `agentType: "session-marker"` (see Architecture) so a worker that filters its own
  queue reads by `agentType` excludes it — this is a real, constraint-2-compliant narrowing, not a
  claim that the window is closed: `agentType` is never enforced by the server, and a caller that
  queries `ready`/`list_jobs` with no `agentType` filter at all (as `otto-factory-worker` currently
  does) still sees and can still claim the job. Fully closing this would mean updating
  `otto-factory-worker` (or `of-mcp`) to filter by `agentType`, which is out of scope for this
  change. The net effect a reader of `list_jobs`/`stats` sees once the sequence completes is
  unchanged from the first draft: a completed marker, never present in `ready` after that point,
  needing no lease and no claim-TTL renewal loop (the default 900s TTL is vastly longer than the
  gap between claim and complete) and no session-end hook to close it out.
  This is still a deliberate, documented simplification of the developer's literal phrasing
  ("added ... in the ... queue"); the alternative of leaving it open until a session-end hook
  completes it was considered and rejected because most clients have no reliable session-end
  signal (a crashed or force-quit session leaves it stuck), and the coordination value — "who was
  recently active on this repo/branch" — is already fully served by a completed job's timestamp
  and metadata, which `list_jobs` can filter and sort on. Flagged in Risks for the developer to
  reconsider.
- **No lease and no renewal loop in any template, but `claim_jobs` is used.** The reference
  behavior calls `resolve_repo`, `add_job`, `claim_jobs`, and `complete_job` — never
  `acquire_lease` (a session-start marker isn't a resource another agent could collide on) and
  never `renew_claim` (claim and complete happen back-to-back in the same script run, nowhere
  near the 900s default TTL). This keeps every template implementable as a single one-shot hook
  invocation, with no background process and no keep-alive concern — the exact category of
  problem `otto-factory-worker`'s claim/lease renewal machinery exists to solve, which a template
  this simple has no need to reproduce.
- **Every marker job costs otto-factory's billable allowance, not just its `ready`-list
  visibility.** Per the otto-factory MCP server's own instructions, queueing, claiming, and
  completing are all billable (only reads, `watch`, and lease renewals are free). A genuinely new
  marker (new branch, or a new UTC day) costs three billable calls; a same-day replay on the same
  branch costs **zero** — confirmed against `crates/of-billing/src/meter.rs`: `add_job`'s replay
  path calls `Meter::record_replay`, which records the call as `Free` regardless of `add_job`'s own
  classification (`tx.record_usage(Some(user), tool, false)` — the third argument is `billable`),
  short-circuited at step 3's `completed` branch before any claim/complete — see Architecture.
  This is a real, ongoing cost every session start incurs once a developer installs this hook, not
  a one-time setup cost — worth the developer weighing explicitly, not just noting as a technical
  aside. Flagged again in Risks.
- **Idempotency, not a session-end hook, prevents queue spam.** A developer restarting their
  agent five times in one afternoon on the same branch would otherwise create five marker jobs.
  `add_job`'s `idempotencyKey` is set to a value derived from repo slug + branch name + UTC date
  (e.g. `session-<repo-slug>-<branch>-<yyyy-mm-dd>`), so repeated session starts on the same
  branch the same day collapse to one job; a genuinely new day, or a new branch, is a new marker.
  Branch name is read from `git rev-parse --abbrev-ref HEAD`; a detached HEAD (no branch) skips
  queuing entirely — there is nothing stable to key an idempotency key on, and generating a
  synthetic one from the commit SHA would create a fresh job on every commit rather than
  collapsing anything.
- **Failure is always silent to the developer's session, never blocking.** Every template's
  contract, stated in whichever of the two shapes actually carries out the otto-factory calls (a
  deterministic script for a client that can reach a credential itself; the model, on Claude
  Code, for one that can't — see the mechanism assumption above): if `resolve_repo` fails (repo
  not registered), if the MCP call errors, if the client has no network path to the otto-factory
  server, or if the client isn't even configured with an otto-factory MCP connection at all, the
  actor performing the call stops and produces no visible output (a script exits `0` and prints
  nothing, or — where the client's hook surface supports a non-fatal warning channel — one line
  to stderr; the model, on Claude Code, says nothing about it in its reply) — it must never block,
  delay, or fail the developer's actual session start. This mirrors the scanner/worker skills' own
  rule against auto-registering a repo on a resolution failure: a missing repo registration is
  reported nowhere by this hook at all (unlike the scanner/worker skills, there is no orchestrator
  here to report it *to* — nobody is watching a session-start hook's output the way
  `otto-factory-worker` watches a dispatched subagent's), which is an accepted, documented gap,
  not a silent contradiction of that rule.
- **Auth is out of scope — every template assumes the client already has a working otto-factory
  MCP connection.** These templates add automatic queue registration; they do not configure OAuth
  or a personal access token. `docs/clients/matrix.md` and `web/src/lib/clients.ts` already own
  that setup step for every client this directory targets.
- **Launch scope: one fully working reference template (Claude Code), stub invitations for the
  rest.** `.github/skills/otto-factory-worker/SKILL.md` and `otto-factory-scanner/SKILL.md` both
  describe themselves as "Claude-Code-native," and this codebase's own conformance work
  (`docs/clients/matrix.md`) has only ever driven Claude Code and Copilot CLI live. Shipping a
  fabricated, never-run template for Cursor, Codex CLI, Copilot CLI, or Otto CLI would read as
  working when nobody has verified it — worse than an honest "not yet, here's the shape a PR
  should take" stub. Copilot CLI, Cursor, Codex CLI, and Otto CLI each get a `README.md` stub
  (target client named, its likely automation surface named as a starting hypothesis, and the
  contribution checklist below) and no functioning hook code at launch. `generic` (the console's
  catch-all "any other MCP client" entry, see the subdirectory-naming bullet above) also gets a
  stub, but one that explains why it names no single automation surface to target rather than
  offering a hypothesis — see Architecture.
- **The console link (`web/src/lib/clients.ts`) is out of scope for this change.** Adding a
  pointer there is real, user-visible surface — it would need a new `note`/link field on
  `ClientRecipe`, a decision about whether that field is translated prose (the file's own "a new
  console string costs six catalog entries" rule) or a verbatim URL, and console tests. Bundling
  that into the same change as launching the directory itself risks the same "two logical
  changes in one PR" shape `otto-factory-development`'s own fast-path guidance already warns
  against elsewhere in this repo. This spec scopes the console link as a clearly-named follow-up
  (see Scope) rather than silently expanding this change to cover it.
- **No GitHub issue exists for this yet.** This spec originates from a direct conversation with
  the developer, not a filed issue. Per this repo's own tracker convention
  (`otto-factory-development`'s "GitHub Issues when an issue exists, ticketless otherwise") a
  ticketless PR is fully legitimate here — but the developer's own global instructions (separate
  from this repo) require every PR to reference an associated GitHub issue. The plan's first task
  therefore opens a tracking issue in `savvagent/otto-factory` before any code lands, so the
  eventual PR has one to reference; this is process, not a design decision, and is called out
  here only so the plan's Task 1 doesn't look unmotivated.
- **Per-template subdirectory naming matches `ClientRecipe.id`, exhaustively.** `client-skills/claude-code/`,
  `client-skills/copilot-cli/`, `client-skills/cursor/`, `client-skills/codex/`,
  `client-skills/otto-cli/`, and `client-skills/generic/` — reusing the exact `id` strings already
  assigned in `web/src/lib/clients.ts::CLIENTS` (all six entries, `generic` included — an earlier
  draft of this change shipped five of the six subdirectories and left `generic` out, which
  quietly broke this bullet's own stated rationale below for every reader who noticed the gap), so
  a future console link (the follow-up above) can construct
  the URL from `id` alone with no separate mapping table to keep in sync.

## Goal & Success Criteria

Ship a `client-skills/` directory that lets a customer's coding agent automatically register a
job on otto-factory when a developer starts working in a registered repo, distributed and
community-maintained from this open-source repo rather than built into the server — with one
fully working, documented reference implementation and a clear, checkable bar for contributing
the rest.

- `client-skills/README.md` explains the directory's purpose, the behavior contract every
  template must follow (repo resolution, idempotent marker job, silent-on-failure, no auth setup),
  and the contribution checklist for a new client template.
- `client-skills/claude-code/` contains a working `SessionStart` hook (script + a
  `.claude/settings.json` snippet to wire it in) that a developer can drop into any repo or their
  user-level settings, verified against a real `of-server` instance.
- `client-skills/{copilot-cli,cursor,codex,otto-cli}/README.md` each state the target client, a
  starting hypothesis for that client's own automation surface, and invite a contribution against
  the same checklist — no fabricated, unverified hook code. `client-skills/generic/README.md`
  completes the `ClientRecipe.id` set by explaining why that entry names no single automation
  surface to target.
- The repo's root `README.md` gains a short "Client skills" pointer in its getting-started flow,
  linking to `client-skills/README.md`.
- A tracking GitHub issue exists in `savvagent/otto-factory` before the PR opens, and the PR
  references it.
- `cargo test`, `cargo clippy`, `npm run check`, `npm run lint`, `npm test`, and `npm run build`
  all still pass unchanged — this feature touches no Rust or SvelteKit source at all.

## Scope

**In:**

- New top-level `client-skills/` directory: a root `README.md`, one fully working subdirectory
  for Claude Code, and five stub subdirectories (Copilot CLI, Cursor, Codex CLI, Otto CLI,
  generic).
- A short pointer from the repo's root `README.md`.
- A tracking GitHub issue.

**Out:**

- Any change to `crates/*` or `web/*` source. No new MCP tool, no server-side awareness of this
  directory at all — constraint 3 requires the server stay ignorant of which, if any, client-side
  automation exists.
- A link from `web/src/lib/clients.ts` / the console's connect page. Named explicitly as a
  follow-up (see Assumptions) rather than folded in here.
- Working, verified hook code for Copilot CLI, Cursor, Codex CLI, or Otto CLI. These ship as
  stubs only; a community PR (or a later otto-factory-development run) fills each in against the
  checklist this change establishes. (`generic` is a stub for a different reason — see
  Architecture — and is not expected to graduate the same way, since it names no single client to
  verify against.)
- Any CI enforcement of `client-skills/`'s contents (no lint job, no schema validator). The
  templates are prose-plus-scripts consumed by a human outside this repo's own build, not code
  this repo compiles or tests. Revisiting this is a fair follow-up once there's more than one
  working template to check consistency across, not a gap this change needs to close.
- A session-end hook, a lease, or any claim-renewal logic (see Assumptions — deliberately
  designed out of the reference behavior entirely).

## Architecture

```
client-skills/
  README.md                  — directory purpose, behavior contract, contribution checklist
  claude-code/
    README.md                — what this template does, install steps, what it calls
    session-start-hook.sh    — captures repo/branch/date, injects an instruction for the model to
                                run resolve_repo -> add_job -> claim_jobs -> complete_job itself
                                (the script cannot call these tools directly — see Assumptions)
    settings-snippet.json    — the `.claude/settings.json` SessionStart entry to add
  copilot-cli/
    README.md                — stub: target client, hypothesis, "contribute this" checklist
  cursor/
    README.md                — stub, same shape
  codex/
    README.md                — stub, same shape
  otto-cli/
    README.md                — stub, same shape
  generic/
    README.md                — stub: explains why `generic` (any MCP client) has no single
                                automation surface to target, points contributors at a
                                named-client subdirectory instead
```

**Claude Code reference template — data flow.** Two actors, because of the constraint above: the
hook script (deterministic, no otto-factory access) prepares facts and hands off an instruction;
the model (already otto-factory-authenticated, but only compliance-, not code-, driven) carries
it out.

*The script (`session-start-hook.sh`), synchronous, runs once per session start:*

1. Claude Code fires the `SessionStart` hook (matcher: `startup` only — a `resume` or `clear`
   restart of an existing session is not a new work session. **Corrected justification**: an
   earlier draft of this bullet argued firing on `resume` too "would defeat the idempotency key's
   once-per-branch-per-day intent" — that is not actually true, since a same-day resume would
   still collapse to the existing job via the idempotency key regardless of matcher. The real
   reason `startup`-only is correct is the first one: a resume is not a new work session, and a
   marker job exists to answer "when did a session start," not "how many times was this session
   resumed." This is a hook-level choice recorded in the settings snippet's matcher field, not a
   script-level check).
2. The script runs `git remote get-url origin`, `git rev-parse --abbrev-ref HEAD`, and
   `date -u +%Y-%m-%d` in the hook's working directory (Claude Code passes the project directory
   as `cwd`). Either git command failing (not a git repo, no `origin` remote, detached HEAD) exits
   `0` immediately with no stdout — nothing injected, nothing queued.
3. **Both captured values are validated against a strict allowlist before use.** The remote and
   branch are local git state, but attacker-influenceable — anyone who controls a remote you add,
   or a branch you fetch, controls these bytes, and step 4 below embeds them in text a model is
   told to treat as instructions. A remote containing anything outside
   `[A-Za-z0-9._:/@+~-]`, or a branch containing anything outside `[A-Za-z0-9._/-]`, or either
   exceeding a fixed length cap, is rejected the same way as "not a git repo": exit `0`, no stdout,
   nothing injected. This is not optional hardening; it is what keeps step 4's data block honestly
   labeled "safe to treat as inert data" rather than merely asserting it.
4. On success, the script prints one JSON object to stdout in whatever shape Claude Code's current
   `SessionStart` hook documentation specifies for injecting text into the model's own next turn
   (at the time of writing, `hookSpecificOutput.additionalContext` — **verify the exact field name
   and shape against current Claude Code hook documentation before implementing; do not assume
   this spec's description is still accurate**, per the standing Risk on hook-schema drift). The
   injected text carries the validated remote URL, branch, and date inside a clearly labeled,
   fenced *data* block — not spliced directly into the imperative instruction steps below — with
   the instruction referring back to that block by name rather than reading as if the model itself
   wrote the values into a sentence telling it what to do. The model is never asked to compute or
   guess any of these three values itself, only to follow the steps using them as data.

*The model, processing that injected context on its own next turn (before or alongside responding
to whatever the developer actually typed):*

5. Call `resolve_repo` with `remote` set to the "remote" value in the data block above. A
   resolution failure: do nothing further, and do not mention this to the developer — an
   unregistered repo is not this hook's problem to fix (see Assumptions: no auto-registration). On
   success, keep the resolved repo's canonical `slug` from the response — the one identifier used
   in every following call, never the raw remote URL and never re-derived a second way, so the repo
   argument on `add_job` and the slug embedded in the idempotency key can never disagree with each
   other.
6. Call `add_job` with:
   - `repo`: the resolved `slug` from step 5 — required; `add_job` has no session-level notion of
     "current repo" to fall back to, and a job with no resolvable repo is refused outright (this
     repo's own `repo_id NOT NULL` rule), so omitting this would make every marker-job creation
     fail before the claim/complete sequence below ever runs
   - `title`: a fixed, literal `"session marker"` — **not** `"session: <branch>"` as an earlier
     draft had it. The branch name is attacker-influenceable (see step 3's validation), and `title`
     is the field most likely to be read as prose by another agent browsing `ready`/`list_jobs`;
     keeping it fixed and literal means an attacker-chosen branch string never lands there. The
     branch still appears, but only inside `metadata` below, which tooling reads as structured data
     rather than narrated prose.
   - `description`: fixed, literal text from the injected instruction — the same wording on every
     call for the same idempotency key (see step 7; `add_job` errors if a replayed key's other
     arguments differ), so the instruction text itself must not embed a timestamp or any other
     value that would vary between today's earlier session starts
   - `agentType`: `"session-marker"` — a routing *hint*, never enforced by the server (any agent
     may still claim any job). An agent that filters its own `ready`/`claim_jobs` reads by its own
     `agentType` will not see this job; one that queries with no `agentType` filter at all still
     does. See the correction to the `add_job`→`claim_jobs` window above — this narrows that gap,
     it does not close it.
   - `metadata`: `{"kind": "session-marker", "source": "client-skills/claude-code", "branch":
     "<branch>"}` — an opaque, server-uninterpreted field per constraint 2, present so a customer's
     own tooling (or a future console filter) can distinguish marker jobs from real work without
     otto-factory itself needing to know the distinction exists
   - `idempotencyKey`: `"session-<repo-slug>-<branch>-<yyyy-mm-dd>"`, using the embedded date and
     the exact same step-5 `slug` used for the `repo` argument above

   `add_job` returns the created job on a first call, or — on a same-day replay for the same
   branch — "the original job unchanged," per its own documented idempotency contract. Either way
   the response carries that job's current `status`, which the next step branches on.
7. Branch on the returned job's `status`:
   - **`completed`:** stop here — today's marker for this branch already exists and is already
     closed out; do nothing further (no claim, no complete).
   - **`pending`** (the normal case for a job just created): call `claim_jobs` with
     `jobs: [<the returned job id>]`. This targets exactly the job just resolved — never anything
     else `ready` might be holding — so there is no risk of this accidentally claiming an
     unrelated real job. If the claim fails (for example, a concurrent duplicate hook invocation —
     two sessions starting in the same instant — claimed it first), stop here: the other
     invocation owns completing it.
   - **Anything else (in practice, `in-progress` or `active`** — the residue of a previous
     invocation that crashed after claiming but before completing): stop here rather than guessing.
     This session's own identity may or may not be the current claim holder, and attempting a
     `complete_job` that fails (wrong holder) or succeeds on a job it never actually did anything
     new for is worse than leaving it for a human to notice and resolve — see Error Handling.
8. On a successful claim, call `complete_job` on that job id with a fixed `result` string
   (`"session marker — no work performed"`).
9. Throughout steps 5-8: keep any acknowledgement in the reply to the developer to at most one
   short line — not an unconditional vow of silence on a genuine failure (an earlier draft of this
   instruction asked for exactly that, which a review round correctly flagged: telling a model to
   suppress every failure signal, including ones a developer would want to notice, converts a
   survivable problem into an unobserved one), and never let it delay or block addressing whatever
   the developer actually asked for in their first message. The tool calls themselves are not
   hidden — they appear in the session's normal tool-call transcript/UI regardless of what the
   reply says in prose. Unlike the script's own steps 1-4, nothing mechanically enforces steps 5-9
   to completion, and that is a deliberate, load-bearing trade-off (see Risks), not an oversight.

Every other client's `README.md` documents whichever of these two shapes actually fits that
client — a deterministic script if that client's automation surface can itself hold or reach a
valid credential, or an injected-instruction handoff to the model if (as here) it cannot — with a
"propose a different mechanism if neither shape fits this client" escape hatch. The five-step
job-lifecycle contract (resolve, create with a slug-derived idempotency key, branch on status,
claim, complete) is the strong default regardless of which shape carries it out.

## Error Handling & Edge Cases

- **Repo not a git repo, or no `origin` remote, or detached HEAD.** The script (Architecture
  step 2) exits `0` with no stdout — no `additionalContext` is ever injected, so the model never
  even sees an instruction to act on.
- **The model never sees, ignores, or only partially follows the injected instruction.** Unique to
  this mechanism (see the new Assumptions bullet on why the script can't call otto-factory
  itself): nothing mechanically guarantees the model performs steps 5-8. A distracted or
  differently-tuned model could narrate it, skip it, or perform only part of the sequence (e.g.
  `add_job` without following through to `claim_jobs`/`complete_job` — see the next bullet for why
  that specific partial failure is still safe). This is an accepted, load-bearing limitation of
  building on `additionalContext` rather than a script's own guaranteed control flow — flagged
  prominently in Risks, not something a future revision of the instruction wording can fully close.
- **Repo not registered with otto-factory.** `resolve_repo` fails inside the model's own tool call
  (step 5); per the injected instruction, it does nothing further and says nothing about it.
- **No otto-factory MCP connection configured, or the server unreachable.** The model's
  `resolve_repo`/`add_job`/etc. call itself errors; per the injected instruction, it does not
  narrate this failure to the developer or retry. A developer who never configured otto-factory
  should see no visible difference in their session (modulo the general compliance caveat above).
- **Detached HEAD / no branch.** Skip entirely at the script stage — nothing stable to key the
  idempotency key on, and no instruction is ever injected.
- **Repeated session starts, same branch, same day.** Collapses to one job via the idempotency
  key; the second and later `add_job` calls return the existing job, already `completed`, and step
  7's status check stops immediately without a second claim/complete.
- **Repeated session starts, same branch, next day.** A new marker job — this is intended: it is
  what makes "who was active on this repo recently, and when" a genuinely useful query over
  `list_jobs`, rather than one permanent marker that never updates.
- **Two sessions start in the same instant on the same branch (a genuine race, not a sequential
  replay).** Both get back the same job id from `add_job`; only one of them wins `claim_jobs`. The
  loser sees the claim fail, per Architecture step 7, and stops — the winner completes it. No
  duplicate job, no stuck claim.
- **`add_job`/`claim_jobs` succeed but `complete_job` fails or is never attempted** (a transient
  tool-call error, or the model simply not following through — see above). The job is left claimed
  (`in-progress`) rather than `pending` — not claimable by `ready`, but also never reaching
  `completed`, so a later session-start on the same branch the same day will see
  `status: in-progress` on its `add_job` replay, landing on Architecture step 7's third branch
  (anything but `pending` or `completed`), which — as that step already specifies — stops and does
  nothing further rather than guessing: this session's own identity may or may not be the current
  claim holder, and attempting a `complete_job` that fails (wrong holder) or succeeds on a job it
  never actually did anything new for is worse than leaving it for a human to notice and resolve.
  Documented explicitly in `client-skills/claude-code/README.md` as a known gap rather than
  silently risked; the practical mitigation is that a stray `in-progress` marker job is easy for a
  human to spot (title `"session marker"`, `metadata.kind: "session-marker"`) and resolve by hand
  (`complete_job`/`fail_job`). A more robust fix (e.g. a single combined server-side call) would
  be a server-side feature and is explicitly out of scope (see Scope) — constraint 3 already rules
  out adding anything to the server that only exists to make one client's convenience script
  simpler.
- **Branch names containing `/` (e.g. `feature/foo`), or other punctuation, land inside the
  `idempotencyKey` string verbatim.** Neither `add_job`'s schema nor this spec's reading of it
  documents a character restriction on `idempotencyKey`, so this is treated as unconstrained; the
  plan's implementation task should confirm this against a live server rather than assume it, and
  the template's `README.md` should say plainly that this is what was checked, not silently
  assumed.
- **A customer's repo uses a different remote name than `origin`, or a monorepo with multiple
  remotes.** Out of scope for the reference implementation, which reads `origin` only, matching
  the same assumption `otto-factory-worker`/`otto-factory-scanner`'s own `Step 1` already makes
  (`git remote get-url origin`). Documented as a known limitation in the template's `README.md`.

## Testing Approach

- **Claude Code template:** manual verification against a locally running `of-server`
  (`cargo run -p of-server`) with a registered test repo — install the hook, start a session,
  and observe both halves: that the script actually emits the `additionalContext` JSON (checkable
  directly by running the script by hand with a fake `SessionStart` stdin payload, no live session
  needed), and that a real Claude Code session, given that injected context, actually performs the
  full tool-call sequence and a completed `"session marker"` job appears via `list_jobs`/`stats` — this
  second half is exactly the compliance-based step the design can't mechanically guarantee (see
  Risks), so it is the one part of this template that has to be watched happen, not just read from
  code. Then restart the session on the same branch the same day and confirm no second job is
  created (idempotency), then switch branches and confirm a second, distinct job is created. This
  is a manual run recorded honestly in the plan, the same register `docs/clients/matrix.md`
  already uses for "not installed on the conformance machine" entries — there is no CI harness
  that can drive a real Claude Code `SessionStart` hook, and inventing one is disproportionate to
  a shell script this small.
- **Stub READMEs:** no functional testing — reviewed for accuracy of the target client's
  currently-known automation surface (best-effort, explicitly caveated as a starting hypothesis
  for a contributor to verify, not a claim this repo has verified it).
- **Repo-level gates unaffected:** `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --all --check`, and the `web/` gate set (`npm run check`, `npm run lint`, `npm test`,
  `npm run build`) all still pass because nothing in `crates/` or `web/` changes.
- **`pr-title` CI check:** the eventual PR title needs a `<type>(<scope>):` shape. This change has
  no single existing crate/area scope; `docs` is the closest fit (mirroring how doc-only PRs in
  this repo's own history — e.g. "docs: add otto-factory-scanner and otto-factory-worker skills"
  — are scoped), even though the content here is customer-facing rather than repo-development
  docs. Flagged as a minor naming tension in Risks, not a blocker.

## Risks & Open Questions

- **The reference implementation's core mechanism is compliance-based, not deterministic, and
  this is the single biggest change from this spec's earlier drafts.** A `SessionStart` hook
  cannot itself call an otto-factory MCP tool (it has no access to the session's authenticated
  connection — see Assumptions), so the script only prepares facts and injects an instruction; a
  model that ignores, narrates, or partially follows that instruction produces no marker job, or a
  stuck `in-progress` one, with nothing in this design that mechanically catches or corrects it.
  This is a materially weaker guarantee than "the script runs and does exactly what it says," and
  it is worth the developer's explicit sign-off before implementation proceeds — the alternative
  (the script speaking MCP directly, reading Claude Code's stored credentials itself) was rejected
  as fragile and unsupported, not because this compliance-based approach is obviously the better
  trade in every reader's judgment. **Status: this sign-off was not obtained before implementation
  — recorded here honestly rather than silently treated as settled.** The mandatory review trio's
  security and architecture findings on the resulting PR independently converged on the same
  compliance-based design being the source of both the prompt-injection surface (closed by
  validating and data-fencing every interpolated value — see the Claude Code template's own
  Security section) and the `add_job`→`claim_jobs` race (narrowed, not closed, by the `agentType`
  tag — see the correction above). Proceeding past review with those mitigations in place is a
  judgment call the review-response pass made in the developer's stead; it is not a substitute for
  the developer's own sign-off on the underlying compliance-based approach, which remains open.
- **Directory name (`client-skills/` vs. `skills/` vs. something else).** A defensible judgment
  call made to avoid colliding with `.github/skills/`'s existing meaning (see Assumptions) — the
  developer may prefer a different name; renaming before the plan is executed is cheap, renaming
  after a community has already forked/starred/linked to `client-skills/<id>/README.md` URLs is
  not.
- **`add_job` → `claim_jobs` → `complete_job` as the "queue" mechanism is a deliberate
  reinterpretation of "added ... in the queue."** The alternative — leave the job open, claimed
  for the session's duration, completed or cancelled on session end — was rejected because most
  clients have no reliable session-end signal and a crashed session would leave a permanently
  `in-progress` claim needing manual cleanup. If the developer's actual intent was closer to
  "show as *currently* working," not "show that work *started and immediately finished*," this
  design under-delivers and a different primitive (a lease, held for the session's duration, or a
  `send_message` announcement with no job at all) would fit better. **Status: also not confirmed
  before implementation, same as the bullet above — still open**, and worth confirming before this
  design is treated as final, since it changes the reference implementation materially.
- **Every session start costs the org's billable otto-factory allowance** (three calls for a
  genuinely new marker, **zero** for a same-day replay — `add_job`'s replay path is recorded via
  `Meter::record_replay`, always free regardless of `add_job`'s own classification; see
  Assumptions). A developer who starts many sessions a day across many repos accumulates this
  automatically, with no per-call visibility from inside the hook itself (failures are silent by
  design). Worth the developer weighing this trade explicitly rather than discovering it later in
  a usage report.
- **`ListJobsArgs`/`stats` have no way to exclude marker jobs from general view.** A session
  marker counts toward job-list and `stats` totals like any other job. The `agentType:
  "session-marker"` tag (see Architecture and the correction above) is the practical,
  constraint-2-compliant mitigation available today — a caller can filter it out of its own reads
  by that tag — but there is no server-side "hide markers" switch, and adding one would mean the
  server interpreting `metadata`/`agentType` semantics itself, which constraint 2 (substrate, not
  workflow) rules out. Not a full fix; worth a future console-side filter as a separate change.
- **No CI enforcement means directory drift is possible.** A community contribution could add a
  template that violates the behavior contract (blocks the session, auto-registers a repo, etc.)
  and nothing catches it mechanically — the `README.md` contribution checklist is the only guard,
  enforced at PR-review time by a human, same as any other open-source contribution to this repo.
  Accepted for launch; revisit once there is more than one community-contributed template to
  justify building a checker.
- **Claude Code's exact hook lifecycle-event names and `settings.json` shape should be verified
  against current Claude Code documentation at implementation time**, not assumed from this
  spec's description of `SessionStart` — hook surfaces change between Claude Code releases, and
  this spec's author's knowledge of the exact schema may already be stale by the time this plan
  executes. Flagged explicitly rather than treated as settled, matching the standing register this
  repo already uses for other clients in `docs/clients/matrix.md` ("confirmed against source, not
  a live run").
- **`docs` as the PR-title scope is a minor fit tension**, since this content is customer-facing,
  not repo-development docs. Not worth blocking on; noted for the implementer.
