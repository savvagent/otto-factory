# CI assertion for the `.claude/skills` symlink design

> **Status:** DRAFT — assert in CI that `.claude/skills` still resolves to `../.github/skills`,
> closing savvagent/otto-factory#127.

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

None — the issue's proposed shell assertions were checked directly against the current tree
(`git ls-tree HEAD .claude/skills`, `git cat-file blob HEAD:.claude/skills`) and match reality
exactly: a `120000` (symlink) mode blob whose content is the literal text `../.github/skills`, and
`.claude/skills/otto-factory-development/SKILL.md` resolves and exists.

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
    mode_and_type=$(git ls-tree HEAD .claude/skills)
    blob_sha=$(git rev-parse HEAD:.claude/skills)
    if [ "$mode_and_type" != "120000 blob $blob_sha" ]; then
      echo "::error::.claude/skills is not a symlink blob in HEAD (git ls-tree: $mode_and_type)"
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
  materialize symlinks).
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
- A manual negative check performed once locally (not committed) to confirm the assertions actually
  fail on a broken symlink, e.g. by temporarily repointing `.claude/skills` in a scratch checkout and
  running the same three `git`/`test` commands by hand — this repo's CI doesn't have a "test the
  test" harness for workflow YAML, so this is done by hand during implementation and reported in the
  PR body rather than committed as an automated meta-test.

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
- **`.claude/skills` deleted entirely.** `git ls-tree HEAD .claude/skills` returns empty output, the
  first `[ ... ]` comparison fails against the empty string, and the step fails with the first error
  message rather than a shell syntax error from an unset variable (`set -euo pipefail` plus the
  explicit `!=` comparison, not a bare command substitution assumed non-empty).
- **`.claude/skills` becomes a real directory instead of a symlink** (e.g. someone `rm`s the link and
  copies files in, forking the single-source-of-truth CLAUDE.md warns against). `git ls-tree` reports
  it as `040000 tree ...`, not `120000 blob ...`, so the first assertion fails.

## Risks & Open Questions

None outstanding — this is a narrow, additive CI-only change with no runtime or schema surface.
