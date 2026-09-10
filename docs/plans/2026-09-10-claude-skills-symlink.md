# Claude Code skill discovery via `.claude/skills` — implementation plan

**Fast-path: no design spec per otto-factory-development trivial-task criteria** — single tracked
path added (`.claude/skills` as a directory symlink), no new public interface, no breaking change,
no auth/tenant/metering/migration/crate-boundary/deploy-shape surface touched, no tested behavior
changed, acceptance criterion fits one sentence: *a fresh clone has every skill under
`.github/skills/` discoverable by Claude Code with no manual step.*

## Goal

Claude Code discovers project skills from `.claude/skills/`. This repo's skills live at
`.github/skills/` (so they stay usable by any other coding-agent tool that already reads that
path, per this repo's coding-agent-agnostic stance) — today `.claude/skills/` doesn't exist as a
tracked path at all, so a fresh clone gives Claude Code nothing. A developer can create a local
symlink by hand, but that's untracked and machine-specific: invisible to another contributor, to
CI, or to another agent. Implements savvagent/otto-factory#123.

## Status — 2026-09-10

🚧 Implemented, PR open against savvagent/otto-factory#123. Flip to ✅ Shipped with the merge
commit SHA once merged (record-as-shipped).

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

## Final gate

- [x] `cargo test --workspace` — vacuously satisfied, no Rust source changed; state this explicitly
      in the PR body rather than running it pointlessly (no `.rs` file in this diff).
- [x] `cargo clippy --all-targets -- -D warnings` / `cargo fmt --all --check` — same, vacuous.
- [x] `cd web && npm run check && npm run lint && npm test` — vacuous, no `web/` file changed.
- [x] No container image, console bundle, Cloudflare Worker, or migration touched — Phase 5
      out-of-band verification is vacuously satisfied; state so explicitly.
