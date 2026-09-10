# Leases: generalize (repo, branch) to (repo, resource) design

> **Status:** DRAFT — closes savvagent/otto-factory#69.

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

UPDATE repo_leases SET resource = 'branch:' || resource WHERE resource NOT LIKE 'branch:%';

DROP INDEX repo_leases_live_key;
CREATE UNIQUE INDEX repo_leases_live_key
  ON repo_leases (repo_id, resource)
  WHERE released_at IS NULL;
```

The `WHERE resource NOT LIKE 'branch:%'` guard makes the `UPDATE` idempotent against a
migration re-run in a context where it partially applied (defensive; sqlx applies each
migration inside its own transaction, so a clean re-run is the only case that matters, but
the guard costs nothing and documents the intent). The index drop+recreate is required
because a `UNIQUE INDEX` cannot be renamed to track a column rename automatically in a way
that also changes its definition comment; recreating it under the same name keeps
`\d repo_leases` readable. `repo_leases_org_expiry_idx` does not reference the column and is
untouched. The `repo_leases_notify` trigger (`0003_jobs.sql`) fires on the table, not a
named column list, so it needs no change.

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

Handler resolves the resource before calling `tx.acquire_lease`. The `branch` alias's
emptiness is checked **before** prefixing — `format!("branch:{}", ...)` on an empty or
whitespace-only branch would otherwise produce the non-empty string `"branch:"`, silently
sailing past `of-core`'s `resource.is_empty()` guard and creating a lease on a meaningless
resource, which would be a real regression from today's `if branch.is_empty()` check:

```rust
let resource = match (args.resource.as_deref(), args.branch.as_deref()) {
    (Some(r), _) => r.trim().to_string(),
    (None, Some(b)) => {
        let b = b.trim();
        if b.is_empty() {
            return Err(ErrorData::invalid_params("branch must not be empty", None));
        }
        format!("branch:{b}")
    }
    (None, None) => {
        return Err(ErrorData::invalid_params(
            "acquire_lease needs resource (or the deprecated branch)",
            None,
        ))
    }
};
```

Passing both `resource` and `branch` is not an error — `resource` simply wins, same
last-one-wins-is-confusing tradeoff already accepted elsewhere in this codebase (e.g. `repo`
winning over `remote` in `resolve_repo`) rather than adding a new error path for a case that
costs the caller nothing to get "wrong."

Tool description updated to state the convention explicitly:

> "Announce that you are taking exclusive use of something in a repository — a branch, or
> anything else your team needs to serialize on, such as a staging slot or a migration
> lock — so other agents can see it and go elsewhere. Pass `resource` as a free-form name;
> for a branch, use the form `branch:<name>` (e.g. `branch:main`) so every caller converges
> on the same spelling. `branch` is accepted as a deprecated shorthand for
> `resource: "branch:<branch>"`. Take one before you start and renew it while you work. If
> someone already holds it the error names them and says when it expires, so you can wait,
> message them, or pick different work. Leases are advisory: the server cannot see your git
> operations, so this makes collisions visible rather than impossible."

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

- **Empty resource.** `resource: Some("")` (or whitespace-only) reaches `of-core`'s
  `resource.is_empty()` guard after trimming, same as today. `branch: Some("")` (or
  whitespace-only) is rejected earlier, by the dedicated check in §4's `of-mcp` handler —
  it never reaches `of-core` at all, because prefixing it first would produce the non-empty
  string `"branch:"`, which would sail past `of-core`'s guard undetected.
- **Both `resource` and `branch` omitted.** New `invalid_params` error in `of-mcp` (§4) —
  today's `AcquireLeaseArgs::branch` is a required field so serde already rejects a missing
  branch; making both optional means the "neither given" case must be checked explicitly
  now that serde can no longer do it for us.
- **A resource string containing `:` that isn't `branch:...`.** Never rejected — free-form
  means free-form; a customer's skill might reasonably use `deploy:staging` or
  `fixture:accounts-db`.
- **Migration idempotency.** Covered by the `WHERE resource NOT LIKE 'branch:%'` guard in §1.
- **Existing live leases across the migration boundary.** A lease acquired on `main` before
  the migration and still live after it is transparently `branch:main` afterward — the
  unique index still enforces exclusivity on the same logical resource, and `list_leases`
  during the deploy window returns the new field name with the migrated value. No
  in-flight-lease special casing is needed because the rename is a single blocking `ALTER
  TABLE ... RENAME COLUMN` plus an `UPDATE`, both inside one migration transaction.

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
