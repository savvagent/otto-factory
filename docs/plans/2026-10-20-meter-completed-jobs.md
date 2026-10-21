# Meter completed jobs — Implementation Plan

> Execute task-by-task (fresh implementer per task, review between tasks). Steps use checkbox
> (`- [ ]`) syntax for tracking.

**Spec:** [`docs/specs/2026-10-20-meter-completed-jobs-design.md`](../specs/2026-10-20-meter-completed-jobs-design.md) — read it first.

**Goal:** every MCP tool becomes free, and a successful `complete_job` ships exactly one
`otto-factory/jobs_completed` meter event, enforced by the platform's `blocked_reason`.

**Architecture:** the usage row each tool already writes inside its own transaction carries a
meter and quantity for `complete_job` only; the outbox ships them on the platform's v0.10.0
`UsageEvent`; the meter is declared before the shipper starts; `complete_job` warms the platform
status before its transaction and is refused when `billable_blocked()`.

**Tech stack:** Rust (axum, sqlx, rmcp), Postgres 16, `otto-resource`/`otto-tenant` from
otto-platform v0.10.0.

**Ticket:** savvagent/otto-factory#222

## Status — 2026-10-20

⬜ Not started.

## Global constraints

- Worktree only (`.worktrees/billing/222-meter-completed-jobs`); trunk is `master`, reached only
  through the PR. Never edit the main checkout.
- Gates after every task, from the worktree: `cargo fmt --all -- --check`,
  `cargo clippy --all-targets -- -D warnings`,
  `DATABASE_URL=postgres://factory:factory@localhost:15433/otto_factory cargo test --workspace`
  (DB tests must run, not skip; report pass/fail/ignored counts), and
  `podman build -t otto-factory .` (the image must build with the new git dependency rev). Run
  `cargo fmt --all` before each commit.
- **Red first, every task:** after writing the failing tests, run them with the `DATABASE_URL=…`
  command above (filtered to the new tests is fine) and record that they fail for the expected
  reason before implementing. Report that red run in the task report.
- Invariants: CLAUDE.md "Metering" rules 1–4 (metering in the tool's own transaction before the
  work; idempotent delivery with a dead letter; every tool classified; enforcement never blocks a
  read and a lookup failure never blocks work), append-only migrations, tenant isolation (no new
  table), no silent fallbacks, errors written for an LLM caller. The outbox SQL in `of-billing` is
  an existing exception to "every SQL statement lives in of-core"; extend it, don't add new places.
- All interface changes are additive (spec "Public-interface classification"): keep every
  existing `Status` field, add new ones.
- Commits: conventional, `<type>(<scope>): <imperative, lower-case>` with a why-body ending
  `Refs #222`. No attribution of any kind.

## Task 1 — Pin otto-platform v0.10.0 ⬜

Model: sonnet

- [ ] In `Cargo.toml:32-33` set both `otto-tenant` and `otto-resource` to
  `rev = "bfda55e155be8985761850a7c42980cc926fd4cb"` (v0.10.0); update the comment at :27-31; run
  `cargo update -p otto-resource -p otto-tenant` so `Cargo.lock` moves both.
- [ ] Fix the struct literals the new fields break, with no behaviour change:
  `crates/of-billing/src/outbox.rs:150` (`UsageEvent`: `service: None, meter: None,
  quantity: None`) and `crates/of-testkit/src/lib.rs:224,310,364` (`UsageStatus`: `service: None,
  meters: vec![], budget: None, blocked_reason: None`, keep `features` as is).
- [ ] Run all gates; expect the baseline count (510 passed) or more, 0 failed.
- [ ] Commit `build: pin otto-platform v0.10.0`.

## Task 2 — Meter columns on the outbox ⬜

Model: sonnet

