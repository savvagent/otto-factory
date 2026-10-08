# Agent label validation — bound the `agent` label and quote it in peer-facing errors

Goal: refuse an `agent` label on `claim_jobs` / `acquire_lease` that is longer than 128
characters or contains control, line-break, or invisible formatting characters
(`invalid_agent_label`), and render a stored label as `agent "<label>"` (or `user <uuid>`) in the
`AlreadyClaimed` and `LeaseHeld` errors other agents read. Closes savvagent/otto-factory#163.

**Spec:** `docs/specs/2026-10-08-agent-label-validation-design.md` — read it first. This plan
implements it exactly.

## Status — 2026-10-08

✅ Shipped in savvagent/otto-factory#208, merged as `6a8c770`
(`fix(of-core)!: bound and quote the agent label on claims, leases, and messages`), closing
savvagent/otto-factory#163. Tasks 1–3 were implemented as planned. Review round 1 added work beyond these tasks, recorded in the
spec's Addendum: the policy now covers `send_message` too, a non-conforming legacy label is
withheld from every serialized job, lease, and message (`agent_label::serialize_stored`) and from
`sync_ticket`, `holder` quotes explicitly instead of with `{:?}`, the deny-list is wider, and a test
pins the "128" in the tool descriptions to `MAX_LEN`. Later Copilot rounds made `validate` trim
only the plain space, so forbidden edge characters are refused rather than stripped, and let a
keyed message stored before the policy still replay (spec Addendum items 5 and 6). That replay
exception applies only to an exact idempotency-key and fingerprint match on a row that already
exists, and the replayed message's label is still withheld (`null`) by
`agent_label::serialize_stored`. Every call that inserts is validated.

## Global Constraints

- No AI self-attribution anywhere (commits, code comments, docs, PR body).
- Run `cargo fmt --all` before every Rust commit.
- Every SQL statement lives in `of-core`. This plan adds no SQL; the validation runs in
  `of-core` before the existing statements, and the test that plants a legacy label does so
  from a `#[sqlx::test]` in `crates/of-core/tests/`.
- Errors are written for an LLM caller: stable `Error::code()`, say what the valid input is,
  never echo the refused label.
- Constraint 3: the rule is about shape only — never a list of known agents.
- No `unwrap()` outside tests.
- Tests need `podman compose up -d` (Postgres on 15433) and `.env` (`cp .env.example .env`).
- Breaking change (spec "Public interface note"): PR title `fix(of-core)!: …` with a
  `BREAKING CHANGE:` footer.

## File Structure

| File | Responsibility |
| --- | --- |
| **Create.** `crates/of-core/src/agent_label.rs` | `MAX_LEN`, `validate`, `holder`, unit tests |
| **Modify.** `crates/of-core/src/lib.rs` | `pub mod agent_label;` |
| **Modify.** `crates/of-core/src/error.rs` | `Error::InvalidAgentLabel { problem }`, code `invalid_agent_label`, not retriable |
| **Modify.** `crates/of-core/src/jobs.rs` | `claim_jobs` validates `label`; `ensure_claim_held` renders via `agent_label::holder` |
| **Modify.** `crates/of-core/src/leases.rs` | `acquire_lease` validates `label`; `LeaseHeld` renders via `agent_label::holder` |
| **Modify.** `crates/of-core/tests/queue.rs` (also holds the `acquire_lease` tests) | claim and lease refusal, trim/blank, rendered-holder, legacy-label tests |
| **Modify.** `crates/of-mcp/src/tools/jobs.rs`, `crates/of-mcp/src/tools/coord.rs` | `agent` field docs and tool descriptions |
| **Modify.** `crates/of-web/src/error.rs` | map `InvalidAgentLabel` to `400` in the console's exhaustive status match (the console never claims or leases, but the match must stay exhaustive) |
| **Modify.** `crates/of-mcp/tests/tools.rs` | end-to-end refusal surfaces `invalid_agent_label` |

## Task Order & Rationale

Task 1 builds the pure policy and error with unit tests (no database). Task 2 wires it into the
two write paths and two error renderings, against Postgres. Task 3 documents it on the MCP
surface and proves the code reaches an MCP caller. Each depends on the one before.

## Task 1 — The policy module and error ✅

**Files:** `crates/of-core/src/agent_label.rs`, `crates/of-core/src/lib.rs`,
`crates/of-core/src/error.rs`.
**Interfaces:** produces `agent_label::{MAX_LEN, validate, holder}` and
`Error::InvalidAgentLabel`.

