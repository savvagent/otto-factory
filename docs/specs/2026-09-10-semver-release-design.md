# SemVer release automation design

> **Status:** DRAFT — release-please-driven versioning, tagged releases, an MCP identity fix, and a
> release-gated deploy.

## Goal & Success Criteria

Issue savvagent/otto-factory#84 ("Follow SEMVER when releasing/deploying") has no body. Scope was
confirmed directly with the reporter before drafting this spec: all four of workspace/package
versions, tagged releases + CHANGELOG, MCP/console API versioning, and gating deploys on a release
step are in scope, using Conventional Commits + automation (not a manual or documentation-only
bump).

- Every crate stops being permanently frozen at `0.1.0`. The workspace carries one product version
  (crates share it today via `version.workspace = true`), computed automatically from commit
  history by [release-please](https://github.com/googleapis/release-please), following SemVer:
  a `fix` bumps patch, a `feat` bumps minor, a breaking change bumps major.
- `web/package.json`'s version stays in lockstep with the workspace version — both are the same
  product, deployed together.
- Every release produces a git tag, a GitHub Release, and a `CHANGELOG.md` entry, generated from
  Conventional Commits history — no hand-written changelog, no manual tag.
- The MCP server reports otto-factory's own name and the real product version at the `initialize`
  handshake — not `rmcp`'s crate identity, which is what it reports today (see Premise corrections).
  The console's OpenAPI document already does this correctly and gets a regression test.
- `.github/workflows/ci.yml`'s `deploy` job runs only when release-please has just cut a release
  (merged its own maintained release PR), not on every push to `master` — a deliberate, documented
  change from `docs/specs/2026-09-08-master-autodeploy-design.md`'s "every push deploys" model (see
  Premise corrections and Assumptions).
- Non-Negotiable Rule 6 (breaking-change discipline for the MCP tool surface, the console API, the
  OAuth/discovery endpoints, the config surface, and the schema) is wired to the automation: the
  same `!`/`BREAKING CHANGE:` marker a PR is already supposed to carry when it breaks a public
  interface is what release-please reads to cut a major version.

## Assumptions

- **One product version, not one version per crate.** otto-factory is deployed as a single binary
  (`of-server`) built from a private, unpublished workspace — the crates are a compile-time
  layering discipline (per `CLAUDE.md`), not independently consumed libraries. SemVer's audience
  here is the MCP/console API surface and the deploy artifact, both of which are one product. A
  per-crate version scheme would imply seven independent compatibility contracts that don't exist.
- **release-please over hand-rolled tooling or `cargo-release`.** `cargo-release` bumps versions and
  tags but does not compute the bump level from commit history or write a changelog — a human still
  decides "is this a minor or a patch," which the reporter's chosen "conventional commits +
  automation" answer rules out. release-please is purpose-built for "commits decide the version,"
  and its GitHub Action model (open/maintain a release PR, cut the release when that PR merges) is
  what makes gating deploy on "a release just happened" simple: the action's `release_created`
  output.
- **`release-type: "simple"` with explicit `extra-files`, not the `"rust"` release-type's built-in
  Cargo awareness.** The `"rust"` strategy can update per-package `Cargo.toml` versions and
  `Cargo.lock` directly, but its handling of a *workspace-inherited* version
  (`[workspace.package] version` + every member's `version.workspace = true`, this repo's actual
  shape) is strategy-internal behavior this spec cannot fully pin down without running it. `"simple"`
  with an explicit `extra-files` entry (a `toml` updater at `$.workspace.package.version`, a `json`
  updater for `web/package.json` at `$.version`) is fully specified by this document: it replaces
  exactly those two values and nothing else. See §1 and Risks for `Cargo.lock`.
- **Adopting Conventional Commits for PR titles is required, not optional, for this to work.**
  release-please's bump computation reads commit *type* (`feat`, `fix`, …) from Conventional Commits
  syntax. This repo's existing convention is `<scope>: <subject>` (scope = crate/area, no type) —
  compatible-looking but semantically different: `of-core: fix the thing` has no `type` token
  release-please can parse. Squash-merge means the PR title becomes the master commit (per
  `CLAUDE.md`/house convention), so the format has to change to `<type>(<scope>): <subject>`,
  enforced on the PR title by CI (§2), or release-please silently drops every commit it can't parse
  and no release is ever computed. This is the one genuinely disruptive part of this change — it's
  recorded in CLAUDE.md (§5) so it's discoverable, not just implied by a CI failure.
