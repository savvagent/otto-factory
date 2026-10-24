# Message channels design

> **Status:** IMPLEMENTED (PR #254, 2026-10-24) — replace the org-wide broadcast and
> person-directed DMs with two required, mutually exclusive message channels (repo or team) with
> per-channel read cursors. A **breaking** change to the MCP surface (`feat(of-mcp)!:`). Closes
> savvagent/otto-factory#177.
>
> **Revised 2026-10-23** to match the code after the platform split and the migration squash
> (`savvagent/otto-factory#192`). The 2026-09-11 draft was written against an earlier schema
> with an in-database `teams` table and migrations numbered up to `0030`. What changed:
> teams are the platform's (no `teams` table, no foreign keys to it, `VerifiedTeam`/`TeamScope`
> instead of `resolve_team`); team visibility is now enforced and fails closed, including on
> messages (`scope_teams`, `0004_message_scope.sql`); the ack clamp is visibility-aware; and
> dropping `recipient_user_id` without deleting the DM rows would have leaked them (see §1).
> The paired plan (`docs/plans/2026-10-24-message-channels.md`) was regenerated from this revision the same day.
>
> **Signed off 2026-10-23** by the org owner: deleting DMs, not seeding per-channel cursors,
> refusing `to` as an unknown field, and keeping `"recipientUserId": null` in the idempotency
> fingerprint (the open items in Risks below are accepted).

## Brief

Redesign otto-tower's messaging (`crates/of-core/src/messages.rs`, and `send_message` /
`inbox` / `ack_messages` / `unread_count` in `crates/of-mcp/src/tools/coord.rs`). Today it is
an org-wide broadcast plus person-directed private DMs (`to: <email>`). It becomes two scoped
coordination channels:

- A **repo channel**: coordination scoped to one repo, the anchor jobs and leases already use.
- A **team channel**: a team's coordination across repos.

Every new message targets exactly one of the two. There is no more unscoped broadcast and no
more DM. Person-directed messaging is the one part of this feature that is chat-shaped rather
than coordination-shaped, and otto-tower is not becoming a chat product (constraint 2,
substrate not workflow).

An "ask another agent for help" primitive was considered and set aside. The job queue
(`add_job` + `claim_jobs` + `metadata`) plus these channels already covers it through a
customer's own skill; a bespoke delegation primitive would make the server opinionated about
how an agent orchestrates work, which constraints 2 and 3 put in the skill.

## What exists today

Facts this design builds on. All verified against `master` at `b98761d`.

- **Schema** (`0001_baseline.sql`, `0004_message_scope.sql`): `messages` has
  `recipient_user_id uuid` (NULL = broadcast), `team_id uuid` (opaque, no FK; the platform owns
  teams), `repo_id uuid REFERENCES repos (id) ON DELETE SET NULL`, `job_id` (composite FK to
  `jobs`, `ON DELETE SET NULL (job_id)`), `in_reply_to`, the idempotency pair, and
  `scope_teams uuid[]`: every team the message was bound to when sent (its team, its repo's
  team, its job's and that job's repo's team, and its thread's `scope_teams`), which never
  changes. `message_cursors` is one row per `(org_id, user_id)`, a single global
  `last_read_id`. Both tables are in the baseline's `tenant_tables`, so both have a
  `<table>_tenant_isolation` policy and `FORCE ROW LEVEL SECURITY`.
- **Visibility.** `message_visible_sql` restricts every read to messages whose `scope_teams`,
  `team_id`, live repo team, and live job (and job-repo) team are all within the reader's
  `TeamScope`. Owners and admins are `TeamScope::All`.
