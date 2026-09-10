# Leases: generalize (repo, branch) to (repo, resource) design

> **Status:** DRAFT — closes savvagent/otto-factory#69.
>
> **PR review round found six further corrections, all applied before merge:** (1) the
> migration's backfill `UPDATE` ran under `repo_leases`' `FORCE ROW LEVEL SECURITY` with no
> `app.org_id` set, so it silently affected zero rows on any deployment where FORCE actually
> matters (managed Postgres) — empirically verified both broken and fixed against a real
> `FORCE ROW LEVEL SECURITY` table; the migration now suspends FORCE for the owner-run
> backfill and restores it immediately after, with the reasoning as an inline comment. (2)
> The backfill's `WHERE resource NOT LIKE 'branch:%'` guard could collide two distinct
> branches (`main` and a branch literally named `branch:main`) and abort the migration;
> it now prefixes unconditionally. (3) The `DROP INDEX`/`CREATE UNIQUE INDEX` pair was a
> no-op — `RENAME COLUMN` already carries an index's definition forward — and is removed.
> (4) §4's resolution logic now refuses `resource` and `branch` both being given
> (`invalid_params`) instead of silently preferring `resource`, and treats a blank
> `resource` the same as an absent one (falling back to `branch`) rather than masking a
> valid alias — both fixed several real "guessed instead of stopped" bugs three independent
> reviewers converged on. (5) `Lease.resource` gained a `MAX_RESOURCE_LEN` (200 bytes,
> matching `idempotency_key`'s precedent) — unbounded, the column could grow arbitrarily on
> a table whose rows are never deleted, and an oversized value would have failed at
> Postgres's btree index limit as a retriable internal error. (6) A documentation sweep
> found "branch" surviving in the MCP server's top-level `INSTRUCTIONS` (the first thing an
> agent reads, unchanged by this PR's other description updates), the `renew_claim` tool
> description, `crates/of-web/src/openapi.rs`'s hand-written `Lease` schema (which still
> marked `branch` required — a public, unauthenticated document contradicting the actual
> response), and several internal doc comments; all updated, and a new test
> (`the_lease_schema_matches_the_wire_field_it_actually_returns`) guards the OpenAPI schema
> specifically, since nothing else would catch that class of drift on a hand-written schema.
> See the `Post-review correction:` notes inline in §1 and §4 for detail.

## Goal & Success Criteria

`repo_leases` is keyed `(repo_id, branch)`. Branch is the right first resource for two
agents to contend over, but it is not the only one: two agents on *different* branches of
the same repo can still both try to run a migration against a shared dev database, both
grab the one staging deploy slot, or both rewrite the same fixture, and the lease system
has nothing to say about any of that today. Generalizing `branch` to a free-form `resource`
— with `branch:main` as the conventional value for the case that exists today — buys the
whole class without the server taking an opinion about what a "resource" is. It stays
repo-anchored (constraint 1 is unchanged: there is no lease floating free of a repo), and it
is a strict generalization of an existing primitive rather than a new concept, which is the
test for belonging in substrate rather than in a customer's own skill (constraint 2).

Success:

- `repo_leases.branch` becomes `repo_leases.resource` via a forward-only migration that
  rewrites every existing value to `branch:<old value>` — the conventional form for the one
  resource kind that exists in production today. Existing rows keep their meaning: a lease
  that was blocking `main` still blocks the same thing under its new name.
- The partial unique index still guarantees at most one live lease per `(repo, resource)`,
  and the expiry reap inside `acquire_lease` is unchanged in shape — same reap-then-insert
  pattern, now keyed on `resource`.
- `Tx::acquire_lease` / `renew_lease` / `release_lease` / `list_leases` and the `Lease`
  struct speak `resource` throughout `of-core`.
- The MCP surface (`acquire_lease`) accepts a free-form `resource` argument. `branch`
  survives as a **deprecated input alias**: a caller that passes `branch` instead of
  `resource` gets it silently translated to `branch:<value>` and a lease is acquired as
  today, so no currently-deployed client breaks. `renew_lease` / `release_lease` are
  unaffected (they always addressed a lease by id, never by branch). `list_leases`'
  *output* changes shape (see Public interface note) but takes no `resource`/`branch` input
  either way.
- The resource string is never validated against a list or a format — same reasoning as
  `agent_type` (`CLAUDE.md`, constraint 3): a customer's own skill decides what "a staging
  slot" or "a migration lock" looks like, and the server has no business rejecting a value
  it doesn't recognize.
