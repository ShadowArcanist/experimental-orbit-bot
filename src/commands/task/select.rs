//! Buttons, menus, and forms: the "Update Status" menu on task cards, the
//! Previous/Next buttons on task lists, and the `/task add` form.
//!
//! Poise does not handle component interactions, so a raw event handler reads
//! them. Serenity 0.12 usually parses these interactions; when it cannot
//! (Components V2 in the message), it passes them on as `Event::Unknown` with
//! the raw JSON. Both forms are handled.

use std::sync::Arc;

use anyhow::Context as _;
use poise::serenity_prelude as serenity;
use serde_json::Value;

use super::add::{self, Fields};
use super::format::{LIST_PREFIX, STATUS_SELECT_PREFIX};
use super::list::ListFilter;
use super::{card, resolve, set_status};
use crate::bot::Data;
use crate::ui::{self, Interaction};

/// Interaction types of a message component and of a modal submission.
const MESSAGE_COMPONENT: u64 = 3;
const MODAL_SUBMIT: u64 = 5;

/// Handles buttons and menus on the bot's messages.
pub struct ComponentHandler {
    pub data: Arc<Data>,
}

#[serenity::async_trait]
impl serenity::RawEventHandler for ComponentHandler {
    async fn raw_event(&self, _ctx: serenity::Context, event: serenity::Event) {
        let Some(click) = Click::from_event(&event) else {
            return;
        };
        if !click.custom_id.starts_with(STATUS_SELECT_PREFIX)
            && !click.custom_id.starts_with(LIST_PREFIX)
            && click.custom_id != add::MODAL_ID
        {
            return;
        }
        if let Err(error) = handle(&self.data, &click).await {
            tracing::warn!("`{}` failed: {error:#}", click.custom_id);
            let notice = ui::notice(ui::RED, &format!("⚠️ {error}"));
            if let Err(e) = self.data.discord.followup(&click.interaction, notice).await {
                tracing::error!("could not send the error message: {e:#}");
            }
        }
    }
}

/// A click on a button, a choice in a menu, or a submitted form.
struct Click {
    interaction: Interaction,
    user_id: u64,
    custom_id: String,
    /// The chosen values of a menu; empty for a button.
    values: Vec<String>,
    /// The fields of a submitted form; empty otherwise.
    fields: Fields,
}

impl Click {
    /// The component interaction in a gateway event, if it is one.
    fn from_event(event: &serenity::Event) -> Option<Self> {
        match event {
            serenity::Event::InteractionCreate(event) => match &event.interaction {
                serenity::Interaction::Component(component) => Self::from_component(component),
                serenity::Interaction::Modal(modal) => Self::from_modal(modal),
                _ => None,
            },
            serenity::Event::Unknown(event) if event.kind == "INTERACTION_CREATE" => {
                Self::parse(&event.value)
            }
            _ => None,
        }
    }

    /// Read an interaction serenity parsed.
    fn from_component(component: &serenity::ComponentInteraction) -> Option<Self> {
        let values = match &component.data.kind {
            serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
            _ => Vec::new(),
        };
        Some(Self {
            interaction: Interaction {
                id: component.id.to_string(),
                application_id: component.application_id.to_string(),
                token: component.token.clone(),
            },
            user_id: component.user.id.get(),
            custom_id: component.data.custom_id.clone(),
            values,
            fields: Fields::new(),
        })
    }

    /// Read a form submission serenity parsed. The field values come from the
    /// `component` of each Label (patched serenity, see `vendor/README.md`).
    fn from_modal(modal: &serenity::ModalInteraction) -> Option<Self> {
        let fields = modal
            .data
            .components
            .iter()
            .filter_map(|row| row.component.as_ref())
            .map(|c| (c.custom_id.clone(), (c.value.clone(), c.values.clone())))
            .collect();
        Some(Self {
            interaction: Interaction {
                id: modal.id.to_string(),
                application_id: modal.application_id.to_string(),
                token: modal.token.clone(),
            },
            user_id: modal.user.id.get(),
            custom_id: modal.data.custom_id.clone(),
            values: Vec::new(),
            fields,
        })
    }