- **MCP surface.** `send_message` takes `to` (email, resolved through the platform with
  `member_by_email`, failing closed), `repo`/`remote` (optional, `maybe_repo_for_write`), `job`,
  `in_reply_to`, `kind`, `agent`, `idempotencyKey`. It **never sets `team_id`**: no MCP tool
  takes a team today. A reply to a DM is forced to stay between its two participants
  (`ReplyParent::reply_recipient`). `inbox` and `unread_count` read the whole org's broadcasts
  plus the caller's DMs, through the caller's `TeamScope`. `ack_messages_for` clamps the cursor
  to the newest message the caller could read, so where the cursor lands reveals nothing
  about hidden traffic.
- **Lifecycle** (`of_core::platform_events`): `member.removed` deletes the user's
  `message_cursors` rows; `org.deleted` deletes `messages` and `message_cursors` (children
  before `repos`); `team.deleted` leaves `team_id` in place as a tombstone, so the team's rows
  become visible to owners and admins only. No code path deletes a single repo.
- **Console.** No console code calls a messaging route or tool (`web/src` has no hits; the
  `messages` there are the Paraglide catalogs).

## Decisions

- **A team channel is readable and writable by that team's members, plus owners and admins.**
  This replaces the 2026-09-11 draft's "any member may read any channel", which predates
  enforced team visibility. It is the same rule repos, jobs, and messages already follow
  (`TeamScope::allows`), so it adds no new authorization concept. A team the caller is not on
  is reported exactly like a team that does not exist (`team_not_found`). That matches how a
  hidden repo is `repo_unresolved`, so the tool cannot be used to list teams.
- **A repo channel is readable by anyone who can see the repo** (`resolve_repo_visible`). Each
  message is still filtered through `message_visible_sql`, so a message in a visible repo's
  channel that names a job, or replies into a thread, bound to a team the reader is not on
  stays hidden. That is today's rule, unchanged.
- **Teams are named by slug** (`team`), resolved through `PlatformClient::team_by_slug` and
  checked against the caller's org: the same lookup the console uses for `?team=`
  (`of-web/src/routes/jobs.rs::resolve_team`). An unknown slug cannot list the valid ones,
  because team slugs are the platform's. The error says so and points at the platform console,
  as the console's does. A platform that cannot answer is `503 platform_unavailable`, never
  "no team".
- **Per-channel read cursors.** A single cursor cannot work with reading one channel at a
  time: acking a repo channel up to id N would silently mark lower-numbered messages in every
  other channel read. Each `(org, user, channel)` gets its own cursor.
- **The DMs are deleted, not orphaned** (see §1 for why orphaning leaks). Unscoped historical
  broadcasts are left in place. Under the new reads they belong to no channel, so they are never
  served again, and the new `CHECK` is `NOT VALID` so their existence never fails a migration.
- **`watch` stays org-wide.** It already wakes on any job, lease, or message change in the org
  and carries no data. A channel filter would need `repo_id`/`team_id` in the `pg_notify`
  payload and a filter in `Watcher::wait`, a separate design. The accepted timing signal that
  CLAUDE.md records for `watch` is unchanged.
- **`in_reply_to` may point into another channel.** It is a threading hint, not a
  same-channel invariant. It cannot widen anything: a reply already inherits its parent's
  `scope_teams`, so a cross-channel reply is visible only to readers who could also see the
  parent. The parent must still be readable by the sender, with the same
  "not a message you can read" error that cannot be used to probe ids.
- **`job` stays optional on any message**, in either channel. A team-channel message about a
  job carries `job_id` and no `repo_id`; its `scope_teams` still picks up the job's repo's team.

## Goal & Success Criteria

Turn the message channel from an org broadcast plus DMs into per-repo and per-team channels,
matching how jobs and leases are scoped, so a fleet working across many repos sees only the
coordination relevant to its work.

1. `send_message` requires exactly one of `{repo, remote}` or `team`. Supplying neither, or
   both a repo and a team, is `invalid` naming the two valid forms. `to` is no longer
   accepted: it is removed from the schema, and the struct denies unknown fields so an old
   caller sending `to` gets an error instead of a silent broadcast into a channel it did not
   choose.
