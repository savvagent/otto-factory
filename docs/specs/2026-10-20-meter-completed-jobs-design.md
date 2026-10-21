# Meter completed jobs instead of charging management tools

> **Status:** DRAFT
> **Issue:** savvagent/otto-factory#222 (part of savvagent/otto-platform#55)

## Brief

From #222 (verbatim acceptance criteria):

- [ ] No tool call except a successful `complete_job` writes a billable usage event.
- [ ] `complete_job` ships exactly one `otto-factory/jobs_completed` event with quantity 1, and a
  retried or duplicate completion ships none.
- [ ] A `budget_exceeded` refusal maps to an MCP error with `code: budget_exceeded`,
  `retriable: false`.
- [ ] `whoami` and `usage` report the meter and the budget state.

Approach in the issue: move every tool in `BILLABLE` to `FREE` (keep `exhaustive_over`); add meter
`otto-factory/jobs_completed`, charged once when `complete_job` moves a job to completed, in the
same transaction; bump `otto-tenant`/`otto-resource` to the platform release with meters,
quantity and `budget_exceeded`, and declare the meter at startup; update the server instructions
and docs.

The platform half shipped in savvagent/otto-platform v0.10.0 (#84, tag commit `bfda55e`), and this
service's production slug is already set (`service=otto-factory`).

## Premise corrections

- **Two refusal reasons, not one.** v0.10.0 reports `blocked_reason` = `budget_exceeded` (an
  owner's opt-in hard stop) or `payment_required` (no card in good standing once a service's
  spend passes its free tiers; only when platform billing is on). `UsageStatus::billable_blocked()`
  covers both. This service refuses on either, with the matching code.
- **The legacy family-wide quota is gone.** v0.10.0 always serves `hard_stop: false`, so
  `UsageStatus::is_blocked()` is always false. The current enforcement (`refuse_if_spent`, the
  unshipped-billable count, `quota_exceeded`) can never fire and is removed rather than kept dead.
- **Enforcement can't stay behind `OF_ENFORCE_QUOTAS` (default off, unset in production).** An
  owner's hard stop is the owner's opt-in; if this service ignored it unless an operator also
  flipped a flag, the epic's criterion 4 would not hold. The platform's `blocked_reason` is
  enforced unconditionally (failing open), and `OF_ENFORCE_QUOTAS` is retired.

## Assumptions

- The repo has no `## Development Workflow` section. Its facts come from
  `.github/skills/otto-factory-development/SKILL.md` (conventions table) and `CLAUDE.md`, which
  wins where they disagree: commits and PR titles are conventional commits (the `pr-title` CI
  check), and production deploys only when release-please cuts a release (CLAUDE.md "Releases").
- Pin to the v0.10.0 tag commit `bfda55e155be8985761850a7c42980cc926fd4cb` for both crates.
- The meter is carried on `complete_job`'s own usage row, not a second row (see Architecture).
- Retiring `OF_ENFORCE_QUOTAS`: if it is still set, the server logs a warning that it no longer has
  any effect, rather than failing to boot (an operator's stale `.env` must not take the service
  down).
- `fail_job` is not metered (#222 risk item, owner Rob; the issue's approach names only
  `complete_job`).

## Goal & success criteria

Agents' coordination calls become free; otto-factory bills the one thing that delivers value, a
completed job, through the platform's per-service meter, and honours the platform's budget hard
stop and payment gate.

- Every MCP tool classifies `Free`; `exhaustive_over` and `every_tool_has_a_price` still pass.
- A successful `complete_job` writes exactly one outbox row carrying
  `meter = "otto-factory/jobs_completed"`, `quantity = 1`; a failed, retried or duplicate
  completion writes none (the transaction rolls back).
- The meter is declared at startup before the shipper runs.
- When the platform reports `blocked_reason`, `complete_job` is refused with MCP error
  `code: budget_exceeded` or `payment_required`, `retriable: false`; every other tool keeps
  working; a platform outage never blocks work.
- `whoami` and `usage` report the meter's quantity this month, the budget, thresholds reached and
  the blocked reason.

## Scope

**In:** `of-billing` (classify, outbox, meter/enforcement, error), `of-mcp` (charge path for
`complete_job`, error mapping, `whoami`/`usage` output and descriptions, instructions),
`of-core` migration for meter columns, `of-server` startup (declaration, retired flag),
`of-testkit` `MockPlatform` (meters endpoint, meter-aware status, budget/blocked helpers), the
dependency bump, docs (`CLAUDE.md` Metering, `README.md`, `docs/deploy/fly.md`,
`client-skills/claude-code/README.md`, `.env.example`), the web test fixture that mentions the old
fields.

**Out:** prices and the platform side (#55); metering `fail_job`; any console UI change (the
console does not consume `of_billing::Status`).

## Architecture

### Dependency bump

`Cargo.toml:32-33`: `otto-tenant` and `otto-resource` move together to rev `bfda55e…`; update the
comment above them and `Cargo.lock`. New public fields break struct literals:
`of-billing/src/outbox.rs:150` (`UsageEvent`) and `of-testkit/src/lib.rs:224,310,364`
(`UsageStatus`).

### Classification

`of-billing/src/classify.rs`: every entry of `BILLABLE` (:65-85) moves to `FREE`; `BILLABLE`
becomes empty (kept, so a future tool can still be priced). Rewrite the module doc and the unit
test `work_is_billable_and_looking_is_not` to assert everything is free.

### The meter row

- **Migration `crates/of-core/migrations/0005_usage_meter.sql`** (additive): `usage_outbox` and
  `usage_outbox_rejected` gain `meter text NULL` and `quantity bigint NULL`, with
  `CHECK ((meter IS NULL) = (quantity IS NULL))` and `CHECK (quantity IS NULL OR quantity > 0)`.
  Nullable with no default: a catalog-only change. Old code still running during rollout inserts
  rows without them (unmetered), which is correct.
- **`of_billing::METER_JOBS_COMPLETED = "otto-factory/jobs_completed"`**, plus its declaration
  (display name "Jobs completed", unit "job").
- **Recording.** Every tool already writes a usage row inside its own transaction via
  `Factory::charge` before the work. `complete_job` calls a metered variant that writes its row
  with `meter = METER_JOBS_COMPLETED`, `quantity = 1`, `billable = true`; every other row is
  `billable = false`, no meter. Because the row is in the tool's transaction, a completion that
  fails (`WrongStatus` on an already-completed job, `AlreadyClaimed`, a stale `expected_attempts`)
  rolls it back: exactly once on success, none on a retry or duplicate. Shipping is already
  idempotent per `event_id`.
- **Shipping.** `of-billing/src/outbox.rs` reads `meter`/`quantity` (claim `RETURNING`, `Row`),
  sets them on `UsageEvent` (`service: None`: the platform takes the service from the caller),
  and carries them into the dead-letter `INSERT … SELECT`.

### Declaration at startup

`of-server/src/main.rs`: between `check_platform` (:63) and the shipper spawn (:92), the shipper
task first calls `PlatformClient::declare_meters(&[jobs_completed])`, retrying with backoff on
transport or 5xx errors, and only then starts shipping. Server startup is never blocked on the
platform (CLAUDE.md: "never requires the platform"). A refused declaration (`service_not_set`, a
rejected meter) is logged at `error` and retried slowly; the outbox keeps the rows, so nothing is
lost while it waits.

### Enforcement

`of-billing/src/meter.rs`:

- Remove `refuse_if_spent`, `unshipped_billable` and `would_refuse`, and the `enforce` flag.
- A metered charge (only `complete_job`) looks up the org's status through the existing cached,
  fail-open `status_for` (2 s timeout, failure remembered 5 s) and refuses when
  `status.billable_blocked()`, with `BillingError::Blocked { reason, … }`; on lookup failure it
  allows the call. Free charges never look anything up.
- `BillingError::Blocked` maps `code()` to the platform's reason (`budget_exceeded` or
  `payment_required`; any other value → `budget_exceeded`), `retriable() = false`. The message
  says what happened, that free tools keep working, and where an owner raises the budget or adds
  a card (`PlatformClient::usage_page_url`, falling back to `OF_UPGRADE_URL`).