- [ ] Failing tests first (see below), run red, then: new `crates/of-core/migrations/0005_usage_meter.sql`
  (applies at boot in production; additive only): on `usage_outbox` and
  `usage_outbox_rejected` add `meter text`, `quantity bigint`, `CHECK ((meter IS NULL) = (quantity
  IS NULL))`, `CHECK (quantity IS NULL OR quantity > 0)`; header comment says why (#222).
- [ ] Failing tests first in `crates/of-billing/tests/outbox.rs`: a row enqueued with a meter ships
  a `UsageEvent` with `meter = Some(..)`, `quantity = Some(1)` (assert on `MockPlatform`'s received
  events); a rejected metered row lands in `usage_outbox_rejected` with its meter and quantity.
- [ ] `crates/of-billing/src/outbox.rs`: `enqueue` takes an `Option<&Metered>` (meter key +
  quantity) and writes the columns; the claim's `RETURNING`, `Row` (:105-113) and the `UsageEvent`
  build (:150) carry them; the dead-letter `INSERT … SELECT` (:193-203) copies them. Add
  `of_billing::METER_JOBS_COMPLETED = "otto-factory/jobs_completed"`.
- [ ] Update every other caller of `enqueue` to pass `None` (no behaviour change yet).
- [ ] Gates green; commit `feat(of-billing): carry a meter and quantity on usage rows`.

## Task 3 — Everything free; complete_job metered ⬜

Model: sonnet

- [ ] Failing tests first in `crates/of-mcp/tests/tools.rs`: a successful `complete_job` writes
  exactly one outbox row with `meter = "otto-factory/jobs_completed"`, `quantity = 1`,
  `billable = true`; a second `complete_job` on the same job fails and writes no new row;
  a `complete_job` refused for a stale `expected_attempts` or a non-holder writes no row;
  `add_job`, `claim_jobs`, `send_message`, `acquire_lease` write unmetered rows with
  `billable = false`.
- [ ] `crates/of-billing/src/classify.rs`: move every `BILLABLE` entry (:65-85) to `FREE`; keep
  `BILLABLE` (now empty) and `exhaustive_over`; rewrite the module doc and the unit test
  `work_is_billable_and_looking_is_not` to assert every tool is free.
- [ ] `crates/of-billing/src/meter.rs` + `crates/of-mcp/src/server.rs`: add a metered charge
  (`Factory::charge_metered(tx, caller, "complete_job", &Metered::jobs_completed())`) that records
  the row with the meter and `billable = true`; `complete_job` (`tools/jobs.rs:1013-1043`) uses it
  in place of `charge`. Every other tool keeps `charge` (now always free).
- [ ] Retarget every existing test that uses a formerly billable tool as its billable example to
  `complete_job` (still billable, and still under the old quota enforcement until Task 4), so the
  gates stay green at the end of this task:
  `crates/of-mcp/tests/tools.rs` `work_is_billed_and_looking_is_not` (:2190),
  `usage_is_counted_per_org` (:2379), `enforcement_stops_work_but_never_reads` (:2293),
  `with_enforcement_off_an_over_budget_org_keeps_working` (:2361), the `sync_ticket` quota tests
  (~:2703-2853; `sync_ticket` is free now, so these move to asserting it is never refused);
  `crates/of-billing/tests/outbox.rs` `a_spent_hard_stop_bucket_refuses_billable_calls_and_records_nothing`
  (:311), `unshipped_usage_counts_against_the_bucket` (:427),
  `an_outage_is_remembered_so_it_does_not_slow_every_call` (:453),
  `warm_looks_the_org_up_before_any_transaction` (:489), and any other `charge(.., "add_job")`
  there. Name each changed test and why in the commit body.
- [ ] Gates green; commit `feat(of-mcp): bill completed jobs, make coordination calls free`.

## Task 4 — Enforce the platform's blocked reason ⬜

Model: opus

- [ ] Failing tests first (`crates/of-billing/tests/outbox.rs`, `crates/of-mcp/tests/tools.rs`):
  with `MockPlatform` reporting `blocked_reason = budget_exceeded`, `complete_job` fails with MCP
  error data `code: "budget_exceeded"`, `retriable: false`, and writes no row; same for
  `payment_required`; `billable_blocked()` true with no reason → `budget_exceeded`; free tools
  (`add_job`, `claim_jobs`, `whoami`, `usage`, `watch`) keep working while blocked; a platform
  outage lets `complete_job` through (fail open) and still meters it; free tools make no status
  lookup at all (assert via `MockPlatform::usage_calls()` or the status-call counter).
- [ ] `crates/of-testkit/src/lib.rs`: helpers to set an org's `blocked_reason` and `budget`; a
  dedicated counter of `GET /internal/orgs/{org}/usage-status` calls (`status_calls()`), separate
  from `lookup_calls()` (which also counts member/team lookups); and make `usage_status` compute
  `meters` from the shipped metered events it has received (per org, this month).
- [ ] `crates/of-billing/src/meter.rs`: remove `enforce` (so `Meter::new` loses its `enforce`
  argument — callers: `crates/of-mcp/src/lib.rs:184` and every `Meter::new(.., true|false, ..)` in
  `crates/of-billing/tests/outbox.rs`), `refuse_if_spent`, `unshipped_billable`, `would_refuse`; the metered charge refuses when `status_for(org)` returns a status with
  `billable_blocked()`; `warm` always warms (it is only called for metered tools now). Rewrite the
  module doc.
- [ ] `crates/of-billing/src/error.rs`: replace `QuotaExceeded` with `Blocked { reason, fix_url }`;
  `code()` returns the reason (`budget_exceeded` | `payment_required`, unknown/none →
  `budget_exceeded`), `retriable()` false; the message says what happened, that free tools keep
  working, and that an owner raises the budget or adds a card in the otto console at `fix_url`.
  `fix_url` is `OF_UPGRADE_URL` when set, else `OF_PLATFORM_URL` (the platform console home, which
  lists the user's orgs). No org-slug lookup: the charge path has only the org id, and a lookup
  inside the transaction would break Metering rule 4. Plumb the URL into `Meter::new` (it already
  takes `upgrade_url`).
- [ ] `crates/of-mcp/src/server.rs`: `Factory::tx` (:267-271) stops calling `warm`; `complete_job`
  calls `self.meter().warm(org)` before `self.tx(...)`; delete `would_refuse` and rewrite the
  doc comments on `charge` (:150-195) to the new model.
- [ ] `crates/of-mcp/src/tools/` (`sync_ticket`): remove the `would_refuse` pre-check.
- [ ] `crates/of-mcp/src/error.rs`: `from_billing` maps `Blocked` like `QuotaExceeded` did; update
  its unit tests.
- [ ] Retire `OF_ENFORCE_QUOTAS`: remove `enforce_quotas` from `crates/of-mcp/src/lib.rs:85,124,184`,
  `crates/of-server/src/lib.rs:166,236`, `crates/of-server/src/main.rs:115`,
  `crates/of-server/tests/server.rs:44` and `config.rs`; when the variable is set, log a warning
  that it has no effect (test it). Delete or rewrite the old quota tests
  (`enforcement_stops_work_but_never_reads`, `with_enforcement_off_…`, the `sync_ticket` quota
  tests, `unshipped_usage_counts_against_the_bucket`, `a_spent_hard_stop_bucket_…`,
  `the_call_that_lands_on_the_limit_…`, `an_overage_plan_…`) — each replaced by a test of the new
  rule, named in the commit body.
- [ ] Gates green; commit `feat(of-billing): refuse completed jobs on the platform's budget or payment block`.

## Task 5 — Declare the meter before shipping ⬜

Model: sonnet

- [ ] `crates/of-testkit/src/lib.rs`: `PUT /internal/meters` route recording declarations, with
  switchable modes: `service_not_set` (409), outage (503), and "reject" (200 with the meter in
  `rejected`).
- [ ] Failing tests first, in `crates/of-billing/tests/outbox.rs` against `of_billing::outbox::run`
  (the server-test harness does not run the shipper): the shipper declares
  `otto-factory/jobs_completed` (display "Jobs completed", unit "job") before its first
  `POST /internal/usage`; in each failure mode (409, 503, 200-with-rejected) nothing ships and rows
  stay queued; once the platform recovers, it declares and drains; shutdown during the wait returns
  promptly.
- [ ] `crates/of-billing/src/outbox.rs` `run`: before the ship loop, `declare_meters` with
  exponential backoff (cap ~1 min), honouring shutdown. A declaration counts as done only when the
  call returns `Ok(receipt)` with `receipt.is_complete()`; an `Err` (transport, 5xx, 409
  `service_not_set`) or an incomplete receipt (a meter in `rejected`) is a failure: log at `error`
  (with the platform's reason) once a minute with the number of rows waiting, and retry.
  `of_billing::METER_DECLARATIONS` holds the declaration.
- [ ] Gates green; commit `feat(of-billing): declare the completed-jobs meter before shipping`.

## Task 6 — whoami and usage report the meter and budget ⬜

Model: sonnet

- [ ] Failing tests first (`crates/of-mcp/tests/tools.rs`): after a `complete_job` has shipped,
  `whoami` and `usage` include `meters: [{meter: "otto-factory/jobs_completed", displayName, unit,
  quantity: 1}]` (the mock computes it, Task 4), `budget` (null without one; populated with
  `monthlyBudgetCents`, `spendCents`, `percentUsed`, `thresholdsReached`, `hardStop`, `blocked`),
  and `blockedReason`; every existing field is still present.
- [ ] `crates/of-billing/src/meter.rs` `Status` (:292): add `meters`, `budget`, `blockedReason`
  (camelCase, JsonSchema) — no per-org usage-page URL (no slug on this path; see Task 4);
  keep the legacy fields with deprecated doc comments, and say in `enforced`'s that it now always
  reports `true` because the platform's blocked reason is always enforced.
- [ ] `crates/of-mcp/src/tools/org.rs:92-97` descriptions and `crates/of-mcp/src/server.rs:82-84`
  instructions: coordination calls are free; each completed job is billed on the org's
  otto-factory meter; `usage` shows this month's completed jobs, budget and any block. Keep
  `the_instructions_name_the_opening_moves` green.
- [ ] Gates green; commit `feat(of-mcp): report the completed-jobs meter and budget in whoami and usage`.

## Task 7 — Docs and the console fixture ⬜

Model: haiku

- [ ] Update to the new model: `CLAUDE.md` "Metering" (rule 3 wording; rule 4 now: only
  `complete_job` can be refused, on the platform's blocked reason, lookup before the transaction,
  fail open) and the startup bullets; `.github/skills/otto-factory-development/SKILL.md`
  Load-Bearing Invariant 11; `README.md` Metering; `docs/deploy/fly.md` (outbox, retired
  `OF_ENFORCE_QUOTAS`, meter declaration and its operator prerequisite `resource set-service`, done);
  `client-skills/claude-code/README.md:348,449-452`; `.env.example` (`OF_ENFORCE_QUOTAS` retired).
- [ ] `web/src/routes/o/[org]/page.render.test.ts:71-72` fixture: keep it valid (fields are
  additive; change only if a test asserts removed meaning). Run `cd web && npm run check && npm
  run lint && npm test && npm run build`.
- [ ] Gates green (Rust + web); commit `docs: describe metering by completed jobs`.

## Out-of-band

- The platform slug is already set in production (`service=otto-factory`); no operator step is
  needed for this change.
- Migration `0005` applies at boot. The change ships with the next release-please release, whose PR
  the agent merges through the normal gate.

## Known plan gaps

None yet.
