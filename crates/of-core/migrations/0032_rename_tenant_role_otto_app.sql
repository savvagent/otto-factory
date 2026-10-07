-- Renames the tenant-isolation role from `of_app` to `otto_app`, so that
-- `otto_tenant::Db` (otto-platform) works against this database with its
-- default role name and no per-deployment override. otto-factory now runs on
-- the otto-platform crates (savvagent/otto-factory#191), and every otto-*
-- service shares the one role name.
--
-- Same shape and same reasons as `0018_rename_tenant_role.sql` (`df_app` ->
-- `of_app`), which this follows step for step; read its header for why the
-- grant block runs unconditionally whenever the role exists. In short: roles
-- are cluster-scoped, GRANTs are per-database, and `#[sqlx::test]` migrates
-- many throwaway databases in one cluster, so "the role already exists" must
-- never mean "this database is already granted".
--
-- Migrations 0007 and 0018 are not edited (sqlx checksums). They still create
-- and rename `df_app`/`of_app`; this one finishes the job. A role rename keeps
-- every privilege, default privilege, and role membership attached to the
-- role's oid, so on a database that already ran 0018 the rename alone carries
-- everything over and the grants below merely re-assert it.
--
-- The rename needs CREATEROLE. Where the migrating role lacks it (managed
-- Postgres), the rename and any CREATE ROLE are skipped with a NOTICE and
-- tenant transactions fall back to FORCE ROW LEVEL SECURITY, which
-- `Db::verify_tenant_isolation` re-checks at startup and refuses to serve
-- without.
DO $$
DECLARE
  have_old boolean;
  have_new boolean;
BEGIN
  have_old := EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'of_app');
  have_new := EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'otto_app');

  IF have_old AND NOT have_new THEN
    BEGIN
      ALTER ROLE of_app RENAME TO otto_app;
      have_new := true;
    EXCEPTION WHEN insufficient_privilege THEN
      RAISE NOTICE
        'could not rename role of_app to otto_app (no CREATEROLE). Tenant '
        'transactions will run as the connecting role and rely on FORCE ROW '
        'LEVEL SECURITY, which holds only while that role is neither a '
        'superuser nor BYPASSRLS. Startup verifies this and refuses to serve '
        'otherwise.';
    END;
  ELSIF NOT have_new THEN
    BEGIN
      CREATE ROLE otto_app NOLOGIN;
      have_new := true;
    EXCEPTION WHEN insufficient_privilege THEN
      RAISE NOTICE
        'could not CREATE ROLE otto_app (no CREATEROLE). Tenant transactions '
        'will run as the connecting role and rely on FORCE ROW LEVEL '
        'SECURITY, which holds only while that role is neither a superuser '
        'nor BYPASSRLS. Startup verifies this and refuses to serve otherwise.';
    END;
  END IF;

  IF have_new THEN
    BEGIN
      EXECUTE 'GRANT otto_app TO CURRENT_USER';
    EXCEPTION WHEN insufficient_privilege THEN
      RAISE NOTICE
        'otto_app exists but could not be granted to the migrating role. '
        'Tenant transactions will fall back to FORCE ROW LEVEL SECURITY; '
        'startup verifies that.';
    END;

    EXECUTE 'GRANT USAGE ON SCHEMA public TO otto_app';
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO otto_app';
    EXECUTE 'GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO otto_app';
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO otto_app';
    EXECUTE 'ALTER DEFAULT PRIVILEGES IN SCHEMA public '
            'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO otto_app';
    EXECUTE 'ALTER DEFAULT PRIVILEGES IN SCHEMA public '
            'GRANT USAGE, SELECT ON SEQUENCES TO otto_app';

    -- The blanket grant above re-granted UPDATE and DELETE on audit_events,
    -- reversing 0008_audit.sql's deliberate narrowing. Re-apply it, exactly as
    -- 0018 did for of_app.
    IF EXISTS (SELECT 1 FROM pg_class WHERE relname = 'audit_events' AND relkind = 'r') THEN
      EXECUTE 'REVOKE UPDATE, DELETE ON audit_events FROM otto_app';
      EXECUTE 'GRANT SELECT, INSERT ON audit_events TO otto_app';
      EXECUTE 'GRANT USAGE, SELECT ON SEQUENCE audit_events_id_seq TO otto_app';
    END IF;
  END IF;
END $$;
