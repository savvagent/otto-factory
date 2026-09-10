# Retriable `RaceLost` error for the lost-unique-violation-race-with-no-winner path design

> **Status:** APPROVED — closes savvagent/otto-factory#100. Spec critique approved on first
> pass with only a trivial line-number-citation correction (applied below).

## Goal & Success Criteria

Four sites in `of-core` recover from a `SAVEPOINT`-guarded unique-violation by re-querying
for the concurrent winner's row: `Tx::create_from_ticket`, `Tx::link_ticket`,
`Tx::add_job`'s idempotency-key recovery, and `Tx::send_message`'s idempotency-key recovery.
When that re-query finds nothing — the winner's row vanished between the violation and the
re-query, e.g. a concurrent `delete_job` — all four currently return
`Error::Invalid(format!("... lost a unique-violation race for ... but no concurrently-created
{job,message} was found"))`. `Error::Invalid` maps to `code() == "invalid_argument"` and
`retriable() == false`, telling the calling agent "your input is wrong, don't retry" — but the
actual situation is a transient server-side race with nothing wrong about the request itself,
and a retry would very plausibly succeed once the key/ticket_ref/winner-row-visibility settles.

Success:

- A new `Error` variant, `RaceLost(String)`, distinct from `Invalid`, with `code() ==
  "race_lost"` and `retriable() == true`.
- All four sites construct `Error::RaceLost` instead of `Error::Invalid` for this specific
  failure, with their existing descriptive message text carried over (each already names the
  operation and the key/ticket_ref).
- `of-mcp`'s `from_core` maps `RaceLost` to the JSON-RPC `INTERNAL_ERROR` code, alongside
  `Db`, not `INVALID_PARAMS` — the doc comment on `from_core` already frames this split as
  "your arguments are wrong" vs. "the server broke," and a lost race is the latter.
- Existing `retriable() == false` / `code() == "invalid_argument"` behavior for every other
  `Error::Invalid` use (empty title, empty ticket_ref, unknown status string, etc.) is
  unchanged — this is a narrow reclassification of one specific failure shape, not a change
  to `Invalid`'s general meaning.

## Public interface note

Per Non-Negotiable Rule 6, this is **additive, not breaking**:

- `Error` gains one new variant. No existing variant is renamed or removed, and no existing
  variant's `code()`/`retriable()` mapping changes.
- `Error::code()` gains one new possible return value, `"race_lost"`. An MCP agent that
  branches on known codes and treats an unrecognized one conservatively (the only sound way
  to consume an open-ended `code` string) is unaffected; nothing currently branches on
  `"invalid_argument"` specifically expecting this failure shape, since it was
  indistinguishable from every other `Invalid` use until now.
- The four call sites' **behavior** changes: a caller retrying after this specific failure
  used to get an immediate, confident "don't retry" signal and now gets "retry is plausible."
  This is the bug fix the issue asks for, not a schema or route change — no MCP tool's input
  or output schema changes, no console route changes.
- No version bump is required; every crate stays at the workspace `0.1.0`.

## Scope

**In:**

- `crates/of-core/src/error.rs`: new `RaceLost(String)` variant, `code()` arm, `retriable()`
  inclusion.
- `crates/of-core/src/jobs.rs`: `create_from_ticket`, `link_ticket`, `Tx::add_job`'s
  idempotency-key recovery — three `Error::Invalid(format!(...))` construction sites become
  `Error::RaceLost(format!(...))`.
- `crates/of-core/src/messages.rs`: `Tx::send_message`'s idempotency-key recovery — the
  fourth site, same change.
- `crates/of-mcp/src/error.rs`: `from_core`'s JSON-RPC code match gains `RaceLost` alongside
  `Db` in the `INTERNAL_ERROR` arm.
- Tests: `crates/of-core/tests/*` exercising each of the four sites' lost-race branch directly
  (not just its sibling branches, which existing tests already cover — see §3), and
  `crates/of-mcp/src/error.rs`'s own `every_error_carries_a_code_and_a_retriable_flag`
  test gains a `RaceLost` case plus a dedicated JSON-RPC-code assertion mirroring
  `database_internals_do_not_reach_the_caller`.

**Out:**

- Any change to `Error::Invalid`'s other ~15 use sites (empty title, empty ticket_ref, unknown
  status string, `claim_jobs` empty-ids, etc.). Those are genuine caller-input problems and
  correctly stay non-retriable `invalid_argument`.
- A generic "is this error retriable" heuristic replacing the explicit `matches!` list in
  `retriable()`. The existing style is an explicit allow-list, and this change extends it by
  one entry rather than restructuring it.
