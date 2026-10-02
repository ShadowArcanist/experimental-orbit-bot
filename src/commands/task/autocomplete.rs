//! Autocomplete for the `/task` commands.
//!
//! Poise runs the global command check (the owner lock) before every
//! autocomplete callback. Discord waits at most 3 seconds for suggestions, so
//! each callback has one overall time limit. On an error or a timeout it
//! returns no suggestions instead of failing.

use std::collections::BTreeSet;
use std::future::Future;
use std::time::Duration;

use poise::serenity_prelude as serenity;
use serenity::{AutocompleteChoice, CommandDataOption, CommandDataOptionValue};

use super::{format, input, resolve};
use crate::bot::Context;
use crate::catalog::Catalog;
use crate::orbit::models::{Condition, Status, StatusCategory, TaskQuery};

/// The most suggestions Discord accepts.
const MAX_CHOICES: usize = 25;
/// Discord's limit for a choice name and value.
const MAX_CHOICE_LEN: usize = 100;
/// Overall time limit for one autocomplete answer.
const TIME_LIMIT: Duration = Duration::from_millis(2500);

/// Projects, shown as `Name (KEY)`. The key is sent.
pub async fn project(ctx: Context<'_>, partial: &str) -> Vec<AutocompleteChoice> {
    answer(async {
        let catalog = ctx.data().catalog().await?;
        let needle = partial.trim().to_lowercase();
        Ok(catalog
            .projects
            .iter()
            .filter(|p| {
                p.name.to_lowercase().contains(&needle) || p.key.to_lowercase().contains(&needle)
            })
            .take(MAX_CHOICES)
            .map(|p| choice(&format!("{} ({})", p.name, p.key), p.key.clone()))
            .collect())
    })
    .await
}

/// Workspace members by display name. The user id is sent.
pub async fn member(ctx: Context<'_>, partial: &str) -> Vec<AutocompleteChoice> {
    answer(async {
        let catalog = ctx.data().catalog().await?;
        let needle = partial.trim().to_lowercase();
        Ok(catalog
            .members
            .iter()
            .filter(|m| m.display_name.to_lowercase().contains(&needle))
            .take(MAX_CHOICES)
            .map(|m| choice(&m.display_name, m.user_id.clone()))
            .collect())
    })
    .await
}

/// Statuses of the project implied by the other options (see
/// [`chosen_project`]). Without a project, each status name once.
pub async fn status(ctx: Context<'_>, partial: &str) -> Vec<AutocompleteChoice> {
    answer(async {
        let catalog = ctx.data().catalog().await?;
        let needle = partial.trim().to_lowercase();
        let usable = |s: &&Status| {
            s.category != StatusCategory::Duplicate && s.name.to_lowercase().contains(&needle)
        };

        if let Some(project_id) = chosen_project(ctx, &catalog).await? {
            return Ok(catalog
                .statuses_of(&project_id)
                .filter(usable)
                .take(MAX_CHOICES)
                .map(|s| choice(&s.name, s.id.clone()))
                .collect());
        }
        // Commands resolve the name in every project. Names too long for a
        // choice value cannot be sent, so they are skipped.
        let names: BTreeSet<&str> = catalog
            .statuses
            .iter()
            .filter(usable)
            .map(|s| s.name.as_str())
            .filter(|name| name.chars().count() <= MAX_CHOICE_LEN)
            .collect();
        Ok(names
            .into_iter()
            .take(MAX_CHOICES)
            .map(|name| choice(name, name.to_owned()))
            .collect())
    })
    .await
}

/// Open tasks (not completed, cancelled, or duplicate).
pub async fn open_task(ctx: Context<'_>, partial: &str) -> Vec<AutocompleteChoice> {
    answer(tasks(ctx, partial, false)).await
}

/// All tasks, open and closed.
pub async fn any_task(ctx: Context<'_>, partial: &str) -> Vec<AutocompleteChoice> {
    answer(tasks(ctx, partial, true)).await
}

/// Tasks whose title or description contains `partial`, shown as
/// `CODE · Title · Status`. The task id is sent.
async fn tasks(
    ctx: Context<'_>,
    partial: &str,
    include_closed: bool,
) -> anyhow::Result<Vec<AutocompleteChoice>> {
    let catalog = ctx.data().catalog().await?;
    let text = partial.trim();
    let conditions = if text.is_empty() {
        Vec::new()
    } else {
        vec![Condition::text(text)]
    };
    let query = TaskQuery::new(conditions, include_closed, MAX_CHOICES as u32);
    let page = ctx
        .data()
        .orbit
        .query_tasks(&catalog.workspace.id, &query)
        .await?;
    Ok(page
        .items
        .iter()
        .map(|t| choice(&format::choice_label(&catalog, t), t.id.clone()))
        .collect())
}

/// The project for status suggestions: the `project` option, else the project
/// of the `task` option, else (for `/task add`) the project `add` would use.
async fn chosen_project(ctx: Context<'_>, catalog: &Catalog) -> anyhow::Result<Option<String>> {
    if let Some(project) = option_value(ctx, "project") {
        return Ok(catalog.find_project(&project).map(|p| p.id.clone()));
    }
    if let Some(task) = option_value(ctx, "task") {
        if !input::is_uuid(&task) {
            return Ok(None);
        }
        let task = ctx.data().orbit.task(&catalog.workspace.id, &task).await?;
        return Ok(Some(task.project_id));
    }
    if ctx.command().name == "add" {
        return Ok(resolve::project(ctx.data(), catalog, None)
            .ok()
            .map(|p| p.id.clone()));
    }
    Ok(None)
}

/// Run one autocomplete answer within the time limit; log and drop errors.
async fn answer(
    work: impl Future<Output = anyhow::Result<Vec<AutocompleteChoice>>>,
) -> Vec<AutocompleteChoice> {
    match tokio::time::timeout(TIME_LIMIT, work).await {
        Ok(Ok(choices)) => choices,
        Ok(Err(error)) => {
            tracing::warn!("autocomplete failed: {error:#}");
            Vec::new()
        }
        Err(_) => {
            tracing::warn!("autocomplete timed out");
            Vec::new()
        }
    }
}

/// A choice whose name fits Discord's limit. `value` must already fit.
fn choice(name: &str, value: String) -> AutocompleteChoice {
    AutocompleteChoice::new(input::truncate(name, MAX_CHOICE_LEN), value)
}

/// The current text of another option of this command (also when it is the
/// focused one), searched through subcommands.
fn option_value(ctx: Context<'_>, name: &str) -> Option<String> {
    let poise::Context::Application(app) = ctx else {
        return None;
    };
    find_option(&app.interaction.data.options, name)
}

fn find_option(options: &[CommandDataOption], name: &str) -> Option<String> {
    options.iter().find_map(|option| match &option.value {
        CommandDataOptionValue::SubCommand(inner)
        | CommandDataOptionValue::SubCommandGroup(inner) => find_option(inner, name),
        CommandDataOptionValue::String(value)
        | CommandDataOptionValue::Autocomplete { value, .. }
            if option.name == name && !value.trim().is_empty() =>
        {
            Some(value.clone())
        }
        _ => None,
    })
}