- **Breaking-change commits reuse Non-Negotiable Rule 6's existing signal.** The otto-factory
  development skill already requires a PR that makes a non-additive change to a public interface to
  flag it explicitly. `<type>(<scope>)!: <subject>` (or a `BREAKING CHANGE:` footer) is Conventional
  Commits' own way of saying the same thing, and it's what release-please reads for a major bump —
  one signal, two consumers, rather than a second convention to keep in sync.
- **Deploy is gated on `release_created`, reversing `docs/specs/2026-09-08-master-autodeploy-design.md`'s
  "every push to master deploys" model.** The reporter picked this explicitly (see Goal). The
  tradeoff is real and is named here rather than glossed over: deploys go from "every merge, within
  minutes" to "whenever the release PR is merged" — a second, deliberate action. Given this repo
  currently merges many small PRs per day (see recent git history), this is a meaningful velocity
  change, not a free lunch; it is what SemVer-gated deploys means by construction (you cannot deploy
  a numbered release before its number exists), and it was chosen with that tradeoff visible.
  §1 keeps the release PR to a fast, low-friction merge — it is auto-maintained and typically a
  one-click approval.
- **`googleapis/release-please-action` pinned to the exact commit behind its `v5.0.0` tag
  (`45996ed1f6d02564a971a2fa1b5860e934307cf7`)**, matching this repo's existing precedent
  (`docs/specs/2026-09-08-master-autodeploy-design.md`'s `superfly/flyctl-actions` pin) for any
  action running with elevated permissions — here `contents: write` + `pull-requests: write` on
  `GITHUB_TOKEN`, scoped to just the `release-please` job via a job-level `permissions:` block.
  Confirmed via the GitHub API immediately before writing this spec; v5.0.0's only breaking change
  vs v4 is a Node 24 runtime bump, no config-schema change.
- **`amannn/action-semantic-pull-request` pinned to the `v6` major tag** (not a SHA) for the PR-title
  lint — it never sees a secret (default read-only `GITHUB_TOKEN` is enough to read a PR title),
  matching this repo's existing precedent for non-privileged actions (`Swatinem/rust-cache@v2`,
  `dorny/paths-filter@v3`).
- **No branch protection change.** `master` currently has no configured branch protection
  (`gh api repos/savvagent/otto-factory/branches/master/protection` → 404, checked directly) — the
  PR-title lint (§2) reports a failed check either way, and enabling required-status-checks is a
  separate, out-of-scope decision for whoever administers the repo.
- **`Cargo.lock`'s per-crate version entries are not rewritten by this change.** Cargo does not
  fail on lockfile/manifest drift for local path dependencies unless a build passes `--locked`
  (`cargo test --workspace` in `ci.yml` does not); the next ordinary build reconciles them. Called
  out explicitly so a stale-looking `Cargo.lock` diff on a release PR isn't mistaken for a bug.

## Premise corrections

- **The MCP server does not report its own identity today — it reports `rmcp`'s.**
  `crates/of-mcp/src/server.rs`'s `get_info()` builds `ServerInfo::new(...)` and never sets
  `server_info`, so it inherits `ServerInfo::default()`'s `Implementation::from_build_env()`. That
  function is defined inside the `rmcp` crate itself and expands `env!("CARGO_CRATE_NAME")` /
  `env!("CARGO_PKG_VERSION")` in *rmcp's own* build context (confirmed by reading
  `rmcp-2.0.0/src/model.rs` directly, the exact version this workspace pins) — so every MCP client
  that completes an `initialize` handshake against otto-factory today sees `name: "rmcp"`,
  `version: "2.0.0"`, regardless of what otto-factory itself is running. This is a pre-existing
  latent defect the issue's "MCP … versioning" scope item surfaces, not something this change
  introduces.
- **`of-web`'s OpenAPI document already does this correctly.** `crates/of-web/src/openapi.rs`'s
  `info.version` uses `env!("CARGO_PKG_VERSION")` evaluated inside the `of-web` crate itself, which
  correctly resolves to the workspace version. No code change needed there — just a regression test
  (§3) so it stays correct once the version starts moving.
- **`docs/specs/2026-09-08-master-autodeploy-design.md` is not being retracted — its Status stays
  IMPLEMENTED.** It shipped and worked exactly as designed: every push to master that passed CI
  deployed. This spec changes the trigger condition going forward (§4); a one-line pointer is added
  to that spec's top matter so a reader lands on the current behavior (see §4).

## Scope

**In:**
- `release-please-config.json` + `.release-please-manifest.json` at the repo root; a
  `release-please` job in `.github/workflows/ci.yml`.
