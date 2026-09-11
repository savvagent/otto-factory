# `Db::audit_global_on` takes an `Unpinned` transaction, not any `PgExecutor`

> **Status:** IMPLEMENTED — closes `savvagent/otto-factory#133`, filed during PR #131's mandatory
> review trio (`docs/specs/2026-09-10-passkey-registration-atomic-audit-design.md`'s Risks & Open
> Questions), which landed a doc-comment warning plus a runtime regression test
> (`audit_global_on_refuses_a_pinned_connection`) as the cheap interim mitigation. This spec is the
> stronger fix both reviewers asked for: make the misuse unrepresentable at the type level. Shipped
> in PR #165. The mandatory review trio on #165 converged on a residual gap the type alone could not
> close — a caller could still pin an `Unpinned` by hand via `conn()` before calling
> `audit_global_on` — closed in the same PR by a runtime guard inside `audit_global_on` itself (see
> its doc comment) plus a restored DB-level policy test independent of the Rust API.

## Goal & Success Criteria

`Db::audit_global_on<'e, E: sqlx::PgExecutor<'e>>(conn: E, e: Entry)` (`crates/of-core/src/
audit.rs`) is generic over any `PgExecutor`, which includes `Tx::conn()` — a pinned connection with
`app.org_id` already set. Calling it there compiles today, and the outcome is deployment-shape-
dependent: RLS-enforced deployments reject the write at runtime (proven by
`audit_global_on_refuses_a_pinned_connection`); RLS-bypassed deployments (this deployment's actual
shape today, per `docs/deploy/fly.md`) silently write a `NULL`-org row no tenant's own audit trail
will ever show.

- `Db::begin_unpinned` returns a new `Unpinned` type instead of a bare `Transaction<'static,
  Postgres>`. `Unpinned` mirrors `Tx`'s own shape exactly: a private field, a `conn()` accessor, and
  `commit`/`rollback` consuming methods — the same three members `Tx` exposes, for the same reason.
- `Db::audit_global_on` takes `&mut Unpinned` instead of `E: sqlx::PgExecutor<'e>`. A pinned `Tx`
  exposes only `&mut PgConnection` via `Tx::conn()`, never an `Unpinned` — there is no accessor, on
  either type, that can produce one from the other. The misuse the issue names becomes a type
  mismatch the compiler rejects, not a runtime check.
- Every existing call site of `begin_unpinned` keeps compiling with no change to its own logic,
  because `Unpinned::conn()` returns the same `&mut sqlx::PgConnection` the old `&mut *tx` pattern
  did — the only site whose *behavior* is caller-visible is `finish_registration`'s
  `Db::audit_global_on` call, which now passes `tx.conn()` instead of `&mut *tx` (equivalent output,
  required because the parameter type is no longer generic).
- `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --all
  --check` all pass. `audit_global_on_refuses_a_pinned_connection`'s scenario (passing a pinned
  `Tx`'s connection to `audit_global_on`) no longer compiles at all — the strongest possible proof —
  so that runtime test is removed and replaced by a comment at its old location explaining why no
  runtime test is needed (Testing, below).

## Premise corrections

None. The issue's description of the problem, the current signatures of `Db::audit_global_on` and
`Db::begin_unpinned`, and `finish_registration`'s single call site all match the current tree
exactly (`crates/of-core/src/audit.rs` lines 255–260, `crates/of-core/src/db.rs` line 188,
`crates/of-auth/src/passkeys.rs` lines 268–297, as of this writing).

## Scope

**In:**

- A new `Unpinned` type in `crates/of-core/src/db.rs`, beside `Tx`, wrapping
  `Transaction<'static, Postgres>`.
- `Db::begin_unpinned`'s return type changes from `Result<Transaction<'static, Postgres>>` to
  `Result<Unpinned>`.
- `Db::audit_global_on`'s parameter changes from `conn: E where E: sqlx::PgExecutor<'e>` to
  `conn: &mut Unpinned`.
- Every call site of `begin_unpinned` across the workspace updated to keep compiling: production
  code in `crates/of-auth/src/tokens.rs` (2 sites), `crates/of-auth/src/passkeys.rs` (2 sites,
  one of which also calls `audit_global_on`), `crates/of-auth/src/ratelimit.rs` (2 sites),
  `crates/of-core/src/orgs.rs` (1 site), `crates/of-core/src/invites.rs` (1 site); test code in
  `crates/of-core/tests/isolation.rs` (9 sites).
- Removing `audit_global_on_refuses_a_pinned_connection` (its scenario no longer compiles) and
  replacing it with a short comment at the same location recording why.

**Out:**

