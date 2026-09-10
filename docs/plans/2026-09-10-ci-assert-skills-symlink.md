# CI assertion for the `.claude/skills` symlink — implementation plan

Goal: add a CI step to `.github/workflows/ci.yml`'s `rust` job that fails when `.claude/skills`
stops being a symlink to `../.github/skills`, or when that symlink no longer resolves to this
repo's development skill — closing savvagent/otto-factory#127.

**Spec:** `docs/specs/2026-09-10-ci-assert-skills-symlink-design.md` — read it first. This plan
implements it exactly.

---

## Status — 2026-09-10

Approved after two critique rounds (one real bug found and fixed: the deleted-path negative
check originally pointed at a pre-#124 commit that still had `.claude/skills` as a real tracked
directory, not an absent path — fixed to point at the repo's actual root commit `7cbddbb`).
Implementation not yet started.

---

## Global Constraints

These hold for the one task in this plan:

- No AI self-attribution anywhere — commits, comments, docs, PR body.
- This is a pure `.github/workflows/ci.yml` change. No Rust or `web/` source changes, so `cargo
  fmt`/`cargo clippy`/`cargo test`/`npm run check`/`npm run lint`/`npm test` are all
  vacuously unaffected — none of them exercise workflow YAML. State that explicitly in the PR
  body rather than running them pointlessly.
- No tenant table, no SQL, no MCP tool, no console route, no config surface, no migration — the
  tenant-isolation, metering, and breaking-change checklists in the plan-critique template are
  all inapplicable here; nothing to fill in.
- The script must be verified empirically, not just read — this task follows two real bugs
  the spec's own critique loop found in an earlier draft of the same script (a `git ls-tree`
  trailing-pathname comparison bug, and an unguarded-failure ordering bug on a deleted path).
  Re-derive confidence from running the commands, not from re-reading the spec's claims.

## File Structure

| File | Responsibility |
|---|---|
| **Modify.** `.github/workflows/ci.yml` | Adds the symlink-assertion step to the `rust` job, immediately after `actions/checkout@v4` and before `Install Rust toolchain`. |

## Task Order & Rationale

One task — one file, one step, no dependencies to sequence.

---

## Task 1 — Add the CI assertion step ⬜

**Files:** `.github/workflows/ci.yml`.

**Interfaces:** none produced or consumed — this is a CI-only change with no Rust or `web/`
interface surface.

- [ ] Read the current `rust` job in `.github/workflows/ci.yml` and confirm the insertion point:
      immediately after the `- uses: actions/checkout@v4` step and before the
      `- name: Install Rust toolchain` step. (Already confirmed during spec review; re-confirm
      here since a merge from `master` could have shifted it before this task starts.)
- [ ] There is no automated test harness for workflow YAML in this repo (the spec's §2 Testing
      section documents this explicitly), so this task's "failing test first" step is a manual
      empirical run of the intended script against the current tree, executed as scratch shell
      commands in the worktree (not committed) — expected to **pass** on the untouched tree,
      confirming the baseline before the step is added:
      ```bash
      set -euo pipefail
      line=$(git ls-tree HEAD .claude/skills)
      test -n "$line"
      fields=$(printf '%s' "$line" | cut -f1)
      mode=$(printf '%s' "$fields" | awk '{print $1}')
      type=$(printf '%s' "$fields" | awk '{print $2}')
      test "$mode" = "120000" && test "$type" = "blob"
      target=$(git cat-file blob HEAD:.claude/skills)
      test "$target" = "../.github/skills"
      test -f .claude/skills/otto-factory-development/SKILL.md
      echo ALL PASS
      ```
- [ ] Add the step from spec §1 verbatim to `.github/workflows/ci.yml`'s `rust` job, between
      `actions/checkout@v4` and `Install Rust toolchain`:
      ```yaml
      - name: Assert .claude/skills resolves to .github/skills
        run: |
          set -euo pipefail
          line=$(git ls-tree HEAD .claude/skills)
          if [ -z "$line" ]; then
            echo "::error::.claude/skills does not exist in HEAD (expected a symlink)"
            exit 1
          fi
          fields=$(printf '%s' "$line" | cut -f1)
          mode=$(printf '%s' "$fields" | awk '{print $1}')
          type=$(printf '%s' "$fields" | awk '{print $2}')
          if [ "$mode" != "120000" ] || [ "$type" != "blob" ]; then
            echo "::error::.claude/skills is not a symlink blob in HEAD (git ls-tree: $line)"
            exit 1
          fi
          target=$(git cat-file blob HEAD:.claude/skills)
          if [ "$target" != "../.github/skills" ]; then
            echo "::error::.claude/skills points at '$target', expected '../.github/skills'"
            exit 1
          fi
          if [ ! -f .claude/skills/otto-factory-development/SKILL.md ]; then
            echo "::error::.claude/skills/otto-factory-development/SKILL.md not reachable through the symlink"
            exit 1
          fi
      ```
- [ ] Validate the edited YAML parses: `gh workflow view ci.yml --repo savvagent/otto-factory`
      (reads the version on `master`, so this only confirms syntax generically before push — the
      real validation is the branch's own PR run in a later step) or a local YAML parse, e.g.
      `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml'))"`. Expect no
      parse error.
- [ ] Run the negative checks by hand once, in scratch locations, to confirm the script's error
      paths actually fire as designed (this is what the spec's §2 Testing section commits to
      reporting in the PR body — do this now so the PR body can state real results, not
      predictions):
      - Deleted path: `.claude/skills` has existed as a path since the repo's very first
        content-bearing commits — #124 converted an existing *directory* into a symlink, it did
        not create the path from nothing, so a commit "before #124" (e.g. `74d6062`, its
        immediate parent) still has `.claude/skills` present as a `040000 tree`, which exercises
        the *directory* branch below, not this one. To exercise a genuinely absent path, run
        against the repo's first commit instead: `git ls-tree 7cbddbb .claude/skills` — expect
        empty output, confirming the `[ -z "$line" ]` branch would fire.
      - Wrong target: in a scratch git repo (`/tmp` or the scratch directory, never this repo),
        create a symlink pointing somewhere else, commit it, and run the target-comparison
        portion of the script against it — expect the target mismatch branch to fire with the
        actual vs. expected values in the message.
      - Real directory instead of a symlink: confirm via `git ls-tree` on any tracked directory
        in this repo (e.g. `git ls-tree HEAD docs`) that the mode reported is `040000 tree`, not
        `120000 blob` — confirms the mode/type branch would fire for this case without needing a
        dedicated scratch repo.
      Record the three outcomes for the PR body's test plan.
- [ ] Format and commit: this task touches no Rust or `web/` source, so there is nothing for
      `cargo fmt` or `npm run lint` to reformat — commit directly:
      `git add .github/workflows/ci.yml && git commit -m "ci: assert .claude/skills still resolves to .github/skills"`.

---

## Rule for this task

Done when: the step is present at the correct insertion point in `.github/workflows/ci.yml`, the
YAML parses, the baseline (positive) case was empirically confirmed to pass before the step was
added, and all three negative cases (deleted, wrong target, real directory) were empirically
confirmed to fail with the intended `::error::` message — with results ready to report in the PR
body, since this PR's own CI run is what proves the positive case in production (Phase 5 step 13
of the otto-factory-development skill; there is no separate "vacuous, no out-of-band surface"
claim here — CI itself *is* the surface this task touches, so its own green run is the
verification, not a substitute for it).
