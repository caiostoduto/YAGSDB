use poise::serenity_prelude as serenity;

pub mod config;
pub mod db;
pub mod events;
pub mod utils;
pub mod git;
pub mod github;
pub mod search;

pub struct Data {
    pub config: config::Config,
    pub db: sqlx::SqlitePool,
} // User data, which is stored and accessible in all command invocations
type Error = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main]
async fn main() {
    // Check if config.yaml exists, if not copy from config.example.yaml
    if !std::path::Path::new("config.yaml").exists() {
        if std::path::Path::new("config.example.yaml").exists() {
            std::fs::copy("config.example.yaml", "config.yaml")
                .expect("Failed to copy config.example.yaml to config.yaml");
            eprintln!(
                "Warning: config.yaml was not found. A new one has been created from config.example.yaml."
            );
            eprintln!("Please fill in the required fields and run the bot again.");
            std::process::exit(1);
        } else {
            eprintln!("Error: config.yaml not found and config.example.yaml is missing.");
            std::process::exit(1);
        }
    }

    // Load config.yaml
    let config = config::Config::load("config.yaml").expect("Failed to load config.yaml");
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
