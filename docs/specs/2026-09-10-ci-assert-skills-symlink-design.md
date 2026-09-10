# CI assertion for the `.claude/skills` symlink design

> **Status:** APPROVED — assert in CI that `.claude/skills` still resolves to `../.github/skills`,
> closing savvagent/otto-factory#127. Flipped to IMPLEMENTED once the PR merges.
>
> **Revised after spec critique:** §1's script originally reconstructed `git ls-tree`'s mode/type/sha
> into a string and compared it against `"120000 blob $blob_sha"` — but `git ls-tree` appends a
> tab-separated pathname (`120000 blob <sha>\t.claude/skills`), so that comparison failed even on the
> correct, untouched tree. The fixed script below extracts and checks mode/type directly (via
> `cut -f1` + `awk`) instead of reconstructing and comparing a full line, and no longer calls
> `git rev-parse` at all (it was only feeding the now-removed reconstructed-string comparison), which
> also fixes the reviewer's second finding: `.claude/skills` deleted entirely now fails via the
> intended `::error::` message, not a mid-script `git rev-parse` abort. All four cases (current tree,
> deleted, wrong target, real directory instead of a symlink) were re-verified empirically after the
> fix — see the updated Premise corrections and Error Handling sections.

## Goal & Success Criteria

savvagent/otto-factory#124 added `.claude/skills` as a git-tracked symlink to `../.github/skills` so
Claude Code discovers this repo's skills (`.github/skills/`, the single source of truth per
`CLAUDE.md`'s Development skills section) on a fresh clone. Nothing today notices if that symlink
ever breaks or gets repointed — a later commit changing its target, or `.github/skills/` getting
renamed or moved (it has already been renamed once, per the otto-factory rename), would silently
stop Claude Code from discovering the skill, with no signal anywhere.

- A CI step fails the `rust` job (or a new lightweight job) when `.claude/skills` is not a symlink,
  or is a symlink whose target is not exactly `../.github/skills`.
- The same step fails when the symlink target directory doesn't actually contain this repo's
  development skill (`otto-factory-development/SKILL.md`) — catching a target that resolves to the
  right *path* but no longer to a directory with skill content in it (e.g. `.github/skills/` itself
  got renamed and the symlink's literal text wasn't updated to match).
- The check passes today, on the current tree, with no other changes required.
- Runs on every PR and every push to `master`, same as every other CI check in this workflow.

## Premise corrections

The issue's proposed shell assertions were checked directly against the current tree and match
reality: a `120000` (symlink) mode blob whose content is the literal text `../.github/skills`, and
`.claude/skills/otto-factory-development/SKILL.md` resolves and exists. But the issue's own
`test "$(git ls-tree HEAD .claude/skills | cut -f1)" = "120000 blob $(git rev-parse HEAD:.claude/skills)"`
form includes the `cut -f1` needed to strip `git ls-tree`'s trailing tab-separated pathname — an
earlier draft of this spec's §1 dropped that `cut -f1` and reconstructed the comparison differently,
which broke on the current, correct tree (see the Status blockquote's revision note). The current
§1 script was re-verified empirically:

```
$ git ls-tree HEAD .claude/skills
120000 blob 3e73f3a383d2de5d19f362c1be5b040015e0295c	.claude/skills
$ git cat-file blob HEAD:.claude/skills
../.github/skills
$ test -f .claude/skills/otto-factory-development/SKILL.md && echo present
present
```

## Scope

**In:**
- A new CI step asserting the symlink's git mode, its literal target text, and that
  `otto-factory-development/SKILL.md` is reachable through it.
- Placing that step in the existing `rust` job (cheapest, runs on every PR/push already) rather than
  a new job, per the issue's own "a new lightweight job" being explicitly optional wording — a
  three-line `run:` step costs less CI time and complexity than a whole new job with its own
  `actions/checkout@v4` and runner spin-up for the same three assertions.

**Out:**
- Any change to what `.github/skills/` or `.claude/skills` actually contain — this only asserts the
  existing wiring, per CLAUDE.md's "single source of truth ... never forks into per-agent copies"
  rule already in force.
- Windows checkout behavior (`core.symlinks` disabled checks the link out as a text file instead of
  a symlink). CLAUDE.md already documents that as an accepted, silent non-failure equivalent to not
  having the skill at all — not something this CI check (which runs on `ubuntu-latest`, a real POSIX
  checkout) needs to detect. Out of scope for the same reason it wasn't addressed in #123/#124: this
  workflow runs on `ubuntu-latest` only, so it has no visibility into what a Windows checkout would
  do regardless.
- Any change to other repos' skill-discovery symlinks (out of scope for this repo's CI).
- otto-factory-development's own fast-path eligibility check for this task — it is explicitly
  disqualified because it edits `.github/workflows/ci.yml` (deploy/distribution surface), so the
  full spec+plan path is used deliberately here, matching the issue's own note that this "wasn't
  done as part of #124/#123" for exactly that reason.

Checked against the three constraints in `CLAUDE.md`: this doesn't touch repo-anchored coordination,
substrate/workflow scope, or coding-agent-agnosticism — it's dev-tooling CI for this repository's own
skill-discovery wiring, not a product capability a customer's skill could implement instead.

