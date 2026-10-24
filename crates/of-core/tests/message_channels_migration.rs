//! `0008_message_channels.sql` moves messages from org broadcasts and DMs to
//! repo and team channels. Its data step deletes every org's DMs (a DM could
//! carry a `repo_id`, so leaving it would publish it to that repo's channel)
//! and legacy cursors, and nulls `job_id` on channel-less legacy broadcasts
//! (a `NOT VALID` CHECK still binds later UPDATEs, so `delete_job` would fail
//! on them). It must do so for every org under both shapes a migration can run
//! in: a role that bypasses RLS (this deployment's superuser) and a role
//! subject to RLS with no `app.org_id` set (`otto_app`, standing in for the
//! fallback).

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

const MIGRATION: &str = include_str!("../migrations/0008_message_channels.sql");

/// Return the schema to its shape at `0007`. `#[sqlx::test]` has already
/// applied every migration, `0008` included, so this stands in for a database
/// that has not run it yet: the DM column back, the channel constraints and
/// indexes gone, and the cursor keyed by `(org_id, user_id)` again (the `id`
/// sequence goes with its column).
async fn undo_0008(pool: &PgPool) {
    sqlx::raw_sql(
        "ALTER TABLE messages DROP CONSTRAINT messages_channel_ck;
         DROP INDEX messages_repo_channel_idx;
         DROP INDEX messages_team_channel_idx;
         ALTER TABLE messages ADD COLUMN recipient_user_id uuid;
         CREATE INDEX messages_recipient_idx ON messages (org_id, recipient_user_id, id DESC)
           WHERE recipient_user_id IS NOT NULL;
         ALTER TABLE message_cursors
           DROP CONSTRAINT message_cursors_channel_ck,
           DROP CONSTRAINT message_cursors_pkey;
         DROP INDEX message_cursors_repo_idx;
         DROP INDEX message_cursors_team_idx;
         ALTER TABLE message_cursors
           DROP COLUMN id,
           DROP COLUMN repo_id,
           DROP COLUMN team_id,
           ADD PRIMARY KEY (org_id, user_id);",
    )
    .execute(pool)
    .await
    .unwrap();
}

/// The migration's per-org `DO` block, alone. `otto_app` owns nothing, so it
/// can run neither the `CREATE POLICY` nor the `ALTER TABLE`s around it.
fn data_block() -> &'static str {
    let blocks: Vec<&str> = MIGRATION
        .match_indices("DO $$")
        .map(|(start, _)| {
            let rest = &MIGRATION[start..];
            let end = rest
                .find("END $$;")
                .expect("a DO block without its END $$;");
            &rest[..end + "END $$;".len()]
        })
        .filter(|block| block.contains("FOREACH o IN ARRAY"))
        .collect();
    assert_eq!(
        blocks.len(),
        1,
        "expected exactly one per-org DO block in 0008, found {}",
        blocks.len()
    );
    blocks[0]
}

struct Seeded {
    org: Uuid,
    job: &'static str,
}

/// Per org: a repo and a job; three DMs (one in a repo, one without, and a
/// reply to a DM, itself a DM); a repo broadcast, an unscoped broadcast, and an
/// unscoped broadcast naming the job; and two legacy cursors. Written directly
/// on the pool, as data a migration (not application code) would rewrite.
async fn seed(pool: &PgPool) -> Seeded {
    let org = Uuid::new_v4();
    let sender = Uuid::new_v4();
    let recipient = Uuid::new_v4();
    let job = "job-1";

    let repo: Uuid = sqlx::query_scalar(
        "INSERT INTO repos (org_id, slug, name) VALUES ($1, 'api', 'api') RETURNING id",
    )
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO jobs (id, org_id, repo_id, title) VALUES ($1, $2, $3, 'seeded')")
        .bind(job)
        .bind(org)
        .bind(repo)
        .execute(pool)
        .await
        .unwrap();

    let insert = |body: &'static str,
                  to: Option<Uuid>,
                  repo_id: Option<Uuid>,
                  job_id: Option<&'static str>,
                  reply: Option<i64>| {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO messages (org_id, sender_user_id, recipient_user_id, body, repo_id, job_id, in_reply_to) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
        )
        .bind(org)
        .bind(sender)
        .bind(to)
        .bind(body)
        .bind(repo_id)
        .bind(job_id)
        .bind(reply)
        .fetch_one(pool)
    };

    insert("dm-in-repo", Some(recipient), Some(repo), None, None)
        .await
        .unwrap();
    let dm = insert("dm-no-repo", Some(recipient), None, None, None)
        .await
        .unwrap();
    insert("dm-reply", Some(sender), None, None, Some(dm))
        .await
        .unwrap();
    insert("broadcast-repo", None, Some(repo), None, None)
        .await
        .unwrap();
    insert("broadcast-unscoped", None, None, None, None)
        .await
        .unwrap();
    insert("broadcast-job", None, None, Some(job), None)
        .await
        .unwrap();

    for user in [sender, recipient] {
        sqlx::query(
            "INSERT INTO message_cursors (org_id, user_id, last_read_id) VALUES ($1, $2, 1)",
        )
        .bind(org)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
    }

    Seeded { org, job }
}

