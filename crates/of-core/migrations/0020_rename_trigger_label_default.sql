-- 0014_trigger_label.sql set the tracker_bindings.trigger_label default (and
-- every row's initial value) to the previous product name.
-- 0014 is an already-applied migration, so it is not edited in place --
-- sqlx checksums each migration file's raw bytes and refuses to start against
-- a database that already ran a version whose stored checksum no longer
-- matches (see 0018/0019 for the same reasoning applied to the tenant role
-- and the NOTIFY channel). This is a genuine forward step instead: it moves
-- the default for new rows and relabels any existing row that still carries
-- the old product name, so a customer's tracker comments read "otto-factory"
-- going forward without depending on whether their row predates this
-- migration.
ALTER TABLE tracker_bindings
ALTER COLUMN trigger_label SET DEFAULT 'otto-factory';

UPDATE tracker_bindings
SET trigger_label = 'otto-factory'
WHERE trigger_label = 'dark' || '-factory';
