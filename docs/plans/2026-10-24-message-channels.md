# Message channels — scope messages to a repo or team channel

> **Regenerated 2026-10-23** from the revised spec. It replaces the
> 2026-09-11 plan, which targeted migration `0031`, a `teams` table, and `resolve_team`, none of
> which exist any more. The org owner signed off on the spec's open decisions on 2026-10-23:
> DMs are deleted, legacy cursors are not seeded, `to` is refused as an unknown field, and
> `"recipientUserId": null` stays in the idempotency fingerprint.

**Goal:** replace the org-wide broadcast and person-directed DM on the message channel with two
required, mutually exclusive channels (repo or team), so `send_message` / `inbox` /
`ack_messages` / `unread_count` all work per channel, with per-channel read cursors.

**Spec:** `docs/specs/2026-10-24-message-channels-design.md` — read it first. This plan
implements it exactly. Closes savvagent/otto-factory#177.

## Status — 2026-10-24

✅ Tasks 1–5 shipped in PR #254 (squash-merged as `e432963`). Review-round changes are recorded in the
spec's addendum (migration lock, backstop and repo cascade; exclusive channel predicates; acks
refused outside their channel; team slug shape check; `TeamChannelNotFound`).

---

## Global constraints

- Work per the `otto-tower-development` skill. Every SQL statement lives in `of-core`.
- No attribution anywhere: commits, comments, docs, PR body.
- `cargo fmt --all` before every commit.
- Tests need Postgres: `podman compose up -d`, `cp .env.example .env`.
- Migrations are append-only. `0008` is the next free number (`0007_trigger_label_default_otto_tower.sql`
  is the head); re-check before creating the file.
- The migration's two `DELETE`s and its `UPDATE` rewrite tenant data. They use CLAUDE.md's per-org loop with
  **both** the explicit `org_id` predicate and `set_config('app.org_id', …, true)`, plus the
  temporary read-only `current_org() IS NULL` policies that let the org enumeration see rows
  where RLS applies (the `0004_message_scope.sql` precedent). Never `TRUNCATE` and never
  `COPY`.
- Platform calls (team lookup, `team_scope`) happen **before** `Factory::tx` opens, never inside
  a transaction.
- **This is not additive.** About 25 existing tests construct DMs or call the channel-less
  signatures. Each task repairs its own crate's tests.
- **Intermediate commits are not all green, and that is accepted** (the PR squash-merges). Every
  query in `messages.rs` is a runtime `sqlx::query*`, so after Task 1 the message suites in
  `of-core` fail at runtime (`recipient_user_id` is gone, and `ack_messages_for`'s
  `ON CONFLICT (org_id, user_id)` has no matching constraint) until Task 3. `of-mcp` does not
  compile between Tasks 3 and 4. Each task's gate below names exactly what must pass at that
  point; the whole-workspace gate is Task 4's.
- PR title: `feat(of-mcp)!: scope messages to a repo or team channel` with a
  `BREAKING CHANGE:` footer saying that `send_message` no longer takes `to`, and that the four
  message tools require `repo`/`remote` or `team`. Reference the issue.

## File structure

| File | Change |
|---|---|
| **Create** `crates/of-core/migrations/0008_message_channels.sql` | Per-org delete of DMs and legacy cursors; drop `recipient_user_id`; `messages_channel_ck` (`NOT VALID`); two channel indexes on `messages`; `message_cursors` gets `id` PK, `repo_id`, `team_id`, a validated `message_cursors_channel_ck`, two partial unique indexes. |
| **Create** `crates/of-core/tests/message_channels_migration.rs` | The migration's per-org behavior under both role shapes. |
| **Modify** `crates/of-core/src/teams.rs` | `VerifiedTeam::by_slug`. |
| **Modify** `crates/of-core/src/error.rs` | `TeamNotFound` message made neutral (it now serves repos, jobs, and message channels). |
| **Modify** `crates/of-core/src/messages.rs` | `ChannelRef`; `recipient_user_id` removed; exactly-one-channel check; per-channel `inbox` / `unread_count_for` / `ack_messages_for`; `ReplyParent` removed; `reply_parent` returns `()`. |
| **Modify** `crates/of-core/tests/{queue,isolation,platform_events}.rs` | Repair and add tests (Task 3). |
| **Modify** `crates/of-mcp/src/tools/mod.rs` | `channel_of`, `team_channel`. |
| **Modify** `crates/of-mcp/src/tools/coord.rs` | Args, handlers, descriptions; delete `member_by_email` and `reply_recipient`. |
| **Modify** `crates/of-mcp/tests/tools.rs` | Repair, delete, and add tests (Task 4). |
| **Modify** `CLAUDE.md` | The "Identity reads" bullet: `member_by_email` is gone; `team_by_slug` resolves team channels. |

