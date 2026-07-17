use crate::{Data, Error};
use poise::serenity_prelude as serenity;

// ─────────────────────────────────────────────────────────────────────────────
// Low-level helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Inserts a thread record into the DB and returns `true` if it was newly added.
async fn insert_thread(thread: &serenity::GuildChannel, db: &sqlx::SqlitePool) -> bool {
    // Get IDs as strings
    let thread_id = thread.id.get().to_string();
    let parent_id = thread.parent_id.unwrap().get().to_string();
    let guild_id = thread.guild_id.get().to_string();

    // Get tags as JSON string
    let tags_json = serde_json::to_string(&thread.applied_tags).unwrap_or_default();

    // Insert into DB if not already present
    sqlx::query(
        "INSERT OR IGNORE INTO threads (id, forum_channel_id, guild_id, applied_tags, name) VALUES (?, ?, ?, ?, ?)",
    )
        .bind(&thread_id)
        .bind(&parent_id)
        .bind(&guild_id)
        .bind(&tags_json)
        .bind(&thread.name)
        .execute(db)
        .await
        .map(|res| res.rows_affected() > 0)
        .unwrap_or(false)
}

/// Returns the highest message ID already stored for a thread, if any.
async fn newest_stored_message_id(
    thread_id: &str,
    db: &sqlx::SqlitePool,
) -> Option<serenity::MessageId> {
    let max_id: Option<String> =
        sqlx::query_scalar("SELECT MAX(CAST(id AS INTEGER)) FROM messages WHERE thread_id = ?")
            .bind(thread_id)
            .fetch_one(db)
            .await
            .ok()
            .flatten();

    max_id
        .and_then(|s| s.parse::<u64>().ok())
        .map(serenity::MessageId::new)
}

/// Returns the lowest message ID already stored for a thread, if any.
/// Used to resume an interrupted before-cursor (oldest-first) import.
async fn oldest_stored_message_id(
    thread_id: &str,
    db: &sqlx::SqlitePool,
) -> Option<serenity::MessageId> {
    let min_id: Option<String> =
        sqlx::query_scalar("SELECT MIN(CAST(id AS INTEGER)) FROM messages WHERE thread_id = ?")
            .bind(thread_id)
            .fetch_one(db)
            .await
            .ok()
            .flatten();

    min_id
        .and_then(|s| s.parse::<u64>().ok())
        .map(serenity::MessageId::new)
}

/// Fetches messages from a thread and persists non-bot messages in the DB.
///
/// Three cases:
///   1. No messages stored yet            → full history import (before-cursor).
///   2. Messages stored but import may be incomplete → resume before-cursor
///      from the oldest stored ID so we keep going backwards.
///   3. History fully imported (oldest stored message is the thread's very
///      first message) → incremental update via after-cursor.
async fn index_thread_messages(
    ctx: &serenity::Context,
    thread: &serenity::GuildChannel,
    db: &sqlx::SqlitePool,
) {
    let thread_id = thread.id.get().to_string();

    let oldest = oldest_stored_message_id(&thread_id, db).await;
    let newest = newest_stored_message_id(&thread_id, db).await;

    match (oldest, newest) {
        // Case 1: nothing stored yet — start a fresh full import.
        (None, _) => fetch_all_messages(ctx, thread, &thread_id, None, db).await,

        // Case 2 & 3: we have some messages already.
        (Some(oldest_id), Some(newest_id)) => {
            // Check whether there are messages older than our oldest stored one.
            // We do this by asking Discord for one message before `oldest_id`.
            let probe = serenity::GetMessages::new().limit(1).before(oldest_id);
            let history_complete = match thread.id.messages(ctx, probe).await {
                Ok(msgs) => msgs.is_empty(), // nothing before → fully imported
                Err(_) => true,              // on error, skip backwards pass
            };

            if history_complete {
                // Case 3: all history is present, do incremental update.
                fetch_messages_after(ctx, thread, &thread_id, newest_id, db).await;
            } else {
                // Case 2: import was interrupted — resume backwards from oldest.
                fetch_all_messages(ctx, thread, &thread_id, Some(oldest_id), db).await;
                // After backfilling, also pull any new messages since newest.
                fetch_messages_after(ctx, thread, &thread_id, newest_id, db).await;
            }
        }

        // Impossible (oldest=Some implies newest=Some), but handle gracefully.
        (Some(_), None) => {}
    }
}

