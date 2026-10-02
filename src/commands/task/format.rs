//! Components V2 cards for tasks (see `crate::ui`).

use chrono::{DateTime, Datelike, Utc};
use chrono_tz::Tz;
use serde_json::Value;

use super::input::truncate;
use crate::bot::Data;
use crate::catalog::Catalog;
use crate::orbit::models::{StatusCategory, Task};
use crate::ui;

/// Most characters of the description shown under a card title.
const SUMMARY_LIMIT: usize = 100;
/// Most non-image files listed by name on a card.
const MAX_FILE_NAMES: usize = 10;

/// Most sub-issues listed on a card.
const MAX_SUB_ISSUES: usize = 8;

/// Sub-issues and attachments shown on a task card.
#[derive(Debug, Default)]
pub struct CardExtras {
    /// Direct sub-issues (open and closed).
    pub sub_issues: Vec<Task>,
    /// `(attachment://… URL, file name)` of images uploaded with the message.
    pub images: Vec<(String, String)>,
    /// Names of the other files (not shown inline).
    pub others: Vec<String>,
}

/// The task card: linked title and short description, status, project,
/// deadline, attachments, and a menu to change the status.
pub fn task_card(
    data: &Data,
    catalog: &Catalog,
    task: &Task,
    notice: Option<&str>,
    extras: &CardExtras,
) -> Vec<Value> {
    let mut header = format!(
        "### [{}]({})",
        escape(&truncate(&task.title, 200)),
        data.task_url(&task.workspace_id, &task.id)
    );
    let summary = summary(&task.description);
    if !summary.is_empty() {
        header.push('\n');
        header.push_str(&summary);
    }

    let mut body = vec![
        ui::text(header),
        ui::separator(true, true),
        ui::text(facts(data, catalog, task).join("\n\n")),
    ];

    if !extras.sub_issues.is_empty() {
        body.push(ui::separator(true, true));
        body.push(ui::text(sub_issues(
            data,
            catalog,
            task,
            &extras.sub_issues,
        )));
    }

    let files = extras;
    if !files.images.is_empty() || !files.others.is_empty() {
        let mut title = "**Attachments**".to_owned();
        for name in files.others.iter().take(MAX_FILE_NAMES) {
            title.push_str(&format!("\n📎 {}", escape(&truncate(name, 80))));
        }
        body.push(ui::separator(true, true));
        body.push(ui::text(title));
        if !files.images.is_empty() {
            body.push(ui::gallery(&files.images));
        }
    }

    let options: Vec<Value> = catalog
        .statuses_of(&task.project_id)
        .filter(|s| s.category != StatusCategory::Duplicate)
        .take(25)
        .map(|s| {
            ui::option(
                &truncate(&s.name, 100),
                &s.id,
                data.status_icon_json(s),
                s.id == task.status_id,
            )
        })
        .collect();
    if !options.is_empty() {
        body.push(ui::separator(true, true));
        body.push(ui::text("**Update Status**"));
        body.push(ui::select(
            &status_select_id(&task.id),
            "Choose a status",
            options,
        ));
    }

    let mut card = Vec::new();
    if let Some(notice) = notice {
        card.push(ui::text(notice));
    }
    card.push(ui::container(None, body));
    card
}

/// STATUS, PRIORITY, ASSIGNEE, PROJECT, and DEADLINE lines of a task.
fn facts(data: &Data, catalog: &Catalog, task: &Task) -> [String; 5] {
    let status = catalog.status(&task.status_id);
    let project = catalog.project(&task.project_id);
    let closed = status.is_some_and(|s| s.category.is_closed());
    let blocked = if task.blocked { " · ⛔ Blocked" } else { "" };
    [
        format!(
            "**STATUS:** {} {}{blocked}",
            data.status_icon(status),
            status.map_or("Unknown", |s| s.name.as_str()),
        ),
        format!(
            "**PRIORITY:** {} {}",
            data.priority_icon(task.priority),
            task.priority.label(),
        ),
        format!("**ASSIGNEE:** {}", assignees(data, catalog, task)),
        format!(
            "**PROJECT:** {}",
            escape(project.map_or("Unknown", |p| p.name.as_str())),
        ),
        format!(
            "**DEADLINE:** {}",
            deadline(task.due_at, closed, data.timezone, Utc::now())
        ),
    ]
}

