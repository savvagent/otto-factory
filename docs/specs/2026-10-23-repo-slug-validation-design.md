# Repo slug validation design

> **Status:** DRAFT — bound and shape-check the repo slug at `register_repo`, and render the
> registered-slug list in `RepoUnresolved` quoted, capped, and without any slug that fails today's
> rule. A **breaking** change (`fix(of-core)!:`). Closes savvagent/otto-factory#206.

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
- `attempted` in `RepoUnresolved` is the caller's own input echoed back to that caller, not text
  another member chose. It is unchanged.

## Scope

**In:**
- A slug rule enforced in `ReposExt::register_repo`, before any SQL.
- A new `Error::InvalidRepoSlug { problem }` (`invalid_repo_slug`), mapped like `InvalidAgentLabel`.
- `RepoUnresolved`'s `known` list: each slug quoted, at most 50 shown, a non-conforming legacy slug
  never printed, and the sentence says how many were left out and where the full list is.
- Tool description for `register_repo` states the rule.

**Out:**
- Rewriting or migrating already-registered slugs. A legacy slug keeps working as an explicit
  `repo` argument; it is only withheld from error prose. (No migration: the per-org loop CLAUDE.md
  requires buys nothing a read-side filter does not.)
- Bounding repo `name`, `defaultBranch`, or `defaultAgentType`. None is rendered into server prose.
- Withholding a legacy slug from structured output (`list_repos`, `Repo.slug`). A slug is the handle
  an agent must pass back; hiding it would strand the repo. See Risks.
- Anything about which clients or hosts a slug may name (constraint 3: shape only).

## 1. The rule — `of_core::repo_slug`

New module beside `agent_label`, same layering: pure functions, no SQL.

`validate(slug: &str) -> Result<&str>` accepts a slug that is 1 to `MAX_LEN` (128) characters,
starts with an ASCII letter or digit, and otherwise contains only ASCII letters, digits, `-`, `_`,
`.` and `/`. It does **not** trim: a slug is an exact identifier, and a padded one is refused, not
rewritten. `/` is allowed because `owner/repo` is a natural slug and GitHub's own limits
(39 + 1 + 100) exceed a smaller cap. A refusal says what was wrong and what is valid, and never
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
- keep those that pass `is_displayable`, in the existing `ORDER BY slug` order;
- render each as `"slug"` (quotes are written by the code; a conforming slug contains no `"` or `\`),
  joined by `, `;
- show at most 50 (`MAX_LISTED`); append `and N more` if cut;
- if any were withheld for shape, append `M registered under names this message will not print`;
- if there are none to show and none withheld, keep `none registered yet`;
- when anything was cut or withheld, the error's tail points at `list_repos`.

`Error::RepoUnresolved { attempted, known }` keeps its shape, so no caller changes. The tail
sentence of its `#[error]` text is extended to mention `list_repos` only when relevant, so `known`
carries that clause itself.

## 4. Tenant isolation

No table, policy, or SQL statement is added. `unresolved` already reads through `list_repos_visible`,
scoped to the caller's org and `TeamScope`; the list shown is exactly what it showed before, minus
withheld entries. A cross-org negative test for the new behavior is not applicable (no new
tenant-scoped function), but the existing `isolation.rs` `RepoUnresolved` assertions stay green.

## 5. Metering

No new tool. `register_repo` is already classified.

## 6. Tests

- Unit tests in `repo_slug`: accepted shapes (`api`, `acme/api-v2`, `a.b_c`), each refusal class
  (empty, 129 chars, leading `-`, whitespace, newline, `"`, non-ASCII, `..` is accepted as shape-valid
  only after a leading alphanumeric), and that no refusal message contains the input.
- `of-core` integration: `register_repo` with an invalid slug returns `invalid_repo_slug` and creates
  no row; a valid one still registers.
- `of-core` integration: a legacy slug planted with raw SQL (a multi-line instruction-shaped string)
  is absent from the `RepoUnresolved` text, the withheld count is present, conforming slugs are
  quoted, and the legacy repo still resolves by its explicit slug.
- `of-core` integration: more than 50 repos yields the `and N more` clause.
- `of-mcp` `tools.rs`: `register_repo` with a bad slug is `invalid_repo_slug`; its description states
  the limit and contains `invalid_repo_slug`.
- `of-web`: `POST /repos` with a bad slug is `400` with that code.

## Assumptions

1. 128 characters and the `[A-Za-z0-9][A-Za-z0-9._/-]*` shape. Every slug in the repo's tests and
   fixtures already conforms. A stricter or looser rule is a one-constant change.
2. No trimming, unlike `agent_label`: an identifier an agent passes back verbatim must round-trip.
3. 50 listed slugs is enough to orient an agent and bounds the sentence to about 7 KB worst case.
4. A legacy slug that fails the rule is rare to nonexistent in practice (registration only ever
   required non-empty, and the registered base is small), so withholding rather than escaping is
   acceptable.

## Error handling & edge cases

- Duplicate slug: still `repo_slug_taken`; the validation runs first, so a nonconforming duplicate
  is `invalid_repo_slug`.
- The slug that a racing registration inserts between `validate` and the INSERT is handled by the
  existing unique-violation mapping.
- All-withheld org: `known` reads `none to show; N registered under names this message will not
  print. Call list_repos for the full list`.

## Risks & open questions

- **Breaking:** a `register_repo` or `POST /repos` that used to succeed with a spaced, non-ASCII, or
  over-long slug now fails. Recorded in the PR body and CHANGELOG via the `!` marker.
- A legacy slug still reaches peers as the structured `slug` field of `list_repos`. That is data an
  agent must be able to pass back, not server prose, so it is accepted here, as `Repo.slug` already
  was before this change.
- `docs/clients/matrix.md` records client wire behavior, not repo naming; no change is expected.
