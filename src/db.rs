//! Database connection management and migrations.

use crate::error::{DbError, Result};

use anyhow::Context as _;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Database connections bundle for per-agent databases.
pub struct Db {
    /// SQLite pool for relational data.
    pub sqlite: SqlitePool,

    /// LanceDB connection for vector storage.
    pub lance: lancedb::Connection,

    /// Redb database for key-value config.
    pub redb: Arc<redb::Database>,
}

impl Db {
    /// Connect to all databases and run migrations.
    pub async fn connect(data_dir: &Path) -> Result<Self> {
        // SQLite — per-agent agent.db. If an old spacebot.db exists from
        // before the rename, move it to agent.db.
        let agent_db = data_dir.join("agent.db");
        let legacy_db = data_dir.join("spacebot.db");
        if legacy_db.exists() && !agent_db.exists() {
            std::fs::rename(&legacy_db, &agent_db).with_context(|| {
                format!(
                    "failed to rename legacy per-agent DB {} -> {}",
                    legacy_db.display(),
                    agent_db.display()
                )
            })?;
        }
        let sqlite = connect_sqlite_pool(&agent_db).await?;

        // Run migrations
        sqlx::migrate!("./migrations")
            .run(&sqlite)
            .await
            .with_context(|| "failed to run database migrations")?;

        // LanceDB
        let lance_path = data_dir.join("lancedb");
        std::fs::create_dir_all(&lance_path).with_context(|| {
            format!(
                "failed to create LanceDB directory: {}",
                lance_path.display()
            )
        })?;

        let lance = lancedb::connect(lance_path.to_str().unwrap_or("./lancedb"))
            .execute()
            .await
            .map_err(|e| DbError::LanceConnect(e.to_string()))?;

        // Redb
        let redb_path = data_dir.join("config.redb");
        let redb = redb::Database::create(&redb_path)
            .with_context(|| format!("failed to create redb at: {}", redb_path.display()))?;

        Ok(Self {
            sqlite,
            lance,
            redb: Arc::new(redb),
        })
    }

    /// Close all database connections gracefully.
    pub async fn close(self) {
        self.sqlite.close().await;
        // LanceDB and redb close automatically when dropped
    }
}

/// Connect to the instance-level spacebot database and run its migrations.
///
/// The instance database lives at `{instance_dir}/data/spacebot.db` and holds
/// data shared across all agents: tasks, projects, repos, worktrees. This
/// replaces per-agent task and project tables.
///
/// If an old `tasks.db` exists from before the rename, it is moved to
/// `spacebot.db` first.
pub async fn connect_instance_db(data_dir: &Path) -> Result<SqlitePool> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("failed to create data directory: {}", data_dir.display()))?;

    let db_path = data_dir.join("spacebot.db");
    let legacy_tasks_db = data_dir.join("tasks.db");
    if legacy_tasks_db.exists() && !db_path.exists() {
        std::fs::rename(&legacy_tasks_db, &db_path).with_context(|| {
            format!(
                "failed to rename legacy tasks.db -> spacebot.db at {}",
                data_dir.display()
            )
        })?;
    }
    let pool = connect_sqlite_pool(&db_path).await?;

    sqlx::migrate!("./migrations/global")
        .run(&pool)
        .await
        .with_context(|| "failed to run instance database migrations")?;

    Ok(pool)
}

