//! Turn command options (ids, keys, names, codes, titles) into Orbit objects.

use std::sync::Arc;

use anyhow::{Context as _, bail};

use super::input;
use crate::bot::Data;
use crate::catalog::Catalog;
use crate::orbit::models::{Condition, Member, Project, Status, StatusCategory, Task, TaskQuery};

/// Pages of 100 tasks read when a typed code is matched against a project.
const CODE_SEARCH_PAGES: usize = 20;

/// The catalog, reloaded once if it does not know `project_id` yet (for
/// example, a project created after the last load).
pub async fn catalog_with(data: &Data, project_id: &str) -> anyhow::Result<Arc<Catalog>> {
    let catalog = data.catalog().await?;
    if catalog.project(project_id).is_some() {
        return Ok(catalog);
    }
    data.catalog.invalidate().await;
    data.catalog().await
}

/// The project named by the user, the configured default, or the only project.
pub fn project<'a>(
    data: &Data,
    catalog: &'a Catalog,
    input: Option<&str>,
) -> anyhow::Result<&'a Project> {
    let keys = || {
        catalog
            .projects
            .iter()
            .map(|p| p.key.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    if let Some(input) = input {
        return catalog
            .find_project(input)
            .with_context(|| format!("no project `{input}`. Projects: {}", keys()));
    }
    if let Some(default) = &data.default_project {
        return catalog.find_project(default).with_context(|| {
            format!(
                "ORBIT_DEFAULT_PROJECT `{default}` is not a project. Projects: {}",
                keys()
            )
        });
    }
    match catalog.projects.as_slice() {
        [only] => Ok(only),
        [] => bail!("Orbit has no projects yet. Create one in Orbit first."),
        _ => bail!(
            "choose a project ({}), or set ORBIT_DEFAULT_PROJECT",
            keys()
        ),
    }
}

/// A workspace member given by user id or display name.
pub fn member<'a>(catalog: &'a Catalog, input: &str) -> anyhow::Result<&'a Member> {
    let input = input.trim();
    catalog
        .members
        .iter()
        .find(|m| m.user_id == input)
        .or_else(|| {
            catalog
                .members
                .iter()
                .find(|m| m.display_name.eq_ignore_ascii_case(input))
        })
        .with_context(|| {
            let names: Vec<&str> = catalog
                .members
                .iter()
                .map(|m| m.display_name.as_str())
                .collect();
            format!("no member `{input}`. Members: {}", names.join(", "))
        })
}

/// A status of `project` given by id or name. Duplicate is refused: Orbit sets
/// it itself.
pub fn status<'a>(
    catalog: &'a Catalog,
    project: &Project,
    input: &str,
) -> anyhow::Result<&'a Status> {
    let input = input.trim();
    let found = catalog
        .statuses_of(&project.id)
        .find(|s| s.id == input)
        .or_else(|| {
            catalog
                .statuses_of(&project.id)
                .find(|s| s.name.eq_ignore_ascii_case(input))
        });
    match found {
        Some(status) if status.category == StatusCategory::Duplicate => {
            bail!("Orbit sets the Duplicate status itself; choose another status")
        }
        Some(status) => Ok(status),
        None => {
            let names = catalog
                .statuses_of(&project.id)
                .filter(|s| s.category != StatusCategory::Duplicate)
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "`{input}` is not a status of {}. Statuses: {names}",
                project.name
            )
        }
    }
}

/// The task the user chose. Autocomplete sends the task id; typed input may
/// also be a display code (`PROD-4A5B`) or part of the title.
pub async fn task(data: &Data, catalog: &Catalog, input: &str) -> anyhow::Result<Task> {
    let input = input.trim();
    if input.is_empty() {
        bail!("choose a task");
    }
    let ws = &catalog.workspace.id;

    if input::is_uuid(input) {
        return match data.orbit.task(ws, &input.to_ascii_lowercase()).await {
            Err(e) if e.code() == Some("task_resource_not_found") => {
                bail!("no task with that id. It may have been deleted.")
            }
            other => Ok(other?),
        };
    }

    if let Some((key, suffix)) = input::parse_code(input)
        && let Some(project) = catalog
            .projects
            .iter()
            .find(|p| p.key.eq_ignore_ascii_case(key))
    {
        let mut query = TaskQuery::new(vec![Condition::project(&project.id)], true, 100);
        let mut matches = Vec::new();
        for _ in 0..CODE_SEARCH_PAGES {
            let page = data.orbit.query_tasks(ws, &query).await?;
            matches.extend(page.items.into_iter().filter(|t| t.id.ends_with(&suffix)));
            match page.next_cursor {
                Some(cursor) => query.cursor = Some(cursor),
                None => break,
            }
        }
        return one(matches, input);
    }

    let found = data
        .orbit
        .query_tasks(ws, &TaskQuery::new(vec![Condition::text(input)], true, 10))
        .await?
        .items;
    let exact: Vec<Task> = found
        .iter()
        .filter(|t| t.title.eq_ignore_ascii_case(input))
        .cloned()
        .collect();
    if exact.len() == 1 {
        return one(exact, input);
    }
    one(found, input)
}

fn one(mut tasks: Vec<Task>, input: &str) -> anyhow::Result<Task> {
    match tasks.len() {
        0 => bail!("no task matches `{input}`"),
        1 => Ok(tasks.remove(0)),
        n => {
            let titles = tasks
                .iter()
                .take(5)
                .map(|t| format!("“{}”", input::truncate(&t.title, 60)))
                .collect::<Vec<_>>()
                .join(", ");
            bail!("{n} tasks match `{input}` ({titles}). Pick one from the suggestions.")
        }
    }
}
