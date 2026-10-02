//! Orbit API types. Only the fields the bot uses are modelled; Orbit may add
//! more, so response types never reject unknown fields. See `orbit-api.md` §6.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One page of a cursor-paginated list.
#[derive(Debug, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub key: String,
}

/// The kind of a status. "Done" means [`StatusCategory::Completed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusCategory {
    Unstarted,
    Started,
    Completed,
    Cancelled,
    Duplicate,
    /// A category added by a newer Orbit.
    #[serde(other)]
    Unknown,
}

impl StatusCategory {
    /// Completed, cancelled, and duplicate tasks are closed.
    pub fn is_closed(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Duplicate)
    }

    /// Orbit's name of the category.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unstarted => "unstarted",
            Self::Started => "started",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Duplicate => "duplicate",
            Self::Unknown => "unknown",
        }
    }

    /// A Unicode stand-in, used when the custom emoji is not uploaded.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Unstarted => "⭕",
            Self::Started => "🔄",
            Self::Completed => "✅",
            Self::Cancelled => "❌",
            Self::Duplicate => "📑",
            Self::Unknown => "❔",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Status {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub category: StatusCategory,
    pub position: i64,
}

/// Task priority. Serialized as Orbit's lowercase strings; also a Discord choice.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, poise::ChoiceParameter,
)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    #[name = "Urgent"]
    Urgent,
    #[name = "High"]
    High,
    #[name = "Medium"]
    Medium,
    #[name = "Low"]
    Low,
    #[default]
    #[name = "None"]
    None,
}

impl Priority {
    /// Orbit's name of the priority.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Urgent => "urgent",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::None => "none",
        }
    }

    /// The label Orbit shows.
    pub fn label(self) -> &'static str {
        match self {
            Self::Urgent => "Urgent",
            Self::High => "High",
            Self::Medium => "Medium",
            Self::Low => "Low",
            Self::None => "No priority",
        }
    }

    /// A Unicode stand-in, used when the custom emoji is not uploaded.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Urgent => "🔴",
            Self::High => "🟠",
            Self::Medium => "🟡",
            Self::Low => "🔵",
            Self::None => "▫️",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    pub id: String,
    pub workspace_id: String,
    pub project_id: String,
    pub status_id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub priority: Priority,
    /// User ids of the assignees.
    #[serde(default)]
    pub assignee_ids: Vec<String>,
    pub due_at: Option<DateTime<Utc>>,
    pub version: u64,
    /// True while an open task blocks this one.
    #[serde(default)]
    pub blocked: bool,
    /// Live direct sub-issues, and how many of them are closed.
    #[serde(default)]
    pub sub_issue_count: u32,
    #[serde(default)]
    pub sub_issue_closed_count: u32,
}

/// A workspace label.
#[derive(Debug, Clone, Deserialize)]
pub struct Label {
    pub id: String,
    pub name: String,
}

/// A member of the workspace.
#[derive(Debug, Clone, Deserialize)]
pub struct Member {
    pub user_id: String,
    pub display_name: String,
}

/// A file attached to a task.
#[derive(Debug, Clone, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub display_name: String,
    /// Detected from the file content by Orbit, e.g. `image/png`.
    pub media_type: String,
    pub byte_size: u64,
}

/// Body of `POST /tasks`.
#[derive(Debug, Serialize)]
pub struct NewTask {
    pub project_id: String,
    pub status_id: String,
    pub title: String,
    pub priority: Priority,
    /// User ids.
    pub assignee_ids: Vec<String>,
    pub label_ids: Vec<String>,
}

/// Body of `PATCH /tasks/{id}`. Absent fields stay unchanged.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TaskPatch {
    pub expected_version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_id: Option<String>,
}

/// Body of `POST /tasks/query`.
#[derive(Debug, Serialize)]
pub struct TaskQuery {
    pub filter: FilterGroup,
    pub order_by: &'static str,
    pub order_direction: &'static str,
    pub show_completed: &'static str,
    pub limit: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Only direct sub-issues of this task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FilterGroup {
    pub op: &'static str,
    pub children: Vec<Condition>,
}

#[derive(Debug, Serialize)]
pub struct Condition {
    pub field: &'static str,
    pub operator: &'static str,
    pub value: serde_json::Value,
}

impl TaskQuery {
    /// Tasks matching all `conditions`, urgent first.
    pub fn new(conditions: Vec<Condition>, include_closed: bool, limit: u32) -> Self {
        Self {
            filter: FilterGroup {
                op: "and",
                children: conditions,
            },
            order_by: "priority",
            order_direction: "asc",
            show_completed: if include_closed { "all" } else { "none" },
            limit,
            cursor: None,
            parent_task_id: None,
        }
    }
}

impl Condition {
    pub fn project(id: &str) -> Self {
        Self::is("project", id)
    }

    /// Any of the given status ids.
    pub fn statuses(ids: &[&str]) -> Self {
        Self {
            field: "status",
            operator: "is",
            value: serde_json::json!(ids),
        }
    }

    /// Assigned to this user.
    pub fn assignee(user_id: &str) -> Self {
        Self::is("assignee", user_id)
    }

    /// Due between Monday and Sunday of the current week (UTC days in Orbit).
    pub fn due_this_week() -> Self {
        Self {
            field: "due_date",
            operator: "between",
            value: serde_json::json!([{ "relative": "start_of_week" }, { "relative": "end_of_week" }]),
        }
    }

    /// Title or description contains `text` (Orbit needs 1–200 characters).
    pub fn text(text: &str) -> Self {
        Self {
            field: "text",
            operator: "contains",
            value: serde_json::Value::String(text.chars().take(200).collect()),
        }
    }

    fn is(field: &'static str, id: &str) -> Self {
        Self {
            field,
            operator: "is",
            value: serde_json::json!([id]),
        }
    }
}
