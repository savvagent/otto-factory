-- Messages move to channels: a repo or a team, nothing else
-- (savvagent/otto-factory#177). Org-wide broadcasts and direct messages go.
--
-- The DMs are deleted, not orphaned. A DM could carry a `repo_id`
-- (`send_message` accepted `to` and `repo` together), so dropping
-- `recipient_user_id` and keeping the row would turn it into an ordinary
-- message in that repo's channel, readable by everyone who can see the repo:
-- it would publish private messages. Replies to DMs are themselves DMs, so
-- whole private threads go together; any other row pointing at one keeps
-- working through `in_reply_to ... ON DELETE SET NULL`.
--
-- The legacy per-member cursors are deleted in the same pass. Under
-- per-channel reads they match nothing, and removing them lets the cursor
-- CHECK below be fully validated instead of NOT VALID.
--
-- The statements in this loop rewrite tenant-table data, so they loop over orgs with both the
-- explicit `org_id` predicate and `app.org_id` (see CLAUDE.md).
--
-- Finding the orgs is the one read that cannot be pinned to an org. Where the
-- migrating role is subject to row-level security, the tenant policies hide
-- every row from an unpinned read and the loop would silently do nothing. As
-- in 0004_message_scope.sql, these read-only policies let an unpinned read see
-- the rows for the length of this migration. They are created and dropped
-- inside this migration's own transaction, so no other session ever sees them,
-- and they grant no write.
CREATE POLICY messages_channels_0008 ON messages FOR SELECT USING (current_org() IS NULL);
CREATE POLICY message_cursors_channels_0008 ON message_cursors FOR SELECT USING (current_org() IS NULL);

-- The org list is materialized into an array before the loop, not iterated as
-- a plpgsql `FOR ... IN <query>` loop. That form fetches through a cursor in batches and
-- evaluates the policies (and so `current_org()`) as it scans; once the first
-- iteration pins an org, the temporary `current_org() IS NULL` policy is false
-- and later orgs can be silently skipped where RLS applies, leaving their DMs
-- to become channel posts when `recipient_user_id` is dropped. (0004 has the
-- same latent pattern; it is applied and cannot be edited.)
DO $$
DECLARE
  o uuid;
  orgs uuid[];
BEGIN
  SELECT array_agg(DISTINCT org_id) INTO orgs
    FROM (SELECT org_id FROM messages UNION SELECT org_id FROM message_cursors) s;
  FOREACH o IN ARRAY COALESCE(orgs, '{}') LOOP
    PERFORM set_config('app.org_id', o::text, true);
    DELETE FROM messages WHERE org_id = o AND recipient_user_id IS NOT NULL;
    DELETE FROM message_cursors WHERE org_id = o;
    -- Every legacy row that fails `messages_channel_ck` loses its job link:
    -- unscoped broadcasts (neither channel) and rows naming both a repo and a
    -- team (only a direct of-core caller could write one). The NOT VALID CHECK
    -- below skips the rows already here but still binds every later UPDATE of
    -- them, and deleting the job would update such a row twice (`delete_job`'s
    -- `scope_teams` widening and the FK's `ON DELETE SET NULL (job_id)`), both
    -- violating `messages_channel_ck`, so `delete_job` would fail. These rows
    -- belong to no single channel and are never served again; after this
    -- nothing updates them.
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

-- The CHECK must come after the data step: the DM delete fires `in_reply_to ... ON DELETE
-- SET NULL` on surviving rows, and the job_id update is itself an UPDATE it would bind.
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
