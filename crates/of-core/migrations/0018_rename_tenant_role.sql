-- Renames the tenant-isolation role from `df_app` to `of_app`, following the
-- otto-factory rename. This is guard 2 of the two-guard tenant isolation rule
-- (see 0007_rls.sql), so this migration is the one part of the rename that can
-- silently disable a security control if it gets the fallback wrong.
--
-- `0007_rls.sql` and `0008_audit.sql` are not edited — migrations are
-- forward-only, and this file is deliberately additive: it renames the role
-- where it exists, and falls back to the same CREATEROLE-tolerant creation
-- shape 0007 uses where it does not.
--
-- Roles are cluster-scoped but the GRANTs below are per-database, and
-- `#[sqlx::test]` runs every migration against many throwaway databases that
-- share one cluster. Once `of_app` has been created in the cluster by an
-- earlier database's migration run, a later database's own `0007` run still
-- creates a *fresh* `df_app` there (it only ever checks for `df_app`, which is
-- correctly absent — it was renamed away, in a different database's
-- namespace, but roles have no per-database namespacing). Skipping the grant
-- block on the grounds that `of_app` "already exists" would leave that later
-- database's schema ungranted to the role every tenant transaction actually
-- runs as. So the grant block below always runs when `of_app` exists, whether
-- it was just renamed, just created, or already present from the cluster —
-- `GRANT` is idempotent, so re-running it against a database that was already
-- granted is a no-op, not a hazard.
DO $$
DECLARE
  have_old boolean;
  have_new boolean;
BEGIN
  have_old := EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'df_app');
  have_new := EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'of_app');

  IF have_old AND NOT have_new THEN
    ALTER ROLE df_app RENAME TO of_app;
    have_new := true;
  ELSIF NOT have_new THEN
    BEGIN
      CREATE ROLE of_app NOLOGIN;
      have_new := true;
    EXCEPTION WHEN insufficient_privilege THEN
      RAISE NOTICE
        'could not CREATE ROLE of_app (no CREATEROLE). Tenant transactions will '
        'run as the connecting role and rely on FORCE ROW LEVEL SECURITY, which '
        'holds only while that role is neither a superuser nor BYPASSRLS. '
        'of-server verifies this at startup and refuses to serve otherwise.';
    END;
  END IF;

  IF have_new THEN
    -- `SET LOCAL ROLE of_app` requires the connecting role to be a member of
    -- of_app (a superuser may assume any role, but the application should not
    -- connect as one). Granting to CURRENT_USER covers both the migrating
    -- role in tests and a single-role deployment; a deployment that connects
    -- as a separate least-privilege user must also `GRANT of_app TO <that
    -- user>`.
    BEGIN
      EXECUTE 'GRANT of_app TO CURRENT_USER';
    EXCEPTION WHEN insufficient_privilege THEN
      RAISE NOTICE
        'of_app exists but could not be granted to the migrating role. Tenant '
        'transactions will fall back to FORCE ROW LEVEL SECURITY; of-server '
        'verifies that at startup.';
    END;

    EXECUTE 'GRANT USAGE ON SCHEMA public TO of_app';
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO of_app';
    EXECUTE 'GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO of_app';
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO of_app';
    EXECUTE 'ALTER DEFAULT PRIVILEGES IN SCHEMA public '
            'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO of_app';
    EXECUTE 'ALTER DEFAULT PRIVILEGES IN SCHEMA public '
            'GRANT USAGE, SELECT ON SEQUENCES TO of_app';
  END IF;
END $$;