2. `inbox`, `unread_count`, and `ack_messages` take the same selector, also required, and read
   and acknowledge one channel. Reading or acking one channel never changes another channel's
   unread state.
3. A team channel of a team the caller is not on is `team_not_found` on all four tools,
   indistinguishable from a slug that does not exist. A platform outage while resolving a team
   is `503`.
4. After the migration no DM row survives, in any org. No message is reachable through a
   channel it was not posted to, and no historical unscoped broadcast is served.
5. Tests demonstrate cross-channel isolation (repo A ↔ repo B ↔ team T), per-channel cursors,
   team-channel visibility, and the migration's per-org scoping (below).
6. Tool descriptions describe channels and pass `crates/of-mcp/tests/tools.rs`'s
   self-description assertion. `every_tool_has_a_price` is unaffected (no tool is added or
   removed).
7. This removes a tool argument, so it is a breaking change to the MCP surface. The PR title is
   `feat(of-mcp)!: …` with a `BREAKING CHANGE:` footer (CLAUDE.md, Releases & versioning).

## Scope

**In scope:** migration `0008_message_channels.sql`; `of-core::messages` and a slug
constructor in `of-core::teams`; `of-mcp::tools::coord` and a channel resolver in
`of-mcp::tools`; removal of `member_by_email`, `ReplyParent::reply_recipient`, and the
`not_a_member` error; tests in `of-core` and `of-mcp`.

**Non-goals:** an agent-delegation protocol; console UI; a channel-aware `watch`; a "read
every channel I'm in" digest (that is the firehose this removes); seeding per-channel cursors
from the old global cursor (see Risks); deleting unscoped historical broadcasts.

## Architecture

### §1 Migration `0008_message_channels.sql`

`0007_trigger_label_default_otto_tower.sql` is the current head. Confirm at implementation
time.

**Why the DMs must be deleted, not orphaned.** A DM could carry a `repo_id`: `send_message`
accepted `to` and `repo` together. Dropping `recipient_user_id` and leaving the row would turn
it into an ordinary message in that repo's channel, readable by everyone who can see the repo.
That would publish private messages. So the migration deletes every row with
`recipient_user_id IS NOT NULL` first. Replies to DMs are themselves DMs
(`reply_recipient` forced it), so whole private threads go together. Any other row pointing at
one keeps working through `in_reply_to ... ON DELETE SET NULL`.

**Why legacy broadcasts lose their job link.** A `NOT VALID` CHECK skips the rows already
there, but it still binds every later `UPDATE` of them. Today's `send_message` accepts `job`
without `repo`, so production can hold unscoped broadcasts with a `job_id` and neither channel
column. Deleting such a job after `0008` updates those rows twice: `delete_job`'s `scope_teams`
widening and the FK's `ON DELETE SET NULL (job_id)`. Both would violate `messages_channel_ck`,
so `delete_job` would fail. Those rows belong to no channel and are never served again, so the
same per-org pass nulls their `job_id` before the constraint exists. The same applies to a row
with **both** `repo_id` and `team_id` (only a direct `of-core` caller could write one; MCP never
set `team_id`), so the update covers every row that fails the new CHECK, not just channel-less
ones. After that nothing updates
them: `delete_job` matches by `job_id`, no path deletes a repo, and replies to them are
themselves legacy rows or carry a channel. A test deletes a job a legacy broadcast once named.

The legacy global cursors are deleted in the same pass. Under per-channel reads they match
nothing, and removing them lets the cursor `CHECK` be fully validated instead of `NOT VALID`.

