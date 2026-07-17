use poise::serenity_prelude as serenity;

pub mod config;
pub mod db;
pub mod events;
pub mod git;
pub mod github;
pub mod search;
pub mod utils;

pub struct Data {
    pub config: config::Config,
    pub db: sqlx::SqlitePool,
} // User data, which is stored and accessible in all command invocations
type Error = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main]
async fn main() {
    // Load config.yaml
    let config = config::Config::load().expect("Failed to load config.yaml");
    let token = config.discord_token.clone();
    let intents =
        serenity::GatewayIntents::non_privileged() | serenity::GatewayIntents::MESSAGE_CONTENT;

    // Load database
    let db = db::setup_db().await;

    // Start GitHub background job
    tokio::spawn(github::start_sync_job(db.clone(), config.clone()));

    // Start Git repository clone/fetch job
    tokio::spawn(git::start_sync_job(db.clone(), config.clone()));

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            // commands: vec![],
            event_handler: |ctx, event, framework, data| {
                Box::pin(events::event_handler(ctx, event, framework, data))
            },
            ..Default::default()
        })
        .setup(|_ctx, _ready, _framework| {
            Box::pin(async move {
                // poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                Ok(Data { config, db })
            })
        })
        .build();

    // Start the bot
    let client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await;
    client.unwrap().start().await.unwrap();
}
