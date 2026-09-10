# Claude Code skill discovery via `.claude/skills` — implementation plan

**Fast-path: no design spec per otto-factory-development trivial-task criteria** — single tracked
path added (`.claude/skills` as a directory symlink), no new public interface, no breaking change,
no auth/tenant/metering/migration/crate-boundary/deploy-shape surface touched, no tested behavior
changed, acceptance criterion fits one sentence: *a fresh clone has every skill under
`.github/skills/` discoverable by Claude Code with no manual step.*

## Goal

Claude Code discovers project skills from `.claude/skills/`. This repo's skills live at
`.github/skills/` and stay there — today `.claude/skills/` doesn't exist as a tracked path at
all, so a fresh clone gives Claude Code nothing. A developer can create a local symlink by hand,
but that's untracked and machine-specific: invisible to another contributor, to CI, or to another
agent. The fix is one link, not a copy: `.github/skills/` remains the single source of truth for
skill content, and `.claude/skills` becomes a tracked discovery alias into it, so a second agent's
own discovery convention (when one shows up) is another symlink beside this one, never a fork of
the content. (`.github/skills/otto-factory-development` is itself already Claude-Code-shaped
content — this PR does not make it agent-neutral, only avoids forking it; that's a narrower claim
than "usable by any coding-agent tool" and the honest one.) Implements savvagent/otto-factory#123.

## Status — 2026-09-10

🚧 Implemented, PR open: savvagent/otto-factory#124 (closes #123). Flip to ✅ Shipped with the
merge commit SHA once merged (record-as-shipped).

## Global Constraints

- No AI self-attribution anywhere — commits, comments, docs.
- `.github/skills/` stays the single source of truth for skill content; nothing under it is copied
  or forked.
- No SQL, no crate, no MCP tool, no console route, no migration, no `OF_*` config key is touched by
  this change — it is a repo-tooling path only.

## File Structure

| File                | Responsibility                                                                 |
| ------------------- | -------------------------------------------------------------------------------- |
| **Create.** `.claude/skills` | Directory symlink to `../.github/skills`, so every current and future skill under `.github/skills/` is discoverable by Claude Code without a per-skill entry. |
| **Modify.** `.gitignore` | Scope `.claude/` down to the tracked `skills` symlink so Claude Code's own machine-local state (`settings.local.json`, `scheduled_tasks.lock`) never becomes trackable. |
| **Modify.** `.dockerignore` | Exclude `.claude` alongside the existing `.git`/`.github` exclusions, so the build context doesn't carry a dangling symlink (`.github/skills` is already excluded). |
| **Modify.** `CLAUDE.md` | Record the "content lives at `.github/skills/`, per-agent paths are symlinks into it, never copies" convention durably, per the mandatory review trio's architect-review finding. |

## Task Order & Rationale

Single task — there is only one change to make.

## Task 1 — Symlink `.claude/skills` to `.github/skills` ✅

**Files:** `.claude/skills` (new symlink)

**Interfaces:** none — no code path consumes or produces anything; this only changes what Claude
Code's skill-discovery glob finds on disk.

- [x] Create the directory `.claude/` if it doesn't already exist as a tracked path.
- [x] Create `.claude/skills` as a symlink to `../.github/skills` (relative, so it resolves the
      same way regardless of where the repo is cloned): `ln -s ../.github/skills .claude/skills`.
- [x] Verify it resolves: `ls .claude/skills/otto-factory-development/SKILL.md` must show the real
      file, and `readlink .claude/skills` must print `../.github/skills`.
- [x] Verify git tracks it as a symlink, not a copy: `git add .claude/skills && git status` should
      show `new file: .claude/skills` as a single tree entry (mode `120000`), not a directory of
      files — confirm with `git ls-files -s .claude/skills`.
- [x] Confirm no other repo tooling treats `.claude/` specially in a way this would break: `grep -rn
      "\.claude/" --include="*.yml" --include="*.yaml" .github/workflows/` should show nothing (CI
      does not reference `.claude/`).
- [x] Format and commit: `git add .claude/skills && git commit -m "docs: symlink .claude/skills to .github/skills for Claude Code discovery"`.

**Review hardening (from the mandatory rust-pro/architect-reviewer/security-auditor trio + pr-review-toolkit:code-reviewer on PR #124 — no Critical/High findings, these are the Important/Suggestion items applied in the same PR):**

- [x] Scope `.gitignore`'s `.claude/` entry down to the tracked symlink (`.claude/*` +
      `!.claude/skills`), so Claude Code's machine-local state never becomes stageable.
- [x] Add `.claude` to `.dockerignore` beside the existing `.git`/`.github` exclusions.
- [x] Record the source-of-truth/symlink-alias convention in `CLAUDE.md` (new "Development
      skills" section after `## Commands`), including the Windows `core.symlinks` caveat.
- [x] Reword this plan's Goal to the honest justification (one copy, no fork — not "already
      agent-neutral", which the content isn't).
- [x] Format and commit.

Deferred to follow-up issues (not this PR): a `CODEOWNERS` entry gating `.github/skills/` and
`.claude/` (security-auditor, repo-wide governance gap, needs a real team/user handle); a CI step
asserting `.claude/skills` still resolves to `.github/skills` (security-auditor + architect —
correct idea, but adding it touches `.github/workflows/`, one of the fast-path's own disqualifying
surfaces, so it's follow-up work, not a same-PR change).

## Final gate

- [x] `cargo test --workspace` — vacuously satisfied, no Rust source changed; state this explicitly
      in the PR body rather than running it pointlessly (no `.rs` file in this diff).
- [x] `cargo clippy --all-targets -- -D warnings` / `cargo fmt --all --check` — same, vacuous.
- [x] `cd web && npm run check && npm run lint && npm test` — vacuous, no `web/` file changed.
- [x] No container image, console bundle, Cloudflare Worker, or migration touched — Phase 5
      out-of-band verification is vacuously satisfied; state so explicitly.
