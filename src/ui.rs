//! Discord Components V2 messages.
//!
//! Serenity 0.12 has no Components V2 types and cannot parse messages that
//! use them, so components are raw JSON and replies go through Discord's
//! interaction endpoints with our own HTTP client. These endpoints are
//! authorized by the interaction token; no bot token is needed.
//! Docs: https://docs.discord.com/developers/components/reference
//!
//! Limits: 40 components per message (nested ones count), 4000 characters of
//! text across all Text Displays, 10 items per Media Gallery.

use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context as _, bail};
use reqwest::Method;
use reqwest::multipart::{Form, Part};
use serde_json::{Value, json};

use crate::bot::Context;

const API: &str = "https://discord.com/api/v10";
/// Message flag: only the user who ran the command sees the message.
const EPHEMERAL: u64 = 1 << 6;
/// Message flag: the message uses Components V2.
const COMPONENTS_V2: u64 = 1 << 15;

/// Text budget for one message, below Discord's 4000 to leave room for
/// headers and footers that are added after the budget is spent.
pub const TEXT_BUDGET: usize = 3600;

pub const RED: u32 = 0xED4245;

// ---- Messages ---------------------------------------------------------------

/// A message: top-level components and files they show with `attachment://`.
#[derive(Debug, Default)]
pub struct Reply {
    pub components: Vec<Value>,
    pub files: Vec<Upload>,
}

impl From<Vec<Value>> for Reply {
    fn from(components: Vec<Value>) -> Self {
        Self {
            components,
            files: Vec::new(),
        }
    }
}

/// A file sent with a message.
#[derive(Debug)]
pub struct Upload {
    pub filename: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
}

// ---- Component builders -----------------------------------------------------

/// A Text Display (Markdown).
pub fn text(content: impl Into<String>) -> Value {
    json!({ "type": 10, "content": content.into() })
}

/// A Separator: a line (`divider`) or only space; `large` doubles the space.
pub fn separator(divider: bool, large: bool) -> Value {
    json!({ "type": 14, "divider": divider, "spacing": if large { 2 } else { 1 } })
}

/// A Container with an optional accent colour bar.
pub fn container(accent: Option<u32>, components: Vec<Value>) -> Value {
    let mut container = json!({ "type": 17, "components": components });
    if let Some(accent) = accent {
        container["accent_color"] = accent.into();
    }
    container
}

/// A Media Gallery of `(url, description)` items (at most 10).
pub fn gallery(items: &[(String, String)]) -> Value {
    let items: Vec<Value> = items
        .iter()
        .take(10)
        .map(|(url, description)| json!({ "media": { "url": url }, "description": description }))
        .collect();
    json!({ "type": 12, "items": items })
}

/// An Action Row with one string select menu.
pub fn select(custom_id: &str, placeholder: &str, options: Vec<Value>) -> Value {
    json!({
        "type": 1,
        "components": [{
            "type": 3,
            "custom_id": custom_id,
            "placeholder": placeholder,
            "options": options,
        }],
    })
}

/// One option of a select menu. `emoji` is a partial emoji object.
pub fn option(label: &str, value: &str, emoji: Value, default: bool) -> Value {
    json!({ "label": label, "value": value, "emoji": emoji, "default": default })
}

/// A custom emoji of the application.
#[derive(Debug, Clone)]
pub struct AppEmoji {
    pub id: String,
    pub name: String,
}

impl AppEmoji {
    /// The emoji in message text.
    pub fn markdown(&self) -> String {
        format!("<:{}:{}>", self.name, self.id)
    }

    /// The emoji as a partial emoji object (buttons, select options).
    pub fn json(&self) -> Value {
        json!({ "id": self.id, "name": self.name })
    }
}

/// A button that sends an interaction. Style 2 is grey (secondary).
pub fn button(custom_id: &str, label: &str, disabled: bool) -> Value {
    json!({ "type": 2, "style": 2, "label": label, "custom_id": custom_id, "disabled": disabled })
}

/// A link button: `(emoji, label, url)`.
pub fn link_button(emoji: &str, label: &str, url: &str) -> Value {
    json!({ "type": 2, "style": 5, "label": label, "url": url, "emoji": { "name": emoji } })
}

/// An Action Row of up to 5 buttons.
pub fn row(buttons: Vec<Value>) -> Value {
    json!({ "type": 1, "components": buttons })
}

