-- The `df_changes` Postgres NOTIFY channel from 0003/0004 is an internal
-- implementation detail (nothing outside the server process ever LISTENs on
-- it), so unlike 0007's role name it carries no external contract to
-- preserve. Re-defining the trigger functions here — rather than editing
-- 0003/0004 in place — keeps the forward-only migration discipline: a
-- database that already ran 0003/0004 gets this change by applying 0019, not
-- by an already-applied migration silently changing underneath it.
CREATE OR REPLACE FUNCTION notify_change() RETURNS trigger AS $$
DECLARE
  rec record;
BEGIN
  rec := COALESCE(NEW, OLD);
  PERFORM pg_notify(
    'of_changes',
    json_build_object(
      'kind',  TG_ARGV[0],
      'org',   rec.org_id,
      'id',    rec.id::text,
      'op',    TG_OP
    )::text
  );
  RETURN NULL;  -- AFTER trigger; return value ignored
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_message() RETURNS trigger AS $$
BEGIN
  PERFORM pg_notify(
    'of_changes',
    json_build_object(
      'kind',   'message',
      'org',    NEW.org_id,
      'id',     NEW.id::text,
      'op',     TG_OP,
      'sender', NEW.sender_user_id
    )::text
  );
  RETURN NULL;
END;
$$ LANGUAGE plpgsql;
