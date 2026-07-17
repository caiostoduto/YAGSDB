use crate::{Data, Error};
use poise::serenity_prelude as serenity;

pub mod channel_update;
pub mod message;
pub mod ready;
pub mod thread_delete;
pub mod thread_update;

pub async fn event_handler(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    _framework: poise::FrameworkContext<'_, Data, Error>,
    data: &Data,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::Ready { data_about_bot } => {
            ready::handle(ctx, data_about_bot, data).await?;
        }
        serenity::FullEvent::Message { new_message } => {
            message::handle(ctx, new_message, data).await?;
        }
        serenity::FullEvent::ChannelUpdate { old, new } => {
            channel_update::handle(ctx, old, new, data).await?;
        }
        serenity::FullEvent::ThreadUpdate { old, new } => {
            thread_update::handle(ctx, old, new, data).await?;
        }
        serenity::FullEvent::ThreadDelete {
            thread,
            full_thread_data,
        } => {
            thread_delete::handle(ctx, thread, full_thread_data, data).await?;
        }
        _ => {}
    }
    Ok(())
}
