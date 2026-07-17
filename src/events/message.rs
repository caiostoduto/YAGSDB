use crate::{Data, Error, search};
use poise::serenity_prelude as serenity;

use regex::Regex;

/// Handle message creation in forum threads.
pub async fn handle(
    ctx: &serenity::Context,
    new_message: &serenity::Message,
    data: &Data,
) -> Result<(), Error> {
    // Don't store messages from bots
    if new_message.author.bot {
        return Ok(());
    }

    // Must be a guild channel
    let Ok(serenity::Channel::Guild(guild_channel)) = new_message.channel_id.to_channel(ctx).await
    else {
        return Ok(());
    };

    // Must be a thread
    if guild_channel.kind != serenity::ChannelType::PublicThread {
        return Ok(());
    }

    // Must have a parent channel
    let Some(parent_id) = guild_channel.parent_id else {
        return Ok(());
    };

    let forums = &data.config.data_repositories.discord_forums;

    for forum in forums {
        // Parse the guild id and channel id from the forum config
        let (Ok(guild_id), Ok(channel_id)) = (
            forum.guild_id.parse::<u64>(),
            forum.channel_id.parse::<u64>(),
        ) else {
            continue;
        };

        // In Discord forum channels, the first message of a new thread has the
        // same ID as the thread itself. Detect that to trigger a suggestion reply.
        let is_new_thread = guild_channel.guild_id.get() == guild_id
            && parent_id.get() == channel_id
            && new_message.id.get() == guild_channel.id.get();

        if is_new_thread && forum.reply {
            let query = format!("{}\n{}", guild_channel.name, new_message.content);
            let results = search::find_similar(
                &query,
                guild_id,
                &data.db,
                data.config.threshold,
                data.config.max_results,
                &data.config.search_weights,
                &data.config.data_repositories,
            )
            .await;

            println!("[message] {} created a new thread entitled '{}' and had {} results on search.",
                new_message.author.name, guild_channel.name, results.len());

            if !results.is_empty() {
                let header = &data.config.suggestion_header;
                post_suggestions(ctx, new_message.channel_id, &results, header).await;
            }

            break;
        }
    }

    Ok(())
}

// ── Message builder ──────────────────────────────────────────────────────────

/// Send a single plain-text message listing all results.
///
/// Format per line:
///   `[Tag] **Title** — by Author (XX%)`  with a hyperlink on the title when a URL is available
async fn post_suggestions(
    ctx: &serenity::Context,
    channel_id: serenity::ChannelId,
    results: &[search::SearchResult],
    header: &str,
) {
    let mut lines: Vec<String> = Vec::with_capacity(results.len() + 1);
    lines.push(header.to_string());

    for result in results {
        lines.push(format_result_line(result).await);
    }

    let content = lines.join("\n");

    let msg = serenity::CreateMessage::new()
        .content(content)
        .flags(serenity::model::channel::MessageFlags::SUPPRESS_EMBEDS);
    if let Err(e) = channel_id.send_message(ctx, msg).await {
        eprintln!("[search] Failed to post suggestions: {}", e);
    }
}

/// Format a single search result as one line of text.
async fn format_result_line(result: &search::SearchResult) -> String {
    let tag = match result.kind {
        search::ResultKind::Doc           => "`[Docs]`",
        search::ResultKind::GhIssue       => "`[GitHub Issue]`",
        search::ResultKind::DiscordThread => "`[Discord]`",
    };

    let score_pct = (result.score * 100.0).clamp(0.0, 100.0).round() as u32;

    // Title — hyperlinked when a URL is available
    let title_part = match &result.url {
        Some(url) => format!("[**{}**]({})", demoji(result.title.clone()).trim(), url),
        None => format!("**{}**", demoji(result.title.clone()).trim()),
    };

    // Build line: `(XX%) [Tag] title`
    format!("**({}%)** {} {}", score_pct, tag, title_part)
}

/// Remove emojis from a string.
fn demoji(string: String) -> String {
    let regex = Regex::new(concat!(
        "[",
        "\u{01F600}-\u{01F64F}", // emoticons
        "\u{01F300}-\u{01F5FF}", // symbols & pictographs
        "\u{01F680}-\u{01F6FF}", // transport & map symbols
        "\u{01F1E0}-\u{01F1FF}", // flags (iOS)
        "\u{002702}-\u{0027B0}",
        "\u{0024C2}-\u{01F251}",
        "]+",
    ))
    .unwrap();

    regex.replace_all(&string, "").to_string()
}
