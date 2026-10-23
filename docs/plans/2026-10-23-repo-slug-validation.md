# Repo slug validation — shape-check the slug and quote it in peer-facing errors

Goal: refuse a repo slug that is longer than 128 characters or outside `[A-Za-z0-9][A-Za-z0-9._/-]*`
at `register_repo` (`invalid_repo_slug`), render the registered-slug list in `RepoUnresolved`
quoted, capped at 50, and without any slug that fails the rule, and stop `RemoteTaken` printing a
non-conforming owner slug. Closes savvagent/otto-factory#206.

**Spec:** `docs/specs/2026-10-23-repo-slug-validation-design.md` — read it first. This plan
implements it exactly.

## Status — 2026-10-23

⬜ Not started.

## Global Constraints

- No AI self-attribution anywhere (commits, code comments, docs, PR body).
- Run `cargo fmt --all` before every Rust commit.
- Every SQL statement lives in `of-core`. This plan adds no SQL statement; the validation and
  rendering run in `of-core` around the existing ones. A test that plants a legacy slug does so with
  raw SQL from a `#[sqlx::test]` in `crates/of-core/tests/`.
- Errors are written for an LLM caller: stable `Error::code()`, say what the valid input is, never
  echo a refused or withheld slug.
- Constraint 3: the rule is about shape only.
- No `unwrap()` outside tests.
- Tests need `podman compose up -d` (Postgres on 15433) and `.env` (`cp .env.example .env`).
- Breaking change (spec "Risks"): PR title `fix(of-core)!: …` with a `BREAKING CHANGE:` footer.
  `CHANGELOG.md` is release-please's; do not edit it.
- No migration, no tenant table, no new tool, no new route: nothing out-of-band beyond the
  production read-only slug count recorded in the PR body (spec Assumption 4).

## File Structure

| File | Responsibility |
|---|---|
| `crates/of-core/src/repo_slug.rs` | **Create.** `MAX_LEN`, `MAX_LISTED`, `validate`, `is_displayable`, `describe`, `render_known`. Pure, no SQL. |
| `crates/of-core/src/lib.rs` | **Modify.** `pub mod repo_slug;`. |
| `crates/of-core/src/error.rs` | **Modify.** Add `InvalidRepoSlug { problem }` + `code()`; `RemoteTaken` renders its owner through `describe`. |
| `crates/of-core/src/repos.rs` | **Modify.** `register_repo` validates first; `unresolved` builds `known` with `render_known`. |
| `crates/of-web/src/error.rs` | **Modify.** Map `InvalidRepoSlug` to `400` beside `InvalidAgentLabel`. |
| `crates/of-mcp/src/tools/repos.rs` | **Modify.** `register_repo` description and `slug` arg doc state the rule and `invalid_repo_slug`. |
| `crates/of-core/tests/queue.rs`, `crates/of-mcp/tests/tools.rs`, `crates/of-web/tests/team_scoping.rs`, `crates/of-mcp/src/error.rs` | **Modify.** New tests; existing assertions on the `known` text updated for quoting. |

## Task Order & Rationale

Task 1 builds the pure rule and the error variant with unit tests, so Tasks 2–3 only wire it in.
Task 2 is the write side and is the breaking change. Task 3 is the read side, and updates the
existing assertions that Task 2 does not touch.

## Task 1 — The rule and the error ⬜

**Files:** `crates/of-core/src/repo_slug.rs` (create), `lib.rs`, `error.rs`,
`crates/of-web/src/error.rs`.
**Interfaces:** produces `repo_slug::{MAX_LEN, MAX_LISTED, validate, is_displayable, describe,
render_known}` and `Error::InvalidRepoSlug { problem: String }` with code `invalid_repo_slug`.

- [ ] Write failing unit tests in `repo_slug.rs` (`#[cfg(test)]`): accepts `api`, `acme/api-v2`,
      `a.b_c`, `a..b`, a 128-char slug; refuses empty, 129 chars, leading `-`, leading `.`, leading
      `/`, a space, `\n`, `"`, `é`, a trailing space (no trimming); no refusal message contains the
      input; `is_displayable` agrees with `validate`; `describe("api")` is `"api"` with quotes and
      `describe` of a bad slug is `another repo (call list_repos)`; `render_known` returns exactly the
      four strings in the spec §3 table (none visible, all shown, cut by `MAX_LISTED`, none shown) and
      lists at most 50.
- [ ] Run `cargo test -p of-core --lib repo_slug` → fails (module missing).
- [ ] Implement `repo_slug.rs` and the module doc (why: peer-chosen text in a sentence another member
      reads; refuses rather than rewrites; shape only). Add `pub mod repo_slug;` to `lib.rs`.
