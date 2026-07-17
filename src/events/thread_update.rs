use crate::{Data, Error, utils::sync};
use poise::serenity_prelude::{self as serenity, GuildChannel};

/// If thread is archived, sync it to DB. If it is unarchived, remove it from DB.
pub async fn handle(
    ctx: &serenity::Context,
    _old: &Option<GuildChannel>,
    new: &GuildChannel,
    data: &Data,
) -> Result<(), Error> {
    if let Some(meta) = &new.thread_metadata {
        if meta.archived {
            // Sync because it was archived
            println!(
                "[thread_update.rs] Thread '{}' was archived (added to DB).",
                new.name
            );
            sync::sync_single_thread(ctx, new, &data.db).await;
        } else {
            // It is active (re-activated). Remove from DB.
            let thread_id = new.id.get().to_string();
            let _ = sqlx::query("DELETE FROM threads WHERE id = ?")
                .bind(&thread_id)
                .execute(&data.db)
                .await;
            println!(
                "[thread_update.rs] Thread '{}' was re-activated (removed from DB).",
                new.name
            );
        }
    }

    Ok(())
}
