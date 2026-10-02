//! `/task add`: a modal (form) with a title and project, priority, assignee,
//! and label dropdowns. The new task is created in the project's default
//! status when the form is submitted.

use std::collections::HashMap;

use anyhow::{Context as _, bail};
use serde_json::Value;

use super::{card, input, resolve};
use crate::bot::Data;
use crate::catalog::Catalog;
use crate::orbit::models::{NewTask, Priority};
use crate::ui::{self, Interaction};

/// Custom id of the modal.
pub const MODAL_ID: &str = "task-add";

const PRIORITIES: [Priority; 5] = [
    Priority::Urgent,
    Priority::High,
    Priority::Medium,
    Priority::Low,
    Priority::None,
];

/// The "New task" form. Discord allows 25 options per dropdown and 5 fields.
pub fn modal(data: &Data, catalog: &Catalog) -> Value {
    let default_project = resolve::project(data, catalog, None)
        .ok()
        .map(|p| p.id.as_str());
    let projects: Vec<Value> = catalog
        .projects
        .iter()
        .take(25)
        .map(|p| {
            serde_json::json!({
                "label": input::truncate(&p.name, 100),
                "value": p.id,
                "description": p.key,
                "default": Some(p.id.as_str()) == default_project,
            })
        })
        .collect();
    let priorities: Vec<Value> = PRIORITIES
        .iter()
        .map(|&p| {
            ui::option(
                p.label(),
                p.as_str(),
                data.priority_icon_json(p),
                p == Priority::None,
            )
        })
        .collect();

    let members: Vec<Value> = catalog
        .members
        .iter()
        .take(25)
        .map(|m| {
            let mut option = serde_json::json!({
                "label": input::truncate(&m.display_name, 100),
                "value": m.user_id,
            });
            if let Some(emoji) = data.member_icon_json(&m.display_name) {
                option["emoji"] = emoji;
            }
            option
        })
        .collect();
    let labels: Vec<Value> = catalog
        .labels
        .iter()
        .take(25)
        .map(|l| serde_json::json!({ "label": input::truncate(&l.name, 100), "value": l.id }))
        .collect();

    let mut fields = vec![
        ui::label(
            "Title",
            None,
            ui::text_input("title", false, true, 500, "What needs to be done?"),
        ),
        ui::label("Project", None, ui::modal_select("project", projects)),
        ui::label("Priority", None, ui::modal_select("priority", priorities)),
    ];
    // A select menu needs at least one option.
    if !members.is_empty() {
        fields.push(ui::label(
            "Assignee",
            None,
            ui::modal_multi_select("assignees", "Unassigned", members),
        ));
    }
    if !labels.is_empty() {
        fields.push(ui::label(
            "Labels",
            None,
            ui::modal_multi_select("labels", "No labels", labels),
        ));
    }
    serde_json::json!({ "custom_id": MODAL_ID, "title": "New task", "components": fields })
}

/// The submitted form values by field custom id: `(text, chosen values)`.
pub type Fields = HashMap<String, (Option<String>, Vec<String>)>;

/// Create the task from a submitted form and show its card.
pub async fn submit(data: &Data, interaction: &Interaction, fields: &Fields) -> anyhow::Result<()> {
    data.discord.defer_reply(interaction).await?;

    let text = |id: &str| {
        fields
            .get(id)
            .and_then(|(value, _)| value.as_deref())
            .map(str::trim)
            .unwrap_or("")
    };
    let choice = |id: &str| fields.get(id).and_then(|(_, values)| values.first());
    let chosen = |id: &str| {
        fields
            .get(id)
            .map(|(_, values)| values.clone())
            .unwrap_or_default()
    };

    let title = text("title");
    if title.is_empty() {
        bail!("the title cannot be empty");
    }
    // Orbit limits titles to 500 characters and 500 bytes.
    if title.len() > 500 {
        bail!("the title is too long for Orbit (500 bytes at most)");
    }
    let priority = match choice("priority") {
        Some(value) => {
            serde_json::from_value(Value::String(value.clone())).context("unknown priority")?
        }
        None => Priority::None,
    };

    let project_id = choice("project").context("choose a project")?;
    let catalog = resolve::catalog_with(data, project_id).await?;
    let project = catalog
        .project(project_id)
        .context("that project is not in Orbit any more")?;
    let status = catalog
        .default_status(&project.id)
        .with_context(|| format!("project {} has no usable status", project.name))?;

    let new_task = NewTask {
        project_id: project.id.clone(),
        status_id: status.id.clone(),
        title: title.to_owned(),
        priority,
        assignee_ids: chosen("assignees"),
        label_ids: chosen("labels"),
    };
    let task = data
        .orbit
        .create_task(&catalog.workspace.id, &new_task)
        .await?;
    let reply = card::task_reply(data, &catalog, &task, Some("✅ **Task created**")).await;
    data.discord.edit_original(interaction, reply).await
}

#[cfg(test)]
mod tests {
    use super::super::format::tests::{catalog, data};
    use super::*;

    #[test]
    fn form_has_assignee_and_label_dropdowns() {
        let (data, c) = (data(), catalog());
        let form = modal(&data, &c);
        let fields = form["components"].as_array().unwrap();
        let labels: Vec<&str> = fields
            .iter()
            .map(|f| f["label"].as_str().unwrap())
            .collect();
        assert_eq!(
            labels,
            ["Title", "Project", "Priority", "Assignee", "Labels"]
        );

        let assignees = &fields[3]["component"];
        assert_eq!(assignees["custom_id"], "assignees");
        assert_eq!(assignees["min_values"], 0);
        assert_eq!(assignees["max_values"], 2);
        assert_eq!(assignees["required"], false);
        assert_eq!(assignees["options"][0]["value"], "u-1");
        let label = &fields[4]["component"];
        assert_eq!(label["options"][0]["value"], "l-1");
        assert_eq!(label["max_values"], 1);
        // The only project is preselected.
        assert_eq!(fields[1]["component"]["options"][0]["default"], true);
    }
}
