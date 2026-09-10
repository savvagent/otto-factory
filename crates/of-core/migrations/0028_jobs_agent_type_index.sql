-- agent_type went from write-only to filterable in list_jobs/ready — see
-- Tx::list_jobs and Tx::ready. Both filter on (org_id or repo_id, status,
-- agent_type) together, so index the triple rather than agent_type alone:
-- the existing jobs_org_status_idx / jobs_repo_status_idx already cover the
-- unfiltered case, and this one lets the planner satisfy the filtered case
-- without a residual scan over every pending row in a large queue.
CREATE INDEX jobs_org_status_agent_type_idx ON jobs (org_id, status, agent_type);
CREATE INDEX jobs_repo_status_agent_type_idx ON jobs (repo_id, status, agent_type);
