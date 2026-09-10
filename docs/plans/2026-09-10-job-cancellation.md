# Job cancellation — implementation plan

Goal: let anyone with `jobs:write` ask a job to stop — immediately if it is still
`pending`, advisorily (a flag the holder must notice and act on) if it is claimed — via
two new MCP tools, `request_cancel` and `cancel_job`, closing savvagent/otto-factory#67.

**Spec:** `docs/specs/2026-09-10-job-cancellation-design.md` — read it first. This plan
implements it exactly.

---

## Status — 2026-09-10

Not started. All four tasks below are ⬜.

---

## Global Constraints

These hold for every task in this plan:

- No AI self-attribution anywhere — commits, comments, docs, PR body.
- Run `cargo fmt --all` before every Rust commit.
- Every SQL statement lives in `of-core`. A query in `of-mcp`/`of-web` is a bug.
- Tests need a real Postgres: `podman compose up -d` (Postgres 16 on host port 15433) and
  a `.env` with `DATABASE_URL` (`cp .env.example .env`).
- `cargo test` (or `cargo test --workspace`) + `cargo clippy --all-targets -- -D
  warnings` + `cargo fmt --all --check` must all be clean before a task is done.
- A migration is a new file, never an edit to one already applied. `0023`/`0024` are the
  next free numbers (`0022` is the current head).
- A new MCP tool is not done until it is classified in `of-billing::classify` —
  `every_tool_has_a_price`/`exhaustive_over` enforce this.
- A tenant-scoped function is not done without a cross-org negative test — see the spec's
  §1/tenant-isolation reasoning for why no *new* RLS policy registration is needed here
  (new columns on an already-covered table, not a new table).
- Anything touching `web/` adds `npm run check` (svelte-check + tsc; runs
  `scripts/check-messages.mjs` first) and `npm run lint` (prettier) to its gate.
- No console route changes anywhere in this plan — every write this feature adds is an
  MCP tool (spec §1).

## File Structure

| File | Responsibility |
|---|---|
| **Create.** `crates/of-core/migrations/0023_job_cancelled_status.sql` | Adds `'cancelled'` to `job_status`. |
| **Create.** `crates/of-core/migrations/0024_job_cancellation_columns.sql` | Adds `cancel_requested_at`/`cancel_requested_by`/`cancel_reason` to `jobs`. |
| **Modify.** `crates/of-core/src/jobs.rs` | `Status::Cancelled`, `Job`'s three new fields, `Tx::request_cancel`, `Tx::cancel_job`, `Stats.cancelled`, `repend_job` clearing the new fields. |
| **Modify.** `crates/of-core/src/audit.rs` | `JOB_CANCEL_REQUESTED`/`JOB_CANCELLED` action constants. |
| **Modify.** `crates/of-core/tests/queue.rs` | New-behavior tests (see Task 1). |
| **Modify.** `crates/of-core/tests/isolation.rs` | Cross-org negative test for the two new `Tx` methods. |
| **Modify.** `crates/of-mcp/src/tools/jobs.rs` | `request_cancel`/`cancel_job` tools, `sync_ticket`'s new `Cancelled` match arm, tool-description updates. |
| **Modify.** `crates/of-billing/src/classify.rs` | Both new tools added to `BILLABLE`. |
| **Modify.** `crates/of-mcp/tests/tools.rs` | End-to-end tool tests, tool-list/billing assertions. |
| **Modify.** `crates/of-web/src/openapi.rs` | `Job`/`QueueStats` schema additions, status enum, schema-assertion tests. |
| **Modify.** `crates/of-web/src/routes/jobs.rs` | `ListJobsQuery.status` doc comment. |
| **Modify.** `web/src/lib/types.ts` | `JobStatus`/`QueueStats` gain `cancelled`. |
| **Modify.** `web/src/lib/labels.ts` | `statusLabel` switch gains the `cancelled` arm. |
| **Modify.** `web/messages/{en,es,de,fr,it,hi}.json` | `status_cancelled` key, all six. |
| **Modify.** `web/src/lib/components/StatusPill.svelte` | `tones` gains `cancelled`. |
| **Modify.** `web/src/routes/o/[org]/queue/+page.svelte` | `STATUSES` gains `'cancelled'`. |

## Task Order & Rationale

1 → 2 → 3 → 4, strictly sequential: `of-core` (Task 1) is the foundation everything else
compiles against; `of-mcp`+`of-billing` (Task 2) is the first consumer and cannot compile
until Task 1's `Status::Cancelled`/`Tx` methods exist; `of-web` (Task 3) only needs the
`Job` struct's new fields (from Task 1) to serialize them into its hand-written OpenAPI
schema, but doing it after Task 2 keeps the "new status value" story told in one
direction (domain → MCP surface → console API → UI) rather than jumping around; `web/`
(Task 4) is last because it is presentational only and depends on nothing upstream
breaking.