The two deletes and the update rewrite tenant-table data, so they follow CLAUDE.md's one pattern safe under every
deployment shape: loop over orgs, and give every statement **both** an explicit `org_id`
predicate **and** `set_config('app.org_id', …, true)`. Enumerating the orgs is the one read
that cannot be pinned. Where RLS applies to the migrating role, the tenant policies hide every
row from it, and the loop would silently do nothing. So, exactly as `0004_message_scope.sql`
does, a read-only `current_org() IS NULL` policy is created on each table for the length of the
migration and dropped at the end, in the migration's own transaction. The org list is
read into an array **before** the loop: a plpgsql `FOR ... IN <query>` loop reads through a
cursor in batches, re-evaluating `current_org()` per scanned row, so after the first
`set_config` the temporary policy is false and a streaming plan would skip every later org,
whose DMs would then survive the column drop as public channel posts. (`0004` has the same
latent pattern; it is applied and is not edited.)

```sql
CREATE POLICY messages_channels_0008 ON messages FOR SELECT USING (current_org() IS NULL);
CREATE POLICY message_cursors_channels_0008 ON message_cursors FOR SELECT USING (current_org() IS NULL);

DO $$
DECLARE
  o uuid;
  orgs uuid[];
BEGIN
  -- Read every org up front. A `FOR o IN <query>` loop fetches through a cursor in
  -- batches and evaluates current_org() per scanned row, so once set_config has pinned
  -- an org the temporary policy is false and later orgs can be skipped where RLS applies.
  SELECT array_agg(DISTINCT org_id) INTO orgs
    FROM (SELECT org_id FROM messages UNION SELECT org_id FROM message_cursors) s;
  FOREACH o IN ARRAY COALESCE(orgs, '{}') LOOP
    PERFORM set_config('app.org_id', o::text, true);
    DELETE FROM messages WHERE org_id = o AND recipient_user_id IS NOT NULL;
    DELETE FROM message_cursors WHERE org_id = o;
    -- See "Why legacy broadcasts lose their job link" above.
    UPDATE messages SET job_id = NULL
      WHERE org_id = o AND (repo_id IS NOT NULL) = (team_id IS NOT NULL)
        AND job_id IS NOT NULL;
  END LOOP;
  PERFORM set_config('app.org_id', '', true);
END $$;

DROP POLICY messages_channels_0008 ON messages;
DROP POLICY message_cursors_channels_0008 ON message_cursors;

-- Messages are no longer addressed to a person. messages_recipient_idx goes with the column.
ALTER TABLE messages DROP COLUMN recipient_user_id;

-- Exactly one channel, for every row written from here on. NOT VALID because historical
-- unscoped broadcasts remain; they belong to no channel and are never served.
ALTER TABLE messages
  ADD CONSTRAINT messages_channel_ck
  CHECK ((repo_id IS NOT NULL) <> (team_id IS NOT NULL)) NOT VALID;

CREATE INDEX messages_repo_channel_idx ON messages (org_id, repo_id, id) WHERE repo_id IS NOT NULL;
CREATE INDEX messages_team_channel_idx ON messages (org_id, team_id, id) WHERE team_id IS NOT NULL;

-- One cursor per member per channel. team_id is opaque (teams are the platform's; no FK).
-- A synthetic key because a primary key cannot include a nullable column; the two partial
-- unique indexes express "one per channel".
ALTER TABLE message_cursors
  DROP CONSTRAINT message_cursors_pkey,
  ADD COLUMN id bigserial PRIMARY KEY,
  ADD COLUMN repo_id uuid REFERENCES repos (id) ON DELETE CASCADE,
  ADD COLUMN team_id uuid,
  ADD CONSTRAINT message_cursors_channel_ck
    CHECK ((repo_id IS NOT NULL) <> (team_id IS NOT NULL));

CREATE UNIQUE INDEX message_cursors_repo_idx
  ON message_cursors (org_id, user_id, repo_id) WHERE repo_id IS NOT NULL;
CREATE UNIQUE INDEX message_cursors_team_idx
  ON message_cursors (org_id, user_id, team_id) WHERE team_id IS NOT NULL;

-- The new id's sequence. The baseline's default privileges cover it only when the
-- role that ran the baseline runs this migration too; say it explicitly, guarded the
-- way the baseline guards every grant to otto_app.
DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'otto_app') THEN
    GRANT USAGE, SELECT ON SEQUENCE message_cursors_id_seq TO otto_app;
  END IF;
END $$;
```

