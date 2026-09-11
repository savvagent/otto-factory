# Claim-generation fencing for complete_job/fail_job/cancel_job/renew_claim — implementation plan

Goal: give `complete_job`, `fail_job`, `cancel_job`, and `renew_claim` an optional
`expectedAttempts` argument that, when supplied, refuses the call unless the job's current
`attempts` still matches — closing savvagent/otto-factory#103 (two agent instances sharing
one account/token can otherwise clobber each other's claim after one's expires and the
other reclaims). Also formally decides `repo_leases` is out of scope, with a permanent
regression test locking in why.

**Spec:** `docs/specs/2026-09-11-claim-generation-fencing-design.md` — read it first. This
plan implements it exactly.

## Status — 2026-09-11

✅ Shipped in savvagent/otto-factory#161 (merged as `fix(of-core)!: fence
complete_job/fail_job/cancel_job/renew_claim by claim generation`).

**Update (same day, PR review):** the mandatory review trio's independent `security-auditor`
pass found the plan as drafted left an unfenced expiry race open for any caller that never
learns about `expected_attempts` (see the spec's "Addendum: the expiry fence" section, added
after this plan). `ensure_claim_held` gained a second, unconditional expiry check, plus a
narrow `cancel_job`-only exemption when a cancellation is already on file. **This makes the
change as a whole breaking, not additive** — see the Global Constraints correction below.
The two test names this plan specifies below
(`expected_attempts_omitted_preserves_todays_behavior`) were renamed during the review
response to `expected_attempts_omitted_preserves_todays_behavior_for_a_reclaimed_claim`,
since the un-renamed name overstated what the test proves once the expiry check exists
alongside it.

**Update (round 2, blind security-auditor re-review):** the expiry check itself was found
sound (atomic, boundary-consistent, no clock skew, no bypass), but the downstream wording and
documentation were not — the expiry-refusal wording was fixed to stop misattributing an
expired claim to a caller who never held it, the generation-mismatch message no longer echoes
the value it says never to obtain, `cancel_job`'s exemption gained its own dedicated tests,
and all five affected tool descriptions now say plainly that an expired claim cannot be
renewed or finalized. See PR #161's aggregated review-findings comment for the full
Critical/Important/Suggestions/Strengths breakdown across both rounds.

## Global Constraints

- No AI self-attribution anywhere (commits, code comments, docs, PR body).
- Run `cargo fmt --all` before every Rust commit.
- Every SQL statement lives in `of-core` — this plan adds none new, but the modified
  `ensure_claim_held` query stays in `crates/of-core/src/jobs.rs`.
- Tests need a real Postgres: `podman compose up -d` (already running on host port 15433)
  and `.env` with `DATABASE_URL` (already present in this checkout's parent, but the
  worktree needs its own `.env` — copy it, `cp .env.example .env` then set `DATABASE_URL`
  to match, or symlink/copy the existing one).
- `Error::AlreadyClaimed` is reused, not forked — no new error variant, per spec §3.
- **Correction (PR review — see Status above and the spec's Addendum): this is NOT purely
  additive.** `expected_attempts` alone would have been, but the expiry check added during
  review applies unconditionally to every existing caller. The PR title carries a
  `!`/`BREAKING CHANGE:` marker per Non-Negotiable Rule 6, superseding this bullet as
  originally drafted.
- **Signature-change churn is mechanical and compiler-verified**: once `ensure_claim_held`,
  `finalize`, `complete_job`, `fail_job`, `cancel_job`, and `renew_claim` each gain a
  trailing `expected_attempts: Option<i32>` parameter, `cargo build --workspace --tests`
  will fail to compile at every call site missing the new argument. Fix each reported site
  by appending `, None` (existing tests that don't care about generation fencing) or the
  appropriate `Some(n)` (the new tests that do) — do not guess call sites from grep alone;
  let the compiler enumerate them, then re-run until clean.

## File Structure

| File | Responsibility |
|---|---|
| `crates/of-core/src/jobs.rs` | **Modify.** `ensure_claim_held`, `finalize`, `complete_job`, `fail_job`, `cancel_job`, `renew_claim` gain `expected_attempts: Option<i32>`. |
| `crates/of-core/tests/queue.rs` | **Modify.** Every existing `.complete_job(`/`.fail_job(`/`.cancel_job(`/`.renew_claim(` call site gains a trailing `None`. New tests proving the fix, plus the leases regression test from spec §"Premise correction". |
| `crates/of-core/tests/jobs.rs` | **Modify.** Same mechanical update for its 3 call sites. |
| `crates/of-core/tests/isolation.rs` | **Modify.** Same mechanical update for its 4 call sites (cross-org tests; unaffected in substance, per spec's Tenant isolation section). |
| `crates/of-mcp/src/tools/jobs.rs` | **Modify.** `CompleteJobArgs`, `FailJobArgs`, `CancelJobArgs`, `RenewClaimArgs` gain `expected_attempts: Option<i32>`; the four handlers pass it through; the four tool `description`s gain a sentence explaining it. |
| `crates/of-mcp/tests/tools.rs` | **Modify.** 12 existing struct-literal call sites gain `expected_attempts: None`. New tests proving the MCP-level wiring (mismatch refused, matching value succeeds, omitted preserves today's behavior). |

No migration, no `of-billing` change, no `docs/clients/matrix.md` change, no `web/` change
— all confirmed vacuous in the spec's Scope/Out and Public interface note.

## Task Order & Rationale

One task. The `of-core` fix and its `of-mcp` wiring are inseparable in practice: the
compiler-driven call-site sweep (Global Constraints) touches both crates' test suites in
one pass, and splitting into two commits would leave an intermediate commit where
`of-mcp` fails to build against the changed `of-core` signatures. TDD order within the
task: write the new tests first — the generation-fencing test genuinely fails against
unmodified `of-core` (it asserts the fix's behavior, which does not exist yet), proving
the bug via a real `assert` failure rather than a throwaway test that would need deleting
— then the `of-core` fix, then the new failing `of-mcp` tests, then the `of-mcp` wiring,
then the full-workspace mechanical sweep, verified by the compiler and the full test
suite.

## Task 1 — Claim-generation fencing end-to-end ✅

**Files:** all six rows in File Structure above.
**Interfaces:** consumes `Job.attempts` (already exists, `crates/of-core/src/jobs.rs`);
produces the new `expected_attempts`/`expectedAttempts` parameter on four `Tx` methods and
four MCP tools.

- [ ] **Failing test 1 (the fix's behavior, not yet implemented):** in
      `crates/of-core/tests/queue.rs`, add
      `stale_generation_cannot_finalize_or_renew_after_same_account_reclaims` that
      reproduces spec §Goal's exact interleaving using the SAME `UserId` for both claims
      (unlike the existing `a_stale_holder_cannot_finalize_after_someone_else_reclaims`,
      which uses two different users): claim under account R with label "agent-a" (capture
      `attempts`), force-expire (reuse the existing `expire_claim` helper), reclaim under
      the SAME account R with label "agent-a-prime" (capture the new `attempts`), then
      assert `complete_job`/`fail_job`/`cancel_job`/`renew_claim` called with the OLD
      `attempts` value as `expected_attempts` all fail `already_claimed` naming
      "agent-a-prime" (use a separate job per finalizer under test, since
      `complete_job`/`fail_job`/`cancel_job` are each terminal), while the same calls with
      the CURRENT `attempts` value succeed. This test does not compile against the
      not-yet-changed `complete_job`/`fail_job`/`cancel_job`/`renew_claim` signatures (they
      do not accept a 4th/5th argument yet) — that compile failure **is** this step's
      "failing test," standing in for a runtime failure since the change is additive to a
      function signature, not a value comparison. Do not attempt to run it as a passing
      test yet; move directly to the next step, which makes it compile and pass together.
- [ ] **Also add** `expected_attempts_omitted_preserves_todays_behavior` in the same file:
      repeat the same claim/expire/reclaim setup, then confirm the stale caller's call
      **without** `expected_attempts` (i.e. `None`) still succeeds exactly as today — the
      additive-compatibility guarantee from spec §Success bullet 2. Same compile-failure
      note applies until the next step.
- [ ] **Failing test 2 (leases hypothesis):** in `crates/of-core/tests/queue.rs`, add
      `lease_reclaim_after_expiry_mints_a_new_id_fencing_the_stale_holder` per spec
      §"Premise correction": acquire a lease, force-expire it (`UPDATE repo_leases SET
      expires_at = now() - interval '1 second' WHERE id = $1`, same pattern as `queue.rs`'s
      existing `expire_claim` helper for jobs), reacquire under the same `holder_user_id`
      with a different label, assert the returned lease `id` differs from the first, then
      assert `renew_lease`/`release_lease` on the *first* id both fail `lease_not_held`
      while the *second* id's `renew_lease` succeeds. Run
      `cargo test -p of-core --test queue lease_reclaim_after_expiry` and confirm it
      **passes against unmodified `of-core`** — this is a regression test locking in
      existing behavior, not a test of new code, so it must pass before Task 1's `jobs.rs`
      edit and after it identically.
- [ ] **Implement the fix** in `crates/of-core/src/jobs.rs` per spec §2: add
      `expected_attempts: Option<i32>` to `ensure_claim_held` (select `attempts` alongside
      the existing columns; after the existing identity check, add the generation check
      exactly as spec §2 shows — reusing `Error::AlreadyClaimed` with the same `holder`
      resolution, not a new variant); thread the same trailing parameter through
      `finalize`, `complete_job`, `fail_job`, `cancel_job`, `renew_claim`. Update the doc
      comment on `ensure_claim_held` to mention the new check (the existing comment says
      only "confirm `caller` is the one who holds it" — extend it, do not replace it).
      Run `cargo build -p of-core --tests` and fix every reported missing-argument call
      site in `crates/of-core/tests/jobs.rs`, `crates/of-core/tests/queue.rs`, and
      `crates/of-core/tests/isolation.rs` by appending `, None` (these existing tests are
      not exercising the new argument) until the crate builds clean — this also makes the
      two new tests from the previous steps compile. Run
      `cargo test -p of-core --test queue` (whole file) and confirm all pass, including
      `stale_generation_cannot_finalize_or_renew_after_same_account_reclaims`,
      `expected_attempts_omitted_preserves_todays_behavior`, and
      `lease_reclaim_after_expiry_mints_a_new_id_fencing_the_stale_holder`. Also run
      `cargo test -p of-core --test jobs` and `cargo test -p of-core --test isolation` to
      confirm the mechanical `, None` additions changed nothing about their outcomes.
- [ ] **Wire the MCP tools** in `crates/of-mcp/src/tools/jobs.rs` per spec §5: add
      `expected_attempts: Option<i32>` (with the doc comment from spec §5, adapted per
      tool) to `CompleteJobArgs`, `FailJobArgs`, `CancelJobArgs`, `RenewClaimArgs`; pass
      `args.expected_attempts` through in `complete_job`, `fail_job`, `cancel_job`,
      `renew_claim`; extend each of the four tool `description`s with one sentence naming
      `expectedAttempts`. Run `cargo build -p of-mcp --tests` and fix every reported
      missing-field struct literal in `crates/of-mcp/tests/tools.rs` by adding
      `expected_attempts: None` until the crate builds clean.
- [ ] **New MCP-level tests** in `crates/of-mcp/tests/tools.rs`: extend or add alongside
      `renew_claim_extends_the_claim_and_completion_still_works` and
      `complete_job_from_a_non_holder_is_refused` — a test claiming a job, capturing
      `attempts` from the `claim_jobs` response's `jobs[0]["attempts"]`, force-expiring the
      claim via the same raw-SQL helper pattern `of-core`'s tests use — `env.db.begin(org)`
      gives a `Tx` whose `.conn()` takes the same raw
      `UPDATE jobs SET claim_expires_at = now() - interval '1 second' WHERE org_id = $1 AND
      id = $2` statement `crates/of-core/tests/queue.rs`'s `expire_claim` helper uses, then
      `tx.commit()` — reclaiming under the same caller with a different `agent` label, then asserting a
      `complete_job` call with the stale `expectedAttempts` fails with
      `code == "already_claimed"` while one with the current value succeeds. Run
      `cargo test -p of-mcp --test tools` and confirm it passes.
- [ ] **Full-workspace sweep and gate:** run `cargo build --workspace --tests` once more
      to confirm zero missing call sites remain anywhere (`crates/of-web` and
      `crates/of-billing` do not call these `Tx` methods directly per the earlier grep, but
      re-confirm with `grep -rln "\.complete_job(\|\.fail_job(\|\.cancel_job(\|\.renew_claim("
      crates --include="*.rs"` and diff against the File Structure table above — any
      surprise hit gets added to this task before proceeding). Then run the full gate:
      `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`,
      `cargo fmt --all`.
- [ ] **Format and commit:** `cargo fmt --all`, then
      `git add crates/of-core/src/jobs.rs crates/of-core/tests/jobs.rs crates/of-core/tests/queue.rs crates/of-core/tests/isolation.rs crates/of-mcp/src/tools/jobs.rs crates/of-mcp/tests/tools.rs`
      and `git commit -m "of-core: fence complete_job/fail_job/cancel_job/renew_claim by claim generation"`.

**Out-of-band artifacts touched:** none — no migration, no `web/` change, no container/CI
change. State this explicitly in the PR body per Phase 5.
