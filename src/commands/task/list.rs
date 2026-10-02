//! `/task list` pages. The filter travels in the custom ids of the Previous
//! and Next buttons, so paging works without stored state, also after a
//! restart.

use anyhow::bail;
use serde_json::Value;

use super::format::{self, LIST_PREFIX, ListPage};
use crate::bot::Data;
use crate::catalog::Catalog;
use crate::orbit::models::{Condition, Project, Status, Task, TaskQuery};

/// Tasks per page. Eight keeps the page under Discord's 4000-character text
/// limit, even with custom emojis and long titles.
pub const PAGE_SIZE: usize = 8;
/// Most query pages of 100 tasks read for one list.
const MAX_QUERY_PAGES: usize = 10;

/// What a list shows. A status given without a project means every status
/// with that name, in all projects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListFilter {
    pub project_id: Option<String>,
    pub status_id: Option<String>,
    /// User id of an assignee.
    pub assignee_id: Option<String>,
    /// Include completed and cancelled tasks.
    pub closed: bool,
    /// Only tasks due this week (Monday to Sunday, UTC, as in Orbit).
    pub week: bool,
}

impl ListFilter {
    /// The custom id of a button that opens `page`. Discord allows 100
    /// characters, so the project is stored by key and the other ids without
    /// hyphens: `tl:<page>:<flags>:<project key>:<status id>:<assignee id>`
    /// (at most 96 characters).
    pub fn custom_id(&self, page: usize, catalog: &Catalog) -> String {
        let flags = u8::from(self.closed) | (u8::from(self.week) << 1);
        let key = self
            .project_id
            .as_deref()
            .and_then(|id| catalog.project(id))
            .map_or("", |p| p.key.as_str());
        let compact = |id: &Option<String>| id.as_deref().unwrap_or("").replace('-', "");
        format!(
            "{LIST_PREFIX}{page}:{flags}:{key}:{}:{}",
            compact(&self.status_id),
            compact(&self.assignee_id),
        )
    }

    /// Read a custom id made by [`custom_id`](Self::custom_id).
    pub fn parse(custom_id: &str, catalog: &Catalog) -> Option<(Self, usize)> {
        let mut parts = custom_id.strip_prefix(LIST_PREFIX)?.split(':');
        let page = parts.next()?.parse().ok()?;
        let flags: u8 = parts.next()?.parse().ok()?;
        let key = parts.next()?;
        let project_id = if key.is_empty() {
            None
        } else {
            Some(catalog.find_project(key)?.id.clone())
        };
        let status_id = expand(parts.next()?);
        let assignee_id = expand(parts.next()?);
        let filter = Self {
            project_id,
            status_id,
            assignee_id,
            closed: flags & 1 != 0,
            week: flags & 2 != 0,
        };
        Some((filter, page))
    }

    /// The project and the statuses the filter names.
    fn resolve<'a>(
        &self,
        catalog: &'a Catalog,
    ) -> anyhow::Result<(Option<&'a Project>, Vec<&'a Status>)> {
        let project = match &self.project_id {
            Some(id) => match catalog.project(id) {
                Some(project) => Some(project),
                None => bail!("that project is not in Orbit any more"),
            },
            None => None,
        };
        let statuses = match (&self.status_id, project) {
            (None, _) => Vec::new(),
            (Some(id), Some(_)) => catalog.status(id).into_iter().collect(),
            (Some(id), None) => match catalog.status(id) {
                Some(status) => catalog
                    .statuses
                    .iter()
                    .filter(|s| s.name.eq_ignore_ascii_case(&status.name))
                    .collect(),
                None => Vec::new(),
            },
        };
        if self.status_id.is_some() && statuses.is_empty() {
            bail!("that status is not in Orbit any more");
        }
        Ok((project, statuses))
    }
}

/// A UUID stored without hyphens, back in its usual form.
fn expand(compact: &str) -> Option<String> {
    if compact.len() != 32 || !compact.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let (a, rest) = compact.split_at(8);
    let (b, rest) = rest.split_at(4);
    let (c, rest) = rest.split_at(4);
    let (d, e) = rest.split_at(4);
    Some(format!("{a}-{b}-{c}-{d}-{e}"))
}

