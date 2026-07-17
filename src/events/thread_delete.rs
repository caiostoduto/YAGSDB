use crate::{Data, Error};
use poise::serenity_prelude::{self as serenity, GuildChannel};

/// Remove thread from DB if it is deleted.
pub async fn handle(
    _ctx: &serenity::Context,
    thread: &serenity::PartialGuildChannel,
    _full_thread_data: &Option<GuildChannel>,
    data: &Data,
) -> Result<(), Error> {
    let thread_id = thread.id.get().to_string();
    let _ = sqlx::query("DELETE FROM threads WHERE id = ?")
        .bind(&thread_id)
        .execute(&data.db)
        .await;
    println!(
        "[thread_delete.rs] Thread (id={}) was deleted (removed from DB).",
        thread.id
    );

    Ok(())
}
