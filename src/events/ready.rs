use crate::{Data, Error, utils::sync};
use poise::serenity_prelude as serenity;

/// Sync DB if the bot is ready.
pub async fn handle(
    ctx: &serenity::Context,
    data_about_bot: &serenity::Ready,
    data: &Data,
) -> Result<(), Error> {
    println!("[ready.rs] {} is ready!", data_about_bot.user.name);

    set_presence(ctx, data).await;
    let _ = sync::run_sync(ctx, data).await;

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