- [x] Write unit tests in `agent_label.rs` (`#[cfg(test)]`): `None`/`""`/`"  "` → `Ok(None)`;
  `"  a  "` → `Ok(Some("a"))`; 128 chars accepted, 129 refused; 128 × `é` accepted; `\n`,
  `\r`, `\t`, `\u{0}`, `\u{7f}`, `\u{85}`, `\u{2028}`, `\u{200b}`, `\u{202e}`, `\u{feff}`,
  `\u{e0041}` each refused with code `invalid_agent_label`; the refusal's `to_string()` never
  contains the label text (use a distinctive sentinel substring); the problem names the code
  point and 1-based position; `holder(Some("a\"b"), u)` → `agent "a\"b"`; `holder(None, u)` and
  `holder(Some("x\ny"), u)` and `holder(Some(&"x".repeat(200)), u)` → `user <u>`.
- [x] Run `cargo test -p of-core --lib agent_label` — expect compile failure.
- [x] Implement `validate` and `holder` per spec §1/§3; add the variant per §2 with `code()` and
  `retriable()` arms; `pub mod agent_label;` in `lib.rs`.
- [x] Run `cargo test -p of-core --lib agent_label` — expect pass.
- [x] `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, commit
  `of-core: add agent label validation policy`.

## Task 2 — Enforce at write, render at error ✅

**Files:** `crates/of-core/src/jobs.rs`, `crates/of-core/src/leases.rs`,
`crates/of-core/tests/queue.rs`, lease tests.
**Interfaces:** consumes Task 1.

- [x] Failing tests (`crates/of-core/tests/queue.rs`): `claim_jobs` with a 129-char label →
  `invalid_agent_label`, and the job is still `pending` with `claimed_by_label` NULL;
  `claim_jobs` with `"  ci-7  "` stores `"ci-7"`; with `""` stores NULL; a peer's
  `complete_job` on a job claimed with label `ci-7` gets a message containing
  `claimed by agent "ci-7"`; a job whose `claimed_by_label` is planted by direct SQL
  (`UPDATE jobs SET claimed_by_label = $3 WHERE org_id = $1 AND id = $2`, with the value
  **bound from Rust** — a `'\n'` SQL literal is a backslash and an `n`, not a newline — once
  multi-line and once 5,000 characters long) renders `user <uuid>` and never the planted text.
- [x] Failing tests for leases (also `crates/of-core/tests/queue.rs`):
  `acquire_lease` with a label containing `\n` → `invalid_agent_label`, no lease row; a second
  user's `acquire_lease` on a held resource gets `leased by agent "ci-7"`; a holder with no label
  renders `leased by user <uuid>`.
- [x] Run `cargo test -p of-core --test queue` — expect failures.
- [x] Implement: `claim_jobs` calls `agent_label::validate(label)?` after the empty-list check and
  binds the result; `acquire_lease` does the same after its resource checks; both error sites use
  `agent_label::holder`.
- [x] Update any existing test asserting the bare-label wording.
- [x] Run `cargo test -p of-core` — expect pass. No tenant table or tenant-scoped function is
  added, so no new cross-org test is needed; the existing `isolation` suite must stay green
  (`cargo test -p of-core --test isolation`).
- [x] `cargo fmt --all`, clippy, commit `of-core: enforce agent label policy on claims and leases`.

## Task 3 — MCP surface ✅

**Files:** `crates/of-mcp/src/tools/jobs.rs`, `crates/of-mcp/src/tools/coord.rs`,
`crates/of-mcp/tests/tools.rs`.

- [x] Failing test in `crates/of-mcp/tests/tools.rs`: `claim_jobs` over MCP with a multi-line
  `agent` returns an error whose code is `invalid_agent_label` (use the suite's existing
  `code_of` helper).
- [x] Update `ClaimJobsArgs::agent` and `AcquireLeaseArgs::agent` docs and both tool descriptions
  per spec §4 (`AcquireLeaseArgs::agent` has no "Free-form." to replace — append the sentence;
  `SendMessageArgs::agent` is out of scope and stays unchanged — **superseded** by spec
  Addendum item 1: `send_message` is covered, and its `agent` doc states the rule).
- [x] `claim_jobs` handler: pass the claimed jobs' stored `claimed_by_label` (not the raw
  `args.agent`) to `sync_jobs_after_transition`, per spec §4.
- [x] File the out-of-scope follow-up issue for repo slugs/names in `RepoUnresolved`
  (`gh issue create --repo savvagent/otto-factory --label enhancement`) and record its number
  in the spec's Scope/Out and in the PR body.
- [x] Run `cargo test -p of-mcp --test tools` — expect pass.
- [x] Breaking-change step: confirm the spec's "Public interface note" records the break; the PR
  title carries `!` and the body a `BREAKING CHANGE:` footer; `docs/clients/matrix.md` needs no
  entry (no client is known to send such a label).
- [x] Out-of-band: none — no `Dockerfile`, `fly.toml`, `web/`, worker, migration, or `OF_*`
  change.
- [x] `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, commit
  `of-mcp: document the agent label limit on claim_jobs and acquire_lease`.
