# RaceLost retriable error — implementation plan

**Spec:** `docs/specs/2026-09-10-race-lost-retriable-design.md` — read it first. This plan
implements it exactly.

> Plan critique approved on first pass; two non-blocking advisories applied (dropped a
> nonexistent `--test messages` target from Task 2's commands).

## Goal

Closes `savvagent/otto-factory#100`: reclassify the "lost a unique-violation race with no
concurrent winner found" failure at four `of-core` sites from `Error::Invalid`
(`code() == "invalid_argument"`, `retriable() == false`) to a new `Error::RaceLost`
(`code() == "race_lost"`, `retriable() == true`), and map it to `INTERNAL_ERROR` (not
`INVALID_PARAMS`) in `of-mcp`'s JSON-RPC error envelope.

## Status — 2026-09-10

Done. All four tasks shipped in `savvagent/otto-factory#138`
(`a86a8079aa44a7452df2c881d36d7af37a6f842f`), plus a follow-up commit fixing eight review
findings (constraint pinning in Task 2's two ticket-lookup sites, a backoff signal on the
new retriable path, a test gap, a genuinely achievable deterministic test this plan had
wrongly ruled out, two line-length regressions, and this document's own stale task
count/file list — see the PR's aggregated review comment for the full list) before merge.

## Global Constraints

- Errors are written for an LLM caller that has never read the docs: what went wrong, what
  the valid options were, what to call next. `Error::code()` is the stable machine-readable
  branch point; `retriable()` tells an agent whether to back off.
- No `unwrap()` outside tests. No silent fallback on a resolution failure.
- No AI self-attribution anywhere (commits, PR body, comments, code).
- Run `cargo fmt --all` before every Rust commit.
- Every SQL statement lives in `of-core` — not touched by this change (no new SQL).
- Tests need a real Postgres: `podman compose up -d` (host port 15433) and a `.env` with
  `DATABASE_URL` (`cp .env.example .env`).
- No tenant table, RLS policy, MCP tool, or `of-billing::classify` entry is added or changed
  by this plan — nothing here needs a cross-org negative test or a billing classification.
- No migration, no schema change, no out-of-band artifact (container image, console bundle,
  Cloudflare Worker) is touched by this plan.

## File Structure

| File | Responsibility |
|---|---|
| `crates/of-core/src/error.rs` | **Modify.** New `RaceLost(String)` variant, `code()` arm, `retriable()` inclusion, doc-comment update. |
| `crates/of-core/src/jobs.rs` | **Modify.** Three `Error::Invalid(format!(...))` sites become `Error::RaceLost(format!(...))`. |
| `crates/of-core/src/messages.rs` | **Modify.** One `Error::Invalid(format!(...))` site becomes `Error::RaceLost(format!(...))`. |
| `crates/of-web/src/error.rs` | **Modify.** Discovered mid-implementation: `impl From<CoreError> for ApiError`'s exhaustive `status` match gains a `RaceLost` arm, or the workspace fails to compile. |
| `crates/of-mcp/src/error.rs` | **Modify.** `from_core`'s JSON-RPC code match gains `RaceLost` in the `INTERNAL_ERROR` arm; test coverage extended. |

## Task Order & Rationale

Task 1 introduces the variant and proves its `code()`/`retriable()` behavior in isolation —
nothing downstream can compile against it correctly without this landing first. Task 2 swaps
the four call sites, which only typechecks once Task 1 exists. Task 3 (added mid-implementation,
once Task 1 alone was found to break `of-web`'s compile) fixes `of-web`'s exhaustive match — it
must land before `cargo test --workspace` can pass again, but has no dependency on Task 2. Task 4
is the `of-mcp` wire mapping, independent of both Task 2 and Task 3's internals but meaningless to
test without a `RaceLost` value to convert, so it lands last.

## Task 1 — Add the `RaceLost` error variant ✅

**Files:** `crates/of-core/src/error.rs`
**Interfaces:** produces `Error::RaceLost(String)`, `Error::code() -> "race_lost"` for it,
`Error::retriable() -> true` for it. Consumed by Task 2 (construction sites) and Task 3
(`of-mcp` mapping).

- [ ] In `crates/of-core/src/error.rs`, add a failing test first, in the existing
      `#[cfg(test)] mod tests` block, mirroring `idempotency_key_conflict_is_not_retriable`'s
      shape:
      ```rust
      #[test]
      fn race_lost_is_retriable() {
          let e = Error::RaceLost("x".into());
          assert!(e.retriable());
          assert_eq!(e.code(), "race_lost");
      }
      ```
- [ ] Run `cargo test -p of-core error::tests` — confirm it fails to compile (no `RaceLost`
      variant yet).
- [ ] Add the variant next to `Invalid` (both are close siblings — a single free-text
      `String` — but must stay distinct so `code()`/`retriable()` can diverge):
      ```rust
      /// A `SAVEPOINT`-guarded unique-violation recovery re-queried for the
      /// concurrent winner's row and found nothing — the winner's row vanished
      /// between the violation and the re-query (e.g. a concurrent delete). This
      /// is a transient server-side race, not a problem with the caller's
      /// request: unlike `Invalid`, retrying the identical call can plausibly
      /// succeed once the row settles. See `create_from_ticket`, `link_ticket`,
      /// `Tx::add_job`, and `Tx::send_message` for the four sites that raise it.
      #[error("{0}")]
      RaceLost(String),
      ```
- [ ] Add the `code()` match arm, placed next to `Invalid`'s:
      ```rust
      Error::RaceLost(_) => "race_lost",
      ```
- [ ] Update `retriable()`'s body and extend its doc comment with a clause after the existing
      `AlreadyClaimed` paragraph:
      ```rust
      /// `RaceLost` is retriable for the same reason `Db` is: both describe a
      /// condition of the server's transaction, not the caller's arguments, and
      /// an identical retry can land in a different, successful outcome once the
      /// concurrent write that caused it has finished settling.
      pub fn retriable(&self) -> bool {
          matches!(self, Error::LeaseHeld { .. } | Error::Db(_) | Error::RaceLost(_))
      }
      ```
- [ ] Run `cargo test -p of-core error::tests` — confirm `race_lost_is_retriable` passes.
- [ ] Run `cargo clippy -p of-core --all-targets -- -D warnings` — confirm clean.
- [ ] Format and commit: `cargo fmt --all` then
      `git commit -m "of-core: add a retriable RaceLost error variant"`.

## Task 2 — Swap the four lost-race sites to `RaceLost` ✅

**Files:** `crates/of-core/src/jobs.rs`, `crates/of-core/src/messages.rs`
**Interfaces:** consumes `Error::RaceLost` from Task 1. No signature of any function changes
— only which `Error` variant a specific `ok_or_else` closure constructs.

- [ ] Add failing tests first, proving each site constructs `RaceLost` (not `Invalid`) when
      its lost-race branch is reached. Reaching the live race deterministically (the winner's
      row must be gone by the time the recovery `SELECT` runs, after the savepoint rollback)
      needs a second connection deleting the concurrently-inserted row mid-transaction — if
      that proves too complex to set up reliably for the value it adds, a focused unit-level
      check per site is the fallback the spec names as sufficient, since Task 1's test already
      proves the variant's own `code()`/`retriable()` behavior. Prefer the integration route
      first; each of the four functions is exercised elsewhere in
      `crates/of-core/tests/jobs.rs` / `crates/of-core/tests/queue.rs`, so add alongside that
      existing coverage rather than a new file. At minimum, one such test must exist proving a
      real call site now returns `RaceLost` where it used to return `Invalid` (Resolution: determined not achievable without production test-only instrumentation; see the spec's Risks & Open Questions for the full reasoning. Coverage relies on error.rs's unit test plus the mechanical, reviewable nature of the four one-line swaps.).
- [ ] Run the new test(s) with `cargo test -p of-core --test jobs --test queue` (`add_job`/
      `create_from_ticket`/`link_ticket` coverage lives in `tests/jobs.rs`; `send_message`
      coverage lives in `tests/queue.rs`/`tests/isolation.rs` — there is no separate
      `tests/messages.rs`) — confirm they fail against the current `Error::Invalid` sites (or
      fail to compile if written against `Error::RaceLost` before Task 1's variant exists —
      Task 1 must already be committed at this point).
- [ ] In `crates/of-core/src/jobs.rs`, change `Tx::add_job`'s idempotency-key recovery
      `ok_or_else` (currently `crates/of-core/src/jobs.rs:437-441`) from `Error::Invalid` to:
      ```rust
      .ok_or_else(|| {
          Error::RaceLost(format!(
              "add_job lost a unique-violation race for idempotency key {key:?} but no \
               concurrently-created job was found — this is a transient server-side race; \
               retry the call"
          ))
      })?;
      ```
- [ ] In the same file, change `link_ticket`'s fallback `ok_or_else` (currently
      `crates/of-core/src/jobs.rs:672-676` — only this inner arm, not the outer
      `TicketAlreadyLinked` conflict-naming arm it feeds) to:
      ```rust
      .ok_or_else(|| {
          Error::RaceLost(format!(
              "link_ticket lost a unique-violation for {ticket_ref:?} but no conflicting job \
               was found — this is a transient server-side race; retry the call"
          ))
      })?;
      ```
- [ ] In the same file, change `create_from_ticket`'s fallback `ok_or_else` (currently
      `crates/of-core/src/jobs.rs:784-788`) to:
      ```rust
      .ok_or_else(|| {
          Error::RaceLost(format!(
              "create_from_ticket lost a unique-violation race for {ticket_ref:?} but no \
               concurrently-created job was found — this is a transient server-side race; \
               retry the call"
          ))
      })?
      ```
      (this arm's value, not an early `return`, is the match arm's own result — only the
      constructor changes, the trailing `?` placement is unaffected).
- [ ] In `crates/of-core/src/messages.rs`, change `Tx::send_message`'s idempotency-key
      recovery `ok_or_else` (currently `crates/of-core/src/messages.rs:307-312`) to:
      ```rust
      .ok_or_else(|| {
          Error::RaceLost(format!(
              "send_message lost a unique-violation race for idempotency key {key:?} but no \
               concurrently-created message was found — this is a transient server-side \
               race; retry the call"
          ))
      })?;
      ```
- [ ] Run `cargo test -p of-core --test jobs --test queue --test isolation` — confirm the new
      test(s) from step 1 now pass and every pre-existing test in those suites still passes
      (in particular any test touching `TicketAlreadyLinked`, `IdempotencyKeyConflict`, or the
      happy-path recovery arms, which must be unaffected).
- [ ] Run `cargo test -p of-core` (full crate) and `cargo clippy -p of-core --all-targets --
      -D warnings` — confirm clean.
- [ ] Format and commit: `cargo fmt --all` then
      `git commit -m "of-core: return RaceLost, not Invalid, when a unique-violation race is lost with no winner"`.

## Task 3 — Fix `of-web`'s exhaustive match (compile-blocking) ✅

**Discovered mid-implementation:** Task 1 alone leaves the workspace non-compiling —
`crates/of-web/src/error.rs`'s `impl From<CoreError> for ApiError` matches exhaustively over
`of_core::Error`, and `RaceLost` has no arm. This must land before Task 4 (or in either order,
but both before Final Verification's `cargo test --workspace`).

**Files:** `crates/of-web/src/error.rs`
**Interfaces:** consumes `Error::RaceLost` from Task 1. No `ApiError` struct change, no route
change — only a new arm in an existing exhaustive match plus a test.

- [ ] Add a failing test first, in `crates/of-web/src/error.rs`'s existing `#[cfg(test)] mod
      tests` block (create one following this crate's usual test-module convention if none
      exists yet in this file — check first):
      ```rust
      #[test]
      fn race_lost_maps_to_service_unavailable_with_its_own_message() {
          let e = CoreError::RaceLost("add_job lost a race".into());
          let api_err: ApiError = e.into();
          assert_eq!(api_err.status, StatusCode::SERVICE_UNAVAILABLE);
          assert_eq!(api_err.code, "race_lost");
          // Must NOT be the generic redacted internal() message — RaceLost's
          // Display text is already written to be read by whoever hit it.
          assert_eq!(api_err.message, "add_job lost a race");
      }
      ```
- [ ] Run `cargo test -p of-web` (or the specific test path) — confirm it currently fails to
      compile with `E0004: non-exhaustive patterns: &of_core::Error::RaceLost(_) not covered`.
- [ ] In `crates/of-web/src/error.rs`'s `status` match (current lines ~137-168), add a new arm.
      `RaceLost` must NOT join the early-return redaction branch (lines ~129-135) — its message
      is caller-facing by design, unlike `Db`/`IsolationNotEnforced`/`Config`/`Crypto`. Add it
      to the `status` match instead, in its own group:
      ```rust
      // Retriable, not the caller's fault — the same distinction retriable()
      // already draws at the MCP layer. 503, not 500: this is specifically a
      // "try again" condition, and its message (unlike Db's) is already safe
      // to show as-is.
      RaceLost(_) => StatusCode::SERVICE_UNAVAILABLE,
      ```
- [ ] Run the test again — confirm it passes.
- [ ] Run `cargo test -p of-web` (full crate) and `cargo clippy -p of-web --all-targets -- -D
      warnings` — confirm clean.
- [ ] Format and commit: `cargo fmt --all` then
      `git commit -m "of-web: map RaceLost to a 503 in the console API's error envelope"`.

## Task 4 — Map `RaceLost` to `INTERNAL_ERROR` in the MCP envelope ✅

**Files:** `crates/of-mcp/src/error.rs`
**Interfaces:** consumes `Error::RaceLost` from Task 1/2. No MCP tool schema or result
envelope changes — only which JSON-RPC error code `from_core` picks for this one `Error`
variant, exactly like the existing `Db` arm.

- [ ] Add failing tests first, in `crates/of-mcp/src/error.rs`'s existing `#[cfg(test)] mod
      tests` block:
      - Add `CoreError::RaceLost("boom".into())` to
        `every_error_carries_a_code_and_a_retriable_flag`'s `errors` array.
      - Add a new test mirroring `database_internals_do_not_reach_the_caller`:
        ```rust
        /// A lost race is the server's problem, not the caller's — it must map
        /// to INTERNAL_ERROR at the JSON-RPC level exactly like a raw database
        /// failure does, not INVALID_PARAMS.
        #[test]
        fn race_lost_maps_to_internal_error() {
            let e = CoreError::RaceLost("add_job lost a race".into());
            let converted = from_core(&e);
            assert_eq!(converted.code, ErrorCode::INTERNAL_ERROR);
            assert_eq!(converted.data.unwrap()["retriable"], true);
        }
        ```
- [ ] Run `cargo test -p of-mcp error::tests` — confirm both fail (current code puts
      `RaceLost` in the `_ => ErrorCode::INVALID_PARAMS` catch-all).
- [ ] In `from_core`, change the JSON-RPC code match (current lines ~33-39) to:
      ```rust
      let code = match e {
          // A database failure or a lost race is ours, not the caller's — the
          // request was fine; the transaction lost to a concurrent write.
          // Reporting either as an argument error would send an agent into a
          // rewrite loop over a request that was fine to begin with.
          CoreError::Db(_) | CoreError::RaceLost(_) => ErrorCode::INTERNAL_ERROR,
          _ => ErrorCode::INVALID_PARAMS,
      };
      ```
      (the `message` match arm's `other => other.to_string()` fallback already handles
      `RaceLost` correctly with no new arm — its `Display` text was already written for an
      LLM caller, unlike a raw `sqlx::Error`, so it needs no redaction.)
- [ ] Run `cargo test -p of-mcp error::tests` — confirm both new tests pass and every
      existing test in the module still passes.
- [ ] Run `cargo test -p of-mcp` (full crate — confirms nothing in `tools/*` asserted the old
      `INVALID_PARAMS` mapping for any of the four call sites) and
      `cargo clippy -p of-mcp --all-targets -- -D warnings` — confirm clean.
- [ ] Format and commit: `cargo fmt --all` then
      `git commit -m "of-mcp: map RaceLost to INTERNAL_ERROR, not INVALID_PARAMS"`.

## Final Verification (after Task 4)

- [ ] `cargo test --workspace` — full green.
- [ ] `cargo clippy --all-targets -- -D warnings` — full green.
- [ ] `cargo fmt --all --check` — clean.
- [ ] `git log --oneline` on the branch shows the doc commits plus one commit per task (Task 2
      may carry an extra follow-up commit if its own review found something to fix), none
      carrying AI attribution.
- [ ] No `web/` (SvelteKit console) change — `npm run check`/`lint`/`test`/`build` are
      vacuously satisfied (no frontend surface touched; `crates/of-web` is the Rust console
      API crate, a different thing from `web/`, and Task 3 touches only the former).
- [ ] No migration, no out-of-band artifact touched — vacuously satisfied.
