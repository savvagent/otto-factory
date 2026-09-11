# Fast-path criteria gains a normative-process-docs disqualifier design

> **Status:** DRAFT — adds a fast-path disqualifier to `otto-factory-development`'s
> trivial-task criteria for new-or-materially-changed normative process content under
> `.github/skills/`, using PR #150's escalating five-round review as the motivating example.

## Premise corrections

None. The issue's premises hold on inspection: `.github/skills/otto-factory-development/SKILL.md`'s
current "Fast-Path: Trivial Tasks" criteria (all file/interface/deploy-shaped — file count, public
interfaces, auth spine, tenant isolation, metering, crate boundaries, tested behavior, deploy
shape) have no bullet that would have disqualified a change to `.github/skills/` content itself.
PR #150 (adding `otto-factory-scanner`/`otto-factory-worker`) is real, merged history in this
repo and its round-1 architect review is the origin of this ticket, per the issue body.

## Scope

**In:**

- One new bullet in the "ALL of the following are true" disqualifier list in
  `.github/skills/otto-factory-development/SKILL.md`'s "Fast-Path: Trivial Tasks" section,
  disqualifying a new file under `.github/skills/`, or a substantial rewrite of
  dispatch/orchestration logic in an existing one, from the fast path — regardless of how few
  files it touches.
- That bullet cites the PR #150 precedent (a 511-line-at-the-time skill-file pair that
  fast-pathed on "two logical files" and needed five rounds of trio review to harden) as the
  motivating example, per the issue's second acceptance-criteria line.
- A matching row in the "Common Fast-Path Rationalizations" table (the `| Fast-path
  rationalization | Reality |` table right after the disqualifier list), so the new rule shows
  up in the same place a future run would look when rationalizing into the fast path — this
  table already carries one row per existing disqualifier category (new MCP tool, new console
  route, SQL outside `of-core`, an edited migration, etc.), and leaving the newest disqualifier
  out of it would be the one gap the rest of the section doesn't have.
- Extending the "If you find yourself rationalizing into the fast-path on…" trigger paragraph
  (the sentence immediately above that table) with the same category, for the same
  completeness reason — that paragraph is a prose restatement of the disqualifier list's
  highlights, and every other disqualifier in the list already has a clause there.

**Out:**

- No change to `general-development`'s or `ce-development`'s fast-path criteria — this ticket
  and the PR #150 precedent are both otto-factory-specific (`.github/skills/` is this repo's
  skill-file location; the analogous location in another repo, if any, is that repo's own
  concern).
- No change to how dispatch/orchestration logic actually behaves — this is a criteria-list
  edit governing when a *future* run writes a spec before editing a skill file, not a change
  to any runtime code path, MCP tool, console route, or config key. Per Non-Negotiable Rule 6,
  none of the "public interface" categories it defines (MCP tool surface, console REST API,
  OAuth/discovery endpoints, config surface, schema) include this repo's own skill files, so
  this change carries no breaking-change flag and no version-bump signal.
- No change to the "Concrete examples that qualify" list — that list is unaffected; a typo fix
  inside a skill file's prose (not its criteria or dispatch logic) still qualifies under the
  existing "Fix a typo in a string / comment / docstring" example, per the Assumptions below.
- No renumbering or restructuring of the existing disqualifier bullets — the new bullet is
  appended to the existing list, not interleaved, to keep the diff minimal and reviewable.

## Assumptions

- **"Materially-changed" is scoped to the AC's own parenthetical, not to any edit whatsoever.**
  The issue's acceptance criteria define the disqualified category explicitly: "a new skill
  file or a substantial rewrite of dispatch/orchestration logic in an existing one." A
  one-line prose fix, a typo correction, or a formatting tweak inside an existing skill file is
  **not** a "substantial rewrite of dispatch/orchestration logic" and remains eligible for the
  existing fast-path machinery on its own merits (subject to the *other* disqualifiers, same
  as any other file). Without this scoping, the new rule would make every future one-character
  fix to a skill file go through a full spec, which is disproportionate and not what the issue
  asks for — the issue's own motivating complaint is about *design-heavy* skill content
  (dispatch/orchestration logic), not skill-file edits in general.
