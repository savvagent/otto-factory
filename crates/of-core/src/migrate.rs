//! otto-factory's own migrations: `0001`–`0034` and onward, in
//! `crates/of-core/migrations`.
//!
//! **Never call `otto_tenant::Db::migrate` on this database.** It applies
//! otto-platform's own migration history (`0001_identity` … `0008_…`), whose
//! version numbers collide with this one's and whose checksums differ, so sqlx
//! would refuse to start at best and interleave two histories in one
//! `_sqlx_migrations` table at worst. The platform's schema is already in this
//! database, brought in by this crate's migrations (see `0033`/`0034`), which
//! is the point of keeping a single history here until the Phase 4 split.
//!
//! `tests/migrator.rs` fails the build if a workspace source file calls the
//! platform's `migrate`.

use otto_tenant::Db;

/// The migrator for this crate's `migrations/` directory, for tests that hand
/// it to `#[sqlx::test(migrator = "of_core::MIGRATOR")]`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Apply every otto-factory migration. Safe to run from several replicas at
/// once: sqlx takes a Postgres advisory lock for the duration, so the losers
/// wait rather than racing each other through the same DDL.
pub async fn migrate(db: &Db) -> crate::Result<()> {
    MIGRATOR
        .run(db.pool())
        .await
        .map_err(|e| sqlx::Error::Migrate(Box::new(e)))?;
    Ok(())
}