/// Page `page` of the list (clamped to the last page).
pub async fn page(
    data: &Data,
    catalog: &Catalog,
    filter: &ListFilter,
    page: usize,
) -> anyhow::Result<Vec<Value>> {
    let (project, statuses) = filter.resolve(catalog)?;
    let tasks = fetch(data, catalog, filter, project, &statuses).await?;

    let pages = tasks.len().div_ceil(PAGE_SIZE).max(1);
    let page = page.min(pages - 1);
    let start = page * PAGE_SIZE;
    let shown = &tasks[start..tasks.len().min(start + PAGE_SIZE)];

    let mut parts = Vec::new();
    if filter.week {
        parts.push("This week");
    }
    parts.push(project.map_or("All projects", |p| p.name.as_str()));
    if let Some(status) = statuses.first() {
        parts.push(&status.name);
    }
    if let Some(name) = filter
        .assignee_id
        .as_deref()
        .and_then(|id| catalog.member_name(id))
    {
        parts.push(name);
    }
    let heading = parts.join(" · ");
    Ok(format::list_card(
        data,
        catalog,
        &ListPage {
            heading: &heading,
            tasks: shown,
            total: tasks.len(),
            page,
            pages,
            one_project: project.is_some(),
            previous: (page > 0).then(|| filter.custom_id(page - 1, catalog)),
            next: (page + 1 < pages).then(|| filter.custom_id(page + 1, catalog)),
        },
    ))
}

/// All tasks matching the filter, most urgent first.
async fn fetch(
    data: &Data,
    catalog: &Catalog,
    filter: &ListFilter,
    project: Option<&Project>,
    statuses: &[&Status],
) -> anyhow::Result<Vec<Task>> {
    let mut conditions = Vec::new();
    if let Some(project) = project {
        conditions.push(Condition::project(&project.id));
    }
    if !statuses.is_empty() {
        let ids: Vec<&str> = statuses.iter().map(|s| s.id.as_str()).collect();
        conditions.push(Condition::statuses(&ids));
    }
    if let Some(assignee) = &filter.assignee_id {
        conditions.push(Condition::assignee(assignee));
    }
    if filter.week {
        conditions.push(Condition::due_this_week());
    }
    // Asking for a closed status only makes sense with closed tasks shown.
    let closed = filter.closed || statuses.iter().any(|s| s.category.is_closed());
    let mut query = TaskQuery::new(conditions, closed, 100);
    let mut tasks = Vec::new();
    for _ in 0..MAX_QUERY_PAGES {
        let page = data
            .orbit
            .query_tasks(&catalog.workspace.id, &query)
            .await?;
        tasks.extend(page.items);
        match page.next_cursor {
            Some(cursor) => query.cursor = Some(cursor),
            None => break,
        }
    }
    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use super::super::format::tests::catalog;
    use super::*;

    #[test]
    fn filters_round_trip_through_custom_ids() {
        let c = catalog();
        let filter = ListFilter {
            project_id: Some("p".into()),
            status_id: Some("0199a3e0-1a2d-7a01-8b02-c3d4e5f60718".into()),
            assignee_id: Some("0199a3e0-0f00-7aaa-8bbb-ccccdddd0001".into()),
            closed: true,
            week: true,
        };
        let id = filter.custom_id(123, &c);
        assert_eq!(&id[..13], "tl:123:3:PROD");
        assert_eq!(ListFilter::parse(&id, &c), Some((filter, 123)));

        // The longest possible id fits Discord's 100 characters.
        let longest = format!(
            "tl:999:3:{}:{}:{}",
            "K".repeat(20),
            "a".repeat(32),
            "b".repeat(32)
        );
        assert!(longest.len() <= 100, "{}", longest.len());

        let empty = ListFilter::default();
        assert_eq!(
            ListFilter::parse(&empty.custom_id(0, &c), &c),
            Some((empty, 0))
        );
        assert_eq!(ListFilter::parse("tl:page", &c), None);
        assert_eq!(ListFilter::parse("task-status:x", &c), None);
    }

    #[test]
    fn expands_compact_uuids() {
        assert_eq!(
            expand("0199a3e01a2d7a018b02c3d4e5f60718").as_deref(),
            Some("0199a3e0-1a2d-7a01-8b02-c3d4e5f60718")
        );
        assert_eq!(expand(""), None);
        assert_eq!(expand("xyz"), None);
    }
}
