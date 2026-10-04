use std::borrow::Cow;

use sqlx::SqliteConnection;
use sqlx::migrate::Migrator;

use crate::{Error, Result};

const COMMENT_ONLY_CHECKSUMS: &[(i64, &str, &str)] = &[
    (
        1,
        "e2e2c2ee3d181e2555ba8e6e20544aa41952d51397e897f75a2a4f62cb8e47d8f152b9fc4a0c83019f3fc1d4a7e87e78",
        "b3e9f2d08d1564ae73d5de54421b1713e848847a29533d1faf3ea851f8f1586f815b1e652e64f6d2aa2ffa790c2f0dc0",
    ),
    (
        2,
        "146254a0e732cfd8b5b39b9c000b90f7aaeee308a54739c68407efe9b83143ee6768e5b43083e8f39ee723c0c1f9a51b",
        "4441d5dd098b2be2f3a3d627f3626cc7b8c756f5dc0fbd7097241e1dd2ce2187c0718c759a1b8c3d91365b9c91017f0c",
    ),
    (
        3,
        "f0daad562d8ba331c8031f23c335126f94dac465c3c0bcd535cc614d3f42a3046ac8142a15f1891a82b1b560df0035be",
        "069917dc7c1a0fcfda58a8071989209ae391e6d934d43182312cc635dbbfa579808d6d0a68a41b9b188f23eb2ba4a564",
    ),
    (
        4,
        "253c31e2d782e04933b2c3714dc63780eda1fe81ea651c5722fa16ffbcf7f334006683d5b59d942947bf9023d5d88595",
        "f064926503bb938007abb1ba878e542f01fb28b736aaaceae48b3ac6daab077acc60d5948b68cdfe2ea6d6632ac08d12",
    ),
];

fn matches_checksum(checksum: &[u8], expected: &str) -> bool {
    expected.len().is_multiple_of(2)
        && checksum.len() == expected.len() / 2
        && checksum
            .iter()
            .zip(expected.as_bytes().chunks_exact(2))
            .all(|(byte, pair)| {
                char::from(pair[0]).to_digit(16) == Some(u32::from(byte >> 4))
                    && char::from(pair[1]).to_digit(16) == Some(u32::from(byte & 15))
            })
}

pub(crate) async fn compatible_migrator(connection: &mut SqliteConnection) -> Result<Migrator> {
    let mut migrator = sqlx::migrate!("./migrations");
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
    ).fetch_one(&mut *connection).await?;
    if !exists {
        return Ok(migrator);
    }
    let applied: Vec<(i64, i64, Vec<u8>)> =
        sqlx::query_as("SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut *connection)
            .await?;
    for (version, success, checksum) in &applied {
        if *success != 1 {
            return Err(Error::MigrationHistory(format!(
                "迁移 {version} 未成功完成"
            )));
        }
        let migration = migrator
            .migrations
            .to_mut()
            .iter_mut()
            .find(|m| m.version == *version)
            .ok_or_else(|| {
                Error::MigrationHistory(format!("存在当前版本不认识的迁移 {version}"))
            })?;
        if migration.checksum.as_ref() == checksum {
            continue;
        }
        let compatible = COMMENT_ONLY_CHECKSUMS
            .iter()
            .any(|(known_version, old, current)| {
                version == known_version
                    && matches_checksum(checksum, old)
                    && matches_checksum(&migration.checksum, current)
            });
        if !compatible {
            return Err(Error::MigrationHistory(format!(
                "迁移 {version} 的校验值不匹配"
            )));
        }
        migration.checksum = Cow::Owned(checksum.clone());
    }
    if let Some((latest, _, _)) = applied.last() {
        for migration in migrator.iter().filter(|m| m.version <= *latest) {
            if !applied
                .iter()
                .any(|(version, _, _)| *version == migration.version)
            {
                return Err(Error::MigrationHistory(format!(
                    "缺少迁移 {} 的记录",
                    migration.version
                )));
            }
        }
    }
    Ok(migrator)
}