- `CHANGELOG.md`, seeded empty; populated by release-please from here on.
- A `pr-title` CI job validating PR titles against Conventional Commits syntax with this repo's
  existing scope vocabulary.
- `deploy`'s trigger condition in `ci.yml`, changed from "every push to master" to "release-please
  just cut a release."
- `crates/of-mcp/src/server.rs`: `get_info()` sets `server_info` explicitly to otto-factory's own
  name and the workspace version.
- A regression test pinning `of-web`'s OpenAPI `info.version` to `env!("CARGO_PKG_VERSION")`.
- `CLAUDE.md`: a new "Releases & versioning" section recording the commit/PR-title convention, the
  breaking-change signal, and the deploy-gating change.
- `docs/deploy/fly.md`: updated to describe the new trigger.
- A one-line pointer added to `docs/specs/2026-09-08-master-autodeploy-design.md`'s status block.

**Out:**
- Publishing any crate to crates.io, or any per-crate (rather than per-product) versioning. Nothing
  in this workspace is consumed as a library outside it (checked: `license = "UNLICENSED"` at the
  workspace level).
- A staging/preview release channel, pre-release identifiers (`-beta.1`), or a manual-approval gate
  on the release PR beyond the PR review process this repo already has. Not asked for.
- Branch protection / required status checks on `master` — a separate administrative decision (see
  Assumptions).
- Rewriting this repo's *past* commit history into Conventional Commits form, or backfilling a
  changelog for everything shipped before this change. `CHANGELOG.md` starts from here.
- Any change to `Dockerfile`, `fly.toml`, or the `docker-build` job's own behavior — deploy still
  runs the same `flyctl deploy --remote-only -a otto-factory-mcp` command; only *when* it runs
  changes.
- A version-negotiation or deprecation scheme for the MCP tool surface or console API (e.g. `/v1/`
  URL versioning, a client-sent API-version header). Not asked for, and Non-Negotiable Rule 6
  already governs breaking changes to those surfaces without one — this change makes the *product*
  version real; it does not add a second, surface-specific versioning scheme. That would be
  workflow opinion the substrate (constraint 2) has no business shipping unasked.
- `/healthz` / `/readyz` growing a version field. Neither is asked for by the issue, and
  `CLAUDE.md` is explicit that `/healthz` stays minimal and never touches anything beyond "is the
  process scheduling." Out, not because it's a bad idea, but because it's a separate, unscoped one.

## §1 Version + changelog automation

`release-please-config.json`:

```json
{
  "$schema": "https://raw.githubusercontent.com/googleapis/release-please/main/schemas/config.json",
  "bump-minor-pre-major": true,
  "packages": {
    ".": {
      "release-type": "simple",
      "changelog-path": "CHANGELOG.md",
      "extra-files": [
        { "type": "toml", "path": "Cargo.toml", "jsonpath": "$.workspace.package.version" },
        { "type": "json", "path": "web/package.json", "jsonpath": "$.version" }
      ]
    }
  }
}
```

`.release-please-manifest.json`:

```json
{
  ".": "0.1.0"
}
```

`bump-minor-pre-major: true` so a `feat` commit bumps the minor component even before `1.0.0` —
without it, release-please's default pre-1.0 behavior folds `feat` into patch-level bumps, which
would make "feature vs fix" invisible in the version number for a product that (by the reporter's
own framing) has been running as `0.1.0` for a while and isn't imminently cutting `1.0.0`.

Appended to `.github/workflows/ci.yml`, after `docker-build`:

```yaml
  release-please:
    runs-on: ubuntu-latest
    needs: [rust, web, docker-build]
    if: github.event_name == 'push'
    permissions:
      contents: write
      pull-requests: write
    outputs:
      release_created: ${{ steps.release.outputs.release_created }}
      tag_name: ${{ steps.release.outputs.tag_name }}
    steps:
      - uses: googleapis/release-please-action@45996ed1f6d02564a971a2fa1b5860e934307cf7 # v5.0.0
        id: release
        with:
          config-file: release-please-config.json
          manifest-file: .release-please-manifest.json
```

On an ordinary push to `master` (a feature/fix PR merging), this job opens or updates a
release-please-maintained PR ("chore(main): release X.Y.Z") that accumulates the pending
`CHANGELOG.md` entry and the version bump — `release_created` is `false`, nothing deploys.
When *that* PR is merged, the resulting push IS the trigger: release-please recognizes its own
commit, creates the git tag and GitHub Release, and `release_created` is `true` — the signal `deploy`
below gates on.

