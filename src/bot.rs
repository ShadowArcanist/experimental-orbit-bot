//! Shared bot state, the owner lock, and the global error handler.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use chrono_tz::Tz;
use poise::serenity_prelude::UserId;

use crate::catalog::{Catalog, CatalogCache};
use crate::orbit::OrbitClient;
use crate::orbit::models::{Priority, Status, StatusCategory};
use crate::ui::{AppEmoji, Discord};

/// The bot error type.
pub type Error = anyhow::Error;

/// A poise command context bound to [`Data`].
pub type Context<'a> = poise::Context<'a, Arc<Data>, Error>;

/// State shared by every command.
pub struct Data {
    pub orbit: Arc<OrbitClient>,
    pub catalog: CatalogCache,
    /// Discord users that may use the bot.
    pub allowed_users: HashSet<UserId>,
    /// Time zone used to read due dates.
    pub timezone: Tz,
    /// Project key used when a command names no project.
    pub default_project: Option<String>,
    /// The application's custom emojis by name (status, priority, and member
    /// icons), loaded once the bot is connected.
    pub emojis: OnceLock<HashMap<String, AppEmoji>>,
    /// Client for Discord's interaction endpoints (Components V2 replies).
    pub discord: Discord,
}

impl Data {
    /// The cached Orbit catalog.
    pub async fn catalog(&self) -> anyhow::Result<Arc<Catalog>> {
        self.catalog.get().await
    }

    /// The status icon for message text: the uploaded Orbit emoji, else a
    /// Unicode stand-in.
    pub fn status_icon(&self, status: Option<&Status>) -> String {
        match status.and_then(|s| self.status_emoji(s)) {
            Some(emoji) => emoji.markdown(),
            None => status
                .map_or(StatusCategory::Unknown, |s| s.category)
                .icon()
                .to_owned(),
        }
    }

    /// The status icon as a partial emoji object (select options).
    pub fn status_icon_json(&self, status: &Status) -> serde_json::Value {
        match self.status_emoji(status) {
            Some(emoji) => emoji.json(),
            None => serde_json::json!({ "name": status.category.icon() }),
        }
    }

    fn status_emoji(&self, status: &Status) -> Option<&AppEmoji> {
        self.emoji(&status_emoji_name(status))
    }

    /// The priority icon: the uploaded `priority_<level>` emoji, else a
    /// Unicode stand-in.
    pub fn priority_icon(&self, priority: Priority) -> String {
        self.emoji(&format!("priority_{}", priority.as_str()))
            .map_or_else(|| priority.icon().to_owned(), AppEmoji::markdown)
    }

    /// The priority icon as a partial emoji object (select options).
    pub fn priority_icon_json(&self, priority: Priority) -> serde_json::Value {
        match self.emoji(&format!("priority_{}", priority.as_str())) {
            Some(emoji) => emoji.json(),
            None => serde_json::json!({ "name": priority.icon() }),
        }
    }

    /// The icon of a member: an emoji named after their display name in lower
    /// case (e.g. `shadowarcanist`), if one is uploaded.
    pub fn member_icon(&self, display_name: &str) -> Option<String> {
        self.member_emoji(display_name).map(AppEmoji::markdown)
    }

    /// The member icon as a partial emoji object (select options).
    pub fn member_icon_json(&self, display_name: &str) -> Option<serde_json::Value> {
        self.member_emoji(display_name).map(AppEmoji::json)
    }

    fn member_emoji(&self, display_name: &str) -> Option<&AppEmoji> {
        let name: String = display_name
            .to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        self.emoji(&name)
    }

    fn emoji(&self, name: &str) -> Option<&AppEmoji> {
        self.emojis.get()?.get(name)
    }

    /// Whether this Discord user may use the bot.
    pub fn is_allowed(&self, user: UserId) -> bool {
        self.allowed_users.contains(&user)
    }

    /// The Orbit web link of a task.
    pub fn task_url(&self, workspace_id: &str, task_id: &str) -> String {
        format!(
            "{}/tasks/{task_id}?workspace={workspace_id}",
            self.orbit.base_url()
        )
    }
}

/// The emoji name for a status: `status_<category>`, e.g. `status_started`.
/// One icon per category, in Orbit's default colours (`icons/`).
pub fn status_emoji_name(status: &Status) -> String {
    format!("status_{}", status.category.as_str())
}

/// Global check that runs before every command and every autocomplete
/// callback (poise runs checks for both).
pub async fn command_check(ctx: Context<'_>) -> Result<bool, Error> {
    Ok(ctx.data().is_allowed(ctx.author().id))
}

/// Report errors to the user (ephemeral) and to the log.
pub async fn on_error(error: poise::FrameworkError<'_, Arc<Data>, Error>) {
    match error {
        poise::FrameworkError::Command { error, ctx, .. } => {
            tracing::warn!(
                "command `{}` failed: {error:#}",
                ctx.command().qualified_name
            );
            reply_error(ctx, &format!("⚠️ {error}")).await;
        }
        poise::FrameworkError::CommandCheckFailed { ctx, .. } => {
            // Autocomplete cannot show a message; it just gets no suggestions.
            if !is_autocomplete(ctx) {
                tracing::warn!(user = %ctx.author().id, "refused a user who is not allowed");
                reply_error(ctx, "🔒 This is a private bot.").await;
            }
        }
        poise::FrameworkError::ArgumentParse { error, ctx, .. } => {
            reply_error(ctx, &format!("⚠️ {error}")).await;
        }
        poise::FrameworkError::Setup { error, .. } => {
            tracing::error!("setup failed: {error:#}");
        }
        other => {
            if let Err(e) = poise::builtins::on_error(other).await {
                tracing::error!("error while handling an error: {e}");
            }
        }
    }
}

fn is_autocomplete(ctx: Context<'_>) -> bool {
    matches!(
        ctx,
        poise::Context::Application(app)
            if app.interaction_type == poise::CommandInteractionType::Autocomplete
    )
}

async fn reply_error(ctx: Context<'_>, message: &str) {
    if let Err(e) = crate::ui::send(ctx, crate::ui::notice(crate::ui::RED, message)).await {
        tracing::error!("could not send the error message: {e:#}");
    }
}