/// The Sub-issues section: progress, then one linked line per sub-issue.
fn sub_issues(data: &Data, catalog: &Catalog, task: &Task, children: &[Task]) -> String {
    let mut text = format!(
        "**Sub-issues** · {}/{} done\n",
        task.sub_issue_closed_count, task.sub_issue_count
    );
    for child in children.iter().take(MAX_SUB_ISSUES) {
        text.push_str(&format!(
            "\n{} [{}]({})",
            data.status_icon(catalog.status(&child.status_id)),
            escape(&truncate(&child.title, 60)),
            data.task_url(&child.workspace_id, &child.id),
        ));
    }
    let more = children.len().saturating_sub(MAX_SUB_ISSUES);
    if more > 0 {
        text.push_str(&format!("\n-# and {more} more in Orbit"));
    }
    text
}

/// The assignees with their emoji (if uploaded), or "Unassigned".
fn assignees(data: &Data, catalog: &Catalog, task: &Task) -> String {
    let names: Vec<String> = task
        .assignee_ids
        .iter()
        .map(|id| {
            let name = catalog.member_name(id).unwrap_or("Unknown member");
            match data.member_icon(name) {
                Some(icon) => format!("{icon} {}", escape(name)),
                None => escape(name),
            }
        })
        .collect();
    if names.is_empty() {
        "Unassigned".to_owned()
    } else {
        names.join(", ")
    }
}

/// Prefix of the custom id of the status menu; the task id follows.
pub const STATUS_SELECT_PREFIX: &str = "task-status:";

fn status_select_id(task_id: &str) -> String {
    format!("{STATUS_SELECT_PREFIX}{task_id}")
}

/// The description as one plain line of at most [`SUMMARY_LIMIT`] characters.
fn summary(description: &str) -> String {
    let flat = description.split_whitespace().collect::<Vec<_>>().join(" ");
    escape(&truncate(&flat, SUMMARY_LIMIT))
}

/// A friendly deadline in `tz`: Today, Tomorrow, a weekday, or a date.
fn deadline(due: Option<DateTime<Utc>>, closed: bool, tz: Tz, now: DateTime<Utc>) -> String {
    let Some(due) = due else {
        return "No deadline".to_owned();
    };
    let local = due.with_timezone(&tz);
    let today = now.with_timezone(&tz).date_naive();
    let day = match (local.date_naive() - today).num_days() {
        0 => "Today".to_owned(),
        1 => "Tomorrow".to_owned(),
        -1 => "Yesterday".to_owned(),
        2..=6 => local.format("%A").to_string(),
        _ if local.year() == today.year() => local.format("%b %-d").to_string(),
        _ => local.format("%b %-d, %Y").to_string(),
    };
    if due < now && !closed {
        format!("{day} (overdue)")
    } else {
        day
    }
}

/// One page of a task list, in the task view style: a linked title per task
/// and its status, priority, project, and deadline, with dividers between.
pub struct ListPage<'a> {
    /// What the list shows, e.g. "All projects · Backlog".
    pub heading: &'a str,
    /// The tasks on this page.
    pub tasks: &'a [Task],
    /// Tasks on all pages.
    pub total: usize,
    /// Zero-based page number, and the number of pages.
    pub page: usize,
    pub pages: usize,
    /// Leave out the project on each task (the list is for one project).
    pub one_project: bool,
    /// Custom ids of the previous and next buttons (None at the ends).
    pub previous: Option<String>,
    pub next: Option<String>,
}