- `of-mcp/src/error.rs` `from_billing`: the new variant maps like `QuotaExceeded` did (envelope
  with `code` and `retriable`); `QuotaExceeded` is removed.
- `sync_ticket`'s pre-check (`would_refuse`) is removed; it is free now.
- `of-server/src/config.rs`: `OF_ENFORCE_QUOTAS` is no longer read into config; when present in
  the environment, log a warning that it is retired. `.env.example` and `fly.md` updated.

### `whoami` and `usage`

`of_billing::Status` (`meter.rs:291`) is reshaped (camelCase, JSON schema) to the new model:
`plan`, `periodStart`, `meters: [{meter, displayName, unit, quantity}]` (this service's),
`budget: {monthlyBudgetCents, spendCents, percentUsed, thresholdsReached, hardStop, blocked} |
null`, `blockedReason: string | null`, and `usagePage`. The legacy fields (`includedOps`,
`billableUsed`, `remaining`, `totalCalls`, `warning`, `hardStop`, `enforced`) are dropped: they
describe a model the platform no longer applies. This changes the `whoami`/`usage` MCP output
shape (agents read it as data; no client parses it), which is named in the PR as a deliberate
output change. Tool descriptions (`org.rs:92-97`) and the server instructions (`server.rs:82-84`)
say: coordination calls are free; a completed job is billed per the org's otto-factory meter;
`usage` shows this month's completed jobs and budget.

### MockPlatform

`of-testkit/src/lib.rs`: add `PUT /internal/meters` (records declarations, answers a receipt,
supports a `service_not_set` mode), make `usage_status` compute `meters` from shipped metered
events, and add helpers to set a budget and a `blocked_reason` per org. Literals updated for the
new fields.

## Invariants impact

- **Metering in the tool's own transaction, before the work** (CLAUDE.md Metering rule 1): kept;
  the meter row is written in the same transaction, so success-only by construction.
- **Delivery idempotent, rejected events kept** (rule 2): kept, with meter columns carried into
  the dead letter.
- **Every tool classified** (rule 3): kept; all `Free`.
- **Enforcement never blocks a read, a lookup failure never blocks work** (rule 4): kept, and
  narrowed further: only `complete_job` can be refused, and only on the platform's explicit
  `blocked_reason`.
- **Append-only migrations**: one new file, `0005`.
- **No SQL outside `of-core`**: the outbox SQL already lives in `of-billing`, as today; the new
  migration is in `of-core/migrations`.
- **Tenant isolation**: no new table; RLS on `usage_outbox` unchanged.
- **Config never falls back quietly**: a retired variable is warned about, not silently ignored.

## Migration

`0005_usage_meter.sql`, additive only (two nullable columns and two CHECKs on two tables; both
tables are small queues). Compatible both ways during rollout: the old release ignores the
columns; the new release writes them only for `complete_job`.

## Out-of-band

- **Platform slug:** done (`service=otto-factory` in production).
- **`OF_ENFORCE_QUOTAS`:** not set in `fly.toml`; nothing to remove. Note in `fly.md`.
- **Deploy:** this ships with the next release-please release (agent merges the release PR per the
  repo's process); migration `0005` applies at boot.
- No new secret, scope or config key.

## Error handling & edge cases

- **Platform down during declaration:** the shipper waits and retries; rows accumulate safely.
- **Declaration refused:** logged at error, retried slowly; operator fixes the slug.
- **Platform down during a `complete_job` lookup:** allowed (fail open) and still metered.
- **Duplicate/retried completion:** fails in `ensure_claim_held`; transaction rolls back; no row.
- **Usage shipped before declaration** cannot happen: the shipper only starts after declaring.
- **Old rows in the outbox at upgrade:** unmetered tool rows ship as before (free).

## Testing approach

Gates: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test --workspace` (Postgres 16 on 15433), `podman build -t otto-factory .`, and the web
gates (`npm run check`, `lint`, `test`, `build`) for the fixture change.

- `of-billing/src/classify.rs`: all tools free.
- `of-billing/tests/outbox.rs`: metered row ships with meter and quantity; dead letter keeps them;
  enforcement tests rewritten around `complete_job` and `blocked_reason` (budget, payment,
  outage fails open, free tools never refused).
- `of-mcp/tests/tools.rs`: `complete_job` writes exactly one metered row; a duplicate completion
  writes none; every other tool writes an unmetered free row; `budget_exceeded` and
  `payment_required` map to MCP errors with `retriable: false`; `whoami`/`usage` show the meter
  and budget; existing quota tests replaced.
- `of-server`: the shipper declares before shipping (MockPlatform records the declaration first);
  a refused declaration doesn't block startup; the retired flag warns.
- Migration: fresh cluster applies cleanly; the CHECKs hold.

## Risks & open questions

- Should `fail_job` count as delivered value? Owner: Rob (#222). Not metered here.
- The `whoami`/`usage` output shape changes. Agents read it as data; noted in the PR.
- Enforcement is now always on: a production org with a hard stop set and spend past its budget
  will have `complete_job` refused. That is the epic's intent; with no prices set, spend is zero
  and nothing is refused until prices exist.