- Changing `link_ticket`'s *first* unique-violation arm (the `TicketAlreadyLinked` one, which
  finds a real conflicting job and is working as designed) — only the fallback `ok_or_else`
  inside it, reached when that lookup itself comes up empty, is in scope.
- A retry loop inside `of-core` or `of-mcp` itself. `retriable()` is a signal to the calling
  agent, which otto-factory's "substrate, not workflow" constraint says decides its own retry
  policy — the server does not retry on the caller's behalf.

## §1 — `crates/of-core/src/error.rs`

New variant, placed next to `Invalid` since it is a close sibling (both are a single
free-text `String`) but must stay a distinct variant, not a flag on `Invalid`, so `code()`
and `retriable()` can diverge:

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

`code()` gains:

```rust
Error::RaceLost(_) => "race_lost",
```

`retriable()` becomes:

```rust
pub fn retriable(&self) -> bool {
    matches!(self, Error::LeaseHeld { .. } | Error::Db(_) | Error::RaceLost(_))
}
```

The doc comment on `retriable()` gains one clause after the existing `AlreadyClaimed`
paragraph, extending its existing "here's why each entry is/isn't here" pattern:

```rust
/// `RaceLost` is retriable for the same reason `Db` is: both describe a
/// condition of the server's transaction, not the caller's arguments, and
/// an identical retry can land in a different, successful outcome once the
/// concurrent write that caused it has finished settling.
```

## §2 — `crates/of-core/src/jobs.rs`

Three sites. Each changes only the error constructor — the surrounding savepoint
rollback/release sequence, the re-query itself, and every other branch are unchanged.

`Tx::add_job`'s idempotency-key recovery (current `crates/of-core/src/jobs.rs:437-441`):

```rust
.ok_or_else(|| {
    Error::RaceLost(format!(
        "add_job lost a unique-violation race for idempotency key {key:?} but no \
         concurrently-created job was found — this is a transient server-side race; \
         retry the call"
    ))
})?;
```

`link_ticket`'s fallback (current `crates/of-core/src/jobs.rs:672-676`) — only the inner
`ok_or_else`, not the `TicketAlreadyLinked` it feeds into on the happy path:

```rust
.ok_or_else(|| {
    Error::RaceLost(format!(
        "link_ticket lost a unique-violation for {ticket_ref:?} but no conflicting job \
         was found — this is a transient server-side race; retry the call"
    ))
})?;
```

`create_from_ticket`'s fallback (current `crates/of-core/src/jobs.rs:784-788`):

```rust
.ok_or_else(|| {
    Error::RaceLost(format!(
        "create_from_ticket lost a unique-violation race for {ticket_ref:?} but no \
         concurrently-created job was found — this is a transient server-side race; \
         retry the call"
    ))
})?
```

(Note this third site's `?` — unlike the other two, `create_from_ticket`'s recovery arm's
value, not an early `return`, is the match arm's own result; only the error constructor
changes, the `?` placement is unaffected.)

## §3 — `crates/of-core/src/messages.rs`

`Tx::send_message`'s idempotency-key recovery (current `crates/of-core/src/messages.rs:307-312`),
identical shape to `add_job`'s:

```rust
.ok_or_else(|| {
    Error::RaceLost(format!(
        "send_message lost a unique-violation race for idempotency key {key:?} but no \
         concurrently-created message was found — this is a transient server-side \
         race; retry the call"
    ))
})?;
```

## §4 — `crates/of-mcp/src/error.rs`

`from_core`'s JSON-RPC code split (current lines 33-39) gains `RaceLost` in the
`INTERNAL_ERROR` arm, next to `Db`:

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

