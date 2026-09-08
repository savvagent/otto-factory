# Automatic deploy on merge to master design

> **Status:** DRAFT — a `deploy` job in `.github/workflows/ci.yml` that runs `flyctl deploy` for
> `otto-factory-mcp` after every push to `master` passes CI, closing savvagent/otto-factory#54.

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
- **`superfly/flyctl-actions/setup-flyctl@master`, then a plain `flyctl deploy` shell step** — the
  first-party action installs the CLI; the deploy step itself is one line
  (`flyctl deploy --remote-only -a otto-factory-mcp`) rather than a bespoke deploy action, keeping
  the failure mode identical to running the command by hand per `docs/deploy/fly.md`.
- **No new GitHub Environment / manual-approval gate.** The issue's literal ask is "merge to master
  triggers deployment" — an automatic, unattended deploy once CI is green, matching how the rest of
  this repo's CI already gates merges (fmt/clippy/test/docker-build all block on their own).
  Requiring a human approval click on every merge would defeat the automation the issue asks for.
  This is an infrastructure change, not one of the public interfaces Non-Negotiable Rule 6 governs
  (MCP tools, console API, OAuth/discovery, `OF_*` config, schema), so it carries no
  breaking-change documentation obligation.
- **The existing top-level `concurrency: cancel-in-progress: true` group is left as-is.** A second
  push to `master` while a deploy is still running cancels the whole in-flight workflow run,
  including a not-yet-finished `deploy` job. This already matches today's manual process (nothing
  stops someone running `fly deploy` twice back-to-back), Fly's own release history serializes
  concurrent deploys to the same app on its side, and the alternative (a job-scoped `concurrency:`
  block that queues rather than cancels) would let a stale, already-superseded commit finish
  deploying after a newer one — worse than the cancellation this accepts. Recorded as a risk below,
  not engineered around.
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
- Updating `CLAUDE.md`'s "Deploy" table row, which currently reads "No deploy automation — deploys
  are manual and out of band," since this PR is exactly that automation landing.

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
    concurrency:
      group: fly-deploy-otto-factory-mcp
      cancel-in-progress: false
    steps:
      - uses: actions/checkout@v4

      - uses: superfly/flyctl-actions/setup-flyctl@master

      - name: Deploy to Fly.io
        run: flyctl deploy --remote-only -a otto-factory-mcp
        env:
          FLY_API_TOKEN: ${{ secrets.FLY_API_TOKEN }}
```

The job-level `concurrency` block is a deliberate exception to the workflow-level
`cancel-in-progress: true` group: cancelling a queued check run for a superseded commit is fine
(that's what the top-level group is for), but two overlapping `flyctl deploy` invocations against
the same app is a real race Fly's own release history would otherwise have to arbitrate, so this
job queues rather than cancels. `needs: [rust, web, docker-build]` means the job is automatically
skipped — not merely blocked — if any of those three fail, so a red `rust`/`web`/`docker-build`
never reaches `flyctl deploy`.

## §2 Credential provisioning (out-of-band, done once)

```bash
fly tokens create deploy -a otto-factory-mcp -n "github-actions-deploy"
gh secret set FLY_API_TOKEN --repo savvagent/otto-factory --body "<token from above>"
```

The minted token is scoped by Fly to `otto-factory-mcp` alone (`tokens create deploy` is
documented as "limited to managing a single app and its resources") — it cannot deploy, read, or
modify `nels-api`, `otto-factory-mcp-db`, or any other app on the `savvagent` org. It is never
echoed into a log, a commit, or this document; only the `gh secret set` invocation (run directly
against the GitHub API, value never printed to stdout) stages it. If it is ever rotated, the same
two commands replace it — no workflow change needed.

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
- **Two merges land close together.** The job-level `concurrency: cancel-in-progress: false` group
  queues the second `deploy` job behind the first rather than cancelling either — both eventually
  run, in order, against the same app. (The top-level workflow-level group still cancels the
  *rest* of the first run's now-superseded workflow if a third push arrives before the first
  workflow's `deploy` job starts — see Risks.)
- **`FLY_API_TOKEN` is missing or revoked.** `flyctl deploy` fails immediately with an
  authentication error, failing the job loudly — never silently skipping the deploy attempt.

## Risks & Open Questions

- **The workflow-level `cancel-in-progress: true` group can still cancel an entire run, including
  a `deploy` job, if a third push supersedes it before that run's `deploy` job has started** (the
  job-level `concurrency` block above only serializes already-started `flyctl deploy` invocations
  against each other; it doesn't stop the whole earlier workflow run from being cancelled pre-emptively
  by a newer push). In practice this means: of three rapid merges, the first workflow run is
  cancelled outright (including before `deploy` starts) and only the latest push's `deploy` job
  runs — which is the desired outcome (deploy the newest commit, not an intermediate one) and
  matches this repository's already-accepted `cancel-in-progress: true` convention. Called out
  explicitly rather than engineered around, since building a merge queue is well outside this
  issue's scope.
- **Double image build cost** (see Assumptions) — `docker-build` and `flyctl deploy --remote-only`
  each build the image independently on every push to `master`. Accepted for now; if build time
  becomes a real cost, a follow-up could push `docker-build`'s image to a registry and deploy with
  `flyctl deploy --image <ref>` instead — left as a possible future change, not implemented here.
- **`superfly/flyctl-actions` is a third-party (Fly-maintained, not GitHub-first-party) action**,
  pinned to `@master` per Fly's own documented usage (their action has no versioned release tags
  as of this writing) — matches how `docs/deploy/fly.md` already treats `flyctl` as the trusted,
  vendor-provided deploy tool for this exact app.