The tenant policies themselves are untouched: column, constraint, and index changes are
schema-only. `TRUNCATE` and `COPY` are not used (CLAUDE.md forbids both against tenant tables
in a migration).

### §2 `of-core`

**`teams`.** Add `VerifiedTeam::by_slug(platform, org, slug) -> Result<VerifiedTeam>`, beside
`verify`, failing closed the same way: the platform's "no such team" (or an answer for another
org) is `Error::TeamNotFound`, and a failure to answer is `Error::Platform`. `TeamNotFound`
has one `Display`, and today's says "omit the team to make the repo organization-wide", which
is wrong for a message channel. It is reworded to be neutral (it serves repos, jobs, and
channels) and to include "or you are not on it", because a hidden team gets this same error.
Its code stays `team_not_found`. Both the unknown-slug and not-on-the-team answers are built
from the slug the caller passed, so they are byte-identical.

**`messages`.**

- `ChannelRef` enum: `Repo(RepoId)` | `Team(TeamId)`. The one channel parameter for every read.
- `Message` and `NewMessage` lose `recipient_user_id`. `MSG_COLS`, both `INSERT`s, and
  `message_idempotency_fingerprint` drop it too. `"teamId"` and `"repoId"` are already
  fingerprinted, so a replay's identity becomes "same channel, same everything else" with
  no other change. Keys stored before the migration can no longer replay with the same
  fingerprint, because `"recipientUserId": null` was part of the hashed JSON. Either keep the
  key in the JSON as a constant `null` so pre-migration keyed broadcasts still replay, or
  accept that they conflict. **Decision: keep `"recipientUserId": null`**, for the same reason
  `send_message` keeps hashing the label as sent (#163): a byte-for-byte retry must replay.
- `Tx::send_message` checks exactly one of `repo_id` / `team_id` (`Error::Invalid`) beside
  its existing body and key checks. That application check is what makes the `NOT VALID`
  constraint safe to leave unvalidated: no new row can violate it. `find_replayed_message`
  needs no channel check, because a fingerprint mismatch already covers it.
- `InboxQuery` gains `channel: ChannelRef`. It loses `impl Default`, because no channel is a
  sensible default; call sites name it.
- `inbox`, `unread_count_for`, and `ack_messages_for` take a channel. The `recipient_user_id`
  predicate becomes `m.repo_id = $n` or `m.team_id = $n`. Which column is chosen in Rust
  (two fixed query shapes), never interpolated from input, for the same reason `inbox`
  already picks its `ORDER BY` direction in Rust. Their cursor sub-select reads the channel's
  row. `message_visible_sql` is applied unchanged on top.
- `ack_messages_for` clamps to the newest message **in that channel** that the reader may
  see, then upserts that channel's cursor (`ON CONFLICT (org_id, user_id, repo_id) WHERE
  repo_id IS NOT NULL`, or the team equivalent). The 2026-09-11 draft clamped to the org-wide
  `MAX(id)`. That is wrong now: landing on a hidden message's id reveals that it exists, which
  is exactly what the current visibility-aware clamp prevents.
- The `TeamScope::All` convenience wrappers (`ack_messages`, `unread_count`) either take the
  channel too or are removed. They exist for tests, and the plan picks one.
- `ReplyParent` and `reply_recipient` are deleted. `reply_parent` becomes a readability check
  returning `()`. It keeps holding the parent's repos `FOR SHARE` for a restricted scope, and it
  keeps its probe-proof error. `message_readable` loses its recipient predicate. A legacy
  unscoped broadcast still passes it and can be an `in_reply_to` parent; that reveals nothing,
  because its sender could already read it, and the reply itself must name a channel.

### §3 `of-mcp`

- **Channel resolution** (`tools/mod.rs`, beside `repo_of`): `channel_of(...)` takes
  `repo`/`remote`/`team` and returns `ChannelRef`. Exactly one must be named (blank counts as
  absent, as in `maybe_repo_of`). Otherwise the error is `invalid`, naming
  `repo` / `remote` / `team` and what was received. A repo resolves through `repo_of`, or
  `repo_for_write` with `Hold::Shared` on `send_message`. A team resolves through
  `VerifiedTeam::by_slug`, **before the transaction opens**: platform calls stay outside
  transactions, as `team_scope` does. Then `TeamScope::allows(Some(team))` must hold, or it is
  the same `team_not_found`.
- **Holding a team.** A repo is held during a write so an admin cannot move it between the
  check and the insert. A team channel has no row here to hold, because membership is the
  platform's. Removing a member from a team in that window lets one last message through. That
  is the same 10 s `member_teams` cache window that already applies to every team-scoped read,
  and it is accepted, not closed.
- **`SendMessageArgs`**: remove `to`; add `team: Option<String>`; add
  `#[serde(deny_unknown_fields)]` (criterion 1). `InboxArgs` and `AckMessagesArgs` gain
  `repo` / `remote` / `team`. `unread_count` moves from `NoArgs` to an args struct with the same
  three fields. All four take `deny_unknown_fields`, as the job tools' args already do.
- **Deleted:** `Factory::member_by_email`, the `reply_recipient` helper in `coord.rs`, the
  `not_a_member` error. `PlatformClient::member_by_email` stays in `otto-resource`; it is just
  unused here.
- **Descriptions:**
  - `send_message`: "Post a note to one repo's or one team's shared channel. Pass `repo` (or
    `remote`) for coordination about that repo, or `team` (a team slug) for a team's work
    across repos — exactly one. …" The idempotency and `agent` sentences stay.
  - `inbox`, `unread_count`, and `ack_messages` each say they work on one channel at a time,
    named the same way, and that an unknown team or a team you are not on is
    `team_not_found`.
