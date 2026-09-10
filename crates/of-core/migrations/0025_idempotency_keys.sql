-- An idempotency key rides on the row it produces rather than living in its
-- own table, exactly like 0015's ticket_ref: the row already has the
-- lifetime and the org scoping an idempotency record needs, so it does not
-- need a TTL or a reaper of its own — see the idempotency-key design spec
-- (docs/specs/2026-09-10-idempotency-key-design.md) §1. NULL (the default,
-- and the only value for every call that omits a key) never participates in
-- the unique index below, so this is invisible to every existing caller.
--
-- The key and its payload hash are one logical value split across two
-- columns; every Rust read site decodes the hash into a non-optional
-- Vec<u8> once it has found a key, so a row with a key and a NULL hash
-- would be a decode failure on the one code path whose job is to be the
-- reliable one. A CHECK is cheaper than trusting every future writer to
-- keep the pair together by convention.
ALTER TABLE jobs
  ADD COLUMN idempotency_key text,
  ADD COLUMN idempotency_payload_hash bytea,
  ADD CONSTRAINT jobs_idempotency_pair_ck
    CHECK ((idempotency_key IS NULL) = (idempotency_payload_hash IS NULL));

CREATE UNIQUE INDEX jobs_org_idempotency_key_idx
    ON jobs (org_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

ALTER TABLE messages
  ADD COLUMN idempotency_key text,
  ADD COLUMN idempotency_payload_hash bytea,
  ADD CONSTRAINT messages_idempotency_pair_ck
    CHECK ((idempotency_key IS NULL) = (idempotency_payload_hash IS NULL));

CREATE UNIQUE INDEX messages_org_idempotency_key_idx
    ON messages (org_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
