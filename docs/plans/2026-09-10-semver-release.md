# SemVer release automation — implementation plan

**Spec:** `docs/specs/2026-09-10-semver-release-design.md` — read it first. This plan implements it
exactly.

## Goal

Give otto-factory one automatically-computed SemVer version, a real `CHANGELOG.md` and git tags cut
by [release-please](https://github.com/googleapis/release-please) from Conventional Commits, a
correct MCP identity/version in the `initialize` handshake, and a `deploy` job gated on a release
actually happening rather than on every push to `master`. Implements GH issue
savvagent/otto-factory#84.

## Status — 2026-09-10

⬜ Not started. Five tasks, sequential, single PR.

## Global Constraints

These hold for every task below:

- No AI self-attribution anywhere — commits, comments, docs.
- Run `cargo fmt --all` before every Rust commit.
- Every SQL statement lives in `of-core` — not touched by this plan (no SQL anywhere in it).
- Tests need a real Postgres: `podman compose up -d` (Postgres 16 on host port 15433) and a `.env`
  with `DATABASE_URL` (`cp .env.example .env`) — only Tasks 3 and 4 run Rust tests; Tasks 1, 2, 5
  are config/docs and need no database.
- This change touches no tenant table, no MCP tool's billing classification, and no migration — the
  tenant-isolation, metering, and migration invariants are not in play (confirmed in the spec's
  Scope/Out).
- `.github/workflows/ci.yml` is edited incrementally across Tasks 1 and 2; validate its YAML parses
  after each edit: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"`.

## File Structure

| File | Responsibility |
|---|---|
| **Create.** `release-please-config.json` | release-please package config (§1 of the spec) |
| **Create.** `.release-please-manifest.json` | release-please's version-tracking manifest, seeded `0.1.0` |
| **Create.** `CHANGELOG.md` | Seed file; release-please appends from here on |
| **Modify.** `.github/workflows/ci.yml` | Add `release-please` + `pr-title` jobs; gate `deploy` on `release-please`'s output |
| **Modify.** `crates/of-mcp/src/server.rs` | `get_info()` sets `server_info` to otto-factory's own name/version |
| **Modify.** `crates/of-mcp/tests/tools.rs` | Regression test for the `get_info()` fix |
| **Modify.** `crates/of-web/tests/console.rs` (or wherever the OpenAPI-document tests live — confirm at implementation time) | Regression test pinning `info.version` |
| **Modify.** `CLAUDE.md` | New "Releases & versioning" section |
| **Modify.** `docs/deploy/fly.md` | Describe the new deploy trigger |
| **Modify.** `docs/specs/2026-09-08-master-autodeploy-design.md` | One-line "Superseded in part" pointer |
| **Modify.** `docs/clients/matrix.md` | One-line note on the `server_info` fix |

## Task Order & Rationale

1. **Config + changelog seed first** — nothing downstream depends on code; establishes the files
   `ci.yml`'s new jobs will reference.
2. **CI workflow second** — wires the config from Task 1 into automation, adds the PR-title gate
   the whole scheme depends on, and re-points `deploy`. Grouped as one task (not three) because a
   partial state — `release-please` added but `deploy` not yet re-gated, or `pr-title` added before
   release-please's own PR title is guaranteed to pass it — is a broken intermediate that would
   either double-deploy or red-flag release-please's own PR the moment this branch's changes start
   landing on `master` piecemeal.
3. **MCP identity fix third** — independent of 1–2, ordered here because it is otherwise the most
   likely to be forgotten once the CI ceremony is in place and attention has moved on.
4. **OpenAPI regression test fourth** — smallest task, deliberately last among the code changes so
   it's a fast "did everything above already work" checkpoint before the docs pass.
5. **Docs last** — CLAUDE.md, fly.md, the superseded-pointer, and the matrix.md note all describe
   behavior established by Tasks 1–4; writing them first would risk documenting something that
   changed during implementation.

## Task 1 — release-please config, manifest, and changelog seed

**Files:** `release-please-config.json`, `.release-please-manifest.json`, `CHANGELOG.md`

**Interfaces:** produces the two config files `release-please-action` (Task 2) will point at.

- [ ] Create `release-please-config.json` at the repo root with exactly the content from spec §1:
      `$schema`, `bump-minor-pre-major: true`, and one package `"."` with `release-type: "simple"`,
      `changelog-path: "CHANGELOG.md"`, `pull-request-title-pattern: "chore: release ${version}"`,
      and the two `extra-files` entries (`Cargo.toml` at `$.workspace.package.version`,
      `web/package.json` at `$.version`).
- [ ] Create `.release-please-manifest.json` at the repo root: `{".": "0.1.0"}` — matches the
      current `Cargo.toml`/`web/package.json` version exactly (verify both still read `"0.1.0"`
      before committing; if either has moved since the spec was written, use the current value in
      both files and note the discrepancy in the PR body).
- [ ] Create `CHANGELOG.md` at the repo root, content exactly `# Changelog\n`.
- [ ] Validate all three parse: `python3 -c "import json; json.load(open('release-please-config.json')); json.load(open('.release-please-manifest.json'))"`.
- [ ] Note in the PR body: `web/package-lock.json`'s own root `"version"` field is not covered by
      `extra-files` and will lag `web/package.json`'s version by one `npm install` after a release —
      the same accepted, cosmetic drift the spec's Risks section already names for `Cargo.lock`.
      `npm ci` in the `web` CI job does not fail on this drift (it installs from the lockfile's
      dependency tree, not its root version field), so no action is needed beyond naming it.
- [ ] Format and commit: `git add release-please-config.json .release-please-manifest.json CHANGELOG.md && git commit -m "ci: add release-please config"`.

## Task 2 — CI: release automation jobs

**Files:** `.github/workflows/ci.yml`

**Interfaces:** consumes Task 1's config files; changes `deploy`'s trigger condition (spec §4).

- [ ] Append the `release-please` job from spec §1 after the existing `docker-build` job:
      `needs: [rust, web, docker-build]`, `if: github.event_name == 'push'`, job-level
      `permissions: { contents: write, pull-requests: write }`, `outputs.release_created` and
      `outputs.tag_name` wired from the `googleapis/release-please-action` step (pinned to
      `45996ed1f6d02564a971a2fa1b5860e934307cf7 # v5.0.0`), `with.config-file` and
      `with.manifest-file` pointing at Task 1's two files.
- [ ] Append the `pr-title` job from spec §2: `if: github.event_name == 'pull_request'`,
      `amannn/action-semantic-pull-request` pinned to the exact commit behind `v6.1.1`
      (`48f256284bd46cdaab1048c3721360e808335d50`) — not the mutable `@v6` tag; a security review
      of this PR found the job's original "never sees a secret" tag-pin reasoning assumed a repo
      setting rather than a property of the action, so it earns the same SHA-pin precedent as
      every other privileged step — with the `types`, `scopes`, `requireScope: false`, and
      `subjectPattern` values from the spec verbatim, plus `permissions: pull-requests: read` and
      `timeout-minutes: 5`.
- [ ] Change `deploy`'s `needs:` from `[rust, web, docker-build]` to `[release-please]` and its
      `if:` from `github.event_name == 'push'` to
      `needs.release-please.outputs.release_created == 'true'`. Leave every other line of `deploy`
      (the `flyctl deploy --remote-only -a otto-factory-mcp` step, its `permissions:`, its
      `timeout-minutes`) unchanged.
- [ ] Validate: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"`.
- [ ] Confirm by inspection (no local GitHub Actions runner available) that `pr-title`'s config
      would accept `release-please`'s own PR title `chore: release X.Y.Z`: type `chore` is in the
      allowed list, no scope is present and `requireScope: false`, and the subject `release X.Y.Z`
      does not start with a capital letter. Record this check in the task's commit message body or
      the PR description — it is the one thing in this task that cannot be exercised by a local
      command.
- [ ] Format and commit: `git add .github/workflows/ci.yml && git commit -m "ci: gate deploy on a release-please release, add PR title lint"`.

## Task 3 — MCP server identity fix

**Files:** `crates/of-mcp/src/server.rs`, `crates/of-mcp/tests/tools.rs`

**Interfaces:** `Factory::get_info()` (the `ServerHandler` impl) — no signature change, return
value's `server_info` field changes from the `rmcp` crate's own identity to otto-factory's.

- [ ] Write the failing test first in `crates/of-mcp/tests/tools.rs`, alongside the existing
      tool-surface tests: construct a `Factory` the same way neighboring tests do, call
      `.get_info()`, assert `server_info.name == "otto-factory"` and `server_info.version ==
      env!("CARGO_PKG_VERSION")` (evaluated in the test crate, which also resolves to the
      workspace version). Run it and confirm it fails against the current code (today it would see
      `name == "rmcp"`, `version == "2.0.0"` from `rmcp`'s own build env):
      `cargo test -p of-mcp --test tools server_info -- --nocapture` (adjust the test name to
      whatever you name it).
- [ ] Implement: in `crates/of-mcp/src/server.rs`'s `get_info()`, add
      `info.server_info = Implementation::new("otto-factory", env!("CARGO_PKG_VERSION"));` between
      constructing `info` and setting `info.instructions`. Add `Implementation` to the existing
      `use rmcp::model::{...}` import list at the top of the file.
- [ ] Run the test again and confirm it passes: `cargo test -p of-mcp --test tools`.
- [ ] Run the full crate suite to catch any other assertion on `get_info()`'s output:
      `cargo test -p of-mcp`.
- [ ] Format and commit: `cargo fmt --all && git add crates/of-mcp/src/server.rs crates/of-mcp/tests/tools.rs && git commit -m "of-mcp: report otto-factory's own name and version at initialize, not rmcp's"`.

## Task 4 — OpenAPI version regression test

**Files:** `crates/of-web/tests/console.rs` (confirm this is where the OpenAPI-document tests
actually live before writing — `grep -rn "openapi::document\|fn.*openapi" crates/of-web/tests/`; if
they live in a different file, use that one instead and note the actual path in the commit)

**Interfaces:** none produced or consumed — this task adds coverage for behavior `of-web` already
has (spec Premise corrections: `openapi.rs`'s `info.version` is already `env!("CARGO_PKG_VERSION")`).

- [ ] Add a test asserting `openapi::document()`'s `info.version` equals
      `env!("CARGO_PKG_VERSION")` — this should pass immediately since the underlying behavior is
      not new; the point is to pin it so a future edit to `openapi.rs` can't silently regress it
      back to a hardcoded string. Confirm it passes on the first run:
      `cargo test -p of-web --test console <test name>`.
- [ ] Run the full crate suite: `cargo test -p of-web`.
- [ ] Format and commit: `cargo fmt --all && git add crates/of-web/tests/console.rs && git commit -m "of-web: pin the OpenAPI document's version to the workspace version"`.

## Task 5 — Documentation

**Files:** `CLAUDE.md`, `docs/deploy/fly.md`, `docs/specs/2026-09-08-master-autodeploy-design.md`,
`docs/clients/matrix.md`

**Interfaces:** none — documentation only, describing Tasks 1–4's now-landed behavior.

- [ ] Add the "Releases & versioning" section to `CLAUDE.md`, placed after `## Style` (its current
      last section), with exactly the content from spec §5.
- [ ] Update `docs/deploy/fly.md`: add a paragraph stating deploys now happen when the
      release-please-maintained PR is merged (not on every ordinary merge to `master`), and that
      the manual `fly deploy -a otto-factory-mcp` escape hatch is unchanged. Read the file first to
      match its existing structure/tone rather than appending a mismatched block.
- [ ] Add the one-line "Superseded in part" pointer from spec §4 to
      `docs/specs/2026-09-08-master-autodeploy-design.md`'s status blockquote at the top of the
      file (immediately below its existing `> **Status:** IMPLEMENTED — ...` line) — do not change
      that Status value itself, per the spec's Premise corrections.
- [ ] Add a one-line note to `docs/clients/matrix.md` recording the `server_info` fix, phrased as a
      recorded code-level fix rather than an observed conformance-run finding (per the spec's Scope
      note) — e.g. "`server_info` now reports otto-factory's own name/version at `initialize`, not
      `rmcp`'s — fixed in this change, not re-verified against a live client as part of this run."
      Place it near the existing table or in a follow-up note section, matching the file's current
      structure.
- [ ] Format and commit: `git add CLAUDE.md docs/deploy/fly.md docs/specs/2026-09-08-master-autodeploy-design.md docs/clients/matrix.md && git commit -m "docs: record semver release automation"`.

## Final gate (after all five tasks)

- [ ] `cargo test --workspace`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo fmt --all --check`
- [ ] `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"`
- [ ] `python3 -c "import json; json.load(open('release-please-config.json')); json.load(open('.release-please-manifest.json'))"`
- [ ] No `web/` source changed by this plan, so `npm run check`/`lint`/`test` are vacuously
      satisfied — state this explicitly in the PR body rather than running them pointlessly.