- No change to metering. All four tools stay free, and `charge` stays the first call after
  `self.tx(...)`.

### §4 Elsewhere

- `platform_events`: no change. `member.removed`'s `DELETE FROM message_cursors WHERE org_id
  = $1 AND user_id = $2` removes every channel cursor. `org.deleted` already lists both
  tables. `team.deleted` leaves team-channel messages and cursors under the tombstoned id. They
  are kept but unreachable over MCP: a deleted team's slug no longer resolves through
  `team_by_slug`, so even an owner gets `team_not_found`. Nobody else would match them anyway.
- `jobs.rs`'s `delete_job` `scope_teams` widening: unchanged.
- Console: unchanged.

## Error handling

| Case | Answer |
|---|---|
| Neither repo nor team named | `invalid`: "name a channel: `repo` (or `remote`) for a repo's channel, or `team` for a team's" |
| Both named | `invalid`: "name one channel: a repo or a team, not both" |
| Repo unresolvable or hidden | `repo_unresolved`, listing visible slugs (unchanged) |
| Team slug unknown, foreign, deleted, malformed, or not one of the caller's | `team_not_found` (`Error::TeamChannelNotFound`), naming the slug, pointing at the platform console and at a repo channel |
| `ack_messages` `upTo` not 0 and not a message the caller can read in that channel | `invalid_argument`, the same for a nonexistent, hidden, or other-channel id |
| Platform cannot answer the team lookup or the scope lookup | `503 platform_unavailable`, `Retry-After` |
| `to` sent by an old client | `invalid` (unknown field) |
| `in_reply_to` not readable | unchanged: "not a message you can read…" |

## Testing

This is not additive: about 25 existing tests construct DMs or call the channel-less
signatures and stop compiling. Budget for repairing them.

**Delete** (they test DMs):

- `of-core/tests/queue.rs`: `directed_messages_are_private_within_the_org`
- `of-mcp/tests/tools.rs`: `a_message_cannot_be_addressed_to_someone_in_another_org`,
  `a_direct_message_fails_closed_when_the_platform_cannot_answer`,
  `a_reply_to_a_private_message_stays_private`

**Rework**:

- `of-mcp/tests/tools.rs`: `ack_does_not_land_on_someone_elses_private_message`. Keep the
  property, rewritten as "ack does not land on a hidden message, nor on another channel's".

**Update** (thread a channel through; keep the assertions):

- `of-core/tests/queue.rs`: `unread_excludes_self_and_ack_is_clamped`, the four
  `send_message_*` / `the_same_key_reused_across_add_job_and_send_message_does_not_conflict`
  idempotency tests, `send_message_applies_the_agent_label_policy`,
  `a_keyed_send_message_with_a_padded_label_replays_cleanly`,
  `a_pre_policy_keyed_message_with_a_now_invalid_label_still_replays`
- `of-core/tests/isolation.rs`: `message_idempotency_keys_do_not_cross_org_boundaries`,
  `messages_are_invisible_across_orgs`
- `of-core/tests/platform_events.rs`: `a_message_takes_a_team_from_its_own_org_only_keyed_or_not`
- `of-mcp/tests/tools.rs`: `messages_reach_the_inbox_and_the_cursor_clears_them`, the two
  `send_message_with_*idempotency_key*` tests, `messages_about_a_hidden_repo_or_job_are_not_delivered`,
  `a_deleted_jobs_messages_keep_their_team_scope`, `deleting_a_job_scopes_messages_the_backfill_missed`,
  `a_reply_is_scoped_to_its_thread_and_cannot_probe_hidden_ids`,
  `a_reply_cannot_land_after_its_parents_repo_moves_out_of_sight`, and the tool-list and
  description assertions

**Add**:

- `of-core`: `send_message` refuses neither or both channels; a message in repo A's channel is
  absent from repo B's and from team T's, and vice versa; acking repo A leaves repo B's and
  team T's unread counts unchanged; the ack clamp is per channel; a cross-channel reply
  inherits its parent's `scope_teams`.
- `of-core` migration test, after `crates/of-core/tests/meter_rename.rs`: seed two orgs with
  DMs (one carrying a `repo_id`), broadcasts, and legacy cursors; run `0008`; assert every DM
  and legacy cursor is gone in **both** orgs and every non-DM row survives, with a legacy broadcast's `job_id` nulled. Run it once with
  the migrating role subject to RLS (as a non-owner role, the way
  `rls_scopes_a_migration_style_update_with_no_org_context` does), so the temporary policies are
  proven to be what makes the loop find the orgs.
- `of-mcp`: team channel end to end (`send_message` / `inbox` / `unread_count` /
  `ack_messages` with `team`); a member not on the team gets `team_not_found`, byte-identical to
  an unknown slug; an admin not on the team can read it; `MockPlatform::set_down` during team
  resolution is a `503`; `to` is rejected; a legacy unscoped broadcast is never served.
- `of-core` migration test: after the migration, deleting a job that a legacy unscoped broadcast named
  succeeds (the migration nulled that link; see §1).

Gates: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all`.