## §1 CI step

Add a step to the `rust` job in `.github/workflows/ci.yml`, after `actions/checkout@v4` (the
earliest point a working tree exists) and before the Rust-specific steps (it needs no Rust
toolchain, so ordering it first fails fast and cheaply if it's ever going to fail):

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

- `git ls-tree` / `git cat-file` read the committed tree object directly — this checks what's
  *committed*, the same thing `verify_tenant_isolation`-style guards in this repo check state rather
  than trusting an assumption. It also works identically whether or not the runner's checkout
  actually materialized the symlink on disk (`actions/checkout@v4` on `ubuntu-latest` does, so the
  final `test -f` through the live filesystem is also exercised, but the git-object checks are the
  ones that would still catch a broken *commit* even against a checkout config that didn't
  materialize symlinks). The existence check runs first and exits before any other `git` call reads
  `HEAD:.claude/skills`, so a deleted path fails via the intended `::error::` message rather than a
  raw `fatal: path '.claude/skills' does not exist in 'HEAD'` from an unguarded `git rev-parse` or
  `git cat-file` under `set -e`.
- `::error::` annotations surface directly in the PR's Checks UI at the failing line, consistent
  with this workflow's existing convention of readable failures (the `rust` job's other steps rely
  on the underlying tool's own error output; this one has no underlying tool, so it writes its own).
- Placed in `rust` rather than a new job: the issue's own wording ("in `.github/workflows/ci.yml` or
  a new lightweight job") treats both as acceptable, and `rust` already runs unconditionally on
  every PR and push with a working checkout — a new job would duplicate `actions/checkout@v4` and
  runner startup for three assertions that together run in under a second.

## §2 Testing

No Rust or web test exercises `.github/workflows/ci.yml` — it isn't unit-testable in the normal
sense. Verification is:
- YAML validity: `gh workflow view ci.yml --repo savvagent/otto-factory` (or a local YAML parse)
  after the edit.
- The step actually running and passing on this PR's own CI run — since the symlink is intact on
  this branch (it's untouched by this change), this step is a true positive: it must be observed to
  pass in this PR's own `rust` job log, not just believed to pass from reading the YAML.
- A manual negative check performed locally (not committed) to confirm the assertions actually fail
  on a broken symlink — this repo's CI doesn't have a "test the test" harness for workflow YAML, so
  this is done by hand and reported in the PR body rather than committed as an automated meta-test.
  Performed during spec revision against: the current tree (pass), the pre-#124 commit where
  `.claude/skills` doesn't exist yet (fails on the existence check), a scratch repo with the symlink
  repointed at a different target (fails on the target check), and the current tree read as a `git
  ls-tree` mode/type check to confirm a real directory would fail the same way (`040000 tree` ≠
  `120000 blob`). All four produced the expected `::error::` message.

## Assumptions

- **`rust` job placement, not a new job.** Chosen over a dedicated job because it adds negligible
  runtime to an already-mandatory job and avoids a second `actions/checkout@v4` + runner spin-up
  purely for three assertions. If this check later grows to assert more repo-hygiene properties
  unrelated to Rust, it can be split into its own job at that time — not a concern this change needs
  to anticipate.
- **No new job means no new entry needed in `release-please`'s `needs: [rust, web, docker-build]`.**
  The check is folded into `rust`, an existing dependency, so `release-please` and `deploy`'s gating
  is unaffected.
- **`ubuntu-latest`-only scope, matching every other job in this workflow.** The Windows
  `core.symlinks` caveat in `CLAUDE.md` is a known, accepted, silent equivalent-to-absent failure
  mode on a platform this CI doesn't run on; extending coverage to Windows would need a
  `windows-latest` runner this workflow doesn't otherwise use, which is out of scope per issue #127
  and not requested.

## Error Handling & Edge Cases

- **Symlink target correct but directory empty/missing `SKILL.md`.** Caught by the third assertion —
  this is the scenario where `.github/skills/` itself got renamed or gutted without the symlink's
  literal text being updated to match, which is exactly the "silently stop Claude Code from
  discovering the skill" failure mode the issue names.
- **`.claude/skills` deleted entirely.** `git ls-tree HEAD .claude/skills` returns empty output,
  caught by the explicit `-z "$line"` check before any other `git` call runs, so the step fails with
  the intended `::error::.claude/skills does not exist in HEAD` message rather than a raw `git`
  failure from an unguarded downstream call. Verified empirically against the pre-#124 commit where
  the path didn't exist yet.
- **`.claude/skills` becomes a real directory instead of a symlink** (e.g. someone `rm`s the link and
  copies files in, forking the single-source-of-truth CLAUDE.md warns against). `git ls-tree` reports
  it as `040000 tree ...`, not `120000 blob ...`, so `mode` is `040000` and the mode/type assertion
  fails.
- **Symlink target is a different path** (e.g. repointed at a typo or a stale pre-rename path).
  Verified empirically against a scratch repo: `target` decodes to the wrong string and the second
  assertion's `::error::` fires with the actual and expected values.

## Risks & Open Questions

None outstanding — this is a narrow, additive CI-only change with no runtime or schema surface.