---

## Task 1 — `of-core`: the `Cancelled` status and its two transitions

**Files:** `crates/of-core/migrations/0023_job_cancelled_status.sql` (create),
`crates/of-core/migrations/0024_job_cancellation_columns.sql` (create),
`crates/of-core/src/jobs.rs`, `crates/of-core/src/audit.rs`, `crates/of-core/tests/queue.rs`,
`crates/of-core/tests/isolation.rs`.

**Interfaces produced:** `Status::Cancelled`, `Job.cancel_requested_at/_by`,
`Job.cancel_reason`, `Tx::request_cancel`, `Tx::cancel_job`, `Stats.cancelled`,
`audit::action::JOB_CANCEL_REQUESTED`/`JOB_CANCELLED`. Consumed by Task 2.

- [ ] Write `crates/of-core/migrations/0023_job_cancelled_status.sql` exactly as spec §2
      (single `ALTER TYPE job_status ADD VALUE 'cancelled';`, its own file — Postgres will
      not let this value be used in the same migration transaction it was added in).
- [ ] Write `crates/of-core/migrations/0024_job_cancellation_columns.sql` exactly as spec
      §2 (three nullable columns, `cancel_requested_by` a `REFERENCES users (id) ON DELETE
      SET NULL`).
- [ ] Run `cargo test -p of-core` once with no other code changes yet, purely to prove a
      fresh throwaway database migrates cleanly through both new files in order. Expect:
      passes (nothing references `'cancelled'` yet, so no compile/runtime break possible).
- [ ] In `crates/of-core/tests/queue.rs`, write the failing tests first (they will not
      compile until the `jobs.rs` changes below land — that is expected and fine for this
      failing-test-first step, but note the compile failure in your own working notes
      rather than treating it as a test failure to route around):
      - `request_cancel` on a `pending` job finalizes immediately to `cancelled`, with all
        three new fields set and `completed_at` set, `is_terminal()` true.
      - `request_cancel` on `in-progress`/`active` sets the three fields, leaves status
        unchanged; a second call re-stamps them (assert the second call's
        `cancel_requested_at` differs from the first, or at least does not error).
      - `request_cancel` on `completed`/`failed`/`cancelled` returns `WrongStatus` naming
        `"pending, in-progress, or active"`.
      - `cancel_job` after `request_cancel` on a claimed job succeeds: `status ==
        Cancelled`, `completed_at` set, `error` equals the `note` passed.
      - `cancel_job` on a claimed job with no prior `request_cancel` returns
        `Error::Invalid`.
      - `cancel_job` on a `pending` job returns `WrongStatus` naming `"in-progress or
        active"`.
      - `repend_job` on a `cancelled` job succeeds and clears all three cancellation
        fields; `cancel_job` on the freshly-reclaimed job then fails with
        `Error::Invalid` (no request on file for this attempt).
      - `stats()` reports the new `cancelled` counter and
        `pending + in_progress + active + completed + failed + cancelled == total`.
- [ ] Run `cargo test -p of-core --test queue` — expect compile failure (the new `Status`
      variant, fields, and `Tx` methods do not exist yet).