- [ ] Add `InvalidRepoSlug` to `error.rs` with a message in the `InvalidAgentLabel` style (what is
      wrong, the valid shape with an example, "nothing was changed"), `code()` → `invalid_repo_slug`,
      not retriable. Change `RemoteTaken`'s `#[error]` to render its second field with `describe`.
- [ ] Map `InvalidRepoSlug { .. }` to `BAD_REQUEST` in `crates/of-web/src/error.rs`.
- [ ] Run `cargo test -p of-core --lib` and `cargo clippy --all-targets -- -D warnings` → pass.
- [ ] Format and commit: `cargo fmt --all` then
      `git commit -m "feat(of-core): add the repo slug rule and invalid_repo_slug"`.

## Task 2 — Validate at `register_repo` ⬜

**Files:** `crates/of-core/src/repos.rs`, `crates/of-mcp/src/tools/repos.rs`,
`crates/of-core/tests/queue.rs`, `crates/of-mcp/tests/tools.rs`, `crates/of-web/tests/` (the
existing repos HTTP test file; find it with `grep -ln "POST.*repos\|register_repo" crates/of-web/tests`).
**Interfaces:** consumes `repo_slug::validate`.

- [ ] Write failing tests: (core, `queue.rs`) `register_repo` with `"has space"` and with `""` return
      `invalid_repo_slug` and `list_repos` is unchanged; a valid slug registers and the stored slug is
      the caller's string unchanged; a nonconforming duplicate of an existing slug is
      `invalid_repo_slug`, not `repo_slug_taken`. (mcp, `tools.rs`) the `register_repo` tool with a
      bad slug is `invalid_repo_slug`, and its description contains `invalid_repo_slug` and `128`.
      (web) `POST /api/orgs/{org}/repos` with a bad slug is `400` with `code: invalid_repo_slug`.
- [ ] Run each → fail.
- [ ] Grep for any test asserting `Invalid` for an empty slug and update it to `invalid_repo_slug`.
- [ ] In `repos.rs`, replace the `trim().is_empty()` check with `repo_slug::validate(&new.slug)?`
      before anything else, and use the returned `&str` for the insert. Update the `register_repo`
      tool description and the `slug` arg doc (state the shape, the limit, that it is refused rather
      than rewritten, and that it should be stable).
- [ ] Run `cargo test -p of-core --test queue`, `cargo test -p of-mcp --test tools`,
      `cargo test -p of-web` → pass. Grep for any existing test registering a slug that now fails
      (the spec found none) and fix the fixture if one appears.
- [ ] Format and commit: `git commit -m "fix(of-core): refuse a repo slug that is not a short identifier"`.

## Task 3 — Render the registered list and the owner safely ⬜

**Files:** `crates/of-core/src/repos.rs`, `crates/of-core/tests/queue.rs`,
`crates/of-mcp/tests/tools.rs`, `crates/of-web/tests/team_scoping.rs`.
**Interfaces:** consumes `repo_slug::{render_known, describe}`.

- [ ] Write failing tests: (core) plant a legacy slug with raw SQL `UPDATE repos SET slug = $2
      WHERE org_id = $1 AND id = $3` (multi-line, instruction-shaped) → an unresolved lookup's
      message does not contain it, contains the quoted conforming slugs and the
      `(and 1 more not shown; call list_repos for the full list)` clause, and the legacy repo still
      resolves by `RepoRef { slug: Some(legacy) }`; 52 repos → exactly 50 quoted slugs and
      `and 2 more`; an org whose every slug is withheld → `none shown (N registered; …)`; a
      `RemoteTaken` against the legacy repo says `another repo` and does not contain the slug, and
      against a conforming one contains it quoted. Cross-org negative: a second org with its own slugs
      never appears in the first org's `known` (the existing isolation assertions at
      `isolation.rs:1254` stay green).
- [ ] Run → fail.
- [ ] In `unresolved`, build `known` with `repo_slug::render_known(&slugs)`.
- [ ] Update the existing assertions that quoting breaks: `team_scoping.rs:157,167`
      (`Registered repos: "growth-repo", "shared".`), `team_scoping.rs:170`
      (`"growth-repo", "platform-repo", "shared"`), and `tools.rs:3929`
      (`"growth-repo", "shared"`; its negative half becomes `!contains("\"platform-repo\"")`).
      The `of-mcp/src/error.rs` unit test passes `known` as a raw string and needs no change.
- [ ] Run `cargo test --workspace` against the Postgres service, then
      `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all --check` → pass.
- [ ] Format and commit: `git commit -m "fix(of-core): quote and cap the registered repos an unresolved error lists"`.

## Out-of-band checks (Phase 5)

Container image, console bundle, Worker, migrations, CI config, config surface: none touched —
state it vacuously in the PR. Run the read-only production slug count from the spec's Assumption 4
before merge and record the number in the PR body.
