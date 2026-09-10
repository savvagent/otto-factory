-- Existing rows predate the resource convention and are bare branch names; rewrite
-- them to `branch:<name>` so their meaning is preserved under the renamed column.
ALTER TABLE repo_leases RENAME COLUMN branch TO resource;

UPDATE repo_leases SET resource = 'branch:' || resource
  WHERE resource NOT LIKE 'branch:%';

DROP INDEX repo_leases_live_key;
CREATE UNIQUE INDEX repo_leases_live_key
  ON repo_leases (repo_id, resource)
  WHERE released_at IS NULL;
