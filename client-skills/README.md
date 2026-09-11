# client-skills/

Per-client templates that register a session-start marker job on otto-factory,
maintained by the community rather than by the server.

## Purpose

otto-factory's server is coding-agent agnostic by design: it never depends on, or
special-cases, any one client's hook, plugin, or skill system (see the repo's own
constraint 3 in the root `CLAUDE.md`). But developers still want every coding-agent
session that starts work in a repo to become visible in otto-factory automatically,
rather than relying on the developer (or their agent) to remember to call `add_job` by
hand.

Since that automation can never live in the server, it lives here instead: a directory
of small, per-client templates a customer installs into their own agent, distributed
from this open-source repo so the community — not the otto-factory maintainers — carries
the burden of keeping each template working as that agent's own hook/skill format
evolves.

## The behavior contract

Every template in this directory — whatever shape a given client's automation surface
actually takes — follows the same job lifecycle:

1. **Resolve the repo** via `resolve_repo` (by git remote URL, the normal case). On any
   failure — the repo isn't registered, there's no otto-factory MCP connection
   configured, the server is unreachable — stop immediately and produce no visible
   output. This directory never auto-registers a repo on a resolution failure.
2. **Use the resolved canonical `slug` for everything downstream.** The `slug` `resolve_repo`
   returns is the one identifier used in the job's `repo` argument and in the idempotency
   key below — never a second, independently derived string (e.g. the raw remote URL).
3. **Create an idempotent marker job.** Call `add_job` with an `idempotencyKey` derived
   from repo slug + branch + UTC date (`session-<slug>-<branch>-<yyyy-mm-dd>`), so
   repeated session starts on the same branch the same day collapse to one job rather
   than spamming the queue. A detached HEAD (no stable branch name) skips queuing
   entirely.
4. **Close the job out — never leave it claimable.** `add_job` alone never establishes a
   claim, so a marker left at `add_job` sits `pending` in the general `ready` list, which
   is exactly what "a marker job" must never look like — real work waiting to be picked
   up. Branch on the returned job's `status`: `completed` → nothing further to do;
   `pending` → `claim_jobs` on that exact returned job id, then (only on a successful
   claim) `complete_job`; anything else → stop without guessing.
5. **Fail silently, always.** Every failure at any step above — a resolution failure, an
   MCP call erroring, no otto-factory connection configured at all — must never block,
   delay, or surface an error in the developer's actual session. Where the client's own
   hook surface supports a non-fatal warning channel (e.g. stderr), one line is
   acceptable; nothing that interrupts or narrates over the developer's own work.
6. **Treat every local value you interpolate as untrusted.** A git remote URL, a branch
   name, a file path — anything read from the developer's own repository state and then
   embedded in text a model is told to treat as instructions — must be validated against a
   strict allowlist (safe characters, a length cap) before it is used anywhere, and
   presented to the model as clearly labeled, fenced *data* the instruction refers to by
   name, never interpolated directly into the imperative steps themselves. This matters even
   though the value is "local": a hostile remote or a hostile branch name is still something
   an attacker, not the developer, chose. See `claude-code/session-start-hook.sh` and its
   README's Security section for a worked example, including how to keep a shell script's own
   heredoc construction from re-interpolating a captured value.

## What this directory is not

- **Not an auth setup.** Every template here assumes the client already has a working
  otto-factory MCP connection (OAuth or a personal access token). Configuring that
  connection is `docs/clients/matrix.md`'s job, and the console's connect page's job —
  not this directory's.
- **Not a way to claim or work real jobs.** These templates only ever create, claim, and
  immediately complete a session-marker job. They never touch `ready`'s general pool of
  claimable work.
- **Not a server-side feature.** Nothing here changes `crates/*`. otto-factory the server
  stays exactly as ignorant of whether, or how, any client automates queue registration
  as it was before this directory existed.

## Launch scope

This repo's own conformance work (`docs/clients/matrix.md`) has only ever driven Claude Code
and Copilot CLI live against a real otto-factory server. Shipping a fabricated, never-run
template for a client nobody here has actually exercised would read as working when it
isn't — worse than an honest "not yet, here's the shape a PR should take." So launch ships
exactly one fully working reference template (Claude Code) and a stub `README.md` for every
other client this directory names, each stating the target client, a starting hypothesis for
its automation surface, and the contribution checklist below — no functioning hook code, no
fabricated verification claims. A stub graduates to a working template only once someone has
actually run it against a live `of-server` and can honestly check every contribution-checklist
box.

## Directory layout

```
client-skills/
  README.md          — this file
  claude-code/       — a fully working reference template
  copilot-cli/       — stub: contribution invitation
  cursor/            — stub: contribution invitation
  codex/             — stub: contribution invitation
  otto-cli/          — stub: contribution invitation
  generic/           — stub: contribution invitation
```

Subdirectory names match `web/src/lib/clients.ts::CLIENTS`'s own `id` values exactly (see
that file's own `generic` entry: "exists so that a client nobody here has heard of is still
a first-class citizen"), so a future console link can construct the directory URL from `id`
alone with no separate mapping table to keep in sync.

See `claude-code/README.md` for a worked example — the fullest description of the
contract above applied to one real client. Claude Code's own template needs a two-actor
split (a deterministic hook script handing an instruction to the model, which is the
party that actually holds an authenticated otto-factory MCP connection) because a
`SessionStart` hook process has no access to the session's own MCP credentials. **This
split is specific to Claude Code's own limitation, not part of the contract itself.** A
client whose automation surface can itself reach a valid otto-factory credential
directly should implement the whole sequence above as one deterministic script — no
model hand-off required. If neither shape fits a given client, propose a different
mechanism entirely in that client's own `README.md` and PR description; the job-lifecycle
contract above is the strong default, not a mandate on how it's carried out.

## Contributing a new client template

A new (or updated) client template's PR description should be able to tick every box
below:

- [ ] Names the target client and the minimum version tested against.
- [ ] Names the exact session-start (or equivalent) mechanism used, and links to that
      client's own documentation for it.
- [ ] Lists, in order, every otto-factory MCP tool called and with what arguments.
- [ ] States plainly whether this was verified against a real running `of-server`, or is
      an unverified starting hypothesis (see `docs/clients/matrix.md`'s own register for
      this — "not run" is an acceptable, honest answer; a silent claim of "verified" when
      it wasn't is not).
- [ ] Follows the silent-failure contract above — no blocking, no error surfaced to the
      developer's normal session on any otto-factory-side failure.
- [ ] Does not attempt to auto-register an unregistered repo.
- [ ] Does not call `claim_jobs` against anything from the general `ready` pool — only
      against a job id this same template just created.
- [ ] Validates every local value it interpolates into a model-facing instruction (a
      remote URL, a branch name, a file path) against a strict allowlist before use, and
      presents it as labeled, fenced data the instruction refers to rather than text
      spliced directly into an imperative step — see the behavior contract's point 6 above.

There is no CI enforcement of this directory's contents — no lint job, no schema
validator. The checklist above is the only guard, enforced like any other open-source
contribution: at PR review, by a human.
