# Idempotency key for add_job and send_message — implementation plan

**Spec:** [`docs/specs/2026-09-10-idempotency-key-design.md`](../specs/2026-09-10-idempotency-key-design.md) —
read it first. This plan implements it exactly.

Goal: `add_job` and `send_message` accept an optional caller-supplied `idempotencyKey`.
Replaying a call with the same key, org, and payload returns the original row unchanged and
is not billed a second time; replaying a key with a different payload errors loudly; keys
are enforced org-scoped by a partial unique index, not a read-then-write; omitting the key
reproduces today's behavior exactly, at zero extra cost. Closes savvagent/otto-factory#68.

## Status — 2026-09-10

Not started. All five tasks below are ⬜.

## Global Constraints

These hold for every task:

- No AI self-attribution anywhere (commits, PR body, comments).
- Run `cargo fmt --all` before every Rust commit.
- Every SQL statement lives in `of-core` — nothing in `of-mcp` issues SQL directly.
- Tests need `podman compose up -d` and a `.env` (`cp .env.example .env`) — a real Postgres,
  no mocks.
- Per-task gate: `cargo test --workspace` (or the narrower `-p`/`--test` command named in
  the task, run first for speed, full workspace test before the task's commit),
  `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all`.
- Tenant isolation: `jobs` and `messages` are already registered tenant tables in
  `0007_rls.sql`; the new columns this plan adds need no new RLS policy (Load-Bearing
  Invariant 1 is about new *tables*), but every new `of-core` function still takes its
  scoping from `Tx`/`self.org()` like every existing one, and Task 2/3 each add a cross-org
  negative test proving guard 1 holds for the new code paths.
- Metering: `add_job` and `send_message` are already classified `Billable` in
  `of-billing::classify` — no new tool is added, so no new classify entry is needed. What
  changes is *when* `charge` runs relative to the rest of each handler; Task 4/5 update
  `Factory::charge`'s doc comment in the same commit that changes the ordering, per the
  spec's §5.
- The fingerprint functions (Task 2, Task 3) MUST destructure `NewJob`/`NewMessage` by name
  with no `..` catch-all, exactly as the spec's §4/§6 show — this is a compile-time
  guardrail against silent fingerprint drift when either struct gains a field later, not a
  style preference.
- No public interface is renamed or removed (Non-Negotiable Rule 6 does not apply as a
  breaking-change case here) — this is purely additive: one new optional argument on two
  existing tools, two new nullable columns on two existing tables.

## File Structure

| File | Responsibility |
|---|---|
| `crates/of-core/migrations/0025_idempotency_keys.sql` | **Create.** Two nullable columns + one partial unique index, on both `jobs` and `messages`. |
| `crates/of-core/src/idempotency.rs` | **Create.** Shared key validation + payload fingerprinting, used by `jobs.rs` and `messages.rs`. |
| `crates/of-core/src/lib.rs` | **Modify.** Register the new `idempotency` module. |
| `crates/of-core/Cargo.toml` | **Modify.** Add `sha2.workspace = true`. |
| `crates/of-core/src/jobs.rs` | **Modify.** `NewJob.idempotency_key`, `job_idempotency_fingerprint`, `Tx::find_replayed_job`, `Tx::add_job`'s insert. |
| `crates/of-core/src/messages.rs` | **Modify.** `NewMessage.idempotency_key`, `message_idempotency_fingerprint`, `Tx::find_replayed_message`, `Tx::send_message`'s insert. |
| `crates/of-core/src/error.rs` | **Modify.** `Error::IdempotencyKeyConflict`, its `code()`/`retriable()` arms. |
| `crates/of-core/tests/queue.rs` | **Modify.** New tests for both tools' idempotency behavior. |
| `crates/of-core/tests/isolation.rs` | **Modify.** Cross-org negative coverage for the new columns/index. |
| `crates/of-mcp/src/server.rs` | **Modify.** `Factory::charge`'s doc comment gains the third, distinctly-framed exception. |
| `crates/of-mcp/src/tools/jobs.rs` | **Modify.** `AddJobArgs.idempotency_key`, `add_job` handler restructure, tool description. |
| `crates/of-mcp/src/tools/coord.rs` | **Modify.** `SendMessageArgs.idempotency_key`, `send_message` handler restructure, tool description. |
| `crates/of-mcp/tests/tools.rs` | **Modify.** End-to-end tests: replay returns the same row, mismatched replay errors, metering counts once. Also: every existing `AddJobArgs {}`/`SendMessageArgs {}` struct literal gains the new field. |

## Task Order & Rationale

1. Migration + shared `idempotency` module first — nothing else compiles without them.
2. `jobs.rs` (of-core) before `messages.rs` (of-core) — `jobs.rs` is the tool the issue
   leads with, and `messages.rs` is a near-mechanical repeat of the same shape once
   `jobs.rs` is proven, which de-risks it.
3. `of-core::error` changes ride along with Task 2 (the first task that needs the new error
   variant) rather than being its own task — there's nothing to test about the variant in
   isolation.
4. `of-mcp` tasks (jobs tool, then coord tool) come last, once the `of-core` layer both
   tools sit on is done and tested — mirrors the jobs-then-messages ordering above.

## Task 1 — Migration + shared idempotency module ⬜

**Files:** `crates/of-core/migrations/0025_idempotency_keys.sql` (create),
`crates/of-core/src/idempotency.rs` (create), `crates/of-core/src/lib.rs` (modify),
`crates/of-core/Cargo.toml` (modify).

**Interfaces produced:** `idempotency::MAX_KEY_LEN`, `idempotency::validate(&str) -> Result<()>`,
`idempotency::fingerprint(&serde_json::Value) -> Vec<u8>`.

- [ ] Add `sha2.workspace = true` to `crates/of-core/Cargo.toml`'s `[dependencies]`.
- [ ] Write `crates/of-core/migrations/0025_idempotency_keys.sql` exactly as spec §2 shows:
      two nullable columns (`idempotency_key text`, `idempotency_payload_hash bytea`) on
      `jobs`, a partial unique index `jobs_org_idempotency_key_idx` on
      `(org_id, idempotency_key) WHERE idempotency_key IS NOT NULL`; the identical pair on
      `messages` as `messages_org_idempotency_key_idx`.
- [ ] `podman compose up -d` (if not already running), `cp .env.example .env` (if not
      already done), then `cargo test -p of-core --test isolation -- --list` to confirm the
      migration applies cleanly (a fresh `#[sqlx::test]` database runs every migration; a
      syntax error or ordering problem fails immediately here).
- [ ] Write `crates/of-core/src/idempotency.rs` with `MAX_KEY_LEN = 200`, `validate`, and
      `fingerprint`, exactly as spec §3 shows (module doc comment included — it is the one
      place the "unique index enforces, the SELECT is an optimization" reasoning is written
      once for both `jobs.rs` and `messages.rs` to point back to).
- [ ] Add `pub mod idempotency;` to `crates/of-core/src/lib.rs` in the existing module list
      (alongside `jobs`, `messages`, etc. — match the existing ordering/style, likely
      alphabetical or grouped by concern; check the file before deciding).
- [ ] Unit tests in `idempotency.rs` itself (`#[cfg(test)] mod tests`): `validate` rejects
      empty, whitespace-only, and over-`MAX_KEY_LEN` keys; accepts a normal key.
      `fingerprint` of two `serde_json::json!` values with the same keys in different
      insertion order (e.g. built via two different `json!{}` literals with swapped field
      order) produces identical bytes — this is the concrete proof that
      `serde_json::Value::Object`'s sorted-key serialization (no `preserve_order` feature)
      makes the design's order-insensitivity claim true, not assumed.
- [ ] `cargo test -p of-core --lib` (unit tests only, fast) then the full gate:
      `cargo test -p of-core`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all`.
- [ ] Commit: `of-core: add idempotency-key migration and shared validation/fingerprint module`.

## Task 2 — `jobs.rs`: idempotent `add_job` ⬜

**Files:** `crates/of-core/src/jobs.rs`, `crates/of-core/src/error.rs`,
`crates/of-core/tests/queue.rs`, `crates/of-core/tests/isolation.rs`.

**Interfaces:** consumes `idempotency::{validate, fingerprint}` (Task 1). Produces:
`NewJob.idempotency_key: Option<String>`, `Tx::find_replayed_job`, `Error::IdempotencyKeyConflict`.
`Tx::add_job`'s signature is unchanged (`Result<Job>`) — only its body changes.

- [ ] `crates/of-core/src/error.rs`: add
      `IdempotencyKeyConflict { key: String, tool: &'static str }` with the `#[error(...)]`
      message from spec §7, a `code()` arm returning `"idempotency_key_conflict"`, and
      confirm `retriable()`'s `matches!` does **not** include it (no change needed there
      beyond not adding it — write a unit test asserting
      `!Error::IdempotencyKeyConflict{..}.retriable()` to pin that on purpose).
- [ ] Write a failing test first in `crates/of-core/tests/queue.rs`:
      `add_job_replays_with_the_same_idempotency_key` — call `tx.add_job` twice with an
      identical `NewJob { idempotency_key: Some("k1".into()), .. }`, assert the two returned
      `Job`s have the same `id`, and assert `list_jobs`/a count query shows exactly one row.
      Run it (`cargo test -p of-core --test queue add_job_replays_with_the_same_idempotency_key`)
      and confirm it fails to compile (no `idempotency_key` field yet) — this is the
      red step.
- [ ] `NewJob` gains `pub idempotency_key: Option<String>` with the doc comment from spec
      §4. (`NewJob` already derives `Default`, so every existing `..Default::default()`
      call site — including `tests/common/mod.rs::job()` — needs no change.)
- [ ] Add the private `job_idempotency_fingerprint` function exactly as the spec's
      (post-critique) §4 shows: named-field destructure of `&NewJob` with no `..`,
      `idempotency_key: _` explicitly ignored, `depends_on` sorted before hashing.
- [ ] Add `Tx::find_replayed_job` exactly as spec §4 shows: `Ok(None)` when no key; else
      `idempotency::validate`, `SELECT id, idempotency_payload_hash FROM jobs WHERE org_id =
      $1 AND idempotency_key = $2`, `Ok(None)` if nothing found, `Err(IdempotencyKeyConflict)`
      on a hash mismatch, else `Ok(Some(self.get_job(&id).await?))`.
- [ ] Rewrite `Tx::add_job`'s body per spec §4: title-emptiness check and `idempotency::validate`
      first (when a key is present); unchanged repo lookup + seq bump + `id` computation;
      then branch on `new.idempotency_key.as_deref()` — `Some(key)` path uses the
      SAVEPOINT + 14-column insert + unique-violation fallback (matched on
      `db.constraint() == Some("jobs_org_idempotency_key_idx")`, converging on the race
      winner's row when hashes match, `IdempotencyKeyConflict` when they don't); `None` path
      is byte-for-byte today's existing 12-column insert. Both arms produce `(Job, bool)`
      where the bool is `created`. After the branch, `if created && !new.depends_on.is_empty()
      { self.set_dependencies(&job.id, &new.depends_on, &[]).await?; }` (note: `&job.id`, not
      the locally-computed `&id` — the race-fallback arm's `job.id` is the *winner's* id, not
      the burned local `id`, and using the wrong one here is the exact bug the spec's §4
      commentary calls out).
- [ ] Run the red test from above — should now pass. Add the remaining
      `crates/of-core/tests/queue.rs` cases from spec §9: different-payload replay returns
      `IdempotencyKeyConflict`; `depends_on` in a different order with the same key is not a
      conflict; two calls with no key create two distinct jobs (regression pin); empty /
      over-length key returns `Error::Invalid`.
- [ ] `crates/of-core/tests/isolation.rs`: add a cross-org test (place it near
      `cross_org_mutation_is_refused` or as its own function,
      `idempotency_keys_do_not_cross_org_boundaries`) — org A and org B both call `add_job`
      with the literal same `idempotency_key` string; assert both succeed, produce two
      distinct job ids, and neither org's `get_job`/`list_jobs` can see the other's row.
      This is guard 1's proof for `find_replayed_job` and the new insert columns, following
      the existing file's established pattern of running the same call from both tenants.
- [ ] Full gate: `cargo test -p of-core`, `cargo clippy --all-targets -- -D warnings`,
      `cargo fmt --all`.
- [ ] Commit: `of-core: make add_job idempotent on a caller-supplied key`.

## Task 3 — `messages.rs`: idempotent `send_message` ⬜

**Files:** `crates/of-core/src/messages.rs`, `crates/of-core/tests/queue.rs`,
`crates/of-core/tests/isolation.rs`.

**Interfaces:** consumes `idempotency::{validate, fingerprint}` (Task 1),
`Error::IdempotencyKeyConflict` (Task 2). Produces: `NewMessage.idempotency_key`,
`Tx::find_replayed_message`. `Tx::send_message`'s signature is unchanged.

- [ ] Failing test first in `crates/of-core/tests/queue.rs`:
      `send_message_replays_with_the_same_idempotency_key` — call `tx.send_message` twice
      with an identical `NewMessage { idempotency_key: Some("k1".into()), .. }`, assert the
      same message `id` both times and exactly one row via `inbox`/a count query. Confirm it
      fails to compile first.
- [ ] `NewMessage` gains `pub idempotency_key: Option<String>` (doc comment mirroring
      `NewJob`'s). `NewMessage` already derives `Default`.
- [ ] Add `message_idempotency_fingerprint(sender: UserId, new: &NewMessage) -> Vec<u8>`:
      named-field destructure of `new` with no `..`, `idempotency_key: _` ignored, `sender`
      included explicitly (it is a `send_message` parameter, not a `NewMessage` field), body
      trimmed before hashing (matches `send_message`'s own trim-before-store).
- [ ] Add `Tx::find_replayed_message(&mut self, sender: UserId, new: &NewMessage) ->
      Result<Option<Message>>`, mirroring `find_replayed_job` exactly against `messages`.
- [ ] Rewrite `Tx::send_message`'s body: body-empty/too-long checks and
      `idempotency::validate` first; branch on `new.idempotency_key.as_deref()` — `Some`
      path: SAVEPOINT + insert with the two extra columns + unique-violation fallback
      matched on `db.constraint() == Some("messages_org_idempotency_key_idx")`; `None` path:
      today's unchanged insert. No `set_dependencies`-equivalent follow-up step exists for
      messages, so no `created`-flag branching is needed after the match (the spec notes
      this — thread the bool through anyway only if it falls out naturally from mirroring
      Task 2's shape; do not add dead code to force symmetry).
- [ ] Run the red test — should pass. Add remaining cases from spec §9, mirroring Task 2's
      list: different-payload replay conflicts; two no-key calls create two messages;
      empty/over-length key is `Error::Invalid`.
- [ ] `crates/of-core/tests/isolation.rs`: cross-org test for `send_message` mirroring
      Task 2's — same key string, two orgs, two independent messages, no cross-visibility.
- [ ] Full gate: `cargo test -p of-core`, `cargo clippy --all-targets -- -D warnings`,
      `cargo fmt --all`.
- [ ] Commit: `of-core: make send_message idempotent on a caller-supplied key`.

## Task 4 — `of-mcp`: `add_job` tool ⬜

**Files:** `crates/of-mcp/src/tools/jobs.rs`, `crates/of-mcp/src/server.rs`,
`crates/of-mcp/tests/tools.rs`.

**Interfaces:** consumes `of_core::jobs::{NewJob, Tx::find_replayed_job}` (Task 2). No new
MCP tool — `add_job`'s existing `outputSchema` (`out::JobOut`) is unchanged; only its
`inputSchema` gains one optional property.

- [ ] `crates/of-mcp/src/server.rs`: add the new paragraph to `Factory::charge`'s doc
      comment, exactly as the (post-critique) spec §5 gives it — the "third, different-shaped
      exception" framing, explicit that it is a read with no unrollbackable effect, distinct
      from `watch`/`sync_ticket`'s reasoning. Do this in the same commit as the handler
      change below, not separately — the doc comment is describing code this commit
      introduces.
- [ ] `crates/of-mcp/src/tools/jobs.rs`: add `idempotency_key: Option<String>` to
      `AddJobArgs` with `#[serde(default)]` and the doc comment from spec §5.
- [ ] Restructure `add_job`'s handler exactly as spec §5's (post-critique) code shows: build
      `new_job` once after `repo_of`; call `tx.find_replayed_job(&new_job).await.mcp()?`
      before `self.charge(...)`; on `Some(existing)`, `tx.commit()` and return early with
      `out::JobOut { job: existing }`; otherwise `self.charge(...)` then `tx.add_job(new_job)`
      then commit, unchanged from today past that point.
- [ ] Update `add_job`'s `#[tool(description = "...")]` string to append the
      `idempotencyKey` sentence from spec §5.
- [ ] Compiler will now flag every existing `AddJobArgs { ... }` literal missing the new
      field (no `Default` derive on `AddJobArgs`) — fix each of the 5 sites in
      `crates/of-mcp/tests/tools.rs` (lines ~173, ~259, ~791, ~1441, ~1511 as of this plan's
      writing; re-`grep -n "AddJobArgs {" crates/of-mcp/tests/tools.rs` to get current line
      numbers before editing, they will have shifted) by adding
      `idempotency_key: None,` to each. This is mechanical — the compiler names every site
      that needs it.
- [ ] Failing test first (add before the fix above compiles clean, or immediately after as
      the next red step — whichever is more natural given the mechanical fixes just made):
      in `crates/of-mcp/tests/tools.rs`, `add_job_with_an_idempotency_key_replays_instead_of_duplicating`
      — call `env.factory.add_job` twice through the tool surface with the same
      `idempotency_key` and identical other args, assert the same returned job `id` both
      times, and assert `env.usage(&caller)`'s `billableUsed` increased by exactly 1 across
      both calls (the metering proof from spec §9 / §8's "closed" case).
- [ ] Add `add_job_with_a_reused_idempotency_key_and_a_different_title_errors`: same key,
      different `title` the second call, assert the error's `code_of(&e) ==
      "idempotency_key_conflict"`.
- [ ] Add a regression check that `every_tool_has_a_price`/`exhaustive_over`-style existing
      tests (search `crates/of-billing/src/classify.rs`'s test module and
      `crates/of-mcp/tests/tools.rs` for where the full tool list is asserted) still pass
      unchanged — no new tool was added, this should need no edits, just confirm the gate
      below is green.
- [ ] Full gate: `cargo test -p of-mcp`, `cargo test -p of-core` (regression),
      `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all`.
- [ ] Commit: `of-mcp: accept an idempotency key on add_job`.

## Task 5 — `of-mcp`: `send_message` tool ⬜

**Files:** `crates/of-mcp/src/tools/coord.rs`, `crates/of-mcp/tests/tools.rs`.

**Interfaces:** consumes `of_core::messages::{NewMessage, Tx::find_replayed_message}`
(Task 3). No new MCP tool — same additive-only shape as Task 4.

- [ ] `crates/of-mcp/src/tools/coord.rs`: add `idempotency_key: Option<String>` to
      `SendMessageArgs` with `#[serde(default)]`, doc comment mirroring `AddJobArgs`'s in
      spirit ("call this every time if your connection can drop...").
- [ ] Restructure `send_message`'s handler the same way Task 4 restructured `add_job`'s:
      build the `NewMessage` once, `tx.find_replayed_message(caller.user_id, &new_message)`
      before `self.charge(...)`, early-return on a hit, charge-then-insert otherwise.
- [ ] Update `send_message`'s tool description with the same `idempotencyKey` sentence
      pattern as `add_job`'s.
- [ ] Fix the 2 existing `SendMessageArgs { ... }` literal sites in
      `crates/of-mcp/tests/tools.rs` (~914, ~1046 as of this plan's writing — re-grep before
      editing) adding `idempotency_key: None,`.
- [ ] Failing test first: `send_message_with_an_idempotency_key_replays_instead_of_duplicating`
      — same shape as Task 4's job test, asserting the same message `id` and a single
      metered call across two identical calls.
- [ ] `send_message_with_a_reused_idempotency_key_and_a_different_body_errors` — same key,
      different `body`, asserts `idempotency_key_conflict`.
- [ ] Full gate: `cargo test -p of-mcp`, `cargo test -p of-core` (regression),
      `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all`.
- [ ] Commit: `of-mcp: accept an idempotency key on send_message`.

## Final gate (after Task 5, before opening the PR)

- [ ] `cargo test --workspace`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo fmt --all --check`
- [ ] `web/` is untouched by this plan — no `npm run check`/`lint`/`test`/`build` step is
      needed; state this explicitly in the PR body rather than silently omitting it.
- [ ] Re-read the diff against spec §1/§8 one more time: confirm the no-key path for both
      tools issues the exact same SQL and does the exact same `charge`-then-insert ordering
      as before this change (this is the literal test of "omitting the key preserves
      today's behavior exactly").