pub fn list_card(data: &Data, catalog: &Catalog, list: &ListPage) -> Vec<Value> {
    let count = match list.total {
        1 => "1 task".to_owned(),
        n => format!("{n} tasks"),
    };
    let mut body = vec![ui::text(format!(
        "## {}\n-# {count} · most urgent first",
        escape(&truncate(list.heading, 200))
    ))];
    let mut budget = ui::TEXT_BUDGET.saturating_sub(300);
    for task in list.tasks {
        let block = list_entry(data, catalog, task, list.one_project);
        let size = block.chars().count();
        if size > budget {
            break;
        }
        budget -= size;
        body.push(ui::separator(true, true));
        body.push(ui::text(block));
    }
    if list.tasks.is_empty() {
        body.push(ui::separator(true, true));
        body.push(ui::text("No tasks match."));
    }

    if list.pages > 1 {
        let none = format!("{LIST_PREFIX}none");
        body.push(ui::separator(true, true));
        body.push(ui::row(vec![
            ui::button(
                list.previous.as_deref().unwrap_or(&none),
                "◀ Previous",
                list.previous.is_none(),
            ),
            ui::button(
                &format!("{LIST_PREFIX}page"),
                &format!("Page {} of {}", list.page + 1, list.pages),
                true,
            ),
            ui::button(
                list.next.as_deref().unwrap_or(&none),
                "Next ▶",
                list.next.is_none(),
            ),
        ]));
    }
    vec![ui::container(None, body)]
}

/// Prefix of the custom ids of the list buttons.
pub const LIST_PREFIX: &str = "tl:";

/// A task in a list: the linked title, then status, priority, project, and
/// deadline on one line.
fn list_entry(data: &Data, catalog: &Catalog, task: &Task, one_project: bool) -> String {
    let status = catalog.status(&task.status_id);
    let closed = status.is_some_and(|s| s.category.is_closed());
    let mut facts = vec![
        format!(
            "{} {}",
            data.status_icon(status),
            escape(&truncate(status.map_or("Unknown", |s| s.name.as_str()), 40))
        ),
        format!(
            "{} {}",
            data.priority_icon(task.priority),
            task.priority.label()
        ),
    ];
    if !one_project && let Some(project) = catalog.project(&task.project_id) {
        facts.push(escape(&truncate(&project.name, 40)));
    }
    if task.due_at.is_some() {
        facts.push(format!(
            "Due {}",
            deadline(task.due_at, closed, data.timezone, Utc::now())
        ));
    }
    format!(
        "### [{}]({})\n{}",
        escape(&truncate(&task.title, 80)),
        data.task_url(&task.workspace_id, &task.id),
        facts.join("\u{2003}"),
    )
}

/// The card for a deleted task, in the task view style: the title (no link,
/// it is in the trash), the task details, and a button to Orbit's trash.
pub fn deleted_card(data: &Data, catalog: &Catalog, task: &Task) -> Vec<Value> {
    let trash = format!(
        "{}/tasks-trash?workspace={}",
        data.orbit.base_url(),
        task.workspace_id
    );
    let mut body = vec![
        ui::text(format!(
            "### ~~{}~~\n-# In Orbit's trash for 30 days",
            escape(&truncate(&task.title, 200))
        )),
        ui::separator(true, true),
        ui::text(facts(data, catalog, task).join("\n\n")),
    ];
    if task.sub_issue_count > 0 {
        let count = match task.sub_issue_count {
            1 => "1 sub-issue was".to_owned(),
            n => format!("{n} sub-issues were"),
        };
        body.push(ui::separator(true, true));
        body.push(ui::text(format!("**Sub-issues**\n{count} deleted too.")));
    }
    body.push(ui::separator(true, true));
    body.push(ui::link_buttons(&[("♻️", "Open trash", &trash)]));
    vec![ui::text("🗑️ **Task deleted**"), ui::container(None, body)]
}

/// Short label for an autocomplete choice (Discord allows 100 characters).
pub fn choice_label(catalog: &Catalog, task: &Task) -> String {
    let status = catalog
        .status(&task.status_id)
        .map_or("", |s| s.name.as_str());
    truncate(
        &format!("{} · {} · {status}", catalog.display_code(task), task.title),
        100,
    )
}

