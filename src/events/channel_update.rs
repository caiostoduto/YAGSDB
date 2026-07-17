use crate::{Data, Error};
use poise::serenity_prelude as serenity;

/// Update applied tags for a thread in the DB when it's updated.
pub async fn handle(
    _ctx: &serenity::Context,
    _old: &Option<serenity::GuildChannel>,
    new: &serenity::GuildChannel,
    data: &Data,
) -> Result<(), Error> {
    if new.kind == serenity::ChannelType::PublicThread {
        let thread_id = new.id.get().to_string();
        let tags_json = serde_json::to_string(&new.applied_tags).unwrap_or_default();
        if let Ok(res) = sqlx::query("UPDATE threads SET applied_tags = ? WHERE id = ?")
            .bind(&tags_json)
            .bind(&thread_id)
            .execute(&data.db)
            .await
            && res.rows_affected() > 0
        {
            println!("[channel_update.rs] Thread '{}' tags was updated", new.name);
        }
    }

    Ok(())
}
