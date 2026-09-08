# Automatic deploy on merge to master design

> **Status:** DRAFT — a `deploy` job in `.github/workflows/ci.yml` that runs `flyctl deploy` for
> `otto-factory-mcp` after every push to `master` passes CI, closing savvagent/otto-factory#54.
> Implemented in savvagent/otto-factory#55; status will flip to IMPLEMENTED in a follow-up
> record-as-shipped PR once that PR is merged and the deploy is verified post-merge.

## Goal & Success Criteria

Issue savvagent/otto-factory#54 ("Merge to master triggers deployment") has an empty body, so the
goal below is the spec's own interpretation of the title, made explicit rather than left implicit:
**a commit landing on `master` that passes CI results in that commit running in production on
Fly.io, with no manual step.**

- A push to `master` that passes `rust`, `web`, and `docker-build` triggers a new `deploy` job in
  the same workflow run, with no separate manual command.
- The `deploy` job runs `flyctl deploy --remote-only -a otto-factory-mcp`, and its success/failure
  is visible as an ordinary GitHub Actions job status on that commit — the same place `rust`/`web`
  results already show up.
- A push to `master` that fails `rust`, `web`, or `docker-build` never reaches `flyctl deploy` —
  `needs:` skips the job outright rather than attempting a deploy and letting Fly's own rollout
  health check catch it.
- A pull request, regardless of what it touches (including this workflow file itself), never
  triggers a deploy.
- `docs/deploy/fly.md` and `.github/skills/otto-factory-development/SKILL.md`'s Repository
  Conventions table are updated so the record of "deploys are manual" is corrected the moment this
  ships — a doc that still says "manual" after this lands is itself a defect (see Scope/In).

## Assumptions

- **One workflow, one new job, not a new workflow file.** `.github/workflows/ci.yml` already runs
  `rust`, `web`, and `docker-build` on every push to `master`; adding `deploy` as a fourth job with
  `needs: [rust, web, docker-build]` reuses the existing `on.push.branches: [master]` trigger and
  gets "only deploy if CI passed" for free from GitHub Actions' default `needs` semantics (a job is
  skipped if any of its `needs` fails). A second workflow file watching the same `push` event would
  have to re-derive that ordering itself (either by polling check-run status or by duplicating the
  `rust`/`web`/`docker-build` steps), which is strictly worse.