    /// Read a raw `INTERACTION_CREATE` payload.
    fn parse(value: &Value) -> Option<Self> {
        let kind = value["type"].as_u64();
        if kind != Some(MESSAGE_COMPONENT) && kind != Some(MODAL_SUBMIT) {
            return None;
        }
        let data = &value["data"];
        let user = value["member"]["user"]["id"]
            .as_str()
            .or_else(|| value["user"]["id"].as_str())?;
        let strings = |v: &Value| -> Vec<String> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        let values = strings(&data["values"]);
        let fields = data["components"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        let c = &row["component"];
                        let id = c["custom_id"].as_str()?.to_owned();
                        Some((
                            id,
                            (
                                c["value"].as_str().map(str::to_owned),
                                strings(&c["values"]),
                            ),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            interaction: Interaction {
                id: value["id"].as_str()?.to_owned(),
                application_id: value["application_id"].as_str()?.to_owned(),
                token: value["token"].as_str()?.to_owned(),
            },
            user_id: user.parse().ok()?,
            custom_id: data["custom_id"].as_str()?.to_owned(),
            values,
            fields,
        })
    }
}

async fn handle(data: &Data, click: &Click) -> anyhow::Result<()> {
    let interaction = &click.interaction;
    if click.user_id == 0 || !data.is_allowed(serenity::UserId::new(click.user_id)) {
        tracing::warn!(user = click.user_id, "refused a user who is not allowed");
        return data
            .discord
            .reply(
                interaction,
                ui::notice(ui::RED, "🔒 This is a private bot."),
            )
            .await;
    }
    if click.custom_id == add::MODAL_ID {
        return add::submit(data, interaction, &click.fields).await;
    }
    // Discord needs an answer within 3 seconds; the message is edited after.
    data.discord.defer_update(interaction).await?;

    if let Some(task_id) = click.custom_id.strip_prefix(STATUS_SELECT_PREFIX) {
        let status_id = click.values.first().context("no status was chosen")?;
        return change_status(data, interaction, task_id, status_id).await;
    }
    let catalog = data.catalog().await?;
    let (filter, page) = ListFilter::parse(&click.custom_id, &catalog)
        .context("this list is out of date; run the command again")?;
    let reply = super::list::page(data, &catalog, &filter, page).await?;
    data.discord.edit_original(interaction, reply.into()).await
}

async fn change_status(
    data: &Data,
    interaction: &Interaction,
    task_id: &str,
    status_id: &str,
) -> anyhow::Result<()> {
    let catalog = data.catalog().await?;
    let task = data.orbit.task(&catalog.workspace.id, task_id).await?;
    let catalog = resolve::catalog_with(data, &task.project_id).await?;
    let status = catalog
        .status(status_id)
        .filter(|s| s.project_id == task.project_id)
        .context("that status is not in the task's project any more")?;

    let (task, notice) = if status.id == task.status_id {
        (task, format!("ℹ️ Already in **{}**", status.name))
    } else {
        let notice = format!("🔀 Moved to **{}**", status.name);
        (set_status(data, &catalog, &task, &status.id).await?, notice)
    };
    let reply = card::task_reply(data, &catalog, &task, Some(&notice)).await;
    data.discord.edit_original(interaction, reply).await
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_status_choices() {
        let raw = json!({
            "id": "1", "application_id": "2", "token": "tok", "type": 3,
            "user": { "id": "42" },
            "data": { "custom_id": "task-status:t-1", "component_type": 3, "values": ["s-2"] },
        });
        let choice = Click::parse(&raw).unwrap();
        assert_eq!(choice.user_id, 42);
        assert_eq!(choice.custom_id, "task-status:t-1");
        assert_eq!(choice.values, ["s-2"]);
        assert_eq!(choice.interaction.token, "tok");

        let mut in_guild = raw.clone();
        in_guild["user"] = Value::Null;
        in_guild["member"] = json!({ "user": { "id": "43" } });
        assert_eq!(Click::parse(&in_guild).unwrap().user_id, 43);
    }

    /// Parse a gateway frame the way serenity's shard does.
    fn gateway_event(frame: &str) -> serenity::Event {
        match serde_json::from_str::<serenity::GatewayEvent>(frame).unwrap() {
            serenity::GatewayEvent::Dispatch(_, event) => event,
            _ => panic!("not a dispatch"),
        }
    }

    #[test]
    fn reads_choices_that_serenity_parses() {
        // A real-shaped choice from a user-installed app in a server.
        let event = gateway_event(GUILD_CHOICE);
        assert!(matches!(event, serenity::Event::InteractionCreate(_)));
        let choice = Click::from_event(&event).unwrap();
        assert_eq!(choice.user_id, 42);
        assert_eq!(choice.custom_id, "task-status:t-1");
        assert_eq!(choice.values, ["s-2"]);
        assert_eq!(choice.interaction.token, "tok");
        assert_eq!(choice.interaction.application_id, "2");
    }

    #[test]
    fn reads_choices_that_serenity_cannot_parse() {
        let event = gateway_event(
            r#"{"op":0,"s":5,"t":"INTERACTION_CREATE","d":{"id":"1","application_id":"2","type":3,"token":"tok","version":1,"user":{"id":"42"},"data":{"custom_id":"task-status:t-1","component_type":3,"values":["s-2"]},"message":{"components":[{"type":17,"components":[{"type":10,"content":"hi"}]}]}}}"#,
        );
        assert!(matches!(event, serenity::Event::Unknown(_)));
        let choice = Click::from_event(&event).unwrap();
        assert_eq!((choice.user_id, choice.values[0].as_str()), (42, "s-2"));
    }

    const GUILD_CHOICE: &str = r#"{"op":0,"s":7,"t":"INTERACTION_CREATE","d":{"version":1,"type":3,"token":"tok","message":{"type":20,"tts":false,"timestamp":"2026-10-01T09:43:00.000000+00:00","pinned":false,"mentions":[],"mention_roles":[],"mention_everyone":false,"interaction_metadata":{"user":{"id":"42","username":"u","discriminator":"0","global_name":"U","avatar":null},"type":2,"name":"task view","id":"9","command_type":1,"authorizing_integration_owners":{"1":"42"}},"id":"5","flags":32832,"embeds":[],"edited_timestamp":null,"content":"","components":[{"type":17,"id":1,"components":[{"type":10,"id":2,"content":"hi"},{"type":1,"id":3,"components":[{"type":3,"id":4,"custom_id":"task-status:t-1","options":[{"label":"Backlog","value":"s-1","emoji":{"id":"1","name":"backlog"},"default":true}],"placeholder":"Choose","min_values":1,"max_values":1}]}]}],"channel_id":"3","author":{"id":"2","username":"Orbit Bot","discriminator":"0","global_name":null,"avatar":null,"bot":true},"attachments":[],"application_id":"2"},"locale":"en-US","id":"1","entitlements":[],"data":{"values":["s-2"],"custom_id":"task-status:t-1","component_type":3},"context":0,"channel_id":"3","channel":{"type":0,"id":"3","name":"spam","guild_id":"8"},"authorizing_integration_owners":{"1":"42"},"attachment_size_limit":10485760,"application_id":"2","app_permissions":"0","guild_id":"8","guild_locale":"en-US","guild":{"id":"8","locale":"en-US","features":[]},"member":{"user":{"id":"42","username":"u","discriminator":"0","global_name":"U","avatar":null},"roles":[],"premium_since":null,"permissions":"0","pending":false,"nick":null,"mute":false,"joined_at":"2024-01-01T00:00:00.000000+00:00","flags":0,"deaf":false,"communication_disabled_until":null,"avatar":null}}}"#;

    #[test]
    fn reads_button_clicks() {
        let raw = json!({
            "id": "1", "application_id": "2", "token": "tok", "type": 3,
            "user": { "id": "42" },
            "data": { "custom_id": "tl:1:0:::", "component_type": 2 },
        });
        let click = Click::parse(&raw).unwrap();
        assert_eq!(click.custom_id, "tl:1:0:::");
        assert!(click.values.is_empty());
    }

    #[test]
    fn reads_form_submissions_from_the_gateway() {
        // The shape Discord sends for a modal with Label components. Serenity
        // may parse it or pass it on raw; both must give the fields.
        let event = gateway_event(
            r#"{"op":0,"s":9,"t":"INTERACTION_CREATE","d":{"version":1,"type":5,"token":"tok","locale":"en-US","id":"1","entitlements":[],"context":1,"channel_id":"3","authorizing_integration_owners":{"1":"42"},"application_id":"2","app_permissions":"0","user":{"id":"42","username":"u","discriminator":"0","global_name":"U","avatar":null},"data":{"custom_id":"task-add","components":[{"type":18,"id":1,"component":{"type":4,"id":2,"custom_id":"title","value":"Write docs"}},{"type":18,"id":3,"component":{"type":3,"id":4,"custom_id":"project","values":["p-1"]}},{"type":18,"id":5,"component":{"type":3,"id":6,"custom_id":"priority","values":["high"]}},{"type":18,"id":7,"component":{"type":4,"id":8,"custom_id":"due","value":""}}]}}}"#,
        );
        let form = Click::from_event(&event).unwrap();
        assert_eq!(form.custom_id, "task-add");
        assert_eq!(form.user_id, 42);
        assert_eq!(form.fields["title"].0.as_deref(), Some("Write docs"));
        assert_eq!(form.fields["project"].1, ["p-1"]);
        assert_eq!(form.fields["priority"].1, ["high"]);
        assert_eq!(form.fields["due"].0.as_deref(), Some(""));
    }

    #[test]
    fn reads_raw_form_submissions() {
        let raw = json!({
            "id": "1", "application_id": "2", "token": "tok", "type": 5,
            "user": { "id": "42" },
            "data": { "custom_id": "task-add", "components": [
                { "type": 18, "component": { "type": 4, "custom_id": "title", "value": "x" } },
                { "type": 18, "component": { "type": 3, "custom_id": "project", "values": ["p"] } },
            ] },
        });
        let form = Click::parse(&raw).unwrap();
        assert_eq!(form.fields["title"].0.as_deref(), Some("x"));
        assert_eq!(form.fields["project"].1, ["p"]);
    }

    #[test]
    fn ignores_commands() {
        let command = json!({ "id": "1", "application_id": "2", "token": "t", "type": 2 });
        assert!(Click::parse(&command).is_none());
    }
}
