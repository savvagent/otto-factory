# Automatic deploy on merge to master — implementation plan

**Goal:** Add a `deploy` job to `.github/workflows/ci.yml` that runs
`flyctl deploy --remote-only -a otto-factory-mcp` on every push to `master` that passes `rust`,
`web`, and `docker-build`, authenticated via a newly-minted app-scoped Fly deploy token staged as
the `FLY_API_TOKEN` GitHub secret. Closes savvagent/otto-factory#54. Corrects
`docs/deploy/fly.md` and `.github/skills/otto-factory-development/SKILL.md`'s conventions table,
both of which currently describe deploys as manual.

## Status — 2026-09-08

✅ Done — implemented in this PR: Task 1 (`deploy` CI job), Task 2 (doc updates), and Task 3
(the `FLY_API_TOKEN` secret provisioned and verified via `gh secret list`). Remaining: confirming,
post-merge, that this PR's own merge-commit `push` run actually executes the `deploy` job (not
skipped) and that `fly releases -a otto-factory-mcp` shows a new release for that commit SHA — see
Out-of-band verification below.

## Spec

`docs/specs/2026-09-08-master-autodeploy-design.md` — read it first. This plan implements it
exactly.

## Global Constraints

- No AI self-attribution in the commit or PR (Non-Negotiable Rule 3).
- No SQL, no tenant table, no MCP tool, no schema change, no auth-spine change — vacuously
  satisfied; this is pure CI/deploy infrastructure.
- No change to the MCP tool surface, console API, OAuth/discovery endpoints, or `OF_*` config
  surface — Non-Negotiable Rule 6's public interfaces are untouched.
- No change to `Dockerfile`, `fly.toml`, or the existing `rust`/`web`/`docker-build` jobs'
  behavior — only a new `deploy` job is appended.
- The Fly deploy token must never appear as a literal shell argument, in a commit, in a log, or on
  a terminal — piped directly from `fly tokens create ... --json | jq -r .token` into
  `gh secret set`'s stdin, per spec §2.
- The `deploy` job must carry no job-level `concurrency:` block (per spec Assumptions — one would
  be misleading given the workflow-level `cancel-in-progress: true` group already governs the
  whole run).
- Validate workflow YAML syntax before commit (no `actionlint` available in this environment; use
  a `yaml.safe_load` parse check instead, matching the `ci-docker-build` plan's precedent).

## File Structure

| File | Responsibility |
|---|---|
| `.github/workflows/ci.yml` | **Modify.** Add the `deploy` job alongside the existing `rust`, `web`, `docker-build` jobs. |
| `docs/deploy/fly.md` | **Modify.** Replace the "Deploying is: `fly deploy -a otto-factory-mcp`" manual-only framing with a description of the automatic on-merge deploy, keeping the manual command documented as the escape hatch for an out-of-band deploy. |
| `.github/skills/otto-factory-development/SKILL.md` | **Modify.** Update the Repository Conventions table's "Deploy" row to drop "No deploy automation — deploys are manual and out of band." |

## Task Order & Rationale

Task 1 (the CI job) and Task 2 (docs) have no code dependency on each other and could be reordered,
but Task 1 first means the docs update in Task 2 can cite the exact job name/trigger it's
describing, already landed. Task 3 (credential provisioning) is out-of-band — it doesn't touch a
repo file — and must happen before Task 1's job can succeed for real on `master`, but it has no
ordering dependency on Task 1/2's commits landing first; it's listed last only because it's the
step this plan can't commit to git, and is the one most naturally verified against the actually-
merged job in Phase 5.

## Task 1 — Add the `deploy` CI job — ✅

**Files:** `.github/workflows/ci.yml`

**Interfaces:** Produces a new GitHub Actions job named `deploy` that runs on every push to
`master`, gated on `rust`/`web`/`docker-build` passing. Consumes the `FLY_API_TOKEN` repository
secret (provisioned in Task 3) and the `superfly/flyctl-actions/setup-flyctl` action, pinned to
the commit SHA behind its `v1` tag.

- [x] Add the `deploy` job to `.github/workflows/ci.yml`, placed after the existing `docker-build`
      job, exactly per spec §1:
      - `runs-on: ubuntu-latest`
      - `needs: [rust, web, docker-build]`
      - `if: github.event_name == 'push'`
      - `timeout-minutes: 10`
      - `permissions: contents: read` (the default `GITHUB_TOKEN` grant is unnecessarily broad for
        a job that only checks out code and runs `flyctl deploy`)
      - `actions/checkout@v4`
      - `superfly/flyctl-actions/setup-flyctl@ed8efb33836e8b2096c7fd3ba1c8afe303ebbff1 (v1)`
      - a "Deploy to Fly.io" step: `run: flyctl deploy --remote-only -a otto-factory-mcp`, with
        `env: FLY_API_TOKEN: ${{ secrets.FLY_API_TOKEN }}`
      - a leading comment on the job explaining why it exists and why it doesn't reuse
        `docker-build`'s image (mirrors spec §1's comment)
      - **no job-level `concurrency:` block** (per spec Assumptions/Global Constraints above)