- `Error::LeaseHeld` names the resource and reads correctly for a non-branch resource — the
  message stops assuming "branch" is the right noun.
- `acquire_lease`'s tool description explains the `branch:main` convention plainly enough
  that an LLM caller picks it over inventing a parallel scheme for the branch case, and
  explains that a free-form resource works for anything else.
- `docs/specs/2026-09-01-otto-factory-design.md`'s open risk 4 ("lease semantics are
  advisory") is restated in resource terms rather than branch terms.
- The console (`web/`) and its API route continue to work end-to-end against the renamed
  field: the Repos page still shows what is leased, now labeled generically.

## Public interface note (Non-Negotiable Rule 6)

This is a **breaking change to two public interfaces**, both accepted as the point of the
issue rather than an accidental side effect:

1. **MCP tool output.** `acquire_lease`, `renew_lease`, `release_lease`'s implicit lease
   payload, and `list_leases` all return `Lease` objects. The field named `branch` in that
   object becomes `resource`. There is no dual-field output compatibility shim — the issue
   asks the four tools to "speak resource," and keeping `branch` in the output alongside
   `resource` would let a caller silently read a stale field forever instead of adapting.
   Because every crate in this workspace is still `0.1.0` (no interface-versioning promise
   exists yet — `CLAUDE.md`'s Non-Negotiable Rule 6 treats this as the normal case pre-1.0
   as long as the break is named, which this section does), and because
   `docs/clients/matrix.md`'s existing lease mentions are a dated conformance record of a
   specific run rather than living API documentation (see Assumptions), this does not
   require a matrix.md update — it is noted here for the architect reviewer instead.
2. **MCP tool *input*.** `acquire_lease`'s `branch` argument becomes optional and a new
   `resource` argument is added. `branch` is kept as a deprecated alias (see Success
   criteria) specifically because input compatibility is cheap and a broken input is a
   silent failure for a caller that hasn't updated, where a broken output is at least
   visible as a missing field.
3. **Console REST API.** `GET /api/orgs/{org}/repos/{repo}/leases` returns `Lease[]`
   directly from `of-core` (`of-web`'s "payloads are of-core's own domain types," per
   `CLAUDE.md`), so the same `branch` → `resource` rename applies there. `web/src/lib/types.ts`
   and the Repos page are updated in the same change (§5).

## Scope

**In:**

- The `repo_leases` schema, the forward-only migration, and every `of-core::leases` function
  and the `Lease` struct.
- `Error::LeaseHeld`'s field and message.
- `acquire_lease`'s MCP input schema and all four lease tools' descriptions.
- The console's `Lease` type and the Repos page's lease display.
- `docs/specs/2026-09-01-otto-factory-design.md`'s repo-leases paragraph and open risk 4.
- Cross-org and same-repo lease tests updated to use `resource` (renamed, not new — the
  existing `queue.rs` and `isolation.rs` lease tests already cover the behavior this change
  must preserve).

**Out:**

- Any new resource *kind* beyond what agents choose to pass. The server ships no catalog of
  "staging slot" or "migration lock" — that decision belongs in a customer's own skill,
  exactly as `default_agent_type` is never validated against a list of known agents. Adding
  such a catalog would be exactly the kind of workflow opinion constraint 2 rules out.
- Any change to lease *semantics* — advisory, time-bounded, reaped-then-inserted, renew
  = renew-if-held-by-you. Only the key's shape changes, not what a lease means.
- A dual-write / dual-read compatibility period for the *output* field (see Public
  interface note §1 for why).
- Any change to `repo_leases`' tenant isolation. The table is already in `0007_rls.sql`'s
  `tenant_tables` array with an `org_id`-scoped policy; renaming a non-`org_id` column does
  not touch that guard, and no new migration entry there is needed.
- `docs/clients/matrix.md` (see Public interface note §1 and Assumptions).

## Assumptions

1. **The conventional prefix is `branch:`, applied only to the branch case.** The issue
   names `branch:main` as *the* conventional value for what is today a bare branch name.
   The migration therefore rewrites `repo_leases.branch` values as `'branch:' || branch`,
   not a bare copy — the whole point of a convention is that it's visible in the data, not
   just in documentation new callers might not read.