/// Fetches messages in a thread from a given point backwards to the beginning.
/// If `start_before` is `None` the fetch starts from the very latest message.
async fn fetch_all_messages(
    ctx: &serenity::Context,
    thread: &serenity::GuildChannel,
    thread_id: &str,
    start_before: Option<serenity::MessageId>,
    db: &sqlx::SqlitePool,
) {
    let mut before_cursor: Option<serenity::MessageId> = start_before;

    loop {
        let mut builder = serenity::GetMessages::new().limit(100);
        if let Some(id) = before_cursor {
            builder = builder.before(id);
        }

        match thread.id.messages(ctx, builder).await {
            Ok(messages) if messages.is_empty() => break,
            Ok(messages) => {
                before_cursor = messages.last().map(|m| m.id);
                persist_messages(thread_id, messages, db).await;
            }
            Err(_) => break,
        }
    }
}

/// Fetches only messages newer than `after` (incremental update).
async fn fetch_messages_after(
    ctx: &serenity::Context,
    thread: &serenity::GuildChannel,
    thread_id: &str,
    after: serenity::MessageId,
    db: &sqlx::SqlitePool,
) {
    // `after` paginates forward (oldest-first), so we page until empty.
    let mut after_cursor = after;

    loop {
        let builder = serenity::GetMessages::new().limit(100).after(after_cursor);

        match thread.id.messages(ctx, builder).await {
            Ok(messages) if messages.is_empty() => break,
            Ok(messages) => {
                // The last message in an `after` response is the newest.
                after_cursor = messages.last().map(|m| m.id).unwrap_or(after_cursor);
                persist_messages(thread_id, messages, db).await;
            }
            Err(_) => break,
        }
    }
}

