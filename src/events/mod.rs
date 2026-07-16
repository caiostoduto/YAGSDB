use crate::{Data, Error};
use poise::serenity_prelude as serenity;

pub mod message;
pub mod sync;

pub async fn event_handler(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    _framework: poise::FrameworkContext<'_, Data, Error>,
    data: &Data,
) -> Result<(), Error> {
    match event {
        // Sync DB if the bot is ready.
        serenity::FullEvent::Ready { .. } => {
            let _ = sync::run_sync(ctx, data).await;
            set_presence(ctx, data).await;
        }
        // Handle message creation in forum threads.
        serenity::FullEvent::Message { new_message } => {
            message::handle_message(ctx, new_message, data).await?;
        }
        // Update applied tags for a thread in the DB when it's updated.
        serenity::FullEvent::ChannelUpdate { old: _, new } => {
            if new.kind == serenity::ChannelType::PublicThread {
                let thread_id = new.id.get().to_string();
                let tags_json = serde_json::to_string(&new.applied_tags).unwrap_or_default();
                let _ = sqlx::query("UPDATE threads SET applied_tags = ? WHERE id = ?")
                    .bind(&tags_json)
                    .bind(&thread_id)
                    .execute(&data.db)
                    .await;
            }
        }
        // If thread is archived, sync it to DB. If it is unarchived, remove it from DB.
        serenity::FullEvent::ThreadUpdate { old: _, new } => {
            if let Some(meta) = &new.thread_metadata {
                if meta.archived {
                    // Sync because it was archived
                    println!("Thread {} was archived. Syncing to DB.", new.id);
                    sync::sync_single_thread(ctx, new, &data.db).await;
                } else {
                    // It is active (re-activated). Remove from DB.
                    let thread_id = new.id.get().to_string();
                    let _ = sqlx::query("DELETE FROM threads WHERE id = ?")
                        .bind(&thread_id)
                        .execute(&data.db)
                        .await;
                    println!(
                        "[discord_forum] Thread {} was re-activated. Removed from DB.",
                        thread_id
                    );
                }
            }
        }
        // Remove thread from DB if it is deleted.
        serenity::FullEvent::ThreadDelete { thread, .. } => {
            let thread_id = thread.id.get().to_string();
            let _ = sqlx::query("DELETE FROM threads WHERE id = ?")
                .bind(&thread_id)
                .execute(&data.db)
                .await;
            println!(
                "[discord_forum] Thread {} deleted, removed from database.",
                thread_id
            );
        }
        _ => {}
    }
    Ok(())
}

/// Apply the bot_presence from config (activity + online status) after Ready.
async fn set_presence(ctx: &serenity::Context, data: &Data) {
    use crate::config::{ActivityKind, BotStatus};

    let presence = &data.config.bot_presence;

    let activity = match &presence.activity {
        ActivityKind::Playing => serenity::ActivityData::playing(&presence.message),
        ActivityKind::Watching => serenity::ActivityData::watching(&presence.message),
        ActivityKind::Listening => serenity::ActivityData::listening(&presence.message),
        ActivityKind::Competing => serenity::ActivityData::competing(&presence.message),
    };

    let status = match &presence.status {
        BotStatus::Online => serenity::OnlineStatus::Online,
        BotStatus::Dnd => serenity::OnlineStatus::DoNotDisturb,
        BotStatus::Idle => serenity::OnlineStatus::Idle,
        BotStatus::Invisible => serenity::OnlineStatus::Invisible,
    };

    ctx.set_presence(Some(activity), status);
}