2. **`docs/clients/matrix.md` is a dated historical record, not living API documentation.**
   It opens with "What was verified, in order" for a specific conformance run on 2026-09-02,
   and its lease mentions (`widget@main`, the exact `LeaseHeld` message text of that run)
   describe what that run observed, not a contract future runs must match byte-for-byte.
   Editing it to match today's error text would misrepresent what was actually captured.
   Left as-is, a reader still gets the right idea (a lease names what's held and who holds
   it) even though the exact resource syntax has since generalized.
3. **`branch` stays a deprecated alias on `acquire_lease` input, not on output, and not
   forever.** The issue explicitly frames this as a live surface with real callers and
   leaves survival of `branch` as a spec decision. Input aliasing is cheap and prevents a
   silent break for anything already calling `acquire_lease(branch: "...")`; keeping it
   accepted has no cost to new callers, who simply use `resource` per the tool description.
   No removal date is set — removing a deprecated-but-harmless input alias is a separate,
   later decision this spec does not need to make.
4. **No console-side deprecation banner or migration UI is needed.** The Repos page reads
   `lease.resource` and displays it verbatim (already generic — it was already just
   rendering the branch string in a `<span>`, not interpreting it), so the rename is a
   one-line change with no new UX surface.
5. **The `default_branch` column on `repos` is untouched.** It is a different concept (the
   repo's configured default branch, used for PR conventions) and is not a lease resource;
   nothing in this change touches it.

## §1 Schema

New migration `crates/of-core/migrations/0027_lease_resource.sql`, forward-only per
`CLAUDE.md`'s migration rule — `0002_repos.sql`'s `repo_leases` definition is never edited:

```sql
ALTER TABLE repo_leases RENAME COLUMN branch TO resource;

ALTER TABLE repo_leases NO FORCE ROW LEVEL SECURITY;
UPDATE repo_leases SET resource = 'branch:' || resource;
ALTER TABLE repo_leases FORCE ROW LEVEL SECURITY;
```

**Post-review correction:** the original draft of this section additionally guarded the
`UPDATE` with `WHERE resource NOT LIKE 'branch:%'` and followed the rename with a
`DROP INDEX`/`CREATE UNIQUE INDEX` pair, and neither survived review.

The `WHERE` guard is gone because it could **collide** two distinct pre-existing branches: a
git branch can itself be named `branch:main`, and the guard would map both it and a plain
`main` to the identical string `branch:main`, aborting the migration on the unique index
below. Prefixing unconditionally keeps every row distinct (the one branch that already
collided with the convention gets a harmless double prefix, `branch:branch:main`, rather
than losing its identity) — correct, not merely simpler; there is no re-run case to be
idempotent *for*, since sqlx runs each migration exactly once and each inside its own
transaction.

The index drop/recreate is gone because it was a no-op: `ALTER TABLE ... RENAME COLUMN`
already carries a dependent index's definition forward to the new column name (verified —
`repo_leases_live_key` reads `(repo_id, resource) WHERE released_at IS NULL` immediately
after the rename, with no further statement), so dropping and recreating it under the same
name only took an `ACCESS EXCLUSIVE` lock to reproduce what already existed.
`repo_leases_org_expiry_idx` does not reference the column and needs no attention either
way. The `repo_leases_notify` trigger (`0003_jobs.sql`) fires on the table, not a named
column list, so it needs no change.

**The `NO FORCE`/`FORCE` pair is load-bearing, not decorative**, and its absence was the one
finding in this whole change that would have shipped a silent data-correctness bug:
`repo_leases` is a tenant table under `FORCE ROW LEVEL SECURITY` (`0007_rls.sql`) whose
policy is `USING (org_id = current_org())`. A migration connection never sets
`app.org_id` — there is no tenant transaction to pin one — so `current_org()` reads NULL and
the policy's predicate is never `TRUE` for any row. A bare `UPDATE` (as originally drafted)
would therefore **silently affect zero rows** on exactly the deployment shape where FORCE
matters at all: managed Postgres, where the migrating role is the table's owner and is
neither a superuser nor `BYPASSRLS`. This was invisible everywhere the work got checked
before review — the local compose role and the `#[sqlx::test]` connecting role are both
superusers, which bypass RLS regardless of FORCE, so the backfill "worked" in every test and
every local run and would only have failed the one place `CLAUDE.md`'s own RLS section warns
about. Verified empirically both ways against a real `FORCE ROW LEVEL SECURITY` table:
`UPDATE 0` with the bare statement, `UPDATE 1` (and the row correctly rewritten) with the
`NO FORCE`/`FORCE` pair around it, run as the table's non-superuser owner in both cases. The
consequence of shipping the bug would have been a split lease keyspace with no error: rows
would stay spelled `main` while every new caller — including the `branch` alias this PR adds
specifically to keep old callers working — writes `branch:main`, so two agents could each
successfully lease "the same" branch and neither would see `lease_held`. Suspending FORCE
only around this one statement, run as the table's owner (which the migration already must
be, to `RENAME COLUMN` and to have created the table in the first place), lets the owner's
own migration see and rewrite every row; it is restored in the same migration before commit.