`RaceLost`'s `Display` message (built from the caller-facing text already written into it at
each of the four sites) needs no redaction the way `Db`'s does — it was already written for
an LLM caller, unlike a raw `sqlx::Error`, so the `message` match arm's `other =>
other.to_string()` fallback already handles it correctly with no new arm needed.

## Testing

- `crates/of-core/src/error.rs`: extend the existing `#[cfg(test)] mod tests` with
  `race_lost_is_retriable` (mirrors `idempotency_key_conflict_is_not_retriable`'s shape) —
  `Error::RaceLost("x".into())` has `code() == "race_lost"` and `retriable() == true`.
- `crates/of-core/tests/jobs.rs` (or `queue.rs`, matching wherever `add_job`/
  `create_from_ticket`/`link_ticket` are already exercised in this suite): for at least one of
  the four sites, exercise the lost-race branch directly rather than only its sibling
  "found a winner" branch, since no existing test reaches the `ok_or_else` arm today. The
  branch requires the winner's row to be gone by the time the recovery `SELECT` runs after
  the savepoint rollback — reachable deterministically in a test by deleting the
  concurrently-inserted row from a second connection between the first transaction's
  unique-violation and its recovery query, or by directly unit-testing the `Error` construction
  and its `code()`/`retriable()` (already covered by the `error.rs` test above) if driving the
  actual race deterministically through two live `sqlx` connections proves too complex for the
  value it adds — the `error.rs` test already proves the variant behaves correctly; an
  integration test proving each of the four sites' `ok_or_else` still *constructs* `RaceLost`
  (not `Invalid`) is the higher-value addition and does not require reproducing the live race:
  a focused unit-level check per site (or one shared helper covering the pattern, since all
  four sites share the identical shape) suffices.
- `crates/of-mcp/src/error.rs`: add `CoreError::RaceLost("boom".into())` to
  `every_error_carries_a_code_and_a_retriable_flag`'s array, and a new
  `race_lost_maps_to_internal_error` test mirroring `database_internals_do_not_reach_the_caller`
  — asserts `converted.code == ErrorCode::INTERNAL_ERROR` and
  `converted.data.unwrap()["retriable"] == true`.

## Assumptions

- `RaceLost` is a single free-text `String`, matching `Invalid`/`Config`/`Crypto`'s existing
  shape, rather than a richer struct carrying `tool`/`key`/`ticket_ref` fields separately.
  *Rationale: the four call sites already build a fully-formed, specific message via `format!`
  before this change (unlike, say, `IdempotencyKeyConflict`, whose structured fields are read
  back by `of-mcp` or tests) — nothing downstream needs to pattern-match the key or tool name
  out of this variant, only its `code()`/`retriable()`, so a struct would add ceremony with no
  consumer.*
- The message text keeps each site's existing wording and only appends a short clause noting
  the condition is transient and worth retrying. *Rationale: the existing messages already
  name the operation and the key/ticket_ref per the repo's error-writing convention; the only
  thing missing for an LLM caller deciding what to do next is the "retry" signal itself, which
  `retriable()` provides machine-readably and the appended clause now also states in prose,
  consistent with the style guide's "what to call next."*
- No change to `link_ticket`'s outer `TicketAlreadyLinked` path, which is a real, working
  conflict-naming design, not a lost race. *Rationale: the issue names four *specific*
  `ok_or_else` sites; `link_ticket`'s happy-path conflict resolution is unrelated code the
  issue does not ask to touch.*

## Error Handling & Edge Cases

- A caller retrying after `RaceLost` and hitting the identical lost-race shape again (e.g. the
  winner's row keeps getting deleted by a concurrent process): still returns `RaceLost`,
  still `retriable() == true` — the server makes no promise a retry succeeds, only that it is
  not structurally doomed the way `Invalid` implies. An agent that retries a bounded number of
  times and then gives up is exercising ordinary backoff policy, which is the caller's
  business per the substrate-not-workflow constraint, not this server's.
- `winner.1 != hash` (the idempotency-payload-mismatch branch, immediately after the
  `ok_or_else` in `add_job`/`send_message`) is unaffected — that branch already returns the
  distinct, correctly-non-retriable `IdempotencyKeyConflict`, and this change touches only the
  sibling branch reached when the winner row cannot be found at all.

## Risks & Open Questions

- The live concurrent race (the winner's committed row deleted by a second connection
  strictly between the first connection's SAVEPOINT-violation and its immediately-following
  recovery SELECT, both awaits inside one async function call with no externally-triggerable
  yield point) is not deterministically testable through the public API without adding a
  test-only instrumentation hook to production code, which is out of scope for this
  bug-fix-sized change. Unlike the existing `concurrent_add_job_idempotency_converges_on_one_job`/
  `concurrent_create_from_ticket_converges_on_one_job` tests (which tolerate either of two
  orderings and work with sleep-based `tokio::join!` timing), this scenario needs one specific
  narrow interleaving with a sub-millisecond window, which sleep-based timing cannot reliably
  produce without flaking. Coverage relies on `crates/of-core/src/error.rs`'s
  `race_lost_is_retriable` test (the variant's `code()`/`retriable()` behavior) plus the
  mechanical nature of the four call-site changes (a one-line `Error::Invalid` →
  `Error::RaceLost` swap with unchanged `format!` arguments, reviewable directly against the
  diff) and the full existing test suite continuing to pass unchanged (proving the sibling
  branches — `TicketAlreadyLinked`, `IdempotencyKeyConflict`, the happy-path recovery arms —
  are unaffected).
