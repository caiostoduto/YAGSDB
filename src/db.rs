use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;

pub async fn setup_db() -> sqlx::SqlitePool {
    let db_options = SqliteConnectOptions::from_str("sqlite://sqlite.db")
        .expect("Invalid connection URL")
        .create_if_missing(true);

    let db = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(db_options)
        .await
        .expect("Failed to connect to the database");

    create_tables(&db).await;
    db
}

/// Create tables if they don't exist
async fn create_tables(db: &sqlx::SqlitePool) {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS threads (
            id TEXT PRIMARY KEY,
            forum_channel_id TEXT NOT NULL,
            guild_id TEXT NOT NULL,
            applied_tags TEXT,
            name TEXT
        );",
    )
    .execute(db)
    .await
    .unwrap();

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            thread_id TEXT NOT NULL,
            content TEXT NOT NULL,
            FOREIGN KEY(thread_id) REFERENCES threads(id) ON DELETE CASCADE
        );",
    )
    .execute(db)
    .await
    .unwrap();

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS gh_issues (
            id TEXT PRIMARY KEY,
            repo TEXT NOT NULL,
            number INTEGER NOT NULL,
            title TEXT NOT NULL,
            body TEXT,
            state TEXT,
            is_pr INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT
        );",
    )
    .execute(db)
    .await
    .unwrap();

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS gh_comments (
            id TEXT PRIMARY KEY,
            issue_id TEXT NOT NULL,
            body TEXT,
            updated_at TEXT,
            FOREIGN KEY(issue_id) REFERENCES gh_issues(id) ON DELETE CASCADE
        );",
    )
    .execute(db)
    .await
    .unwrap();

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS gh_releases (
            id TEXT PRIMARY KEY,
            repo TEXT NOT NULL,
            tag_name TEXT NOT NULL,
            name TEXT,
            body TEXT,
            published_at TEXT
        );",
    )
    .execute(db)
    .await
    .unwrap();

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS docs (
            id TEXT PRIMARY KEY,
            repo TEXT NOT NULL,
            file_path TEXT NOT NULL,
            url TEXT,
            title TEXT,
            content TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );",
    )
    .execute(db)
    .await
    .unwrap();

    // Migration tracking table
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _migrations (
            version INTEGER PRIMARY KEY,
            description TEXT NOT NULL,
            applied_at TEXT NOT NULL
        );",
    )
    .execute(db)
    .await
    .unwrap();
}