## §2 `of-core::leases`

- `Lease.branch: String` → `Lease.resource: String`. `LEASE_COLS` updates to list
  `resource` instead of `branch`.
- Every function's `branch: &str` parameter renames to `resource: &str`; every SQL
  statement's `branch = $n` predicate renames to `resource = $n`. The reap-then-`FOR UPDATE`-then-insert
  shape in `acquire_lease` is otherwise unchanged — reaping and the live-lease
  check still happen "on this resource" instead of "on this branch," same logic.
- The empty-string guard (`if branch.is_empty()`) becomes `if resource.is_empty()`, with the
  error message updated to say "lease resource must not be empty."
- Doc comments (`leases.rs`'s module doc, `acquire_lease`'s doc comment) drop "branch" in
  favor of "resource," keeping the advisory/time-bounded reasoning verbatim since that
  reasoning is unchanged.

**Post-review correction:** a new `MAX_RESOURCE_LEN` constant (200 bytes, matching
`idempotency::MAX_KEY_LEN`'s reasoning — long enough for any reasonable caller-chosen name,
short enough that a released lease's row, kept for history and never deleted, cannot become
unbounded free storage) caps `resource`'s length in `acquire_lease`, refused with a
non-retriable `Error::Invalid` above the limit. Generalizing past a git branch name removed
the informal length ceiling a branch name used to imply; unbounded, an oversized value would
have failed only at Postgres's btree index-row-size limit, surfacing as a retriable
`internal_error` an agent could loop on forever.

## §3 `Error::LeaseHeld`

```rust
#[error("{resource} of this repo is leased by {holder} until {expires_at}")]
LeaseHeld {
    resource: String,
    holder: String,
    expires_at: chrono::DateTime<chrono::Utc>,
},
```

For the branch case this reads "`branch:main` of this repo is leased by agent-a until
2026-09-02 21:58:20 UTC" — slightly more verbose than today's "main of this repo is leased
by...", but correct for every resource shape, which today's message is not ("a staging slot
is leased by..." reads fine; "main of this repo" reading as a branch name is what breaks for
a non-branch resource). No behavior change to `code()` (`"lease_held"`) or `retriable()`
(still `true`) — only the field name and message text move.

## §4 MCP surface (`of-mcp::tools::coord`)

`AcquireLeaseArgs`:

```rust
pub struct AcquireLeaseArgs {
    /// What you are taking exclusive use of, e.g. `branch:main` for a git branch,
    /// or a free-form name for anything else you and your team need to serialize —
    /// a staging slot, a migration lock, a shared fixture. Never validated against
    /// a list: pick a name your team will reuse consistently. Prefer this over
    /// `branch`, which is kept only for callers that have not moved to `resource` yet.
    #[serde(default)]
    pub resource: Option<String>,
    /// Deprecated: the branch you are about to work on. Equivalent to passing
    /// `resource: "branch:<branch>"`. Use `resource` instead.
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub job: Option<String>,
    #[serde(default)]
    pub ttl_seconds: Option<i64>,
}
```

Handler resolves the resource before calling `tx.acquire_lease`. A blank string and an
absent field are folded together (`.map(str::trim).filter(|s| !s.is_empty())`) before
matching, then:

```rust
let resource = match (resource, branch) {
    (Some(_), Some(_)) => {
        return Err(ErrorData::invalid_params(
            "acquire_lease got both resource and the deprecated branch; pass only \
             one (resource, or branch as a shorthand for \"branch:<branch>\")",
            None,
        ))
    }
    (Some(r), None) => r.to_string(),
    (None, Some(b)) => format!("branch:{b}"),
    (None, None) => {
        return Err(ErrorData::invalid_params(
            "acquire_lease needs resource (or the deprecated branch)",
            None,
        ))
    }
};
```

**Post-review correction:** the original draft matched `(args.resource.as_deref(),
args.branch.as_deref())` directly (no pre-filtering) with `(Some(r), _) => ...` as the first
arm and no `(Some(_), Some(_))` arm, on the reasoning that "passing both is not an error —
`resource` simply wins." Three independent reviewers converged on the same two defects in
that shape, both closed by the version above:

