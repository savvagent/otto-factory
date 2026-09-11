# Fast-path criteria gains a normative-process-docs disqualifier — implementation plan

**Spec:** `docs/specs/2026-09-11-fastpath-process-docs-design.md` — read it first. This plan
implements it exactly.

## Goal

Closes `savvagent/otto-factory#152`: add a fast-path disqualifier to
`.github/skills/otto-factory-development/SKILL.md`'s trivial-task criteria for new or
materially-changed normative process content under `.github/skills/` — a new skill file, or a
substantial rewrite of dispatch/orchestration logic in an existing one — citing PR #150's
escalating five-round trio review as the motivating example, so such changes go through the
design-spec path even when they are "only" 1-2 files.

## Status — 2026-09-11

⬜ Not started.

## Global Constraints

- No AI self-attribution anywhere (commits, PR body, comments, docs) — Non-Negotiable Rule 3.
- This is a Markdown edit inside `.github/skills/otto-factory-development/SKILL.md`'s
  "Fast-Path: Trivial Tasks" section only. No Rust, SQL, migration, MCP tool, console route,
  `OF_*` config key, or `web/` file is touched.
- No tenant table, RLS policy, MCP tool, or `of-billing::classify` entry is added or touched —
  no cross-org negative test or billing classification needed.
- Not a public-interface change under Non-Negotiable Rule 6 (the MCP tool surface, console
  REST API, OAuth/discovery endpoints, config surface, and schema do not include this repo's
  own skill files) — no breaking-change flag, no version-bump signal.
- The new disqualifier's wording is scoped to the spec's Assumptions: "a new file under
  `.github/skills/`, or a substantial rewrite of dispatch/orchestration logic in an existing
  one" — not "any edit to a skill file." A prose typo fix inside a skill file remains eligible
  for fast-path on its own merits.
- Match the existing disqualifier bullets' terse, one-line, no-sub-clause style (per the spec
  critique's advisory note) when drafting the new bullet.
- No `cargo fmt`/`cargo test` gate applies — no Rust file is touched. The gate for this task is
  a targeted diff review: `git diff` on `SKILL.md` touches only the three points named below.

## File Structure

| File | Responsibility |
|---|---|
| `.github/skills/otto-factory-development/SKILL.md` | **Modify.** "Fast-Path: Trivial Tasks" section: one new disqualifier bullet, one new rationalization-table row, one new clause in the "If you find yourself rationalizing…" trigger paragraph. |

## Task Order & Rationale

Single task — one file, one section, three closely-related edit points that all land together
so the section stays internally consistent in one commit.

## Task 1 — Add the normative-process-docs fast-path disqualifier ⬜

**Files:** `.github/skills/otto-factory-development/SKILL.md`
**Interfaces:** none — this edits a Markdown criteria list read by whichever agent invokes the
`otto-factory-development` skill; it has no compiled interface.

- [ ] Confirm the current section text hasn't drifted since the spec was written:
      `sed -n '/^## Fast-Path: Trivial Tasks/,/^## Repository Conventions/p' .github/skills/otto-factory-development/SKILL.md`
      — should show the disqualifier list (ending "The acceptance criterion fits in one
      sentence"), the "Concrete examples that qualify" list, the fast-path-plan paragraph, the
      "If you find yourself rationalizing…" trigger paragraph, and the rationalization table
      ending with the "No spec, but I'll still write a one-line plan" row. Using a
      heading-anchored range (rather than a fixed line count) keeps this command correct after
      the section grows by the insertions below.
- [ ] Add a new bullet to the disqualifier list (after the existing "No change to
      deploy/distribution shape (...)" bullet and before "The acceptance criterion fits in one
      sentence", so the acceptance-criterion bullet — the tightest, most general check — stays
      last):
      ```
      - No new file under `.github/skills/`, and no substantial rewrite of
        dispatch/orchestration logic in an existing one — process documentation that drives
        future autonomous runs is itself architecture (PR #150 fast-pathed on this reasoning
        and needed five rounds of trio review to harden)
      ```
- [ ] Extend the "If you find yourself rationalizing into the fast-path on…" trigger paragraph
      with the same category, inserted before the existing "or has more than a one-sentence
      AC" clause so the paragraph's final "→ STOP. Write the spec." still reads naturally:
      change
      ```
      If you find yourself rationalizing into the fast-path on something that touches 3+ source files,
      introduces a new interface, touches the auth spine or tenant isolation, adds a migration, or has
      more than a one-sentence AC → STOP. Write the spec. The fast-path is for genuine triviality, not
      "I think this is small."
      ```
      to
      ```
      If you find yourself rationalizing into the fast-path on something that touches 3+ source files,
      introduces a new interface, touches the auth spine or tenant isolation, adds a migration, adds or
      substantially rewrites a skill file under `.github/skills/`, or has more than a one-sentence AC →
      STOP. Write the spec. The fast-path is for genuine triviality, not "I think this is small."
      ```
- [ ] Add a new row to the "Common Fast-Path Rationalizations" table (the `| Fast-path
      rationalization | Reality |` table), after the existing "I'll fast-path the first
      sub-change and spec the rest" row and before the closing "No spec, but I'll still write a
      one-line plan" row:
      ```
      | "The new skill content is tiny — just a couple of files" | Process docs that will drive future autonomous runs are themselves architecture. PR #150 fast-pathed a 511-line-at-the-time skill-file pair on "two logical files" and needed five rounds of trio review before it hardened. Spec first. |
      ```
- [ ] Confirm the diff is scoped exactly as planned: `git diff .github/skills/otto-factory-development/SKILL.md`
      shows three additions (one disqualifier bullet, one trigger-paragraph edit, one table
      row) and nothing else — no reflow of unrelated lines, no change outside the "Fast-Path:
      Trivial Tasks" section.
- [ ] Re-read the full "Fast-Path: Trivial Tasks" section once more
      (`sed -n '/^## Fast-Path: Trivial Tasks/,/^## Repository Conventions/p' .github/skills/otto-factory-development/SKILL.md`)
      to confirm it still reads coherently end to end with the three insertions in place —
      table column alignment doesn't need to be pixel-perfect (the existing table already has
      long, unaligned cells), but the row must parse as a valid Markdown table row.
- [ ] Format and commit: `git commit -m "docs: disqualify normative skill-process changes from otto-factory-development's fast-path"`.
      No `cargo fmt` needed — no Rust file touched. `docs` is the correct Conventional-Commits
      type for the PR title `pr-title` CI checks against.

## Final Verification (after Task 1)

- [ ] `git log --oneline` on the branch shows the spec commit, the plan commit, and the Task 1
      commit, none carrying AI attribution.
- [ ] `cargo test --workspace` — vacuously unaffected (no Rust source touched); confirm the
      `rust` CI job on this PR passes to be sure nothing in the workspace parses `SKILL.md`.
- [ ] No `web/` change — `npm run check`/`lint`/`test`/`build` vacuously satisfied; confirm the
      `web` CI job on this PR passes.
- [ ] No migration, no container image, no Cloudflare Worker touched — vacuously satisfied.
- [ ] Re-read the edited "Fast-Path: Trivial Tasks" section fresh (as a future run invoking
      this skill would) and confirm a hypothetical PR #150-shaped change — a new pair of skill
      files under `.github/skills/`, described in the brief as "two logical files" — would now
      fail the disqualifier list on the new bullet, per the spec critique's advisory
      recommendation.
