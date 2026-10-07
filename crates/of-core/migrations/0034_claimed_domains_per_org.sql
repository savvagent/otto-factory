-- Domain claims: unique per org, and only *verified* claims are unique per
-- domain. Parity with otto-platform's `0008_claimed_domains_per_org.sql`
-- (savvagent/otto-platform#6), which otto-core's `domains` module now assumes
-- (savvagent/otto-factory#191).
--
-- `0005_auth.sql` made `domain` the primary key, so the first org to claim a
-- domain held it whether or not it ever proved control, and could block the
-- real owner from ever setting up SSO. Any number of orgs may now hold a
-- pending claim; the first to pass DNS verification wins.
--
-- Existing rows already satisfy both new constraints: `domain` was globally
-- unique, so (org_id, domain) is too, and so is `domain` among verified rows.

ALTER TABLE claimed_domains DROP CONSTRAINT claimed_domains_pkey;
ALTER TABLE claimed_domains ADD PRIMARY KEY (org_id, domain);

-- At most one verified claim per domain. Routing (idp::resolve_for_domain)
-- reads only verified rows, so this is what keeps a domain routing to exactly
-- one org. Domains are stored lowercased (domains::normalize_domain).
CREATE UNIQUE INDEX claimed_domains_verified_domain_key
  ON claimed_domains (domain)
  WHERE verified_at IS NOT NULL;

-- The new primary key leads with org_id, which covers this index's lookups.
DROP INDEX claimed_domains_org_idx;