## Task order

1. Migration and migration test. Standalone, so it lands first and is reviewed on its own.
2. `teams::by_slug` and the `TeamNotFound` wording. Small and independent.
3. `of-core::messages` and the repair of `of-core`'s tests. Depends on 1 (columns) and 2.
4. `of-mcp` surface and tests. Depends on 3. Carries the workspace-wide gate.
5. `CLAUDE.md`.

---

## Task 1 — migration `0008_message_channels.sql` ✅

- [x] Create the file with the SQL from spec §1, verbatim apart from comments. Keep the
      comment explaining why DMs are deleted and not orphaned (a DM could carry `repo_id`), the
      one explaining the temporary policies, and the one explaining why a legacy broadcast's
      `job_id` is nulled (a `NOT VALID` CHECK still binds later UPDATEs, so `delete_job` would
      fail on it). Include the guarded `GRANT ... ON SEQUENCE message_cursors_id_seq`. The org
      list is materialized into an array before a `FOREACH` loop (spec §1: a `FOR ... IN
      <query>` cursor can skip orgs once `app.org_id` is pinned); the RLS-applying test seeds
      more than 10 orgs so that would show.
- [x] Check that the `DROP CONSTRAINT message_cursors_pkey, ADD COLUMN id bigserial PRIMARY KEY`
      combination applies cleanly on Postgres 16 with rows already deleted. Split it into
      separate `ALTER`s if the combined form complains.