- [x] Validate the YAML parses: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"`
      — expect no exception, and manually re-read the rendered file to confirm the `rust`, `web`,
      and `docker-build` jobs are byte-for-byte unchanged (no accidental reflow/indentation change
      from an editor).
- [x] Confirm the `flyctl deploy --remote-only -a otto-factory-mcp` invocation is syntactically
      valid against the locally installed `flyctl` (`flyctl deploy --help` to confirm `--remote-only`
      and `-a` are recognized flags) — this environment cannot run the step itself without
      `FLY_API_TOKEN` staged (Task 3), so this is a syntax/flag sanity check, not an end-to-end run.
- [x] Format and commit: `git commit -m "ci: deploy to Fly.io on every push to master that passes CI"`.
      Vacuous gates, stated explicitly: `cargo test --workspace`, `cargo clippy --all-targets --
      -D warnings`, `cargo fmt --all` do not apply (no Rust source changed);
      `cd web && npm run check && npm run lint && npm test && npm run build` does not apply (no
      console source changed).

## Task 2 — Update deploy documentation — ✅

**Files:** `docs/deploy/fly.md`, `.github/skills/otto-factory-development/SKILL.md`

**Interfaces:** No code interface — this task keeps the repo's documented-conventions record in
sync with Task 1's behavior change, per spec Goal & Success Criteria's explicit requirement that a
doc still describing deploys as "manual" after this ships is itself a defect.

- [x] In `docs/deploy/fly.md`, replace the closing "Deploying is: `fly deploy -a otto-factory-mcp`"
      paragraph with a description that: (a) states deploys now happen automatically via the
      `deploy` job in `.github/workflows/ci.yml` on every push to `master` that passes CI, (b)
      keeps `fly deploy -a otto-factory-mcp` documented as the still-valid manual escape hatch
      (e.g. re-deploying without a new commit, deploying a specific historical SHA), matching spec
      Scope/In.
- [x] In `.github/skills/otto-factory-development/SKILL.md`'s Repository Conventions table, update
      the "Deploy" row to remove "No deploy automation — deploys are manual and out of band" and
      state that pushes to `master` deploy automatically via the `deploy` CI job, keeping the
      existing Fly.io/Cloudflare Worker file references.
- [x] Format and commit: `git commit -m "docs: record automatic deploy-on-merge in fly.md and SKILL.md"`.
      Vacuous gates (no Rust/`web/` change) stated explicitly, same as Task 1.

## Task 3 — Provision the Fly deploy token (out-of-band, not a repo commit) — ✅

**Files:** none (GitHub repository secret only)

**Interfaces:** Produces the `FLY_API_TOKEN` GitHub Actions secret on `savvagent/otto-factory`,
consumed by Task 1's `deploy` job. Must exist before the first real `push`-triggered run of the
`deploy` job (i.e., before this plan's PR merges) or that run fails loudly on a missing/invalid
token per spec §3/Error Handling — acceptable but pointless to hit on the very first run when it's
avoidable by doing this task before merging.

- [x] Mint an app-scoped deploy token and stage it as the secret, per spec §2, in one pipeline with
      no intermediate file and no literal token argument:
      ```bash
      fly tokens create deploy -a otto-factory-mcp -n "github-actions-deploy" --json \
        | jq -r .token \
        | gh secret set FLY_API_TOKEN --repo savvagent/otto-factory
      ```
- [x] Verify the secret exists (name only, never the value): `gh secret list --repo
      savvagent/otto-factory` should list `FLY_API_TOKEN`.
- [x] No commit — this step has no git artifact. Record completion in the PR body's test-plan
      checklist instead.

## Out-of-band verification (Phase 5, step 14)

- **CI** — `.github/workflows/` changed: confirm the workflow parses (done in Task 1) and that the
  new `deploy` job appears in the Actions UI on this PR (reporting skipped, since `if:
  github.event_name == 'push'` never fires for a PR event — expected per spec §4/Testing, not a
  defect). The real exercise of the job is the `push` event this PR's own merge commit produces:
  after merging, confirm via `gh run list --repo savvagent/otto-factory --branch master --limit 5`
  and `gh run watch <run-id>` that the merge commit's `deploy` job ran (not skipped) and passed,
  then confirm via `fly releases -a otto-factory-mcp` (or `fly status -a otto-factory-mcp`) that a
  new release exists carrying this merge's commit SHA.
- **Config surface** — `FLY_API_TOKEN` is a new GitHub Actions secret, not an `OF_*` environment
  variable read by `Config::from_env`; `.env.example` is not touched and does not need an entry.
- **Container image / Dockerfile / fly.toml** — not modified by this change.
- **Console bundle / Cloudflare Worker / migrations** — not touched; vacuously satisfied.
