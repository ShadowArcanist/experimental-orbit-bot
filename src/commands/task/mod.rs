//! The `/task` command group: Orbit task management.
//!
//! The parent command is user-install only, so the bot follows its owner into
//! servers, DMs, and group DMs. Every reply is ephemeral (only the user sees it).

mod add;
mod autocomplete;
mod card;
mod format;
mod input;
mod list;
mod resolve;
mod select;

pub use select::ComponentHandler;

use std::sync::Arc;

use anyhow::bail;

use crate::bot::{Context, Data, Error};
use crate::catalog::Catalog;
use crate::orbit::models::{Task, TaskPatch};
use crate::ui;

/// Manage your Orbit tasks.
#[poise::command(
    slash_command,
    subcommands("add", "list", "this_week", "view", "status", "done", "delete"),
    install_context = "User",
    interaction_context = "Guild|BotDm|PrivateChannel"
)]
pub async fn task(_ctx: Context<'_>) -> Result<(), Error> {
    // Discord always runs a subcommand.
    Ok(())
}

/// Create a task (opens a form).
#[poise::command(slash_command)]
async fn add(ctx: Context<'_>) -> Result<(), Error> {
    let data = ctx.data();
    // A modal must be the first answer, within 3 seconds; the catalog is cached.
    let catalog = data.catalog().await?;
    let interaction = ui::command_interaction(ctx)?;
    data.discord
        .modal(&interaction, add::modal(data, &catalog))
        .await?;
    if let poise::Context::Application(app) = ctx {
        app.has_sent_initial_response
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    Ok(())
}

/// List tasks, most urgent first.
#[poise::command(slash_command)]
async fn list(
    ctx: Context<'_>,
    #[description = "Project (default: all projects)"]
    #[autocomplete = "autocomplete::project"]
    project: Option<String>,
    #[description = "Only this status"]
    #[autocomplete = "autocomplete::status"]
    status: Option<String>,
    #[description = "Also show completed and cancelled tasks (default: no)"] closed: Option<bool>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let data = ctx.data();
    let catalog = data.catalog().await?;
    let mut filter = list_filter(data, &catalog, project, status, None)?;
    filter.closed = closed.unwrap_or(false);
    ui::send(ctx, list::page(data, &catalog, &filter, 0).await?).await
}

/// Tasks due this week (Monday to Sunday), most urgent first.
#[poise::command(slash_command, rename = "this-week")]
async fn this_week(
    ctx: Context<'_>,
    #[description = "Only tasks assigned to this member"]
    #[autocomplete = "autocomplete::member"]
    assignee: Option<String>,
    #[description = "Only this status"]
    #[autocomplete = "autocomplete::status"]
    status: Option<String>,
    #[description = "Project (default: all projects)"]
    #[autocomplete = "autocomplete::project"]
    project: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let data = ctx.data();
    let catalog = data.catalog().await?;
    let mut filter = list_filter(data, &catalog, project, status, assignee)?;
    filter.week = true;
    ui::send(ctx, list::page(data, &catalog, &filter, 0).await?).await
}

/// The list filter for the project, status, and assignee options.
fn list_filter(
    data: &Data,
    catalog: &Catalog,
    project: Option<String>,
    status: Option<String>,
    assignee: Option<String>,
) -> anyhow::Result<list::ListFilter> {
    let project = project
        .as_deref()
        .map(|p| resolve::project(data, catalog, Some(p)))
        .transpose()?;
    let status_id = match (&status, project) {
        (None, _) => None,
        (Some(status), Some(project)) => {
            Some(resolve::status(catalog, project, status)?.id.clone())
        }
        (Some(status), None) => {
            let wanted = status.trim();
            match catalog
                .statuses
                .iter()
                .find(|s| s.id == wanted || s.name.eq_ignore_ascii_case(wanted))
            {
                Some(status) => Some(status.id.clone()),
                None => bail!("no status `{wanted}` in any project"),
            }
        }
    };
    let assignee_id = assignee
        .as_deref()
        .map(|a| resolve::member(catalog, a).map(|m| m.user_id.clone()))
        .transpose()?;
    Ok(list::ListFilter {
        project_id: project.map(|p| p.id.clone()),
        status_id,
        assignee_id,
        ..Default::default()
    })
}

/// Show one task.
#[poise::command(slash_command)]
async fn view(
    ctx: Context<'_>,
    #[description = "Task (search by title, or type a code such as PROD-4A5B)"]
    #[autocomplete = "autocomplete::any_task"]
    task: String,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let (catalog, task) = load_task(ctx, &task).await?;
    reply(ctx, &catalog, &task, "").await
}

/// Change the status of a task.
#[poise::command(slash_command)]
async fn status(
    ctx: Context<'_>,
    #[description = "Task (search by title, or type a code such as PROD-4A5B)"]
    #[autocomplete = "autocomplete::any_task"]
    task: String,
    #[description = "New status"]
    #[autocomplete = "autocomplete::status"]
    status: String,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let (catalog, task) = load_task(ctx, &task).await?;
    let Some(project) = catalog.project(&task.project_id) else {
        bail!("the task's project is not in Orbit any more");
    };
    let status = resolve::status(&catalog, project, &status)?;
    if status.id == task.status_id {
        return reply(
            ctx,
            &catalog,
            &task,
            &format!("ℹ️ Already in **{}**", status.name),
        )
        .await;
    }
    let message = format!("🔀 Moved to **{}**", status.name);
    let task = set_status(ctx.data(), &catalog, &task, &status.id).await?;
    reply(ctx, &catalog, &task, &message).await
}

/// Mark a task as done.
#[poise::command(slash_command)]
async fn done(
    ctx: Context<'_>,
    #[description = "Task (search by title, or type a code such as PROD-4A5B)"]
    #[autocomplete = "autocomplete::open_task"]
    task: String,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let (catalog, task) = load_task(ctx, &task).await?;
    let Some(done) = catalog.done_status(&task.project_id) else {
        bail!("the task's project has no completed status");
    };
    if catalog
        .status(&task.status_id)
        .is_some_and(|s| s.category == done.category)
    {
        return reply(ctx, &catalog, &task, "ℹ️ **Already done**").await;
    }
    let message = format!("✅ **Done!** Moved to {}", done.name);
    let task = set_status(ctx.data(), &catalog, &task, &done.id).await?;
    reply(ctx, &catalog, &task, &message).await
}

/// Delete a task. Orbit keeps it in the trash for 30 days.
#[poise::command(slash_command)]
async fn delete(
    ctx: Context<'_>,
    #[description = "Task (search by title, or type a code such as PROD-4A5B)"]
    #[autocomplete = "autocomplete::any_task"]
    task: String,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let (catalog, task) = load_task(ctx, &task).await?;
    let data = ctx.data();
    data.orbit
        .delete_task(&catalog.workspace.id, &task.id, task.version)
        .await?;
    ui::send(ctx, format::deleted_card(data, &catalog, &task)).await
}

/// Find the task the user chose, with a catalog that knows its project.
async fn load_task(ctx: Context<'_>, input: &str) -> anyhow::Result<(Arc<Catalog>, Task)> {
    let data = ctx.data();
    let catalog = data.catalog().await?;
    let task = resolve::task(data, &catalog, input).await?;
    let catalog = resolve::catalog_with(data, &task.project_id).await?;
    Ok((catalog, task))
}

async fn set_status(
    data: &Data,
    catalog: &Catalog,
    task: &Task,
    status_id: &str,
) -> anyhow::Result<Task> {
    let patch = TaskPatch {
        expected_version: task.version,
        status_id: Some(status_id.to_owned()),
    };
    Ok(data
        .orbit
        .update_task(&catalog.workspace.id, &task.id, patch)
        .await?)
}

/// Send the task card, with `notice` above it when not empty.
async fn reply(
    ctx: Context<'_>,
    catalog: &Catalog,
    task: &Task,
    notice: &str,
) -> Result<(), Error> {
    let notice = (!notice.is_empty()).then_some(notice);
    ui::send(
        ctx,
        card::task_reply(ctx.data(), catalog, task, notice).await,
    )
    .await
}
