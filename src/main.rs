//! Orbit Bot — a personal, user-installable Discord bot for Orbit tasks.
//!
//! `orbit-bot`        runs the bot.
//! `orbit-bot check`  signs in to Orbit and prints what the bot can see, then exits.

mod bot;
mod catalog;
mod commands;
mod config;
mod orbit;
mod ui;

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use poise::serenity_prelude as serenity;
use tracing_subscriber::EnvFilter;

use crate::bot::Data;
use crate::catalog::{Catalog, CatalogCache};
use crate::config::{Config, OrbitConfig};
use crate::orbit::OrbitClient;
use crate::orbit::models::TaskQuery;
use crate::ui::{AppEmoji, Discord};

/// How long the cached projects and statuses stay fresh.
const CATALOG_TTL: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load a local .env file if present.
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    match std::env::args().nth(1).as_deref() {
        None => run().await,
        Some("check") => check().await,
        Some(other) => anyhow::bail!("unknown argument `{other}`. Use no argument, or `check`."),
    }
}

/// Run the Discord bot.
async fn run() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    let orbit = Arc::new(OrbitClient::new(&config.orbit)?);
    let catalog = CatalogCache::new(
        CATALOG_TTL,
        orbit.clone(),
        config.orbit.workspace_id.clone(),
    );

    // A failed Orbit check is logged, not fatal: the bot retries on each command.
    match catalog.get().await {
        Ok(c) => tracing::info!(
            "Orbit workspace `{}`: {} project(s)",
            c.workspace.name,
            c.projects.len()
        ),
        Err(e) => tracing::error!("Orbit is not ready: {e:#}"),
    }

    let data = Arc::new(Data {
        orbit,
        catalog,
        allowed_users: config.allowed_users,
        timezone: config.timezone,
        default_project: config.default_project,
        emojis: OnceLock::new(),
        discord: Discord::new()?,
    });
    tracing::info!("{} allowed Discord user(s)", data.allowed_users.len());
    let components = commands::ComponentHandler { data: data.clone() };
    let emojis = data.clone();

    let options = poise::FrameworkOptions {
        commands: commands::all(),
        command_check: Some(|ctx| Box::pin(bot::command_check(ctx))),
        on_error: |error| Box::pin(bot::on_error(error)),
        ..Default::default()
    };
    let framework = poise::Framework::builder()
        .options(options)
        .setup(move |ctx, ready, framework| {
            Box::pin(async move {
                load_emojis(ctx, ready, &emojis).await;
                // Global registration: user-install commands are global only.
                // The first registration can take up to an hour to show.
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                tracing::info!(
                    "registered {} command(s)",
                    framework.options().commands.len()
                );
                Ok(data)
            })
        })
        .build();

    // Slash commands need no privileged gateway intents.
    let mut client =
        serenity::ClientBuilder::new(config.discord_token, serenity::GatewayIntents::empty())
            .framework(framework)
            .raw_event_handler(components)
            .await?;
    tracing::info!("starting the bot");
    client.start().await?;
    Ok(())
}

/// Read the application emojis (status, priority, and member icons). Without
/// them the cards use Unicode stand-ins.
async fn load_emojis(ctx: &serenity::Context, ready: &serenity::Ready, data: &Data) {
    ctx.http.set_application_id(ready.application.id);
    let emojis = match ctx.http.get_application_emojis().await {
        Ok(emojis) => emojis,
        Err(e) => {
            tracing::warn!("cannot read the application emojis: {e}");
            return;
        }
    };
    let emojis: HashMap<String, AppEmoji> = emojis
        .into_iter()
        .map(|e| {
            let emoji = AppEmoji {
                id: e.id.to_string(),
                name: e.name.clone(),
            };
            (e.name, emoji)
        })
        .collect();
    tracing::info!("{} application emoji(s) found", emojis.len());
    let _ = data.emojis.set(emojis);
}

/// Sign in to Orbit and print what the bot can see.
async fn check() -> anyhow::Result<()> {
    let config = OrbitConfig::from_env()?;
    let orbit = Arc::new(OrbitClient::new(&config)?);
    println!("Orbit:     {}", config.url);
    println!("Signed in: {}", orbit.me().await?);

    let catalog = Catalog::load(&orbit, config.workspace_id.as_deref()).await?;
    println!(
        "Workspace: {} ({})",
        catalog.workspace.name, catalog.workspace.id
    );
    for project in &catalog.projects {
        let statuses: Vec<&str> = catalog
            .statuses_of(&project.id)
            .map(|s| s.name.as_str())
            .collect();
        println!(
            "Project:   {} [{}] — {}",
            project.name,
            project.key,
            statuses.join(", ")
        );
    }

    let query = TaskQuery::new(Vec::new(), false, 100);
    let open = orbit.query_tasks(&catalog.workspace.id, &query).await?;
    let more = if open.next_cursor.is_some() { "+" } else { "" };
    println!("Open tasks: {}{more}", open.items.len());
    println!("OK");
    Ok(())
}