/// Inserts a batch of non-bot messages into the DB.
async fn persist_messages(
    thread_id: &str,
    messages: Vec<serenity::Message>,
    db: &sqlx::SqlitePool,
) {
    // Iterate over messages
    for msg in messages {
        // Skip if message is from a bot
        if msg.author.bot {
            continue;
        }
        let msg_id = msg.id.get().to_string();

        // Insert into DB if not already present
        let _ =
            sqlx::query("INSERT OR IGNORE INTO messages (id, thread_id, content) VALUES (?, ?, ?)")
                .bind(&msg_id)
                .bind(thread_id)
                .bind(&msg.content)
                .execute(db)
                .await;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Forum-level helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Returns all archived public threads for a forum channel.
async fn collect_archived_threads(
    ctx: &serenity::Context,
    channel_id: serenity::ChannelId,
) -> Vec<serenity::GuildChannel> {
    channel_id
        .get_archived_public_threads(ctx, None, None)
        .await
        .map(|res| res.threads)
        .unwrap_or_default()
}

/// Removes active threads belonging to `channel_id` from the DB and returns
/// the number of rows deleted.
async fn delete_active_threads_from_db(
    ctx: &serenity::Context,
    guild_id: serenity::GuildId,
    channel_id: serenity::ChannelId,
    db: &sqlx::SqlitePool,
) -> u64 {
    let Ok(active) = guild_id.get_active_threads(ctx).await else {
        return 0;
    };

    let mut removed = 0u64;
    for thread in active.threads {
        // Check if thread belongs to the forum channel
        if thread.parent_id != Some(channel_id) {
            continue;
        }

        // Convert to string
        let tid = thread.id.get().to_string();
        // Delete from DB
        if let Ok(res) = sqlx::query("DELETE FROM threads WHERE id = ?")
            .bind(&tid)
            .execute(db)
            .await
        {
            removed += res.rows_affected();
        }
    }
    removed
}

/// Checks every thread in the DB against the Discord API and removes any that
/// no longer exist (404). Returns the number of rows deleted.
async fn prune_deleted_threads(ctx: &serenity::Context, db: &sqlx::SqlitePool) -> u64 {
    use sqlx::Row;

    let Ok(records) = sqlx::query("SELECT id FROM threads").fetch_all(db).await else {
        return 0;
    };

    let mut removed = 0u64;
    for record in records {
        // Get ID as string
        let db_id: String = record.get("id");
        if let Ok(tid) = db_id.parse::<u64>() {
            // Convert to ChannelId
            let thread_id = serenity::ChannelId::new(tid);
            // Check if thread is deleted
            if is_thread_deleted(ctx, thread_id).await {
                // Delete from DB
                if let Ok(res) = sqlx::query("DELETE FROM threads WHERE id = ?")
                    .bind(&db_id)
                    .execute(db)
                    .await
                {
                    removed += res.rows_affected();
                }
            }
        }
    }
    removed
}

/// Returns `true` when the Discord API responds with 404 for a channel lookup.
async fn is_thread_deleted(ctx: &serenity::Context, thread_id: serenity::ChannelId) -> bool {
    match thread_id.to_channel(ctx).await {
        // If the thread is not found, it's deleted
        Err(serenity::Error::Http(http_err)) => {
            if let serenity::all::HttpError::UnsuccessfulRequest(resp) = &http_err {
                return resp.status_code == serenity::all::StatusCode::NOT_FOUND;
            }
            false
        }
        _ => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Indexes a single thread (insert record + all messages) and returns `true`
/// if the thread was newly added to the DB.
pub async fn sync_single_thread(
    ctx: &serenity::Context,
    thread: &serenity::GuildChannel,
    db: &sqlx::SqlitePool,
) -> bool {
    // Insert thread into DB
    let added = insert_thread(thread, db).await;
    // Index messages in thread
    index_thread_messages(ctx, thread, db).await;
    // Return if thread was newly added
    added
}

/// Main sync entry point: collects configured forum threads, removes stale DB
/// records, indexes archived threads, and logs a summary.
pub async fn run_sync(ctx: &serenity::Context, data: &Data) -> Result<(), Error> {
    let mut archived_threads = Vec::new();
    let mut threads_removed = 0u64;
    let mut threads_added = 0u64;

    // Iterate over configured forums
    let forums = &data.config.data_repositories.discord_forums;
    for forum in forums {
        // Parse guild and channel IDs
        let (Ok(guild_raw), Ok(channel_raw)) = (
            forum.guild_id.parse::<u64>(),
            forum.channel_id.parse::<u64>(),
        ) else {
            continue;
        };

        // Convert to ChannelIds
        let guild_id = serenity::GuildId::new(guild_raw);
        let channel_id = serenity::ChannelId::new(channel_raw);

        // Collect archived threads
        archived_threads.extend(collect_archived_threads(ctx, channel_id).await);
        // Remove active threads
        threads_removed += delete_active_threads_from_db(ctx, guild_id, channel_id, &data.db).await;
    }

    // Remove threads that no longer exist
    threads_removed += prune_deleted_threads(ctx, &data.db).await;

    // Index archived threads
    for thread in archived_threads {
        if sync_single_thread(ctx, &thread, &data.db).await {
            threads_added += 1;
        }
    }

    let threads_now: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM threads")
        .fetch_one(&data.db)
        .await
        .unwrap_or(0);

    // Log summary
    println!(
        "[discord_forum] Sync complete — {} threads (+{} added, -{} removed)",
        threads_now, threads_added, threads_removed
    );
    Ok(())
}
