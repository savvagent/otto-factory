# Repo slug validation design

> **Status:** IMPLEMENTED — shipped in savvagent/otto-factory#249 (merged as `b98761d`, a
> **breaking** change: `fix(of-core)!: ...`), closing savvagent/otto-factory#206. Bound and
> shape-check the repo slug at `register_repo`, and render the registered-slug list in
> `RepoUnresolved` quoted, capped, and without any slug that fails today's rule. The production
> count of existing slugs failing the rule was 0 of 6 repos. Follow-ups: savvagent/otto-factory#250,
> #251, #252. See the Review round 1 addendum for what changed after review.

> **Follows from:** `docs/specs/2026-10-08-agent-label-validation-design.md` (savvagent/otto-factory#208),
> which recorded this as out of scope because the compatibility question differs: a slug is an
> identifier agents pass back verbatim, not a label.

## Premise corrections

- The issue says "slugs (and/or names)". Only **slugs** are interpolated: `Repos::unresolved`
  (`crates/of-core/src/repos.rs`) builds `known` from `r.slug` alone. A repo `name` appears only as a
  structured field. Names are out of scope; see Out.
- The issue asks whether `update_repo` needs the rule "if slugs are editable". They are not:
  `RepoPatch` deliberately has no slug field (its doc comment says why). Only `register_repo` writes
  a slug, so only it needs the check.
- `RepoUnresolved` is **not** the only server sentence that carries a peer-chosen slug.
  `Error::RemoteTaken(normalized, slug)` ("remote … is already registered to repo …") renders the
  *existing* repo's stored slug, `Debug`-quoted, to whoever collides with it on `register_repo`.
  `redact_foreign_ids` (`of-mcp`) withholds it only from team-scoped callers, and `of-web` does not
  redact. It is the same shape as #206 and is in scope (§3a).
- `attempted` in `RepoUnresolved` is the caller's own input echoed back to that caller, not text
  another member chose. It is unchanged.

## Scope

**In:**
- A slug rule enforced in `ReposExt::register_repo`, before any SQL.
- A new `Error::InvalidRepoSlug { problem }` (`invalid_repo_slug`), mapped like `InvalidAgentLabel`.
- `RepoUnresolved`'s `known` list: each slug quoted, at most 50 shown, a non-conforming legacy slug
  never printed, and the sentence says how many were left out and where the full list is.
- `RemoteTaken`'s rendering of the owning repo's slug: a non-conforming legacy slug is not printed.
- Tool description for `register_repo` states the rule.

**Out:**
- Rewriting or migrating already-registered slugs. A legacy slug keeps working as an explicit
  `repo` argument; it is only withheld from error prose. (No migration: the per-org loop CLAUDE.md
  requires buys nothing a read-side filter does not.)
- The audit trail's `target("repo", slug)` rendered to admins in the console: structured data in an
  admin-only view, not server prose an agent reads.
- Bounding repo `name`, `defaultBranch`, or `defaultAgentType`. None is rendered into server prose.
- Withholding a legacy slug from structured output (`list_repos`, `Repo.slug`). A slug is the handle
  an agent must pass back; hiding it would strand the repo. See Risks.
- Anything about which clients or hosts a slug may name (constraint 3: shape only).

## 1. The rule — `of_core::repo_slug`

New module beside `agent_label`, same layering: pure functions, no SQL.

`validate(slug: &str) -> Result<&str>` accepts a slug that is 1 to `MAX_LEN` (128) characters,
starts with an ASCII letter or digit, and otherwise contains only ASCII letters, digits, `-`, `_`,
`.` and `/`. It does **not** trim: a slug is an exact identifier, and a padded one is refused, not
rewritten. `/` is allowed because `owner/repo` is a natural slug. Slugs are short caller-chosen
handles, so a maximal GitHub `owner/repo` string (up to 39 + 1 + 100 = 140 characters) would be
refused by the 128 cap; that is accepted.

After the charset check, `/` is split out and no segment may be empty, `.` or `..` (so `a/`, `a//b`,
`x/../y`, `a/./b` and `a/..` are refused; `a/b.c` and `a..b` stay valid, since a segment that merely
contains dots is harmless). The reason: a slug is pasted into URL path segments (`/repos/{repo}`).
The console encodes with `seg()`, but other clients may not, and an unencoded `..` or empty segment
would climb or collapse the path. The OpenAPI schema carries the charset, start and length part
(`repo_slug::PATTERN`, `minLength`, `maxLength`); the segment rule is enforced only in `validate`,
since a regex cannot say it readably. A refusal says what was wrong and what is valid, and never
repeats the slug.

`is_displayable(slug: &str) -> bool` is `validate(slug).is_ok()`; it is how the read side decides
what may appear in prose.

## 2. Write side

`register_repo` calls `repo_slug::validate(&new.slug)` first, replacing the `trim().is_empty()`
check. The empty case is now `invalid_repo_slug` rather than `Error::Invalid`. The stored slug is the
caller's string, unchanged. A refused call writes nothing and, because `charge` runs in the same
transaction, is not billed (it already is not: every tool is free).

`of-web` maps `InvalidRepoSlug` to the same `400` as `InvalidAgentLabel`. `of-mcp` needs no mapping
beyond `from_core` carrying the message and `code()`.

## 3. Read side — `RepoUnresolved`

`Repos::unresolved` builds `known` from the visible slugs:
- keep those that pass `is_displayable`, in the existing `ORDER BY slug` order, and show at most 50
  (`MAX_LISTED`); each is rendered as `"slug"` (quotes written by the code; a conforming slug contains
  no `"` or `\`) and they are joined by `, `;
- `hidden` is the number of visible slugs not shown (cut by the cap, or failing the rule).

`known` is exactly one of these four strings, and the `#[error]` text around it is **unchanged**
(`Registered repos: {known}. Register this one with register_repo, or pass an explicit repo slug.`):

| Case | `known` |
|---|---|
| none visible | `none registered yet` |
| some shown, `hidden == 0` | `"api", "web"` |
| some shown, `hidden > 0` | `"api", "web" (and 3 more not shown; call list_repos for the full list)` |
| none shown, `hidden > 0` | `none shown (3 registered; call list_repos for the full list)` |

`Error::RepoUnresolved { attempted, known }` keeps its shape, so no caller changes. **Quoting is a
visible change to `known` even for conforming slugs** (`api, web` becomes `"api", "web"`), so
existing assertions on that text are updated (`team_scoping.rs` and `tools.rs`; `isolation.rs` and
`of-mcp`'s `error.rs` unit test need no change) along with the `Registered repos:` example in
`docs/clients/matrix.md`; it is not a new error code.

## 3a. `RemoteTaken`

`Error::RemoteTaken(normalized, slug)` renders the owner through `repo_slug::describe(slug)`: the
quoted slug when `is_displayable`, else `another repo (call list_repos)`. The `Debug`-quoting of
the slug in the current `#[error]` text is replaced by `describe`, and the variant's data and
`code()` are unchanged. `redact_foreign_ids` is untouched.

## 4. Tenant isolation

No table, policy, or SQL statement is added. `unresolved` already reads through `list_repos_visible`,
scoped to the caller's org and `TeamScope`; the list shown is exactly what it showed before, minus
withheld entries. A cross-org negative test for the new behavior is not applicable (no new
tenant-scoped function), but the existing `isolation.rs` `RepoUnresolved` assertions stay green.

## 5. Metering

No new tool. `register_repo` is already classified.

## 6. Tests

- Unit tests in `repo_slug`: accepted shapes (`api`, `acme/api-v2`, `a.b_c`), each refusal class
  (empty, 129 chars, leading `-`, leading `.`, whitespace, newline, `"`, non-ASCII; `a..b` is
  shape-valid), and that no refusal message contains the input.
- `of-core` integration: `register_repo` with an invalid slug returns `invalid_repo_slug` and creates
  no row; a valid one still registers.
- `of-core` integration: a legacy slug planted with raw SQL (a multi-line instruction-shaped string)
  is absent from the `RepoUnresolved` text, the withheld count is present, conforming slugs are
  quoted, and the legacy repo still resolves by its explicit slug.
- `of-core` integration: a `RemoteTaken` against a planted legacy slug does not print the slug and
  says `another repo`; against a conforming one it prints it quoted.
- `of-core` integration: more than 50 repos yields the `and N more` clause.
- `of-mcp` `tools.rs`: `register_repo` with a bad slug is `invalid_repo_slug`; its description states
  the limit and contains `invalid_repo_slug`.
- `of-web`: `POST /repos` with a bad slug is `400` with that code.

## Assumptions

1. 128 characters and the `[A-Za-z0-9][A-Za-z0-9._/-]*` shape. Every slug in the repo's tests and
   fixtures already conforms. A stricter or looser rule is a one-constant change.
2. No trimming, unlike `agent_label`: an identifier an agent passes back verbatim must round-trip.
3. 50 listed slugs is enough to orient an agent and bounds the sentence to about 7 KB worst case.
4. Withholding a legacy slug is preferred to escaping it: escaping keeps attacker-chosen text in
   the sentence. Before merge, the author checks the production `repos` table for slugs failing the
   rule (read-only count, `SELECT count(*) FROM repos WHERE slug !~ '^[A-Za-z0-9][A-Za-z0-9._/-]{0,127}$'`)
   and records the result in the PR body; a nonzero count is disclosed, not blocked on.

## Error handling & edge cases

- Duplicate slug: still `repo_slug_taken`; the validation runs first, so a nonconforming duplicate
  is `invalid_repo_slug`.
- The slug that a racing registration inserts between `validate` and the INSERT is handled by the
  existing unique-violation mapping.
- All-withheld org: the fourth row of the §3 table.

## Risks & open questions

- **Breaking:** a `register_repo` or `POST /repos` that used to succeed with a spaced, non-ASCII, or
  over-long slug now fails. Recorded in the PR body and CHANGELOG via the `!` marker.
- **Accepted trade-off:** the 128-character cap refuses the longest possible GitHub `owner/repo`
  strings (up to 140 characters). A slug is a handle, not a mirror of the remote; such a repo takes
  a shorter slug and is still found by its remote.
- A legacy slug still reaches peers as the structured `slug` field of `list_repos`. That is data an
  agent must be able to pass back, not server prose, so it is accepted here, as `Repo.slug` already
  was before this change. The residual exposure of a withheld legacy slug is therefore any peer that
  calls `list_repos`: the structured field lands in the same LLM context as error text. Withholding
  stops unsolicited server-authored prose carrying it, nothing more.
- A legacy nonconforming slug cannot be fixed in place: `RepoPatch` has no slug, and re-registering
  is blocked because remotes are additive-only and a deactivated repo keeps its remotes
  (`RemoteTaken`). If the production count (Assumption 4) is nonzero, the remedy is an operator step
  (a per-org rewrite with both guards, as CLAUDE.md prescribes), tracked as a follow-up and not part
  of this change.
- `unresolved` fetches at most 1000 visible rows, so the "N more not shown" count is a floor above
  1000 repos. A slugs-only or count query is a tracked follow-up.
- `docs/clients/matrix.md` is changed only in its `Registered repos:` example, to the quoted form;
  its client wire-behavior content is untouched.

## Review round 1

Changed: the path-segment rule (§1); the OpenAPI `RegisterRepoRequest.slug` now carries
`description`, `minLength`, `maxLength` and `pattern`, with a unit test that `PATTERN` agrees with
`validate`; a doc comment on `Error::RepoUnresolved` that `known` must come from
`repo_slug::render_known`; the Risks text above.

Considered and not done:
- Lowering `MAX_LEN` or `MAX_LISTED`: the charset already excludes whitespace, quotes and newlines,
  and label text is far wider and accepted.
- Case-insensitive slug uniqueness: pre-existing, follow-up.
- Bounding repo `name`: out of scope.
- Redacting `RemoteTaken` in `of-web` for team-limited callers: pre-existing, follow-up.
- The console's `slugPreview` mismatch and an `invalid_repo_slug` i18n entry: follow-up.