/// Two lines: priority and title, then code (linked), project, and due date.
/// Escape Markdown characters so titles show as typed.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '*' | '_' | '`' | '~' | '|' | '[' | ']' | '\\' | '#' | '>'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
pub(super) mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::time::Duration;

    use chrono::TimeZone;

    use super::*;
    use crate::catalog::CatalogCache;
    use crate::config::OrbitConfig;
    use crate::orbit::OrbitClient;
    use crate::orbit::models::{Member, Priority, Project, Status, Workspace};

    fn status(id: &str, name: &str, category: StatusCategory, position: i64) -> Status {
        Status {
            id: id.into(),
            project_id: "p".into(),
            name: name.into(),
            category,
            position,
        }
    }

    pub(in crate::commands::task) fn catalog() -> Catalog {
        Catalog {
            workspace: Workspace {
                id: "w".into(),
                name: "W".into(),
            },
            projects: vec![Project {
                id: "p".into(),
                name: "Product".into(),
                key: "PROD".into(),
            }],
            statuses: vec![
                status("todo", "Todo", StatusCategory::Unstarted, 1),
                status("doing", "In Progress", StatusCategory::Started, 2),
                status("done", "Done", StatusCategory::Completed, 3),
            ],
            members: vec![
                Member {
                    user_id: "u-1".into(),
                    display_name: "ShadowArcanist".into(),
                },
                Member {
                    user_id: "u-2".into(),
                    display_name: "Orbit Bot".into(),
                },
            ],
            labels: vec![crate::orbit::models::Label {
                id: "l-1".into(),
                name: "Video Tutorial".into(),
            }],
        }
    }

    fn task(n: usize, status: &str, title: &str) -> Task {
        serde_json::from_value(serde_json::json!({
            "id": format!("0199a3f2-6b1c-7d24-9e0a-{n:012x}"),
            "workspace_id": "w", "project_id": "p", "status_id": status,
            "title": title, "description": "", "source_url": null, "priority": "high",
            "label_ids": [], "due_at": "2026-10-10T17:00:00.000Z", "version": 0,
            "created_at": "2026-10-01T12:00:00.000Z"
        }))
        .unwrap()
    }

    #[test]
    fn escapes_markdown_in_titles() {
        assert_eq!(
            escape("fix *bold* [link] #1"),
            "fix \\*bold\\* \\[link\\] \\#1"
        );
    }

    pub(in crate::commands::task) fn data() -> Data {
        let orbit = Arc::new(
            OrbitClient::new(&OrbitConfig {
                url: "https://orbit.example.com".into(),
                email: "bot@example.com".into(),
                password: "x".into(),
                workspace_id: None,
            })
            .unwrap(),
        );
        Data {
            catalog: CatalogCache::new(Duration::from_secs(60), orbit.clone(), None),
            orbit,
            allowed_users: HashSet::new(),
            timezone: chrono_tz::Tz::UTC,
            default_project: None,
            emojis: Default::default(),
            discord: crate::ui::Discord::new().unwrap(),
        }
    }

    #[test]
    fn list_card_stays_within_discord_limits() {
        let (data, c) = (data(), catalog());
        // Worst case: long titles, emojis for every icon, a due date.
        let emojis = ["status_started", "priority_high"]
            .into_iter()
            .map(|name| {
                let emoji = crate::ui::AppEmoji {
                    id: "1555152314770259998".into(),
                    name: name.into(),
                };
                (name.to_owned(), emoji)
            })
            .collect();
        data.emojis.set(emojis).unwrap();
        let tasks: Vec<Task> = (0..8)
            .map(|n| task(n, "doing", &"x*".repeat(250)))
            .collect();
        let list = ListPage {
            heading: &"h".repeat(500),
            tasks: &tasks,
            total: 100,
            page: 3,
            pages: 13,
            one_project: false,
            previous: Some("tl:2:0::".into()),
            next: Some("tl:4:0::".into()),
        };
        let card = list_card(&data, &c, &list);
        assert!(ui::text_len(&card) <= 4000, "{}", ui::text_len(&card));
        assert!(ui::component_count(&card) <= 40);
        let body = card[0]["components"].as_array().unwrap();
        // Heading, then a divider and a text block for each of the 8 tasks.
        let entries = body.iter().filter(|c| c["type"] == 10).count();
        assert_eq!(entries, 1 + 8);
        let buttons = body.last().unwrap()["components"].as_array().unwrap();
        assert_eq!(buttons[0]["custom_id"], "tl:2:0::");
        assert_eq!(buttons[1]["label"], "Page 4 of 13");
        assert_eq!(buttons[2]["custom_id"], "tl:4:0::");
        assert_eq!(buttons.len(), 3);
    }

    #[test]
    fn list_card_disables_buttons_at_the_ends_and_hides_them_for_one_page() {
        let (data, c) = (data(), catalog());
        let tasks = vec![task(1, "todo", "Launch")];
        let mut list = ListPage {
            heading: "Product",
            tasks: &tasks,
            total: 9,
            page: 0,
            pages: 2,
            one_project: true,
            previous: None,
            next: Some("tl:1:0:p:".into()),
        };
        let card = list_card(&data, &c, &list);
        let buttons =
            card[0]["components"].as_array().unwrap().last().unwrap()["components"].clone();
        assert_eq!(buttons[0]["disabled"], true);
        assert_eq!(buttons[2]["disabled"], false);
        let entry = card[0]["components"][2]["content"].as_str().unwrap();
        assert!(entry.starts_with("### [Launch](https://orbit.example.com/tasks/"));
        assert!(!entry.contains("Product"), "{entry}");

        list.pages = 1;
        list.next = None;
        let card = list_card(&data, &c, &list);
        let last = card[0]["components"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(last["type"], 10, "no button row for one page");
    }

    #[test]
    fn task_card_stays_within_discord_limits() {
        let (data, c) = (data(), catalog());
        let mut long = task(1, "doing", &"t".repeat(500));
        long.description = "d".repeat(100_000);
        long.sub_issue_count = 25;
        long.assignee_ids = vec!["u-1".into(), "u-2".into()];
        let files = CardExtras {
            sub_issues: (0..25)
                .map(|n| task(n, "todo", &"s*".repeat(250)))
                .collect(),
            images: (0..10)
                .map(|n| (format!("attachment://image-{n}.png"), "x".repeat(255)))
                .collect(),
            others: (0..20).map(|_| "f".repeat(255)).collect(),
        };
        let card = task_card(&data, &c, &long, Some("✅ Task created"), &files);
        assert!(ui::text_len(&card) <= 4000, "{}", ui::text_len(&card));
        assert!(ui::component_count(&card) <= 40);
        let body = card[1]["components"].as_array().unwrap();
        let menu = &body.last().unwrap()["components"][0];
        assert_eq!(menu["custom_id"], format!("task-status:{}", long.id));
        let defaults: Vec<_> = menu["options"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|o| o["default"] == true)
            .collect();
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0]["value"], "doing");
    }

    #[test]
    fn uses_uploaded_status_emojis_with_a_fallback() {
        let (data, c) = (data(), catalog());
        let doing = c.status("doing").unwrap();
        assert_eq!(crate::bot::status_emoji_name(doing), "status_started");
        assert_eq!(data.status_icon(Some(doing)), "🔄");
        assert_eq!(data.status_icon_json(doing)["name"], "🔄");

        let emoji = crate::ui::AppEmoji {
            id: "123".into(),
            name: "status_started".into(),
        };
        data.emojis
            .set([(emoji.name.clone(), emoji)].into_iter().collect())
            .unwrap();
        assert_eq!(data.status_icon(Some(doing)), "<:status_started:123>");
        assert_eq!(data.status_icon_json(doing)["id"], "123");
        assert_eq!(data.status_icon(None), "❔");
    }

    #[test]
    fn shows_priority_and_assignees_with_their_emojis() {
        let (data, c) = (data(), catalog());
        let mut t = task(1, "doing", "x");
        assert_eq!(assignees(&data, &c, &t), "Unassigned");
        assert_eq!(data.priority_icon(Priority::High), "🟠");

        let emojis = [
            ("shadowarcanist", "1555154041112363040"),
            ("priority_high", "7"),
        ]
        .into_iter()
        .map(|(name, id)| {
            let emoji = crate::ui::AppEmoji {
                id: id.into(),
                name: name.into(),
            };
            (name.to_owned(), emoji)
        })
        .collect();
        data.emojis.set(emojis).unwrap();

        t.assignee_ids = vec!["u-1".into(), "u-2".into(), "gone".into()];
        assert_eq!(
            assignees(&data, &c, &t),
            "<:shadowarcanist:1555154041112363040> ShadowArcanist, Orbit Bot, Unknown member"
        );
        assert_eq!(data.priority_icon(Priority::High), "<:priority_high:7>");
        let card = task_card(&data, &c, &t, None, &CardExtras::default());
        let facts = card[0]["components"][2]["content"].as_str().unwrap();
        let lines: Vec<&str> = facts.split("\n\n").collect();
        assert!(lines[0].starts_with("**STATUS:**"));
        assert_eq!(lines[1], "**PRIORITY:** <:priority_high:7> High");
        assert!(lines[2].starts_with("**ASSIGNEE:** <:shadowarcanist:"));
    }

    #[test]
    fn shows_sub_issues_in_their_own_section() {
        let (data, c) = (data(), catalog());
        let mut parent = task(1, "doing", "Parent");
        parent.sub_issue_count = 10;
        parent.sub_issue_closed_count = 3;
        let extras = CardExtras {
            sub_issues: (0..10).map(|n| task(n + 2, "todo", "Child")).collect(),
            ..CardExtras::default()
        };
        let card = task_card(&data, &c, &parent, None, &extras);
        let body = card[0]["components"].as_array().unwrap();
        // Header, divider, facts, divider, sub-issues.
        assert_eq!(body[3]["type"], 14);
        let section = body[4]["content"].as_str().unwrap();
        assert!(
            section.starts_with("**Sub-issues** · 3/10 done\n"),
            "{section}"
        );
        assert_eq!(section.matches("[Child](").count(), MAX_SUB_ISSUES);
        assert!(section.ends_with("-# and 2 more in Orbit"));

        let card = task_card(
            &data,
            &c,
            &task(1, "doing", "x"),
            None,
            &CardExtras::default(),
        );
        let text = card[0]["components"].to_string();
        assert!(!text.contains("Sub-issues"));
    }

    #[test]
    fn deleted_card_matches_the_task_view() {
        let (data, c) = (data(), catalog());
        let mut t = task(1, "doing", "Old task");
        t.sub_issue_count = 2;
        let card = deleted_card(&data, &c, &t);
        assert_eq!(card[0]["content"], "🗑️ **Task deleted**");
        let body = card[1]["components"].as_array().unwrap();
        assert!(
            body[0]["content"]
                .as_str()
                .unwrap()
                .starts_with("### ~~Old task~~")
        );
        assert!(
            body[2]["content"]
                .as_str()
                .unwrap()
                .starts_with("**STATUS:**")
        );
        assert_eq!(
            body[4]["content"],
            "**Sub-issues**\n2 sub-issues were deleted too."
        );
        assert_eq!(body.last().unwrap()["components"][0]["label"], "Open trash");
        assert!(card[1].get("accent_color").is_none());
    }

    #[test]
    fn describes_deadlines_in_the_time_zone() {
        let berlin: Tz = "Europe/Berlin".parse().unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 1, 21, 0, 0).unwrap(); // Thu 23:00 Berlin
        let at = |d, h| Utc.with_ymd_and_hms(2026, 10, d, h, 0, 0).unwrap();
        assert_eq!(deadline(None, false, berlin, now), "No deadline");
        assert_eq!(deadline(Some(at(1, 21)), false, berlin, now), "Today");
        // 23:30 UTC on 1 Oct is already 2 Oct in Berlin.
        assert_eq!(deadline(Some(at(1, 23)), false, berlin, now), "Tomorrow");
        assert_eq!(deadline(Some(at(5, 12)), false, berlin, now), "Monday");
        assert_eq!(deadline(Some(at(20, 12)), false, berlin, now), "Oct 20");
        assert_eq!(
            deadline(Some(at(1, 8)), false, berlin, now),
            "Today (overdue)"
        );
        assert_eq!(deadline(Some(at(1, 8)), true, berlin, now), "Today");
        let next_year = Utc.with_ymd_and_hms(2027, 1, 3, 12, 0, 0).unwrap();
        assert_eq!(deadline(Some(next_year), false, berlin, now), "Jan 3, 2027");
    }

    #[test]
    fn summarises_descriptions_on_one_line() {
        assert_eq!(
            summary("  Line one\n\n**two**  "),
            "Line one \\*\\*two\\*\\*"
        );
        assert_eq!(summary(&"a".repeat(150)).chars().count(), 100);
    }
}