pub(crate) async fn protect_legacy_tasks(connection: &mut SqliteConnection) -> Result<()> {
    let tracked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
    ).fetch_one(&mut *connection).await?;
    if tracked {
        let replaced: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version = 14 AND success = 1)",
        )
        .fetch_one(&mut *connection)
        .await?;
        if replaced {
            return Ok(());
        }
    }
    let mut tasks = 0;
    let mut leases = 0;
    for (table, count) in [("tasks", &mut tasks), ("leases", &mut leases)] {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        )
        .bind(table)
        .fetch_one(&mut *connection)
        .await?;
        if exists {
            *count = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&mut *connection)
                .await?;
        }
    }
    if tasks != 0 || leases != 0 {
        return Err(Error::LegacyTaskData { tasks, leases });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use crate::Db;
    use sqlx::sqlite::SqlitePoolOptions;

    const OLD_CHECKSUMS: [(i64, &str); 4] = [
        (
            1,
            "e2e2c2ee3d181e2555ba8e6e20544aa41952d51397e897f75a2a4f62cb8e47d8f152b9fc4a0c83019f3fc1d4a7e87e78",
        ),
        (
            2,
            "146254a0e732cfd8b5b39b9c000b90f7aaeee308a54739c68407efe9b83143ee6768e5b43083e8f39ee723c0c1f9a51b",
        ),
        (
            3,
            "f0daad562d8ba331c8031f23c335126f94dac465c3c0bcd535cc614d3f42a3046ac8142a15f1891a82b1b560df0035be",
        ),
        (
            4,
            "253c31e2d782e04933b2c3714dc63780eda1fe81ea651c5722fa16ffbcf7f334006683d5b59d942947bf9023d5d88595",
        ),
    ];

    fn decode_hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    async fn historical_fixture(last: i64) -> Db {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let mut migrator = sqlx::migrate!("./migrations");
        migrator.migrations = Cow::Owned(
            migrator
                .iter()
                .filter(|m| m.version <= last)
                .cloned()
                .collect(),
        );
        migrator.run(&pool).await.unwrap();
        Db { pool }
    }

    async fn ledger(db: &Db) -> Vec<(i64, i64, Vec<u8>)> {
        sqlx::query_as("SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(db.pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn unknown_checksum_stops_before_pending_migrations_and_preserves_ledger() {
        let db = historical_fixture(24).await;
        sqlx::query("UPDATE _sqlx_migrations SET checksum = X'00' WHERE version = 1")
            .execute(db.pool())
            .await
            .unwrap();
        let before = ledger(&db).await;
        assert!(db.migrate().await.is_err());
        assert!(ledger(&db).await == before, "原迁移记录必须保持不变");
    }

    #[tokio::test]
    async fn known_comment_checksums_remain_unchanged() {
        let db = historical_fixture(24).await;
        for (version, checksum) in OLD_CHECKSUMS {
            sqlx::query("UPDATE _sqlx_migrations SET checksum = ?1 WHERE version = ?2")
                .bind(decode_hex(checksum))
                .bind(version)
                .execute(db.pool())
                .await
                .unwrap();
        }
        let before = ledger(&db).await;
        db.migrate().await.unwrap();
        let after = ledger(&db).await;
        assert_eq!(after.len(), before.len() + 1);
        assert!(after[..before.len()] == before, "原迁移记录必须保持不变");
        assert_eq!(after.last().unwrap().0, 25);
    }

    #[tokio::test]
    async fn dirty_migration_does_not_rewrite_any_checksum() {
        let db = historical_fixture(25).await;
        sqlx::query("UPDATE _sqlx_migrations SET checksum = X'00', success = 0 WHERE version = 1")
            .execute(db.pool())
            .await
            .unwrap();
        let before = ledger(&db).await;
        assert!(db.migrate().await.is_err());
        assert!(ledger(&db).await == before, "原迁移记录必须保持不变");
    }

    #[tokio::test]
    async fn migration_gaps_are_reported_before_schema_changes() {
        let db = historical_fixture(25).await;
        sqlx::query("DELETE FROM _sqlx_migrations WHERE version = 7")
            .execute(db.pool())
            .await
            .unwrap();
        let before = ledger(&db).await;
        let error = db.migrate().await.unwrap_err().to_string();
        assert!(error.contains("缺少") && error.contains('7'), "{error}");
        assert!(ledger(&db).await == before, "原迁移记录必须保持不变");
    }

    #[tokio::test]
    async fn a_known_checksum_is_only_accepted_for_its_original_version() {
        let db = historical_fixture(25).await;
        sqlx::query("UPDATE _sqlx_migrations SET checksum = ?1 WHERE version = 5")
            .bind(decode_hex(OLD_CHECKSUMS[0].1))
            .execute(db.pool())
            .await
            .unwrap();
        let before = ledger(&db).await;
        assert!(db.migrate().await.is_err());
        assert!(ledger(&db).await == before, "原迁移记录必须保持不变");
    }

    #[tokio::test]
    async fn newer_migration_versions_are_rejected_without_writing() {
        let db = historical_fixture(25).await;
        sqlx::query("INSERT INTO _sqlx_migrations(version, description, success, checksum, execution_time) VALUES(26, 'future', 1, X'00', 0)")
            .execute(db.pool()).await.unwrap();
        let before = ledger(&db).await;
        let error = db.migrate().await.unwrap_err().to_string();
        assert!(error.contains("不认识") && error.contains("26"), "{error}");
        assert!(ledger(&db).await == before, "原迁移记录必须保持不变");
    }

    #[tokio::test]
    async fn legacy_tasks_and_leases_stop_upgrade_without_losing_data() {
        let db = historical_fixture(13).await;
        sqlx::query("INSERT INTO nodes(id, name, transport, created_at) VALUES('node', 'hub-host', 'local', '0')")
            .execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO tasks(id, title, body, created_at) VALUES('task', 'Keep this task', 'Original draft', '0')")
            .execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO leases(task_id, node_id, token, acquired_at, expires_at) VALUES('task', 'node', 'fixture', '0', '1')")
            .execute(db.pool()).await.unwrap();
        let before = ledger(&db).await;
        let error = db.migrate().await.unwrap_err().to_string();
        assert!(
            error.contains("旧版任务") && error.contains("备份"),
            "{error}"
        );
        assert!(ledger(&db).await == before);
        let body: String = sqlx::query_scalar("SELECT body FROM tasks WHERE id = 'task'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(body, "Original draft");
        let token: String = sqlx::query_scalar("SELECT token FROM leases WHERE task_id = 'task'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(token, "fixture");
    }

    #[tokio::test]
    async fn orphaned_legacy_leases_also_stop_upgrade() {
        let db = historical_fixture(13).await;
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO leases(task_id, node_id, token, acquired_at, expires_at) VALUES('task', 'node', 'fixture', '0', '1')")
            .execute(db.pool()).await.unwrap();
        let before = ledger(&db).await;
        let error = db.migrate().await.unwrap_err().to_string();
        assert!(error.contains("租约") && error.contains("备份"), "{error}");
        assert!(ledger(&db).await == before);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM leases")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn empty_legacy_tables_can_upgrade() {
        let db = historical_fixture(13).await;
        db.migrate().await.unwrap();
        assert_eq!(
            ledger(&db).await.len(),
            sqlx::migrate!("./migrations").iter().count()
        );
        let number_columns: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name = 'number'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(number_columns, 1);
    }

    #[tokio::test]
    async fn current_tasks_are_not_treated_as_legacy_data() {
        let db = historical_fixture(i64::MAX).await;
        sqlx::query("INSERT INTO tasks(id, number, title, created_at, updated_at) VALUES('task', 1, 'Keep this task', '0', '0')")
            .execute(db.pool()).await.unwrap();
        db.migrate().await.unwrap();
        let title: String = sqlx::query_scalar("SELECT title FROM tasks WHERE id = 'task'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(title, "Keep this task");
    }
}