- **No change to `Tx`, `Db::audit_global`, `Db::audit_for_org`, or `Tx::audit`.** Only the unpinned,
  no-org write path changes. `Tx::conn()` keeps its existing `&mut sqlx::PgConnection` return type —
  changing it to something narrower is a separate, much larger refactor (every tenant-scoped query
  in the crate uses it) that this issue does not ask for and Load-Bearing Invariant 3 does not
  require.
- **No change to `finish_registration`'s public parameter list or its transactional shape** — it
  already opens one `begin_unpinned` transaction and commits once (`#108`'s fix); this issue only
  changes what type that transaction has and how `audit_global_on` is called on it.
- **No `Deref`/`DerefMut` impl on `Unpinned`.** A blanket `Deref<Target = PgConnection>` would let
  `&mut *tx` keep compiling unchanged at every call site with a smaller diff, but it would also mean
  `Unpinned` supports exactly the same "hand out a bare connection to anything generic over
  `PgExecutor`" pattern this issue exists to close off for `audit_global_on` — a future second
  caller reaching for `&mut *unpinned_tx` to satisfy some other `E: PgExecutor` bound would work by
  the same accident this issue removes. Mirroring `Tx`'s `conn()`-accessor shape exactly, with no
  `Deref`, is what "mirrors guard 1's own pattern" (the issue's own words) actually means: `Tx` does
  not implement `Deref` either.
- **No `of-billing::classify` change.** No MCP tool is added.
- **No new tenant table, no RLS policy change.** `Unpinned` does not touch tenant data; it is a
  vehicle for the no-org control-plane write path `begin_unpinned` already served.
- **No `trybuild` (or other compile-fail-testing) dependency added.** The removed runtime test's
  scenario is now a plain type error surfaced by `cargo build`/`cargo test` themselves — anyone who
  reintroduces the misuse (e.g., a future refactor that widens `audit_global_on`'s parameter back to
  a generic bound, or gives `Tx` an accessor that exposes its raw `Transaction`) breaks the build
  immediately, on every CI run, with no special test harness required. Adding a dependency to prove
  a fact the ordinary compiler run already proves is unjustified scope.

## Public-interface changes

| Surface | Change | Breaking? |
|---|---|---|
| `of_core::Db::begin_unpinned` (crate-public within the workspace) | Return type changes from `Result<Transaction<'static, Postgres>>` to `Result<Unpinned>` | Source-breaking for any caller depending on the concrete `sqlx::Transaction` return type, but every current caller (`Unpinned::conn()` returns the identical `&mut sqlx::PgConnection` the old `&mut *tx` deref gave them) needs at most a one-line change from `&mut *tx` to `tx.conn()`. Per CLAUDE.md, `of-core` has no external consumers — this is an internal Rust API, not one of the interfaces Non-Negotiable Rule 6 names (MCP tool surface, console REST API, OAuth/discovery endpoints, config surface, DB schema). Not a public-interface change in the Rule 6 sense; called out here anyway per the architect-reviewer flag this issue asks for |
| `of_core::Db::audit_global_on` (crate-public within the workspace) | Parameter changes from `conn: E where E: sqlx::PgExecutor<'e>` to `conn: &mut Unpinned` | Same reasoning as above: internal, not a Rule 6 surface. The only caller, `finish_registration`, is updated in the same change |
| `of_core::db::Unpinned` (new, crate-public within the workspace) | Additive — a new type | No |
| MCP / Console REST / OAuth / Config / Schema | None | — |

This is a backward-compatible internal refactor per Non-Negotiable Rule 6: no MCP tool, console
route, OAuth/discovery endpoint, config key, or migration changes shape. Flagged explicitly to the
architect reviewer per the issue's own request, and because Rule 6 asks that a signature change be
named rather than discovered in the diff even when it falls outside the rule's binding definition of
"public interface."

## §1 `Unpinned` — mirroring `Tx`'s shape for the unpinned, no-org path

`crates/of-core/src/db.rs`, beside `Tx`:

```rust
/// A transaction opened via [`Db::begin_unpinned`] — no [`OrgId`] is attached
/// and no `app.org_id` is ever set on it. Exists so [`Db::audit_global_on`]
/// can require one specifically, the same way [`Tx`] cannot be constructed
/// without an `OrgId` in the first place: `Tx::conn()` hands out a bare
/// `&mut PgConnection`, never an `Unpinned`, and `Unpinned`'s own field is
/// private, so nothing outside this module can wrap one around a pinned
/// connection either. The two types deliberately do not implement a common
/// trait or `Deref` to the same target — collapsing them back to "anything
/// that can run a query" is exactly the shape this type exists to rule out.
pub struct Unpinned {
    tx: Transaction<'static, Postgres>,
}

impl Unpinned {
    /// Borrow the underlying executor for a query. Mirrors [`Tx::conn`]
    /// exactly.
    pub fn conn(&mut self) -> &mut sqlx::PgConnection {
        &mut self.tx
    }

    pub async fn commit(self) -> Result<()> {
        self.tx.commit().await?;
        Ok(())
    }

    pub async fn rollback(self) -> Result<()> {
        self.tx.rollback().await?;
        Ok(())
    }
}
```

`Db::begin_unpinned` changes from:

```rust
pub async fn begin_unpinned(&self) -> Result<Transaction<'static, Postgres>> {
    Ok(self.pool.begin().await?)
}
```

to:

```rust
pub async fn begin_unpinned(&self) -> Result<Unpinned> {
    Ok(Unpinned {
        tx: self.pool.begin().await?,
    })
}
```

Its doc comment (the deployment-shape warning about unscoped reads/writes against tenant tables)
carries over unchanged — nothing about *what* the transaction is unpinned from changes, only its
Rust type.

## §2 `Db::audit_global_on`

`crates/of-core/src/audit.rs`:

```rust
/// Record a global (no-org) event on an unpinned transaction the caller
/// already holds open — typically one that also carries the change the
/// event describes, so both commit or roll back together.
///
/// Takes `&mut Unpinned` rather than `E: sqlx::PgExecutor<'e>` on purpose:
/// the earlier generic signature compiled against a pinned `Tx`'s
/// connection too, which produces a `NULL`-org row that is either rejected
/// at runtime (RLS-enforced deployments) or silently unreachable in any
/// tenant's own audit trail (RLS-bypassed deployments — see
/// `Db::begin_unpinned`'s own doc comment for why nothing in this crate may
/// assume which shape it is running under). `Unpinned` is only ever
/// produced by [`Db::begin_unpinned`]; a pinned `Tx` has no accessor that
/// yields one, so the misuse is now a type error the compiler catches
/// wherever a caller reaches for it, not a policy `WITH CHECK` catching it
/// at runtime or a doc comment asking nicely.
///
/// Unlike [`Self::audit_global`], a failure here is **not** swallowed: it
/// propagates to the caller, who is expected to let it abort the
/// transaction. Use this only when a lost audit row would be worse than
/// failing the whole operation.
pub async fn audit_global_on(conn: &mut crate::db::Unpinned, e: Entry) -> Result<()> {
    e.write(None, conn.conn()).await
}
```

`Entry::write`'s own signature (`async fn write<'e, E>(self, org: Option<OrgId>, conn: E) -> Result<()>
where E: sqlx::PgExecutor<'e>`) is unchanged — `conn.conn()` still yields a `&mut sqlx::PgConnection`,
which satisfies it exactly as before. The generic bound moves from `audit_global_on`'s own signature
(where it was too permissive) to `Entry::write`'s (where it is correct: that helper is intentionally
shared by `Tx::audit`, `Db::audit_global`, and `Db::audit_for_org`, each of which legitimately runs
on a different executor type).

## §3 Call-site updates

**`finish_registration`** (`crates/of-auth/src/passkeys.rs`, lines 268–299) — the one caller-visible
behavior change:

```rust
let mut tx = db.begin_unpinned().await?;   // now Unpinned, was Transaction<'static, Postgres>

sqlx::query(/* … */)
    .execute(tx.conn())                     // was: .execute(&mut *tx)
    .await
    .map_err(/* … unchanged … */)?;

Db::audit_global_on(
    &mut tx,                                // was: &mut *tx
    Entry::new(action::PASSKEY_REGISTERED)
        .actor(user_id)
        .detail(serde_json::json!({ "via": via.as_str() }))
        .from_request(ip, None),
)
.await?;

tx.commit().await?;                         // unchanged — Unpinned::commit has the same signature
```

**Every other `begin_unpinned` call site** needs only `&mut *tx` → `tx.conn()` wherever the old
deref pattern fed a query's `.execute(...)`/`.fetch_one(...)`/`.fetch_all(...)`; `tx.commit()` /
`tx.rollback()` are unchanged since `Unpinned` exposes both with identical signatures to
`Transaction`'s own. Enumerated exactly, for the plan's task breakdown:

- `crates/of-auth/src/tokens.rs`: lines ~344–368 (one `execute` × 2, one `commit`) and ~480–482
  (`commit` only, no query — no change needed there beyond the inferred type).
- `crates/of-auth/src/ratelimit.rs`: lines ~115–131 (`commit` only) and ~242–253 (one
  `count_failures(&mut *tx, …)` call whose own signature is generic over `PgExecutor` and is
  unaffected — only the argument expression changes to `count_failures(tx.conn(), …)`).
- `crates/of-core/src/orgs.rs`: lines ~196–218 (`fetch_one` × 1, `execute` × 1, `commit`).
- `crates/of-core/src/invites.rs`: lines ~261–263 (`commit` only).
- `crates/of-core/tests/isolation.rs`: 9 sites (lines ~710, 774, 865, 973, 1138, 1181, 1202, 1243,
  1285), each following the same `&mut *tx` → `tx.conn()` mechanical substitution.

None of these change what SQL runs or what the test asserts — every substitution replaces one way
of reaching the same `&mut sqlx::PgConnection` with another.

## Testing

- **`audit_global_on_refuses_a_pinned_connection` (`crates/of-core/tests/isolation.rs`) is removed.**
  Its scenario — `of_core::Db::audit_global_on(tx.conn(), …)` where `tx: Tx` — no longer compiles:
  `tx.conn()` yields `&mut sqlx::PgConnection`, and `audit_global_on` now requires `&mut Unpinned`.
  A comment replaces it at the same location:

  ```rust
  // `audit_global_on_refuses_a_pinned_connection` used to live here: it proved
  // that passing a pinned `Tx`'s connection to `Db::audit_global_on` was
  // rejected at runtime by `audit_events_append`'s `WITH CHECK`. Since
  // `savvagent/otto-factory#133`, `audit_global_on` takes `&mut
  // of_core::db::Unpinned` instead of `E: sqlx::PgExecutor<'e>`, and `Tx::conn()`
  // has no way to produce one — the misuse this test caught is now a compile
  // error, which every `cargo build`/`cargo test` run already proves on every
  // commit. No runtime test can prove a stronger fact than "this does not
  // compile"; keeping a test whose only remaining job is to demonstrate that a
  // deleted test scenario is a type error is not useful.
  ```

  This satisfies the acceptance criterion ("unrepresentable", not "refused at runtime") more
  strongly than the removed test did — the removed test verified rejection under one specific
  deployment shape (RLS-enforced); the type-level fix rejects the misuse under both shapes,
  unconditionally, at compile time.
- `cargo test --workspace` — must stay green; this is a mechanical, non-behavioral refactor across
  every `begin_unpinned` call site, so no test's assertions change, only how the local `tx` variable
  is dereferenced.
- `cargo test -p of-core --test isolation` and `cargo test -p of-auth --test passkeys` specifically,
  since these are the two files with the highest concentration of touched call sites.
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.

## Error Handling & Edge Cases

- **`Unpinned::commit`/`Unpinned::rollback`'s own failure modes are unchanged** — they delegate
  directly to `Transaction::commit`/`Transaction::rollback` and propagate the same `sqlx::Error` via
  the same `#[from]` conversion already in place.
- **No change to what happens on a successful or failing `finish_registration` call** — this issue
  changes only how the existing atomic write (`#108`'s fix) is expressed in the type system, not its
  transactional behavior, its error mapping, or its audit-row shape.

## Assumptions

- **This is a backward-compatible internal refactor, not a public-interface change**, per Rule 6's
  binding definition (MCP tools, console routes, OAuth/discovery endpoints, config keys, schema) —
  `of-core` has no external consumers. Flagged explicitly to the architect reviewer regardless, per
  the issue's own request and because the *signature* change is real even though it falls outside
  Rule 6's scope.
- **Mirroring `Tx`'s exact shape (private field + `conn()` + `commit`/`rollback`, no `Deref`) is the
  right level of ceremony**, chosen over a `Deref`-based `Unpinned` that would let every non-audit
  call site keep its `&mut *tx` spelling unchanged. The `Deref` alternative has a smaller diff but
  reintroduces, for `Unpinned`, exactly the "anything generic over `PgExecutor` accepts this" shape
  the issue exists to close off for `audit_global_on` specifically — a `Deref<Target =
  PgConnection>` on `Unpinned` would satisfy `E: sqlx::PgExecutor<'e>` for any future generic
  function the same way the removed signature did, just one level removed. Explicit `conn()` calls
  at each of the ~8 non-audit sites cost a one-line, purely mechanical diff per site and buy back
  that margin permanently.
- **Removing the runtime test rather than replacing it with a `trybuild` compile-fail test** is the
  right tradeoff for this repository: `trybuild` is a new dev-dependency for `of-core`, and the fact
  it would prove (that a specific expression does not typecheck) is already proven by the ordinary
  `cargo build`/`cargo test` gate that runs on every commit — a `trybuild` fixture only adds value
  when the surrounding code *does* compile and the failure mode is otherwise invisible, which is not
  the case here (any accidental reversion breaks the whole workspace build, immediately, loudly).

## Risks & Open Questions

- **None outstanding.** The one open question the parent spec flagged (`docs/specs/2026-09-10-
  passkey-registration-atomic-audit-design.md`'s Risks & Open Questions) — "the blast radius is
  small today (one caller) and grows with every future one" — is exactly what this issue closes:
  after this change, a second caller of `audit_global_on` cannot reintroduce the misuse no matter
  how it is written, because the compiler enforces it rather than a convention.