1. **Silently preferring `resource` over a simultaneously-supplied `branch` is a guess, and
   `CLAUDE.md`'s own style rule says "errors that guess are worse than errors that stop."**
   A caller who meant the `branch` alias — populated, say, by a client that always sends
   `resource` with a stale or empty default — would have leased whatever `resource` happened
   to contain, told the call succeeded, with no signal that `branch` was ignored. Both given
   is now refused outright, naming both fields.
2. **A blank `resource` masked a perfectly good `branch`.** `(Some(r), _)` matched on `Some`
   regardless of content, so `resource: Some("")` alongside `branch: Some("main")` took the
   first arm, trimmed to an empty string, and only failed later inside `of-core`'s own
   guard — after this handler had already opened a transaction and charged the meter for a
   call that was always going to fail, and without ever trying the `branch` alias that was
   right there. Filtering both fields to `None` when blank, before matching, means a blank
   `resource` is now indistinguishable from an absent one: it falls through to `branch` if
   present, exactly like a caller who omitted `resource` entirely — which is what the
   deprecated alias exists to keep working. This also fixes an asymmetry the same reviewers
   flagged: the `branch` arm always rejected an empty value before doing any work, while the
   `resource` arm did not, so two structurally identical "you gave me nothing usable"
   mistakes read as two unrelated failures depending on which field was blank.

The `AcquireLeaseArgs` doc comments for both fields now say "pass exactly one of `resource`
or `branch`, never both" to match.

Tool description updated to state the convention explicitly, including the corrected
precedence rule and — per an architect-review finding — that a lease is scoped to one repo,
so the free-form examples it cites (a staging slot, a migration lock) do not serialize
*across* repos the way the prose alone might suggest:

> "Announce that you are taking exclusive use of something in a repository — a branch, or
> anything else your team needs to serialize on, such as a staging slot or a migration
> lock — so other agents can see it and go elsewhere. Two agents in two different
> repositories can hold the same resource name independently; a lease only ever serializes
> within one repository. Pass `resource` as a free-form name; for a branch, use the form
> `branch:<name>` (e.g. `branch:main`) so every caller converges on the same spelling — a
> bare `main` and `branch:main` are different resources. `branch` is accepted as a
> deprecated shorthand for `resource: "branch:<branch>"`; pass one or the other, never both.
> Take one before you start and renew it while you work. If someone already holds it the
> error names them and says when it expires, so you can wait, message them, or pick
> different work. Leases are advisory: the server cannot see your git operations, so this
> makes collisions visible rather than impossible."

`renew_lease` and `release_lease` currently name "the branch" explicitly ("another agent may
take the branch while you are still in it"; "freeing the branch immediately instead of
waiting for it to expire") — exactly the branch-specific wording the rest of this section
generalizes away from `Error::LeaseHeld`'s message, and for the same reason: read against a
staging-slot or migration-lock resource, "freeing the branch" is confusing or wrong. Both
descriptions are updated to say "the resource" in place of "the branch." `list_leases`
description changes "the holder, the branch, and when each expires" to "the holder, the
resource, and when each expires."

`LeaseOut` / `LeasesOut` in `of-mcp::tools::out` need no code change — they wrap `of_core::leases::Lease`
directly, so the field rename flows through automatically once `of-core` changes; this is
exactly the reuse `out.rs`'s own doc comment argues for (no parallel view struct to
independently update).

`of-billing::classify` keys lease entries by tool name (`acquire_lease`, `renew_lease`,
`release_lease`, `list_leases`), none of which change, so no metering update is needed.

## §5 Console (`web/`)

- `web/src/lib/types.ts`: `Lease.branch: string` → `Lease.resource: string`; doc comment
  updated the same way as `of-core`'s.
- `web/src/routes/o/[org]/repos/+page.svelte:364`: `{lease.branch}` → `{lease.resource}`.
  No new i18n string is needed — the surrounding label text (`repos_show_leases`,
  `repos_lease_expires`, etc.) is already resource-agnostic; only the interpolated value
  changes.
- `web/src/lib/api.ts`'s `leases()` call needs no change — it returns `Lease[]` untyped
  beyond the interface.

## Error Handling & Edge Cases