- **This change is itself worked without invoking the fast path it is patching.** This spec
  exists — i.e., this exact job did not fast-path — because the change it makes is precisely
  "a substantial rewrite of dispatch/orchestration logic in an existing [skill] file": it edits
  the criteria list in `SKILL.md`'s "Fast-Path: Trivial Tasks" section, which is the dispatch
  logic that decides whether a future run writes a spec at all. Applying the disqualifier the
  ticket asks for to the ticket's own change is the only self-consistent reading; fast-pathing
  a change to the fast-path rule under the rule's own pre-patch wording would be exploiting the
  exact loophole this ticket exists to close.
- **The rationalization-table row and the trigger-paragraph clause are in scope, not
  gold-plating.** The issue's acceptance criteria name only the disqualifier bullet and the PR
  #150 citation, but both the table and the trigger paragraph are existing, structurally
  parallel restatements of every other disqualifier already in the list (each disqualifier
  bullet has a matching trigger-paragraph clause and, for most, a rationalization-table row).
  Leaving the newest disqualifier out of either would make it the one entry in the section
  that is not discoverable from the paragraph or the table, which undercuts the section's own
  house style rather than extending it. Both additions are one clause / one table row — not a
  restructuring.
- **No `docs/plans/2026-09-01-milestone-1.md` update is needed.** That plan tracks the
  otto-factory *product's* build (the Rust workspace, the console, the MCP surface), not the
  `otto-factory-development` skill's own internal criteria — this change has no corresponding
  milestone-plan task to mark.

## Goal & Success Criteria

Close the loophole PR #150's round-1 architect review identified: `otto-factory-development`'s
fast-path criteria are framed entirely in Rust/interface/deploy terms and have no category for
new or materially-changed normative process content under `.github/skills/`, even though such
content is itself architecture that drives every future autonomous run. A future change that
adds a skill file or substantially rewrites an existing one's dispatch/orchestration logic
should go through the design-spec path even when it is "only" 1-2 files.

- `.github/skills/otto-factory-development/SKILL.md`'s "Fast-Path: Trivial Tasks" disqualifier
  list carries a new bullet disqualifying a new file, or a substantial dispatch/orchestration
  rewrite of an existing one, under `.github/skills/`.
- That bullet (or text immediately adjacent to it) names PR #150 and its five-round trio-review
  outcome as the motivating example, per the issue's second acceptance-criteria line.
- The rationalization table and the trigger paragraph each gain one matching entry, consistent
  with every other disqualifier already documented there.
- No other content in `SKILL.md` changes — `git diff` on the file touches only the "Fast-Path:
  Trivial Tasks" section.
- No Rust, console, or migration file changes at all; `cargo test --workspace` and the `web/`
  gates are vacuously unaffected.

## Error Handling & Edge Cases

None — this is a documentation edit to a Markdown criteria list with no runtime behavior, no
parse path outside a human or agent reading the file, and no test that reads these specific
lines (`SKILL.md` is not `cargo test`-visible; it is read by whichever agent invokes the skill).

## Risks & Open Questions

- **Risk: the new bullet is read too broadly and blocks legitimate skill-file typo fixes.**
  Mitigated by scoping the bullet's wording to the AC's own parenthetical ("a new skill file or
  a substantial rewrite of dispatch/orchestration logic in an existing one") rather than "any
  change to a file under `.github/skills/`" — see Assumptions.
- **Risk: the new bullet is read too narrowly and misses a skill file added somewhere other
  than a brand-new skill directory** (e.g., a new prompts file inside an existing skill's
  directory, the way `otto-factory-development`'s own `agent-prompts.md` sits alongside its
  `SKILL.md`). Mitigated by wording the bullet as "a new file under `.github/skills/`" rather
  than "a new skill," so an added file inside an existing skill's directory is covered too.
- No open questions — the issue's acceptance criteria are unambiguous and this spec implements
  them directly.

## Tenant isolation / Metering

Not applicable — no tenant table, RLS policy, MCP tool, or `of-billing::classify` entry is
added or touched by this change; it is a Markdown edit to a skill file's criteria list.