## Risks & open questions

- **Deleting DMs is one-way.** It is what the column drop would have done in effect, minus the
  leak. Restoring them means a backup restore. Signed off 2026-10-23.
- **First read of each channel shows old traffic as unread.** Legacy cursors are deleted rather
  than translated, so every member starts each channel at cursor 0. `inbox` is capped at 50 by
  default (200 max), so this is bounded and happens once. Seeding per-channel cursors from the
  old global one is possible (`last_read_id` carried to each channel the member has messages
  in) but adds a data migration for a one-time nuisance. Left out unless the owner wants it.
- **`watch` remains org-wide**, extended to channel messages but not newly introduced.
- **No digest view.** By design.
- **A team-channel post can land in the `member_teams` cache window** after the sender leaves
  the team (≤ 10 s). Same accepted gap as every team-scoped read.
- **Deleting a repo deletes its channel.** `messages.repo_id` becomes `ON DELETE CASCADE` in
  `0008` (addendum, item 4), because `SET NULL` would null a repo-channel message's only
  channel column and fail `messages_channel_ck`. No path deletes a single repo today; whoever
  adds one gets the channel removed with it, as its cursors already are.
- **Every team-channel call makes one uncached platform request.** `team_by_slug` has no cache
  in `otto-resource` (unlike `member_teams`, 10 s), so each team-channel `send_message` /
  `inbox` / `unread_count` / `ack_messages` costs a round trip and answers `503` during a
  platform blip. Accepted for now and said in `unread_count`'s description; a short positive
  slug cache belongs upstream in `otto-resource` if it shows up in practice.