/// Create a SQLite connection pool with conservative, contention-safe settings.
///
/// Shared by the per-agent database and the instance database. Both are hit by
/// channel turns, up to 5 workers, 5 branches, the cortex tick and the HTTP API
/// at the same time, so the pool has to survive real write contention without
/// trading away memory. The database is small (`agent.db` is ~33 MB) and the
/// real bottleneck in this process is the LLM, not page copies — so every
/// setting below is chosen for *stability first*, not for read microbenchmarks.
///
/// - **WAL** — readers never block the writer and the writer never blocks
///   readers. The mode persists in the file, but it is requested per connection
///   so a fresh database starts in the right mode. Note sqlx only issues the
///   pragma when asked: *changing* into or out of WAL needs an exclusive lock
///   that cannot be waited on with `busy_timeout`, so a brand-new database pays
///   that cost once, on the first connection, and every later connection is a
///   no-op.
/// - **`synchronous = NORMAL`** — the standard WAL pairing: a crash can lose the
///   last transaction but cannot corrupt the database. Without this pragma
///   SQLite stays on `FULL` and fsyncs on every commit.
/// - **`busy_timeout = 5s`** — wait for the writer instead of failing with
///   `SQLITE_BUSY`. This is sqlx's own default; the fork had raised it to 10s,
///   which does not remove contention, it only hides it: a lock that would have
///   failed fast instead stalls a channel turn for that long, and that reads to
///   the user as "the agent is slow".
/// - **`cache_size = -16000`** (16 MiB, negative means KiB) — this is *per
///   connection*, so the pool's worst case is `max_connections * cache_size`.
///   At 10 connections that is 160 MiB. The previous `-64000` (64 MiB) put the
///   same bound at 1.25 GiB, which is not affordable next to LanceDB and the
///   embedding model on a 15 GiB machine.
/// - **`mmap_size = 0`** — memory-mapped I/O is disabled. SQLite cannot catch an
///   I/O error on a mapped page: it raises SIGBUS and the process dies (see
///   <https://sqlite.org/mmap.html>). The docs also note mmap "is mostly a
///   benefit for queries" and does not speed up writes. This database fits
///   entirely in the OS page cache, so mapping buys a negligible read win in
///   exchange for an uncatchable crash mode. Not a good trade for a daemon that
///   is expected to run for weeks.
/// - **`temp_store = memory`** — temp b-trees for `ORDER BY`/`GROUP BY` avoid
///   disk round-trips. The sorts in this schema run over small tables.
/// - **`max_connections = 10`** — SQLite serialises writers, so extra
///   connections only buy concurrent readers. 10 is sqlx's default and covers
///   the channel, the worker set, cortex and the API without an oversized
///   per-connection cache.
async fn connect_sqlite_pool(db_path: &Path) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .pragma("cache_size", "-16000")
        .pragma("mmap_size", "0")
        .pragma("temp_store", "memory")
        .pragma("wal_autocheckpoint", "1000");
    SqlitePoolOptions::new()
        .max_connections(10)
        // Waiting longer than this for a pooled connection means something is
        // wedged. Failing here surfaces it in the logs instead of silently
        // stalling a channel turn.
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(options)
        .await
        .with_context(|| format!("failed to connect to SQLite at {}", db_path.display()))
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tuning is the whole reason `connect_sqlite_pool` exists, so assert the
    /// settings actually reach a real connection instead of trusting the builder
    /// chain. A silently ignored pragma is worse than a missing one: it looks
    /// tuned while behaving like the default.
    ///
    /// Only `journal_mode` is stored in the file; every other value below is
    /// per connection, so this has to be read back on the same connection the
    /// pool handed out.
    #[tokio::test]
    async fn pool_connections_carry_the_tuning() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tuning.db");
        let pool = connect_sqlite_pool(&path).await.expect("connect");

        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .expect("journal_mode");
        let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&pool)
            .await
            .expect("synchronous");
        let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&pool)
            .await
            .expect("busy_timeout");
        let cache_size: i64 = sqlx::query_scalar("PRAGMA cache_size")
            .fetch_one(&pool)
            .await
            .expect("cache_size");
        let mmap_size: i64 = sqlx::query_scalar("PRAGMA mmap_size")
            .fetch_one(&pool)
            .await
            .expect("mmap_size");
        let temp_store: i64 = sqlx::query_scalar("PRAGMA temp_store")
            .fetch_one(&pool)
            .await
            .expect("temp_store");

        assert_eq!(journal_mode, "wal");
        assert_eq!(synchronous, 1, "NORMAL");
        assert_eq!(busy_timeout, 5_000);
        assert_eq!(cache_size, -16_000);
        assert_eq!(mmap_size, 0);
        assert_eq!(temp_store, 2, "MEMORY");

        pool.close().await;
    }

    /// Every connection in the pool inherits the tuning, not just the first one
    /// the pool happens to hand out — a pool where only connection 0 is tuned
    /// would look fine in a single-query smoke test and then misbehave under
    /// real concurrency.
    #[tokio::test]
    async fn every_pooled_connection_is_tuned() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tuning.db");
        let pool = connect_sqlite_pool(&path).await.expect("connect");

        let mut connections = Vec::new();
        for _ in 0..3 {
            connections.push(pool.acquire().await.expect("acquire"));
        }

        for connection in &mut connections {
            let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
                .fetch_one(&mut **connection)
                .await
                .expect("busy_timeout");
            let mmap_size: i64 = sqlx::query_scalar("PRAGMA mmap_size")
                .fetch_one(&mut **connection)
                .await
                .expect("mmap_size");
            assert_eq!(busy_timeout, 5_000);
            assert_eq!(mmap_size, 0);
        }

        drop(connections);
        pool.close().await;
    }
}
