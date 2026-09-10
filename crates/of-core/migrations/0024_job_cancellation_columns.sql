-- A cancellation request rides on the job row rather than a new table: a job
-- has at most one live request at a time, and nothing beyond what the audit
-- trail already keeps needs a history of past requests.
ALTER TABLE jobs ADD COLUMN cancel_requested_at timestamptz;
ALTER TABLE jobs ADD COLUMN cancel_requested_by uuid REFERENCES users (id) ON DELETE SET NULL;
ALTER TABLE jobs ADD COLUMN cancel_reason text;