- [ ] Implement `crates/of-core/src/jobs.rs` per spec §3: `Status::Cancelled` variant;
      `as_str`/`FromStr` gain the `"cancelled"` arm (update `FromStr`'s error message to
      list all six values); `is_terminal` includes `Cancelled`; `Job` gains the three
      fields (with the doc comment on `cancel_requested_by` from spec §3 — this is the
      field an LLM caller reads to know what to do, per the spec's Assumptions); `JOB_COLS`
      gains the three columns; `Tx::request_cancel` and `Tx::cancel_job` exactly as spec §3;
      `repend_job`'s `UPDATE` clears the three fields and its `WrongStatus.expected` string
      gains `"or cancelled"`; `Stats.cancelled` field and its query arm.
- [ ] Run `cargo test -p of-core --test queue` — expect all pass.
- [ ] In `crates/of-core/src/audit.rs`, add the `// Jobs.` section to `pub mod action`
      with `JOB_CANCEL_REQUESTED`/`JOB_CANCELLED` per spec §5. No test needed here in
      isolation — Task 2's tool-level tests exercise these constants end-to-end.
- [ ] In `crates/of-core/tests/isolation.rs`, extend `cross_org_mutation_is_refused`
      (guard-1 proof) with `request_cancel` and `cancel_job` called from org `b` against
      org `a`'s job, both asserted `.is_err()`, following the existing pattern in that
      test exactly (see the function body already calling `update_job`/`delete_job`/
      `claim_jobs`/`repend_job` the same way).
- [ ] Run `cargo test -p of-core --test isolation` — expect all pass, including the two
      new assertions.
- [ ] `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --all`.
- [ ] Commit: `git commit -m "of-core: add job cancellation (request_cancel, cancel_job, Cancelled status)"`.

## Task 2 — `of-mcp` + `of-billing`: the two new tools

**Files:** `crates/of-mcp/src/tools/jobs.rs`, `crates/of-billing/src/classify.rs`,
`crates/of-mcp/tests/tools.rs`.

**Interfaces consumed:** everything from Task 1. **Interfaces produced:** MCP tools
`request_cancel`, `cancel_job` (both returning `out::JobOut`, no new envelope type).

- [ ] In `crates/of-mcp/tests/tools.rs`, write the failing tests first:
      - End-to-end: `add_job` → `request_cancel` (job still `pending`) → resulting job has
        `status: "cancelled"`.
      - End-to-end: `add_job` → `claim_jobs` → `request_cancel` → job still
        `"in-progress"` with `cancelRequestedAt`/`cancelRequestedBy`/`cancelReason` set →
        `cancel_job` → job `"cancelled"`.
      - `cancel_job` on a claimed job with no `request_cancel` call returns an error.
      - Both tools appear in the tool list: extend `the_advertised_surface_is_exactly_what_the_design_specifies`'s
        (`crates/of-mcp/tests/tools.rs`) hardcoded `expected` vec with `"request_cancel"`,
        `"cancel_job"` in the `// Jobs` section, alongside `"activate_job"` — this is the
        test that fails once the router gains new tools and nothing else names them.
        (`every_tool_documents_itself` needs no change — it loops generically over the
        router and asserts every tool it finds has a description; it has no name list.)
      - Extend `work_is_billable_and_looking_is_not` (`crates/of-billing/src/classify.rs`)
        with both new tool names in its billable-tools list, matching how `"activate_job"`
        was added there for the prior status-value change. (`every_tool_has_a_price` needs
        no manual edit — it builds its tool list from the live router automatically and
        calls `classify::exhaustive_over`, so it already covers any correctly-classified
        new tool with zero list maintenance.)
- [ ] Run `cargo test -p of-mcp --test tools` — expect compile failure (tools do not exist
      yet) or, if it compiles against stubs, straightforward assertion failures.
- [ ] Implement `RequestCancelArgs`/`CancelJobArgs` and the `request_cancel`/`cancel_job`
      tools in `crates/of-mcp/src/tools/jobs.rs` exactly as spec §4, including:
      - `use of_core::audit::{action, Entry};` (new import — this is the first `of-mcp`
        call site into the audit trail; every existing one is in `of-web`).
      - The audit-write ordering inside the same transaction as the state change (both
        `tx.audit(...)` calls happen before `tx.commit()`), matching spec §4's code
        exactly, including writing both `JOB_CANCEL_REQUESTED` and `JOB_CANCELLED` when
        `request_cancel` finalizes a pending job immediately.
      - The post-commit `sync_jobs_after_transition(..., JobTransition::Failed,
        Some(&detail))` calls in both tools, with the detail-string fallback chains from
        spec §4 (never falling back to the literal `"Failed."` default for a cancelled
        job).
      - `sync_ticket`'s match over `job.status` gains the `Status::Cancelled` arm from
        spec §4.
      - `require_scope(scope::JOBS_WRITE)` on both new tools, matching every other
        job-mutating tool.
- [ ] In `crates/of-billing/src/classify.rs`, add `"request_cancel"` and `"cancel_job"` to
      `BILLABLE` (spec §6). Extend `work_is_billable_and_looking_is_not`'s tool list with
      both if that test enumerates specific tool names (check the existing test body — if
      it is a hardcoded list, extend it in place rather than only relying on
      `exhaustive_over`).
- [ ] Run `cargo test -p of-mcp --test tools` and `cargo test -p of-billing` — expect all
      pass, including `every_tool_has_a_price`/`exhaustive_over` (this will fail loudly if
      `of-mcp`'s actual tool router and `of-billing`'s price list disagree — resolve by
      making sure both new tool names are spelled identically in both crates).
- [ ] Update tool descriptions per spec §4: `repend_job`'s description text ("Return a
      completed, failed, or cancelled job to pending"), `ListJobsArgs.status`/`list_jobs`/
      `stats` doc comments and descriptions listing `cancelled` among valid states.
- [ ] `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --all`.
- [ ] Commit: `git commit -m "of-mcp: add request_cancel and cancel_job tools"`.

## Task 3 — `of-web`: schema and doc-comment updates

**Files:** `crates/of-web/src/openapi.rs`, `crates/of-web/src/routes/jobs.rs`.

**Interfaces consumed:** `Job`'s new fields (Task 1), passed through unchanged —
`routes/jobs.rs` returns `of_core::jobs::Job` directly, so no route code changes.

- [ ] In `crates/of-web/src/openapi.rs`'s test module (wherever the existing `"active"`
      schema assertions live, per the job-active-state precedent — find them near the
      `status_enum`/`QueueStats` checks already shown in the spec's §9), write the failing
      assertions first: the `Job` status enum contains `"cancelled"`; the `Job` schema's
      `properties` contains `cancelRequestedAt`/`cancelRequestedBy`/`cancelReason`; the
      `QueueStats` schema's `properties` and `required` both contain `"cancelled"`.
- [ ] Run the relevant `cargo test -p of-web` test — expect failure (schema literal not
      yet updated).
- [ ] Update `queue_schemas()` in `openapi.rs` exactly as spec §7: the three new `Job`
      properties, the status enum's `"cancelled"` entry, `QueueStats`'s `"cancelled"`
      property and `required` entry.
- [ ] Update `routes/jobs.rs`'s `ListJobsQuery.status` doc comment to list `cancelled`.
- [ ] Run the `cargo test -p of-web` schema tests — expect pass. Then run
      `the_queue_is_read_only_over_the_console` explicitly (it should already pass
      unchanged, since this task adds no route — confirm rather than assume) and
      `cargo test -p of-web` in full.
- [ ] `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --all`.
- [ ] Commit: `git commit -m "of-web: surface job cancellation fields in the console schema"`.

## Task 4 — `web/`: console presentation

**Files:** `web/src/lib/types.ts`, `web/src/lib/labels.ts`,
`web/messages/{en,es,de,fr,it,hi}.json`, `web/src/lib/components/StatusPill.svelte`,
`web/src/routes/o/[org]/queue/+page.svelte`.

**Interfaces consumed:** the `cancelled` status value and `cancelled` stats counter,
already flowing through the read-only console API from Task 3 — this task is purely
presentational, no new API calls.

- [ ] In `web/messages/en.json`, add `"status_cancelled": "Cancelled"` immediately after
      the existing `"status_failed"` key. Add the equivalent translated string (matching
      each locale's existing register for the other four `status_*` values) to
      `es.json`, `de.json`, `fr.json`, `it.json`, `hi.json`. This step must land before the
      TypeScript changes below or `npm run check`'s `scripts/check-messages.mjs` gate has
      nothing to validate against yet in a way that would surface a *missing-key* failure
      distinct from a *missing-usage* one — do it first so the later steps' failures are
      about `m.status_cancelled` not existing as a generated function, not about the
      catalog being incomplete.
- [ ] Run `cd web && npm run check` — expect it to still pass at this point (an unused
      catalog key is not an error).
- [ ] In `web/src/lib/types.ts`: `JobStatus` gains `'cancelled'`; `QueueStats` gains
      `cancelled: number`.
- [ ] Run `npm run check` — expect failures at every place TypeScript now requires a
      `'cancelled'` case: `StatusPill.svelte`'s `tones` (a `Record<JobStatus, string>`)
      and `labels.ts`'s `statusLabel` switch, at minimum. Note every location `tsc`
      reports.
- [ ] Fix each reported location:
      - `web/src/lib/labels.ts`: add `case 'cancelled': return m.status_cancelled();` to
        `statusLabel`'s switch.
      - `web/src/lib/components/StatusPill.svelte`: add `cancelled: 'border-faint/40
        bg-faint/10 text-muted'` to `tones` (same tone family as `pending`, per spec §8's
        reasoning — a cancellation is deliberate and calm, not one of the "needs
        attention" or "succeeded" tones).
      - Any other location `npm run check` names (e.g. a test fixture constructing a
        `QueueStats` literal) — add `cancelled: 0` or the appropriate value.
- [ ] In `web/src/routes/o/[org]/queue/+page.svelte`, add `'cancelled'` to the end of the
      `STATUSES` array.
- [ ] Run `npm run check && npm run lint && npm test` — expect all pass. `npm test`
      should show no behavior change (no Worker routing touched) — treat any failure
      there as a regression to fix, not an expected diff.
- [ ] Commit: `git commit -m "web: recognize the cancelled job status"`.

---

## Rule for every task

A task is done when its own tests pass, `cargo clippy --all-targets -- -D warnings` is
clean, and `cargo fmt --all --check` is clean (Tasks 1–3), or `npm run check && npm run
lint` is clean (Task 4). Tenant-scoped functions are not done without a cross-org
negative test (Task 1). A new MCP tool is not done without a `of-billing::classify` entry
(Task 2).