- [x] Create `crates/of-core/tests/message_channels_migration.rs`, modelled on
      `meter_rename.rs` (`include_str!` the migration; seed rows directly on the pool; run the
      SQL; count). `#[sqlx::test]` has already applied `0008`, so the test re-creates the
      pre-migration shape first: `ALTER TABLE messages ADD COLUMN recipient_user_id uuid`,
      drop the new constraints and indexes, drop `message_cursors`' new `id` primary key and its
      `id` / `repo_id` / `team_id` columns, and restore the old `(org_id, user_id)` primary
      key — otherwise re-running the migration fails at `ADD COLUMN id`. Use a
      helper `undo_0008(pool)`, documented as standing in for a database at `0007`.
      Seed for **two** orgs:
      - one DM with a `repo_id`, one DM without a repo, and one reply to a DM (also a DM);
      - one repo broadcast, one unscoped broadcast, and one unscoped broadcast carrying a
        `job_id` (seed a job for it);
      - two legacy cursors.
      Then:
  - `deletes_every_orgs_dms_and_cursors_when_rls_is_bypassed`: run the migration on the pool
    (superuser). Assert zero DMs and zero cursors in both orgs, both broadcasts per org
    survive, the job-carrying legacy broadcast's `job_id` is NULL, `recipient_user_id` no
    longer exists (`information_schema.columns`), and a new insert with neither channel
    violates `messages_channel_ck`. Then `DELETE FROM jobs` for the seeded job succeeds (the
    FK's `SET NULL` has nothing left to update).
  - `deletes_every_orgs_dms_and_cursors_when_rls_applies`: `otto_app` owns nothing, so it
    can run neither `CREATE POLICY` nor `ALTER TABLE` (the `meter_rename.rs` precedent works
    only because `0006` is a lone `DO` block). So: in one transaction, create the two
    temporary policies as the pool's role, `SET LOCAL ROLE otto_app`, run the migration's `DO`
    block alone, `RESET ROLE`, drop the policies. Same row assertions as above (the DDL is
    the bypass test's to prove). The **control**, run first on its own fresh seed: the same
    `DO` block as `otto_app` with no temporary policies deletes and nulls nothing. That shows
    the policies, not luck, make the loop find the orgs. Split the migration text with a
    helper that extracts the `DO $$ ... END $$;` block containing `FOREACH o IN ARRAY`, and fail the
    test loudly if it is not found exactly once.
- [x] Gate: `cargo test -p of-core --test message_channels_migration --test guards`. The other
      message suites stay red until Task 3 (see Global constraints). The fresh-cluster apply
      is checked at Task 3's gate.
- [x] Commit: `feat(of-core): migrate messages to repo and team channels`.

## Task 2 — `VerifiedTeam::by_slug` ✅

- [x] `crates/of-core/src/teams.rs`: add

      ```rust
      /// Ask the platform for the team with `slug` in `org`. Fails closed exactly as
      /// [`Self::verify`] does.
      pub async fn by_slug(platform: &PlatformClient, org: OrgId, slug: &str) -> Result<Self>
      ```

      Call `platform.team_by_slug(org.as_uuid(), slug)`. `Some(info)` with
      `info.org_id == org.as_uuid()` gives `Self { org, team: info.id.into() }`. Anything else is
      `Error::TeamNotFound { team: format!("{slug:?}") }`. A transport failure is
      `Error::Platform` through `?`. Include a `no_run` doc example as `verify` has.
- [x] `crates/of-core/src/error.rs`: reword `TeamNotFound` so it no longer assumes a repo:
      "team {team} was not found in this organization, or you are not on it. Teams are managed
      in the otto platform console; use a team from there, or omit the team." The "or you are
      not on it" clause is needed because Task 4 gives a hidden team this same error. The code
      stays `team_not_found`. Grep the tests for the old text; the existing assertions found
      are on the code only.
- [x] Tests in `crates/of-core/tests/platform_events.rs`, beside the `VerifiedTeam::verify`
      ones: `by_slug` finds a team; refuses another org's slug; refuses an unknown slug; is
      `Error::Platform` with `MockPlatform::set_down(true)`.
- [x] `cargo test -p of-core --test platform_events`, clippy, commit
      `feat(of-core): resolve a verified team by slug`.

## Task 3 — `of-core::messages` ✅

- [x] Types:
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum ChannelRef { Repo(RepoId), Team(TeamId) }`.
  - Remove `recipient_user_id` from `Message`, `NewMessage`, `MSG_COLS`, and both `INSERT`s.
    Renumber placeholders, and update `SCOPE_TEAMS_SQL`'s doc comment and parameter numbers
    (`$6` team, `$9` repo, `$10` job, `$11` reply each shift down by one).
  - `message_idempotency_fingerprint`: keep `"recipientUserId": serde_json::Value::Null` as a
    literal, with a comment: stored keys hashed it, so dropping it would turn every
    pre-migration keyed retry into `idempotency_key_conflict` (the #163 label precedent).
  - `InboxQuery` gains `pub channel: ChannelRef` and loses `impl Default`.
  - Delete `ReplyParent`. `reply_parent` returns `Result<()>`, keeping its `FOR SHARE` hold
    and its error text. `message_readable` loses the recipient/sender predicate: any message
    passing `message_visible_sql` is readable.
- [x] `send_message`: after the body checks, `match (new.repo_id, new.team_id)`. Neither gives
      `Error::Invalid("a message goes to one channel: set repo_id for a repo's channel or
      team_id for a team's")`. Both gives `Error::Invalid("a message goes to one channel, a repo
      or a team, not both")`. `find_replayed_message` is unchanged.
- [x] A private helper `channel_sql(c: ChannelRef, n: usize) -> (&'static str, uuid::Uuid)`
      returns the column predicate (`"m.repo_id = $n"` / `"m.team_id = $n"`) and the value to
      bind, plus a twin for the cursor sub-select's `repo_id` / `team_id`. The column is chosen
      in Rust and never interpolated from input; say so in the comment, pointing at `inbox`'s
      `ORDER BY` precedent.
- [x] `inbox(reader, q)`: replace the recipient predicate with the channel predicate, and key
      the cursor sub-select by channel too:
      `(SELECT last_read_id FROM message_cursors WHERE org_id = $1 AND user_id = $2 AND
      <channel column> = $n)`. Without the channel key, the sub-select returns several rows
      once a user has two cursors. Update the doc comment ("messages in one channel visible to
      `reader`").
- [x] `unread_count_for(reader, channel, scope)`: the same two changes.
- [x] `ack_messages_for(reader, channel, up_to, scope)`: the clamp is
      `MAX(m.id)` over the channel **and** `message_visible_sql`, without the old
      recipient/sender predicate. The upsert writes `repo_id` or `team_id` with
      `ON CONFLICT (org_id, user_id, repo_id) WHERE repo_id IS NOT NULL` (or the team
      equivalent), in two fixed SQL strings.
- [x] The `TeamScope::All` wrappers `ack_messages` and `unread_count`: delete them and call the
      `_for` versions with `&TeamScope::All` in tests. One shape, not two.
- [x] Repair `crates/of-core/tests/queue.rs` (messages block, from the `messages` marker):
  - **Delete** `directed_messages_are_private_within_the_org`.
  - Thread a repo channel (`t.repo`, `ChannelRef::Repo(t.repo)`) through
    `unread_excludes_self_and_ack_is_clamped`, `oversized_and_empty_bodies_are_refused`, the
    four idempotency tests (`send_message_replays_with_the_same_idempotency_key`,
    `send_message_with_a_reused_key_and_a_different_payload_conflicts`,
    `send_message_without_a_key_always_creates_a_new_message`,
    `the_same_key_reused_across_add_job_and_send_message_does_not_conflict`), and the three
    label tests (`send_message_applies_the_agent_label_policy`,
    `a_keyed_send_message_with_a_padded_label_replays_cleanly`,
    `a_pre_policy_keyed_message_with_a_now_invalid_label_still_replays`). Keep every
    assertion. Grep each for `InboxQuery`, `unread_count`, and `ack_messages` as well as
    `NewMessage`. The pre-policy label test seeds a stored hash: confirm it still matches with
    the `null` kept in the fingerprint. That test is also the regression test for the
    fingerprint decision.
  - **Add**:
    - `a_message_needs_exactly_one_channel`: neither is refused; both is refused (team built
      with `VerifiedTeam::verify` against an `of-testkit` `MockPlatform` team, as
      `platform_events.rs` does).
    - `messages_are_isolated_by_channel`: repo A, repo B, team T. One message to each; each
      channel's `inbox` returns exactly its own message.
    - `acking_one_channel_leaves_the_others_unread`: ack repo A. Repo A's unread count is 0;
      repo B's and team T's are still 1.
    - `ack_is_clamped_to_the_channel`: `up_to = i64::MAX` on repo A lands on repo A's newest
      id, not on a newer message in repo B.
    - `a_cross_channel_reply_keeps_its_parents_team_scope`: parent in a team-scoped repo's
      channel, reply in an org-wide repo's channel. The reply's `scope_teams` contains the
      parent's team, and a reader outside that team does not see the reply.
  - Also: `a_legacy_label_is_withheld_from_structured_output`, and every other test the
    compiler reports. In `a_pre_policy_keyed_message_with_a_now_invalid_label_still_replays`
    the hand-built hash JSON has `"repoId": null`; once the message carries a repo channel that
    literal must carry the same repo id, or the fingerprint regression test fails for the
    wrong reason.
- [x] Repair `crates/of-core/tests/isolation.rs`:
      `message_idempotency_keys_do_not_cross_org_boundaries`,
      `messages_are_invisible_across_orgs`, and `team_scoped_helpers_do_not_cross_orgs` (calls
      `unread_count_for` / `ack_messages_for`) need each org's repo as the channel.
- [x] Repair `crates/of-core/tests/platform_events.rs`:
      `a_message_takes_a_team_from_its_own_org_only_keyed_or_not` builds `NewMessage` with a
      team, so it may already satisfy "exactly one". Fix it to compile and keep its cross-org
      assertion. **Runtime-only breakage the compiler will not flag:** `populate` sends a
      `NewMessage` with only `job_id` plus `..Default::default()`; it compiles, but the
      exactly-one-channel check refuses it, which breaks the `org.deleted` "every table" tests.
      Give it a repo channel. `populate` also calls `tx.ack_messages(t.user, 1)`, the wrapper
      this plan deletes. Also repair `member_removed_releases_what_they_held` (calls
      `ack_messages`). Add `member_removed_drops_every_channel_cursor`: two cursors (repo and team)
      for one user, `member.removed`, zero left.
- [x] `cargo test -p of-core`, `cargo clippy -p of-core --all-targets -- -D warnings`. Also once
      against a fresh cluster (`podman compose down -v && podman compose up -d`, then
      `cargo test -p of-core`) to prove the migrations apply from empty. Commit
      `feat(of-core)!: scope messages to a repo or team channel`.

## Task 4 — `of-mcp` ✅

- [x] `crates/of-mcp/src/tools/mod.rs`:
  - `pub(crate) async fn team_channel(f: &Factory, caller: &Principal, scope: &TeamScope,
    slug: &str) -> Result<VerifiedTeam, ErrorData>`. Calls `VerifiedTeam::by_slug`, then
    refuses with the same `Error::TeamNotFound` if `!scope.allows(Some(team.id()))`. A hidden
    team must be byte-identical to an unknown one, so build the error from the slug in both
    branches.
  - `pub(crate) enum ChannelArg { Repo { slug, remote }, Team(VerifiedTeam) }` and
    `pub(crate) async fn channel_arg(f, caller, scope, repo, remote, team) -> Result<ChannelArg, ErrorData>`.
    Blank strings count as absent, as in `maybe_repo_of`. It runs **before** the transaction:
    it resolves the team (a platform call), and only records the repo for later. Neither or
    both is `invalid_argument`, naming `repo` / `remote` / `team` (spec's error table). Use
    `Err(of_core::Error::Invalid(..)).mcp()` (`.mcp()` is on `Result`, not `Error`) so the code is `invalid_argument`, as other caller
    mistakes in these tools are.
  - `pub(crate) async fn channel_of(tx, scope, arg: ChannelArg, write: bool) -> Result<ChannelRef, ErrorData>`,
    called inside the transaction. A repo goes through `repo_for_write(..., Hold::Shared)` when
    `write`, else `repo_of`. A team returns `ChannelRef::Team(team.id())`.
- [x] `crates/of-mcp/src/tools/coord.rs`:
  - `SendMessageArgs`: remove `to`; add `team: Option<String>` ("A team's slug, for that team's
    channel across repos. Pass this or repo/remote, not both."); add `deny_unknown_fields`.
    Document `repo`/`remote` ("The repo whose channel this goes to.").
  - `InboxArgs` and `AckMessagesArgs` gain `repo` / `remote` / `team` and `deny_unknown_fields`.
    `unread_count` takes a new `ChannelArgs { repo, remote, team }` (also
    `deny_unknown_fields`) in place of `NoArgs`. Per-tool structs, matching the file's style.
  - `send_message`: `team_scope`, then `channel_arg`, then `self.tx`. In **both** branches
    (keyed and unkeyed), replace `maybe_repo_for_write` with
    `channel_of(&mut tx, &team, arg, true)` and set `repo_id` / `team_id` on `NewMessage` from
    the resulting `ChannelRef` (`team_id` takes the `VerifiedTeam` from `ChannelArg::Team`).
    `reply_parent(...)` replaces `reply_recipient(...)`. Keyed-branch ordering is unchanged:
    resolve, then replay check, then charge. The unkeyed branch still charges first.
  - `inbox`, `ack_messages`, `unread_count`: the same `team_scope` → `channel_arg` → `tx` →
    `charge` → `channel_of(.., false)` sequence, then the channel-taking `of-core` calls.
  - Delete `Factory::member_by_email`, the free function `reply_recipient`, and the
    `not_a_member` error.
  - Descriptions, per spec §3. `send_message`'s first sentence: "Post a note to one repo's or
    one team's shared channel: pass repo (or remote) for coordination about that repo, or team
    (a team slug) for a team's work across repos. Exactly one is required." Keep the hand-off
    example, the idempotency sentence, and the agent-label sentence. `inbox`, `ack_messages`,
    and `unread_count` each gain: "One channel at a time, named the same way as send_message
    (repo, remote, or team). A team you are not on is team_not_found."
- [x] Repair `crates/of-mcp/tests/tools.rs`:
  - **Delete** `a_message_cannot_be_addressed_to_someone_in_another_org`,
    `a_direct_message_fails_closed_when_the_platform_cannot_answer`, and
    `a_reply_to_a_private_message_stays_private`.
  - **Rework** `ack_does_not_land_on_someone_elses_private_message` into
    `ack_does_not_land_on_a_hidden_or_other_channels_message`: in `teams_world`, alice acks the
    `shared` repo's channel with `up_to = i64::MAX` after messages land in `shared`,
    `growth-repo` (hidden from her), and team `platform`. The cursor lands on `shared`'s newest.
  - **Update** (add `repo: Some(..)` and `team: None`; register a repo where the test has
    none): `messages_reach_the_inbox_and_the_cursor_clears_them`, the two
    `send_message_with_*idempotency_key*` tests,
    `messages_about_a_hidden_repo_or_job_are_not_delivered`,
    `a_deleted_jobs_messages_keep_their_team_scope`,
    `deleting_a_job_scopes_messages_the_backfill_missed`,
    `a_reply_is_scoped_to_its_thread_and_cannot_probe_hidden_ids`,
    `a_reply_cannot_land_after_its_parents_repo_moves_out_of_sight`, plus every other
    `SendMessageArgs` / `InboxArgs` / `AckMessagesArgs` literal the compiler reports. Update the
    tool-list and description assertions if they pin argument names.
  - **Add** (in `teams_world`, which has `platform` with alice and `growth` with bob):
    - `a_team_channel_reaches_its_members`: alice posts to `team: "platform"`; the owner's
      `inbox` for `platform` sees it; alice's `inbox` for repo `shared` does not; owner
      `unread_count` for `platform` is 1, then 0 after `ack_messages`.
    - `a_team_channel_is_not_found_for_a_non_member`: bob's `send_message`, `inbox`,
      `unread_count`, and `ack_messages` on `team: "platform"` all give `team_not_found`, with
      the same message as `team: "no-such-team"`.
    - `an_admin_reads_a_team_channel_they_are_not_on`: the owner (no team membership) reads
      `platform`.
    - `a_team_channel_fails_closed_when_the_platform_is_down`: `env.platform.set_down(true)`;
      `inbox` with `team` gives the `503` / `platform_unavailable` error the file's existing
      `unavailable` check (a closure local to
      `a_platform_outage_refuses_team_scoped_reads_and_writes`; copy it). Note in the test that
      `team_scope`'s `member` lookup fails first, so this proves the tool fails closed, not
      that `by_slug` does; Task 2's core test covers `by_slug`.
    - `send_message_needs_exactly_one_channel`: neither is `invalid_argument`; repo plus team
      is `invalid_argument`.
    - `send_message_refuses_to`: a raw JSON call with `"to": "x@acme.test"` is rejected as an
      unknown field. Go through the HTTP or JSON layer the file already uses for schema-level
      checks; if none exists, deserialize `SendMessageArgs` from JSON directly in a unit test
      in `coord.rs`.
    - `a_legacy_unscoped_broadcast_is_never_served`: stand in for a pre-migration row. A
      `NOT VALID` check still binds new inserts, so drop `messages_channel_ck` on the pool,
      insert a row with neither channel, and re-add the constraint `NOT VALID`. Then `inbox`
      and `unread_count` for every channel in the fixture return nothing of it.
- [x] `cargo check -p of-mcp --all-targets`, `cargo test -p of-mcp`,
      `cargo clippy -p of-mcp --all-targets -- -D warnings`.
- [x] **Final gate:** `cargo test` and `cargo clippy --all-targets -- -D warnings`, unscoped.
- [x] Commit `feat(of-mcp)!: scope message tools to a repo or team channel`, with a
      `BREAKING CHANGE:` footer.

## Task 5 — `CLAUDE.md` ✅

- [x] In "Authentication": `member_by_email (message recipients)` becomes
      `team_by_slug (message team channels, via VerifiedTeam::by_slug)`. Keep the bullet's
      "fails closed" sentence.
- [x] Grep `CLAUDE.md` for "addressed", "broadcast", "recipient", and "private" and correct any
      remaining description of DMs. The `scope_teams` paragraph stays accurate.
- [x] Commit `docs: describe message channels in CLAUDE.md`.

---

## Out-of-band artifacts

- **Migration** `0008`: verified by Task 1's tests and a fresh-cluster `cargo test -p of-core`.
  Production runs it on deploy. It deletes DMs (signed off) and needs no manual step.
- **Clients**: a breaking MCP change. The release notes come from the `BREAKING CHANGE:`
  footer. `.github/skills/otto-tower-worker/SKILL.md` mentions `send_message` only as a
  string-writing rule, with no `to`, so it needs no change. Re-grep `client-skills/` and
  `.github/skills/` for `send_message` and `inbox` calls that pass arguments before merging.
- **Console, Worker, container image, CI, config**: untouched. No console code calls the
  messaging tools.