- **`flyctl deploy --remote-only`, not the pre-built `docker-build` image.** `docker-build`
  (`docs/specs/2026-09-04-ci-docker-build-design.md`) builds with `push: false` — it proves the
  image builds and pushes nowhere, deliberately (see that spec's Scope/Out). Wiring the deploy job
  to consume that build's layer instead of letting `flyctl deploy` build remotely on Fly's own
  builder would mean either pushing to a registry Fly can pull from (new secret, new step, new
  failure mode) or passing a local image through `--local-only` (requires Docker-in-Docker on the
  runner, more fragile than Fly's hosted remote builder). `flyctl deploy --remote-only` is exactly
  what `docs/deploy/fly.md`'s manual `fly deploy -a otto-factory-mcp` already does — this change
  automates the existing manual step, not a new deploy mechanism. The tradeoff (the image is built
  twice — once to verify in `docker-build`, once for real in `deploy`) is accepted: it keeps this
  change to one job with no new secrets beyond the Fly token, and `docker-build`'s failure already
  blocks `deploy` from running via `needs:` before Fly ever attempts its own build.
- **The deploy job runs only on `push`, never on `pull_request`.** `ci.yml`'s jobs currently run on
  both events (the shared `on:` block). A PR must never deploy — `if: github.event_name == 'push'`
  on the new job is the guard, matching the existing `docker-build` job's own event-gated steps.
- **A Fly deploy token scoped to one app, not a full personal/org API token.**
  `fly tokens create deploy -a otto-factory-mcp` mints a token limited to managing
  `otto-factory-mcp` and its resources — the least-privileged credential that can run `flyctl
  deploy` for this one app, rather than an org-wide token that could also touch `nels-api` or the
  personal apps on the same account. Stored as the `FLY_API_TOKEN` repository secret via
  `gh secret set` (this workflow's `FLY_API_TOKEN` name matches `superfly/flyctl-actions`'
  documented convention, so no extra `env:` remapping is needed).
- **`superfly/flyctl-actions/setup-flyctl`, pinned to a commit SHA, then a plain `flyctl deploy`
  shell step** — the first-party action installs the CLI; the deploy step itself is one line
  (`flyctl deploy --remote-only -a otto-factory-mcp`) rather than a bespoke deploy action, keeping
  the failure mode identical to running the command by hand per `docs/deploy/fly.md`.
- **No new GitHub Environment / manual-approval gate.** The issue's literal ask is "merge to master
  triggers deployment" — an automatic, unattended deploy once CI is green, matching how the rest of
  this repo's CI already gates merges (fmt/clippy/test/docker-build all block on their own).
  Requiring a human approval click on every merge would defeat the automation the issue asks for.
  This is an infrastructure change, not one of the public interfaces Non-Negotiable Rule 6 governs
  (MCP tools, console API, OAuth/discovery, `OF_*` config, schema), so it carries no
  breaking-change documentation obligation.
- **No job-level `concurrency:` block on `deploy` — the existing top-level
  `concurrency: cancel-in-progress: true` group is the only concurrency control, left as-is.** A
  second push to `master` while an earlier workflow run (including its `deploy` job, whether queued
  or already started) is still in flight cancels that entire earlier run, because GitHub Actions
  evaluates the shared `group: ci-${{ github.workflow }}-${{ github.ref }}` the moment the new run
  starts — a job-level `concurrency:` block on `deploy` alone cannot prevent or soften that
  whole-run cancellation, since the workflow-level group governs the run as a whole and is checked
  first. Adding one would therefore be dead weight that implies a serialization guarantee ("queues
  instead of cancelling") the workflow-level group does not honor, so this spec does not add one.
  The net effect — of two rapid merges, the earlier run (deploy included) is cancelled outright and
  only the later push's `deploy` job runs to completion — is accepted as correct: it deploys the
  newest commit, matches this repository's already-established `cancel-in-progress: true`
  convention, and is what today's manual process would produce too if someone ran `fly deploy`
  twice back-to-back and killed the first with Ctrl-C on seeing a newer commit land. See Risks.
- **Deploys are unconditional on every push to `master`.** No path filter (unlike `docker-build`'s
  PR-only filter): every commit that reaches `master` already passed `rust`+`web`+`docker-build`,
  and the issue's premise is that reaching `master` is itself the deploy trigger — filtering by
  "did this touch a deploy-relevant path" would silently skip deploying a commit that, say, only
  changed a dependency inside `Cargo.lock` transitively affecting runtime behavior with no path a
  filter would recognize as "deploy-relevant." Simpler and matches the issue literally: merge to
  master deploys, full stop.

## Premise corrections

- None. The issue's premise — merging to `master` does not currently trigger any deployment,
  deploys are manual (`fly deploy -a otto-factory-mcp`, per `docs/deploy/fly.md`) — was confirmed
  directly: `.github/workflows/ci.yml` has no job invoking `flyctl`/`fly deploy` anywhere, and
  `docs/deploy/fly.md`'s "Deploying is: `fly deploy -a otto-factory-mcp`" section describes a
  command a human runs by hand.

## Scope

**In:**
- A `deploy` job appended to `.github/workflows/ci.yml`, gated to `github.event_name == 'push'`
  and `needs: [rust, web, docker-build]`, that installs `flyctl` and runs
  `flyctl deploy --remote-only -a otto-factory-mcp` authenticated via the `FLY_API_TOKEN` secret.
- Minting an app-scoped Fly deploy token (`fly tokens create deploy -a otto-factory-mcp`) and
  staging it as the `FLY_API_TOKEN` GitHub Actions secret on `savvagent/otto-factory` — an
  out-of-band credential-provisioning step, done once, alongside this PR.
- Updating `docs/deploy/fly.md` to record that deploys are now automatic on merge to `master`
  (superseding its "Deploying is: `fly deploy -a otto-factory-mcp`" manual-only framing) while
  keeping the manual command documented as the still-valid escape hatch for an out-of-band deploy
  (e.g. re-deploying without a new commit, or deploying a specific historical SHA).
- Updating `.github/skills/otto-factory-development/SKILL.md`'s Repository Conventions table
  "Deploy" row, which currently reads "No deploy automation — deploys are manual and out of
  band," since this PR is exactly that automation landing. (`CLAUDE.md` itself has no such row —
  checked directly; it has no "Deploy" table at all, only prose about the deploy pipeline's
  failure semantics scattered through other sections, none of which claims deploys are manual.)

**Out:**
- Any change to `Dockerfile`, `fly.toml`, or the `docker-build` CI job's own behavior.
- Rollback automation, deploy notifications (Slack/etc.), or a deploy status dashboard — not asked
  for, and each would be its own scoped change.
- A staging/preview-environment deploy for PRs. The issue is specifically about `master`.
- A manual-approval / GitHub Environment gate before deploying (see Assumptions).
- Passing the `docker-build` job's already-built image into the deploy step instead of letting
  `flyctl deploy` build remotely (see Assumptions) — accepted double-build cost, not engineered
  around here.
- Any change to the MCP tool surface, console API, OAuth/discovery endpoints, `OF_*` config
  surface, or database schema — this is CI/deploy infrastructure only, so none of Non-Negotiable
  Rule 6's public interfaces are touched. Checked against the three constraints in `CLAUDE.md`:
  this doesn't change what's anchored on a repo, doesn't add workflow opinion to the server, and
  doesn't touch any coding-agent-specific surface — it's pure infrastructure a customer's skill
  could never provide instead, so there's no "this belongs in a skill" question to ask.

## §1 Job definition

Appended to `.github/workflows/ci.yml`, after the existing `docker-build` job:

```yaml
  # Automates docs/deploy/fly.md's manual `fly deploy -a otto-factory-mcp`: every push to master
  # that passes rust+web+docker-build deploys for real. flyctl builds remotely on Fly's own
  # builder (--remote-only), the same path a human runs by hand -- this job doesn't reuse
  # docker-build's image (that job never pushes anywhere; see its own design spec) so the image is
  # built twice, which is an accepted tradeoff for keeping this to one job and one new secret.
  deploy:
    runs-on: ubuntu-latest
    needs: [rust, web, docker-build]
    if: github.event_name == 'push'
    timeout-minutes: 10
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@v4

      - uses: superfly/flyctl-actions/setup-flyctl@ed8efb33836e8b2096c7fd3ba1c8afe303ebbff1 # v1

      - name: Deploy to Fly.io
        run: flyctl deploy --remote-only -a otto-factory-mcp
        env:
          FLY_API_TOKEN: ${{ secrets.FLY_API_TOKEN }}
```

No job-level `concurrency:` block — see Assumptions for why one would be misleading rather than
protective here. `needs: [rust, web, docker-build]` means the job is automatically skipped — not
merely blocked — if any of those three fail, so a red `rust`/`web`/`docker-build` never reaches
`flyctl deploy`.

## §2 Credential provisioning (out-of-band, done once)

```bash
fly tokens create deploy -a otto-factory-mcp -n "github-actions-deploy" --json \
  | jq -r .token \
  | gh secret set FLY_API_TOKEN --repo savvagent/otto-factory
```

The token is piped directly from `fly tokens create` into `gh secret set`'s stdin — it is never a
literal command-line argument (so it never lands in shell history or a process list), never
written to a temp file, and never printed to a terminal. `gh secret set` itself encrypts the value
client-side against the repository's public key before it ever leaves the machine issuing the
command, per GitHub's Actions secrets API.

The minted token is scoped by Fly to `otto-factory-mcp` alone (`tokens create deploy` is
documented as "limited to managing a single app and its resources") — it cannot deploy, read, or
modify `nels-api`, `otto-factory-mcp-db`, or any other app on the `savvagent` org. If it is ever
rotated, the same pipeline replaces it — no workflow change needed.

## §3 Failure semantics

`flyctl deploy` exits non-zero on a failed build, a failed health check during the rollout, or an
auth failure against the Fly API — each surfaces as a failed `deploy` job the same way a failed
`cargo test` fails `rust`. No `continue-on-error`. A failed deploy leaves the previously-running
Fly machine serving traffic (Fly's own rolling-deploy health-check gate — `/readyz`, per
`fly.toml` — never cuts traffic to a machine that hasn't passed its health check), so a broken
deploy degrades to "the last good version keeps serving," never an outage.

## §4 Testing

- Validate the workflow YAML parses: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"`.
- This PR's own diff does not touch `Dockerfile`/`Cargo.toml`/`Cargo.lock`/`web/**`, so
  `docker-build`'s PR-time steps report success via their existing no-op path (per
  `docs/specs/2026-09-04-ci-docker-build-design.md`'s Error Handling) — expected and unrelated to
  this change.
- `deploy`'s `if: github.event_name == 'push'` guard means this PR's own CI run never attempts a
  deploy — by design, since a PR is not `master`. The first real exercise of the new job is the
  `push` event this PR's own merge commit produces; verified post-merge in Phase 5 (this PR's
  merge-commit CI run's `deploy` job must show green, and `fly status -a otto-factory-mcp` /
  `fly releases -a otto-factory-mcp` must show a new release with this merge's commit SHA).
- No `cargo test`/`clippy` impact: no Rust source changes. No `web/` impact: no console source
  changes.

## Error Handling & Edge Cases

- **A PR (not a push) touches `ci.yml`.** `deploy`'s `if: github.event_name == 'push'` skips the
  job entirely on `pull_request` — it never attempts to deploy from a PR's checkout, even one that
  edits the deploy job itself.
- **`rust`, `web`, or `docker-build` fails on a push to `master`.** `needs:` skips `deploy`
  automatically; no `flyctl deploy` is attempted against a commit that failed CI.
- **Two merges land close together.** The workflow-level `cancel-in-progress: true` group cancels
  the earlier push's entire workflow run — `deploy` included, whether it has started or not — the
  moment the later push's run begins. Only the later push's `deploy` job runs to completion. See
  Risks for why this is accepted rather than engineered around.
- **`FLY_API_TOKEN` is missing or revoked.** `flyctl deploy` fails immediately with an
  authentication error, failing the job loudly — never silently skipping the deploy attempt.

## Risks & Open Questions

- **The workflow-level `cancel-in-progress: true` group cancels an entire run, including an
  already-started `deploy` job, the instant a newer push's run begins** — there is no partial
  protection for a `flyctl deploy` that is mid-flight when this happens; GitHub Actions sends the
  cancellation signal to every job in the run, including one already executing. In practice this
  means: of several rapid merges, only the latest push's `deploy` job runs to completion, which is
  the desired outcome (deploy the newest commit, not an intermediate one) and matches this
  repository's already-accepted `cancel-in-progress: true` convention. A `flyctl deploy` killed
  mid-rollout leaves Fly's own health-check gate in control — a machine that hasn't passed
  `/readyz` never receives traffic — so the practical exposure is a cancelled CI job, not a bad
  release serving requests. Called out explicitly rather than engineered around, since building a
  merge queue or a job-level deploy lock that could shield a running deploy from cancellation is
  well outside this issue's scope.
- **Double image build cost** (see Assumptions) — `docker-build` and `flyctl deploy --remote-only`
  each build the image independently on every push to `master`. Accepted for now; if build time
  becomes a real cost, a follow-up could push `docker-build`'s image to a registry and deploy with
  `flyctl deploy --image <ref>` instead — left as a possible future change, not implemented here.
- **`superfly/flyctl-actions` is a third-party (Fly-maintained, not GitHub-first-party) action**,
  pinned to the commit SHA behind its `v1` tag
  (`ed8efb33836e8b2096c7fd3ba1c8afe303ebbff1`, annotated with a `# v1` comment for readability) —
  a stricter pin than this workflow's existing third-party actions (`Swatinem/rust-cache@v2`,
  `dorny/paths-filter@v3`, both pinned to a mutable major-version tag), chosen specifically because
  this step is the one job in the workflow that runs with the `FLY_API_TOKEN` deploy credential in
  its environment — a compromised or force-moved tag on a step holding that credential is a real
  supply-chain exposure the other jobs don't carry. `Swatinem/rust-cache@v2` and
  `dorny/paths-filter@v3` are left as major-version tags rather than retrofitted to SHAs here,
  since neither runs with a secret in scope; tightening them is a separate, unrelated change.