- **Empty or absent `resource`.** `resource: Some("")` (whitespace-only or empty) and
  `resource: None` are treated identically by §4's pre-match filtering — both fall back to
  `branch` if given, or to the "neither given" error if not. A non-empty `resource` that
  somehow reached `of-core` empty (there is no such path today, since §4 filters first) would
  still be caught by `of-core`'s own `resource.is_empty()` guard — belt and braces, not a gap.
- **Empty or absent `branch`.** Symmetric to `resource`: `branch: Some("")` and `branch:
  None` are both folded to "absent" before matching, so an empty-string `branch` never
  reaches the `format!("branch:{b}")` step that would otherwise produce the meaningless
  resource `"branch:"`.
- **Both `resource` and `branch` given (both non-blank).** `invalid_params` naming both
  fields (§4) — refused rather than guessed, per the post-review correction in §4.
- **Both `resource` and `branch` omitted (or both blank).** `invalid_params` naming
  `resource` (§4) — today's `AcquireLeaseArgs::branch` is a required field so serde already
  rejects a missing branch; making both optional means the "neither given" case must be
  checked explicitly now that serde can no longer do it for us.
- **An oversized `resource`.** Rejected by `of-core`'s `MAX_RESOURCE_LEN` guard (§2) with a
  non-retriable `Error::Invalid` naming the byte count and the limit.
- **A resource string containing `:` that isn't `branch:...`.** Never rejected — free-form
  means free-form; a customer's skill might reasonably use `deploy:staging` or
  `fixture:accounts-db`.
- **Migration data correctness under RLS.** See §1's `Post-review correction` — the backfill
  now suspends `FORCE ROW LEVEL SECURITY` for its own duration rather than relying on a
  guard clause, since the guard clause was never the risk that mattered here.
- **Existing live leases across the migration boundary.** A lease acquired on `main` before
  the migration and still live after it is transparently `branch:main` afterward — the
  unique index still enforces exclusivity on the same logical resource (carried forward by
  `RENAME COLUMN` with no further action, per §1), and `list_leases` during the deploy window
  returns the new field name with the migrated value.

## Risks & Open Questions

- **Restating open risk 4.** `docs/specs/2026-09-01-otto-factory-design.md`'s "Lease
  semantics are advisory" risk is updated from "The server cannot see git operations, so a
  determined agent can ignore a lease" to name resources generally: "The server cannot see
  what an agent actually does with a leased resource — a git operation, a migration run, a
  deploy — so a determined agent can ignore any lease it holds. This is documented, not
  hidden; leases make collisions visible, not impossible." (Applied as part of this change,
  not deferred.)
- **The `branch:` prefix is a convention, not a type.** Nothing stops a caller from passing
  `resource: "branch:main"` directly instead of the deprecated `branch` field — that is in
  fact the intended steady state once `branch` is eventually removed, and the tool
  description says so.
- **No removal timeline for the `branch` alias.** Per Assumption 3, this spec deliberately
  does not set one; removing it later is a small, separate, easy-to-scope change once real
  callers are observed to have moved on (or never depended on it in the first place — this
  server has no production traffic history to check yet).
- **Known, documented, not-fixed-in-this-PR limitations found in review** — real, but
  pre-existing (not introduced by this change) or a genuinely separate concern, deliberately
  left as follow-ups per "the same bug pattern is discovered elsewhere — file a follow-up,
  do not silently widen scope":
  - **A lost `acquire_lease` race on a resource with no existing row reports a retriable
    internal error instead of `lease_held`.** `acquire_lease`'s reap → `SELECT ... FOR
    UPDATE` → `INSERT` shape means two concurrent first-time acquires both see no row to
    lock, both attempt the `INSERT`, and the loser's `sqlx::Error::Database` (unique
    violation) surfaces through the generic `Error::Db` path as `"the server could not
    complete this call; retry shortly"` rather than naming the winner and the expiry the way
    a losing `acquire_lease` normally does. This predates this change (the same reap-then-
    insert shape existed for `branch`), but generalizing to free-form resources plausibly
    raises how often it fires — more agents contending on fewer, coarser resources (one
    staging slot vs. many branch names) than before. Fixing it means matching
    `sqlx::Error::Database` on the `repo_leases_live_key` constraint name and re-reading the
    live row to answer with `Error::LeaseHeld` — a real fix, but a distinct one from this
    issue's scope.
  - **`list_leases` has no result cap.** Previously bounded in practice by "one lease per
    branch actively worked"; a free-form resource namespace has no natural ceiling. Not
    fixed here because it is a design choice (add a `LIMIT`, and decide what the MCP tool
    description promises about it) rather than a mechanical follow-on of the rename.