`CHANGELOG.md` is seeded as a single `# Changelog` line — release-please appends new sections above
whatever's already there.

## §2 PR title convention (Conventional Commits)

New CI job in `.github/workflows/ci.yml`, running on `pull_request` only (mirrors why `docker-build`
gates its filter step the same way — PR-only signal, no push-side meaning):

```yaml
  pr-title:
    runs-on: ubuntu-latest
    if: github.event_name == 'pull_request'
    steps:
      - uses: amannn/action-semantic-pull-request@v6
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        with:
          types: |
            feat
            fix
            perf
            refactor
            docs
            test
            build
            ci
            chore
            revert
          scopes: |
            of-core
            of-auth
            of-mcp
            of-billing
            of-trackers
            of-web
            of-server
            web
            docs
            ci
            release
          requireScope: false
          subjectPattern: ^(?![A-Z]).+$
```

`requireScope: false` because a repo-wide change (a workspace-level dependency bump, a CI-only
change with no natural crate) has no honest scope to claim. `subjectPattern` bans a capitalized
first letter (`fix: Broken thing` → `fix: broken thing`), matching this repo's existing lowercase
subject style (checked against recent `git log` subjects).

This is the enforcement point for the format change in Assumptions: `<type>(<scope>): <subject>` —
e.g. `fix(of-core): reap expired job claims` — or `<type>(<scope>)!: <subject>` /
a `BREAKING CHANGE:` footer for a Non-Negotiable-Rule-6 breaking change.

## §3 MCP identity + OpenAPI version regression test

`crates/of-mcp/src/server.rs`, in `get_info()`:

```rust
fn get_info(&self) -> ServerInfo {
    let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
    info.server_info = Implementation::new("otto-factory", env!("CARGO_PKG_VERSION"));
    info.instructions = Some(INSTRUCTIONS.to_string());
    info
}
```