/// An Action Row of link buttons: `(emoji, label, url)`.
pub fn link_buttons(buttons: &[(&str, &str, &str)]) -> Value {
    row(buttons
        .iter()
        .map(|(emoji, label, url)| link_button(emoji, label, url))
        .collect())
}

/// A short message in a container, e.g. an error.
pub fn notice(accent: u32, message: &str) -> Reply {
    vec![container(Some(accent), vec![text(message)])].into()
}

// ---- Sending ----------------------------------------------------------------

/// The parts of an interaction needed to answer it.
pub struct Interaction {
    pub id: String,
    pub application_id: String,
    pub token: String,
}

/// Client for Discord's interaction endpoints.
#[derive(Clone)]
pub struct Discord {
    http: reqwest::Client,
}

impl Discord {
    pub fn new() -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { http })
    }

    /// Answer the interaction with a new ephemeral message.
    pub async fn reply(&self, i: &Interaction, reply: Reply) -> anyhow::Result<()> {
        let url = format!("{API}/interactions/{}/{}/callback", i.id, i.token);
        let data = payload(EPHEMERAL | COMPONENTS_V2, &reply);
        self.call(
            Method::POST,
            &url,
            json!({ "type": 4, "data": data }),
            reply.files,
        )
        .await
    }

    /// Answer the interaction with a modal (a form).
    pub async fn modal(&self, i: &Interaction, modal: Value) -> anyhow::Result<()> {
        let url = format!("{API}/interactions/{}/{}/callback", i.id, i.token);
        self.call(
            Method::POST,
            &url,
            json!({ "type": 9, "data": modal }),
            Vec::new(),
        )
        .await
    }

    /// Acknowledge the interaction with "thinking…"; the ephemeral reply is
    /// sent later with [`edit_original`](Self::edit_original).
    pub async fn defer_reply(&self, i: &Interaction) -> anyhow::Result<()> {
        let url = format!("{API}/interactions/{}/{}/callback", i.id, i.token);
        let body = json!({ "type": 5, "data": { "flags": EPHEMERAL } });
        self.call(Method::POST, &url, body, Vec::new()).await
    }

    /// Acknowledge a component interaction; the message is edited later.
    pub async fn defer_update(&self, i: &Interaction) -> anyhow::Result<()> {
        let url = format!("{API}/interactions/{}/{}/callback", i.id, i.token);
        self.call(Method::POST, &url, json!({ "type": 6 }), Vec::new())
            .await
    }

    /// Replace the message the interaction belongs to (or its deferred reply).
    pub async fn edit_original(&self, i: &Interaction, reply: Reply) -> anyhow::Result<()> {
        let url = format!(
            "{API}/webhooks/{}/{}/messages/@original",
            i.application_id, i.token
        );
        self.call(
            Method::PATCH,
            &url,
            payload(COMPONENTS_V2, &reply),
            reply.files,
        )
        .await
    }

    /// Send an extra ephemeral message.
    pub async fn followup(&self, i: &Interaction, reply: Reply) -> anyhow::Result<()> {
        let url = format!("{API}/webhooks/{}/{}", i.application_id, i.token);
        let data = payload(EPHEMERAL | COMPONENTS_V2, &reply);
        self.call(Method::POST, &url, data, reply.files).await
    }

    async fn call(
        &self,
        method: Method,
        url: &str,
        body: Value,
        files: Vec<Upload>,
    ) -> anyhow::Result<()> {
        let request = self.http.request(method, url);
        let request = if files.is_empty() {
            request.json(&body)
        } else {
            let mut form = Form::new().text("payload_json", body.to_string());
            for (n, file) in files.into_iter().enumerate() {
                let part = Part::bytes(file.bytes)
                    .file_name(file.filename)
                    .mime_str(&file.media_type)?;
                form = form.part(format!("files[{n}]"), part);
            }
            request.multipart(form)
        };
        let response = request.send().await.context("cannot reach Discord")?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!(
                "Discord refused the message ({status}): {}",
                body.chars().take(500).collect::<String>()
            );
        }
        Ok(())
    }
}

/// The message body: flags, components, and the files they reference. The
/// `attachments` list also drops files of an earlier version of the message.
fn payload(flags: u64, reply: &Reply) -> Value {
    let attachments: Vec<Value> = reply
        .files
        .iter()
        .enumerate()
        .map(|(n, file)| json!({ "id": n, "filename": file.filename }))
        .collect();
    json!({ "flags": flags, "components": reply.components, "attachments": attachments })
}