- **`message_cursors.repo_id` references `repos (id)`, not `(org_id, id)`**, like every other
  `repo_id` foreign key in the schema. A direct `of-core` caller writing a cursor in org A that
  names org B's repo succeeds or fails on whether that id exists anywhere. Repo ids are random
  UUIDs and every MCP path resolves the repo through the caller's own org first, so this is not
  reachable from outside. Accepted, consistent with the existing keys.
- **`deny_unknown_fields` on the four message args structs** follows the precedent of the job
  tools' args (`tools/jobs.rs`). It matters most here: a dropped `to` must not silently become
  a channel post. The error serde produces names the unknown field, but not why it went away.
  If that is too terse for an LLM caller, the alternative is an explicit `to` field that always
  errors with a migration hint, for one release.

## Addendum — review round 1 (PR #254), 2026-10-24

The review trio and the PR toolkit reviewers found no critical issue. These changes were made in
response; each supersedes the text above where they differ.

1. **The migration locks both tables first** (`LOCK TABLE messages, message_cursors IN ACCESS
   EXCLUSIVE MODE`). An old image inserting a DM with a `repo_id` between the delete and the
   column drop, during a rolling deploy, would publish it. The temporary policies already took
   that lock as a side effect; now it is explicit.
2. **Rows that fail the new CHECK** (no channel, or both) keep their visibility before losing
   their links: the job's team and the job's repo's team are folded into `scope_teams`, as
   `delete_job` does, and only then are `job_id` and `in_reply_to` nulled (an `in_reply_to ON
   DELETE SET NULL` update would also violate the CHECK).
3. **A backstop refuses the column drop if any DM survived.** After the loop, with `app.org_id`
   unset and the temporary policies still exposing every row, the migration raises an exception
   if a row with `recipient_user_id` (or a legacy cursor) remains. A missed org becomes a failed
   deploy instead of a privacy leak.
4. **`messages.repo_id` is `ON DELETE CASCADE`** (see Risks).
5. **Channel predicates are exclusive**: a repo channel is `m.repo_id = $n AND m.team_id IS
   NULL`, a team channel `m.team_id = $n AND m.repo_id IS NULL`, so a legacy row with both is
   served in neither, matching the CHECK.
6. **`ack_messages` refuses an `upTo` outside its channel.** Ids are global and cursors are per
   channel, so clamping another channel's id (or `i64::MAX`) silently marked the whole channel
   read. `upTo` must be 0 or a message the caller can read in that channel; a nonexistent,
   hidden, or other-channel id gets the same `invalid_argument`, so it cannot probe.
7. **Team slugs are shape-checked** before they reach the platform URL (empty, over 128 bytes,
   `.`/`..`, `/`, whitespace, or control characters are `team_not_found` without a request),
   and a platform answer for another org is logged as an error while still refused.
8. **`TeamChannelNotFound`** carries the channel wording ("or you are not on it … name a team
   you are on, or use a repo's channel"), with the same `team_not_found` code. `TeamNotFound`
   keeps its original text for the repo, job, and console paths, where membership is not checked.