`env!("CARGO_PKG_VERSION")` here is evaluated while compiling `of-mcp` itself (unlike the call
inside `rmcp`'s own source), so it resolves to `of-mcp`'s `Cargo.toml` — `version.workspace = true`
— i.e. the real product version. `Implementation` is already imported by way of `rmcp::model`; the
import list gains `Implementation`.

Test, alongside the existing tool-surface tests in `crates/of-mcp/tests/tools.rs`: construct a
`Factory` the way the other tests there do, call `get_info()`, assert `server_info.name ==
"otto-factory"` and `server_info.version == env!("CARGO_PKG_VERSION")`. This is a regression test
for the fix above, not new production behavior — pins the value so a future refactor of `get_info()`
can't silently regress back to the `rmcp` default.

`crates/of-web/tests/console.rs` (or wherever the existing OpenAPI-document tests live — checked at
implementation time): assert `openapi::document()`'s `info.version` equals
`env!("CARGO_PKG_VERSION")`. This is already true today; the test only pins it.

## §4 Deploy gating

`deploy`'s `needs:`/`if:` in `.github/workflows/ci.yml` change from:

```yaml
  deploy:
    needs: [rust, web, docker-build]
    if: github.event_name == 'push'
```

to:

```yaml
  deploy:
    needs: [release-please]
    if: needs.release-please.outputs.release_created == 'true'
```

`needs: [release-please]` is sufficient (not `[rust, web, docker-build, release-please]`) because
`release-please` already depends on all three — a job's transitive `needs` still has to succeed for
it to run, and `deploy` gains nothing by naming them again. The deploy step itself
(`flyctl deploy --remote-only -a otto-factory-mcp`) is unchanged: when this job runs, the checkout is
already the release-please-authored merge commit that became the new `master` tip, so "deploy the
current checkout" deploys exactly the tagged release.

`docs/specs/2026-09-08-master-autodeploy-design.md`'s status block gets one added line:

```
> **Superseded in part:** docs/specs/2026-09-10-semver-release-design.md §4 changes `deploy`'s
> trigger from "every push to master" to "release-please just cut a release." The job definition
> and failure semantics described below are otherwise unchanged.
```

`docs/deploy/fly.md` gets the equivalent one-paragraph update: deploys now happen when the
release-please-maintained PR is merged, not on every ordinary merge to `master`; the manual
`fly deploy -a otto-factory-mcp` escape hatch is unchanged.

## §5 `CLAUDE.md`: Releases & versioning

New section, placed after "## Style" (the natural home for a convention statement, matching that
section's register):

```markdown
## Releases & versioning

otto-factory is one product — a single deployed binary plus the console it serves — not a set of
independently published crates, so it carries one SemVer version, not seven. The workspace's
`[workspace.package] version` (every crate inherits it via `version.workspace = true`) and
`web/package.json`'s `version` move together.

The version, `CHANGELOG.md`, and the git tag + GitHub Release are computed automatically by
[release-please](https://github.com/googleapis/release-please) from commit history — nobody
hand-edits a version number or writes a changelog entry. Because that computation reads a commit's
*type*, not just its scope, the PR title (which becomes the squash-merge commit, per this repo's
merge convention) must be `<type>(<scope>): <subject>` — e.g. `fix(of-core): reap expired job
claims`, `feat(of-mcp): add a repo-scoped watch filter` — where `<type>` is one of `feat`, `fix`,
`perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`, `revert`, and `<scope>` is the existing
crate/area vocabulary (a crate directory name, or `web`/`docs`/`ci`/`release`), omittable for a
change with no single honest scope. A `pr-title` CI check enforces this on every PR.

**A breaking change to a public interface gets its version bump from the same signal Non-Negotiable
Rule 6 already requires you to raise.** Write `<type>(<scope>)!: <subject>` or add a `BREAKING
CHANGE: …` footer, and release-please cuts a major version from it — the same marker that tells the
architect reviewer to look hard is what tells the release automation to treat it as one.

`deploy` in `.github/workflows/ci.yml` runs only when release-please has just cut a release (i.e.
its auto-maintained "chore(main): release X.Y.Z" PR was just merged), not on every push to `master` —
see `docs/specs/2026-09-10-semver-release-design.md` §4.
```

## Error Handling & Edge Cases

- **A PR title doesn't match the convention.** `pr-title` fails that PR's checks loudly; nothing
  merges silently malformed (given repo owners follow the existing green-CI-before-merge discipline
  this repo already has — `master` itself carries no branch-protection enforcement of that, see
  Assumptions).
- **A commit reaches `master` some other way with a non-conforming subject** (an admin bypass, a
  hand-pushed hotfix). release-please's parser simply can't classify it — it's excluded from the
  changelog and does not contribute to the version bump. This fails closed (under-counts, never
  over-counts) and needs no special handling.
- **Two release PRs somehow both target the same version.** release-please always maintains exactly
  one open release PR per manifest path, force-updating it in place on every subsequent push rather
  than opening a second one — this is upstream behavior, not something this repo's config changes.
- **`release-please` fails (bad token scope, API error).** It's a normal CI job failure, visible the
  same way a failed `rust`/`web` job is; `deploy`'s `needs: [release-please]` means a failed
  `release-please` run skips `deploy` outright, never attempts a deploy with a stale `release_created`
  value.
- **A push to `master` that isn't a release-please merge** (the overwhelmingly common case — an
  ordinary feature/fix PR). `release_created` is `false`; `deploy` is skipped, not failed — same
  "skipped, not red" semantics `needs:` already gives every other gated job in this workflow.

## Risks & Open Questions

- **`Cargo.lock`'s local-crate version entries lag one build behind the workspace version** after a
  release PR merges, until the next `cargo build`/`test` run regenerates them (no `--locked` flag
  anywhere in this repo's CI or plan, see Assumptions) — cosmetic, not a build break, but worth a
  human noticing if `git diff` after a release looks unexpectedly quiet in `Cargo.lock`.
- **Deploy cadence drops from "every merge" to "every release-please PR merge."** Named explicitly
  in Assumptions as the accepted, requested tradeoff — flagged again here because it's the single
  biggest behavioral change in this spec and worth the architect reviewer's attention.
- **The `"simple"` release-type plus explicit `extra-files` was chosen over the `"rust"` strategy's
  built-in Cargo/workspace handling specifically because this spec's author could not verify the
  latter's exact behavior against this workspace's `[workspace.package]`-inherited version shape
  without running it.** If the `"rust"` strategy's workspace support turns out to be equivalent or
  better once observed on a real release PR, switching is a config-only follow-up, not a design
  change — noted so a future maintainer doesn't have to re-derive the reasoning.
- **No pre-1.0 signal beyond the version number itself.** The product has been "0.1.0" through real,
  possibly-breaking changes for a while (per this repo's own history) — nothing in this change
  retroactively marks anything as breaking. The first major bump this automation produces will be
  the first *newly authored* `!`/`BREAKING CHANGE:` commit after it ships, not a reconstruction of
  history.