/// (DMs, broadcasts, cursors, the job-carrying broadcast's job_id).
async fn state(pool: &PgPool, org: Uuid) -> (i64, i64, i64, Option<String>) {
    let count = |sql: &'static str| sqlx::query_scalar::<_, i64>(sql).bind(org).fetch_one(pool);
    (
        count("SELECT count(*) FROM messages WHERE org_id = $1 AND body LIKE 'dm-%'")
            .await
            .unwrap(),
        count("SELECT count(*) FROM messages WHERE org_id = $1 AND body LIKE 'broadcast-%'")
            .await
            .unwrap(),
        count("SELECT count(*) FROM message_cursors WHERE org_id = $1")
            .await
            .unwrap(),
        sqlx::query_scalar(
            "SELECT job_id FROM messages WHERE org_id = $1 AND body = 'broadcast-job'",
        )
        .bind(org)
        .fetch_one(pool)
        .await
        .unwrap(),
    )
}

fn assert_migrated(state: (i64, i64, i64, Option<String>), org: Uuid) {
    assert_eq!(
        state,
        (0, 3, 0, None),
        "org {org}: (DMs, broadcasts, cursors, legacy broadcast job_id)"
    );
}

async fn as_otto_app(tx: &mut Transaction<'_, Postgres>, sql: &str) {
    sqlx::query("SET LOCAL ROLE otto_app")
        .execute(&mut **tx)
        .await
        .unwrap();
    sqlx::raw_sql(sql).execute(&mut **tx).await.unwrap();
    sqlx::query("RESET ROLE").execute(&mut **tx).await.unwrap();
}

#[sqlx::test]
async fn deletes_every_orgs_dms_and_cursors_when_rls_is_bypassed(pool: PgPool) {
    undo_0008(&pool).await;
    let orgs = [seed(&pool).await, seed(&pool).await];

    sqlx::raw_sql(MIGRATION).execute(&pool).await.unwrap();

    for s in &orgs {
        assert_migrated(state(&pool, s.org).await, s.org);
    }

    let column: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.columns \
         WHERE table_name = 'messages' AND column_name = 'recipient_user_id'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(column, 0, "recipient_user_id must be gone");

    let err = sqlx::query(
        "INSERT INTO messages (org_id, sender_user_id, body) VALUES ($1, gen_random_uuid(), 'nowhere')",
    )
    .bind(orgs[0].org)
    .execute(&pool)
    .await
    .expect_err("a message in no channel must be refused");
    assert_eq!(
        err.as_database_error().and_then(|e| e.constraint()),
        Some("messages_channel_ck"),
        "{err}"
    );

    // The job a legacy broadcast once named can be deleted: the FK's SET NULL
    // has nothing left to update, so the NOT VALID CHECK never fires.
    for s in &orgs {
        sqlx::query("DELETE FROM jobs WHERE org_id = $1 AND id = $2")
            .bind(s.org)
            .bind(s.job)
            .execute(&pool)
            .await
            .unwrap();
    }
}

#[sqlx::test]
async fn deletes_every_orgs_dms_and_cursors_when_rls_applies(pool: PgPool) {
    undo_0008(&pool).await;
    let block = data_block();

    // Control, on its own seed: as otto_app with no app.org_id and no
    // temporary policies, the tenant policies hide every row from the loop's
    // org enumeration, so nothing is deleted or nulled. The policies, not
    // luck, are what make the real pass below find the orgs.
    let control = [seed(&pool).await, seed(&pool).await];
    let mut tx = pool.begin().await.unwrap();
    as_otto_app(&mut tx, block).await;
    tx.commit().await.unwrap();
    for s in &control {
        assert_eq!(
            state(&pool, s.org).await,
            (3, 3, 2, Some(s.job.to_owned())),
            "control org {}: the loop must find nothing without the temporary policies",
            s.org
        );
    }

    // The real pass: the temporary policies created as the pool's role (the
    // table owner), the DO block alone as otto_app, then the policies dropped,
    // all in one transaction as the migration's own would be.
    // More than ten orgs: plpgsql fetches a FOR-IN-query loop in batches of
    // ten, so an org list streamed while the loop pins `app.org_id` would skip
    // the later ones. The migration materializes the list first; this proves it.
    let mut orgs = Vec::new();
    for _ in 0..12 {
        orgs.push(seed(&pool).await);
    }
    let mut tx = pool.begin().await.unwrap();
    sqlx::raw_sql(
        "CREATE POLICY messages_channels_0008 ON messages FOR SELECT USING (current_org() IS NULL);
         CREATE POLICY message_cursors_channels_0008 ON message_cursors FOR SELECT USING (current_org() IS NULL);",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    as_otto_app(&mut tx, block).await;
    sqlx::raw_sql(
        "DROP POLICY messages_channels_0008 ON messages;
         DROP POLICY message_cursors_channels_0008 ON message_cursors;",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    for s in control.iter().chain(&orgs) {
        assert_migrated(state(&pool, s.org).await, s.org);
    }
}
