# Fast-path criteria gains a normative-process-docs disqualifier — implementation plan

**Spec:** `docs/specs/2026-09-11-fastpath-process-docs-design.md` — read it first. This plan
implements it exactly.

## Goal

Closes `savvagent/otto-factory#152`: add a fast-path disqualifier to
`.github/skills/otto-factory-development/SKILL.md`'s trivial-task criteria for new or
materially-changed normative process content under `.github/skills/` — a new skill file, or
any change to dispatch/orchestration logic in an existing one, however small (a prose or typo
fix stays eligible) — citing PR #150's escalating five-round trio review as the motivating
example, so such changes go through the design-spec path even when they are "only" 1-2 files.

## Status — 2026-09-11

✅ Shipped in PR #171, merged as commit `276e11f8192f0aa21926a2d7b24780e2933f4133`. CI on that
merge commit is green (rust + web, run against master). One task, complete.

The mandatory review trio (rust-pro, architect-reviewer, security-auditor) found no
Critical/High issues. rust-pro and architect-reviewer independently flagged the same Important
finding: the new disqualifier bullet and its trigger-paragraph clause had drifted in scope, and
"substantial rewrite of dispatch/orchestration logic" reintroduced a size threshold graded by
the same agent deciding whether to skip the spec — the exact self-assessment failure the section
exists to prevent (echoed as a Low finding by security-auditor). Resolved in a follow-up commit
on the PR: "substantial" was dropped entirely, so the bullet, the trigger-paragraph clause, and
the rationalization table now agree that *any* change to dispatch/orchestration logic in an
existing skill file disqualifies, however small, with an explicit prose/typo carve-out.
architect-reviewer also flagged that the committed plan's prescribed trigger-paragraph wording
had drifted from what shipped in `SKILL.md`; fixed in the same commit, and this document now
matches. The spec's Out-of-scope claim that the new bullet was "appended, not interleaved" was
corrected to describe its actual second-to-last insertion point (architect-reviewer Minor,
security-auditor Informational). The one finding left as a follow-up rather than fixed in this
PR: architect-reviewer and security-auditor both noted the disqualifier's rationale applies at
least as strongly to `CLAUDE.md` as to `.github/skills/`, which stays fast-pathable after this
PR — issue #152's acceptance criteria scoped the ask to `.github/skills/` specifically, so this
was filed as `savvagent/otto-factory#172` rather than folded into this PR's scope.

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
  `.github/skills/`, or any change to dispatch/orchestration logic in an existing one, however
  small" — not "any edit to a skill file." A prose or typo fix inside a skill file remains
  eligible for fast-path on its own merits. The rule deliberately does not turn on size — a
  size threshold ("substantial") would be a self-graded judgment call made by the same agent
  deciding whether to skip the spec, which is the exact failure mode this section exists to
  refuse.
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

## Task 1 — Add the normative-process-docs fast-path disqualifier ✅

**Files:** `.github/skills/otto-factory-development/SKILL.md`
**Interfaces:** none — this edits a Markdown criteria list read by whichever agent invokes the
`otto-factory-development` skill; it has no compiled interface.

- [x] Confirm the current section text hasn't drifted since the spec was written:
      `sed -n '/^## Fast-Path: Trivial Tasks/,/^## Repository Conventions/p' .github/skills/otto-factory-development/SKILL.md`
      — should show the disqualifier list (ending "The acceptance criterion fits in one
      sentence"), the "Concrete examples that qualify" list, the fast-path-plan paragraph, the
      "If you find yourself rationalizing…" trigger paragraph, and the rationalization table
      ending with the "No spec, but I'll still write a one-line plan" row. Using a
      heading-anchored range (rather than a fixed line count) keeps this command correct after
      the section grows by the insertions below.
- [x] Add a new bullet to the disqualifier list (after the existing "No change to
      deploy/distribution shape (...)" bullet and before "The acceptance criterion fits in one
      sentence", so the acceptance-criterion bullet — the tightest, most general check — stays
      last):
      ```
      - No new file under `.github/skills/`, and no change to dispatch/orchestration logic in
        an existing one, however small — a prose or typo fix inside one stays eligible
      ```
- [x] Extend the "If you find yourself rationalizing into the fast-path on…" trigger paragraph
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
      introduces a new interface, touches the auth spine or tenant isolation, adds a migration, adds a
      file or changes dispatch/orchestration logic under `.github/skills/` (a prose or typo fix stays
      eligible), or has more than a one-sentence AC → STOP. Write the spec. The fast-path is for
      genuine triviality, not "I think this is small."
      ```
- [x] Add a new row to the "Common Fast-Path Rationalizations" table (the `| Fast-path
      rationalization | Reality |` table), after the existing "I'll fast-path the first
      sub-change and spec the rest" row and before the closing "No spec, but I'll still write a
      one-line plan" row:
      ```
      | "The new skill content is tiny — just a couple of files" | Process docs that will drive future autonomous runs are themselves architecture. PR #150 fast-pathed a 511-line-at-the-time skill-file pair on "two logical files" and needed five rounds of trio review before it hardened. Spec first. |
      | "It's only one bullet, hardly a substantial rewrite" | The rule doesn't turn on size — any change to dispatch/orchestration logic under `.github/skills/` disqualifies, however small. Grading your own edit as "not substantial" is the exact rationalization this section exists to refuse. Spec first (a prose/typo fix is the only carve-out). |
      ```
- [x] Confirm the diff is scoped exactly as planned: `git diff .github/skills/otto-factory-development/SKILL.md`
      shows three additions (one disqualifier bullet, one trigger-paragraph edit, one table
      row) and nothing else — no reflow of unrelated lines, no change outside the "Fast-Path:
      Trivial Tasks" section.
- [x] Re-read the full "Fast-Path: Trivial Tasks" section once more
      (`sed -n '/^## Fast-Path: Trivial Tasks/,/^## Repository Conventions/p' .github/skills/otto-factory-development/SKILL.md`)
      to confirm it still reads coherently end to end with the three insertions in place —
      table column alignment doesn't need to be pixel-perfect (the existing table already has
      long, unaligned cells), but the row must parse as a valid Markdown table row.
- [x] Format and commit: `git commit -m "docs: disqualify normative skill-process changes from otto-factory-development's fast-path"`.
      No `cargo fmt` needed — no Rust file touched. `docs` is the correct Conventional-Commits
      type for the PR title `pr-title` CI checks against.

## Final Verification (after Task 1)

- [x] `git log --oneline` on the branch shows the spec commit, the plan commit, and the Task 1
      commit, none carrying AI attribution.
- [x] `cargo test --workspace` — vacuously unaffected (no Rust source touched); confirm the
      `rust` CI job on this PR passes to be sure nothing in the workspace parses `SKILL.md`.
- [x] No `web/` change — `npm run check`/`lint`/`test`/`build` vacuously satisfied; confirm the
      `web` CI job on this PR passes.
- [x] No migration, no container image, no Cloudflare Worker touched — vacuously satisfied.
- [x] Re-read the edited "Fast-Path: Trivial Tasks" section fresh (as a future run invoking
      this skill would) and confirm a hypothetical PR #150-shaped change — a new pair of skill
      files under `.github/skills/`, described in the brief as "two logical files" — would now
      fail the disqualifier list on the new bullet, per the spec critique's advisory
      recommendation.

## Record-as-shipped

Done — this commit is that record: the spec's `> **Status:**` is flipped to IMPLEMENTED, and
this plan's `## Status` block above carries the merge commit and the review-trio outcome, per
Phase 4 step 12 of `otto-factory-development`.