/// A Label (modal field) around a text input or select menu.
pub fn label(label: &str, description: Option<&str>, component: Value) -> Value {
    let mut field = json!({ "type": 18, "label": label, "component": component });
    if let Some(description) = description {
        field["description"] = description.into();
    }
    field
}

/// A text input for a modal; `long` makes it a paragraph.
pub fn text_input(
    custom_id: &str,
    long: bool,
    required: bool,
    max_length: u32,
    placeholder: &str,
) -> Value {
    json!({
        "type": 4,
        "custom_id": custom_id,
        "style": if long { 2 } else { 1 },
        "required": required,
        "max_length": max_length,
        "placeholder": placeholder,
    })
}

/// A select menu for a modal (inside a [`label`]).
pub fn modal_select(custom_id: &str, options: Vec<Value>) -> Value {
    json!({ "type": 3, "custom_id": custom_id, "options": options, "required": true })
}

/// An optional select menu for a modal where any number of options can be
/// chosen (at most 25 options).
pub fn modal_multi_select(custom_id: &str, placeholder: &str, options: Vec<Value>) -> Value {
    json!({
        "type": 3,
        "custom_id": custom_id,
        "placeholder": placeholder,
        "options": options,
        "required": false,
        "min_values": 0,
        "max_values": options.len().clamp(1, 25),
    })
}

/// The interaction of a slash command, for low-level answers.
pub fn command_interaction(ctx: Context<'_>) -> anyhow::Result<Interaction> {
    let poise::Context::Application(app) = ctx else {
        bail!("only slash commands are supported");
    };
    Ok(Interaction {
        id: app.interaction.id.to_string(),
        application_id: app.interaction.application_id.to_string(),
        token: app.interaction.token.clone(),
    })
}

/// Send `reply` as the ephemeral answer to a slash command. Edits the deferred
/// reply when the command deferred, else answers the interaction.
pub async fn send(ctx: Context<'_>, reply: impl Into<Reply>) -> anyhow::Result<()> {
    let poise::Context::Application(app) = ctx else {
        bail!("only slash commands are supported");
    };
    let interaction = Interaction {
        id: app.interaction.id.to_string(),
        application_id: app.interaction.application_id.to_string(),
        token: app.interaction.token.clone(),
    };
    let discord = &ctx.data().discord;
    if app.has_sent_initial_response.load(Ordering::SeqCst) {
        discord.edit_original(&interaction, reply.into()).await
    } else {
        discord.reply(&interaction, reply.into()).await?;
        app.has_sent_initial_response.store(true, Ordering::SeqCst);
        Ok(())
    }
}

// ---- Test helpers -----------------------------------------------------------

/// Characters of text in `components` (what counts against the 4000 limit).
#[cfg(test)]
pub fn text_len(components: &[Value]) -> usize {
    components
        .iter()
        .map(|c| {
            let own = c["content"].as_str().map_or(0, |s| s.chars().count());
            let children = c["components"].as_array().map_or(0, |a| text_len(a));
            own + children
        })
        .sum()
}

/// Number of components, nested ones included (limit 40).
#[cfg(test)]
pub fn component_count(components: &[Value]) -> usize {
    components
        .iter()
        .map(|c| 1 + c["components"].as_array().map_or(0, |a| component_count(a)))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_components() {
        let card = container(Some(RED), vec![text("hi"), separator(true, false)]);
        assert_eq!(card["type"], 17);
        assert_eq!(card["accent_color"], RED);
        assert_eq!(card["components"][0]["content"], "hi");
        assert!(container(None, Vec::new()).get("accent_color").is_none());
        let row = link_buttons(&[("🔗", "Open", "https://x")]);
        assert_eq!(row["components"][0]["style"], 5);
        assert_eq!(text_len(std::slice::from_ref(&card)), 2);
        assert_eq!(component_count(&[card, row]), 5);
    }

    #[test]
    fn lists_uploaded_files_as_attachments() {
        let reply = Reply {
            components: Vec::new(),
            files: vec![Upload {
                filename: "image-0.png".into(),
                media_type: "image/png".into(),
                bytes: vec![1],
            }],
        };
        let body = payload(COMPONENTS_V2, &reply);
        assert_eq!(body["attachments"][0]["filename"], "image-0.png");
        assert_eq!(body["attachments"][0]["id"], 0);
        assert_eq!(
            payload(COMPONENTS_V2, &Reply::default())["attachments"],
            json!([])
        );
    }
}
