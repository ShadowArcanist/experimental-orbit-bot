//! A cached snapshot of the slow-changing Orbit data: the workspace, its
//! projects, their statuses, the members, and the labels.
//!
//! Commands and autocomplete need status names and project keys on every call.
//! The snapshot lives for a short TTL and is dropped early when Orbit reports
//! that something in it no longer exists.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use tokio::sync::Mutex;
use tokio::task::JoinSet;

use crate::orbit::OrbitClient;
use crate::orbit::models::{Label, Member, Project, Status, StatusCategory, Task, Workspace};

pub struct Catalog {
    pub workspace: Workspace,
    /// Sorted by name.
    pub projects: Vec<Project>,
    /// Sorted by project, then position.
    pub statuses: Vec<Status>,
    pub members: Vec<Member>,
    /// Sorted by name.
    pub labels: Vec<Label>,
}

impl Catalog {
    /// Read the snapshot from Orbit. Statuses are read for all projects in
    /// parallel.
    pub async fn load(
        orbit: &Arc<OrbitClient>,
        workspace_id: Option<&str>,
    ) -> anyhow::Result<Self> {
        let workspace = pick_workspace(orbit.workspaces().await?, workspace_id)?;
        let mut projects = orbit.projects(&workspace.id).await?;
        projects.sort_by_key(|p| p.name.to_lowercase());

        let mut loads = JoinSet::new();
        for project in &projects {
            let (orbit, ws, id) = (orbit.clone(), workspace.id.clone(), project.id.clone());
            loads.spawn(async move { orbit.statuses(&ws, &id).await });
        }
        let mut statuses = Vec::new();
        while let Some(loaded) = loads.join_next().await {
            statuses.extend(loaded.context("a status load stopped")??);
        }
        statuses.sort_by(|a, b| {
            (&a.project_id, a.position, &a.id).cmp(&(&b.project_id, b.position, &b.id))
        });
        let members = orbit.members(&workspace.id).await?;
        let mut labels = orbit.labels(&workspace.id).await?;
        labels.sort_by_key(|l| l.name.to_lowercase());
        Ok(Self {
            workspace,
            projects,
            statuses,
            members,
            labels,
        })
    }

    /// The display name of a user, if they are a member.
    pub fn member_name(&self, user_id: &str) -> Option<&str> {
        self.members
            .iter()
            .find(|m| m.user_id == user_id)
            .map(|m| m.display_name.as_str())
    }

    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    /// Find a project by id, key, or name (case-insensitive).
    pub fn find_project(&self, input: &str) -> Option<&Project> {
        let input = input.trim();
        self.project(input).or_else(|| {
            self.projects
                .iter()
                .find(|p| p.key.eq_ignore_ascii_case(input) || p.name.eq_ignore_ascii_case(input))
        })
    }

    pub fn status(&self, id: &str) -> Option<&Status> {
        self.statuses.iter().find(|s| s.id == id)
    }

    /// The statuses of one project, in board order.
    pub fn statuses_of(&self, project_id: &str) -> impl Iterator<Item = &Status> + use<'_> {
        let project_id = project_id.to_owned();
        self.statuses
            .iter()
            .filter(move |s| s.project_id == project_id)
    }

    /// Orbit's default status for new tasks: the first `unstarted` status,
    /// else the first status that is not `duplicate` (`orbit-api.md` §6.5).
    pub fn default_status(&self, project_id: &str) -> Option<&Status> {
        let usable = || {
            self.statuses_of(project_id)
                .filter(|s| s.category != StatusCategory::Duplicate)
        };
        usable()
            .find(|s| s.category == StatusCategory::Unstarted)
            .or_else(|| usable().next())
    }

    /// The first `completed` status of a project.
    pub fn done_status(&self, project_id: &str) -> Option<&Status> {
        self.statuses_of(project_id)
            .find(|s| s.category == StatusCategory::Completed)
    }

    /// The display code the Orbit web UI shows, e.g. `PROD-4A5B`.
    pub fn display_code(&self, task: &Task) -> String {
        let key = self
            .project(&task.project_id)
            .map_or("TASK", |p| p.key.as_str());
        display_code(key, &task.id)
    }
}

/// `KEY-XXXX`: the project key and the last four characters of the id, as in
/// the Orbit web UI. Not unique; only for display and for typed input.
pub fn display_code(key: &str, task_id: &str) -> String {
    let suffix: String = task_id
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{key}-{}", suffix.to_uppercase())
}

fn pick_workspace(workspaces: Vec<Workspace>, wanted: Option<&str>) -> anyhow::Result<Workspace> {
    let names = || {
        workspaces
            .iter()
            .map(|w| format!("{} ({})", w.name, w.id))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if let Some(id) = wanted {
        let found = workspaces.iter().find(|w| w.id == id).cloned();
        return found.with_context(|| {
            format!(
                "ORBIT_WORKSPACE_ID `{id}` is not a workspace of the bot user. Workspaces: {}",
                names()
            )
        });
    }
    match workspaces.as_slice() {
        [only] => Ok(only.clone()),
        [] => bail!("the Orbit bot user is not a member of any workspace. Invite it first."),
        _ => bail!(
            "the bot user is in several workspaces; set ORBIT_WORKSPACE_ID to one of: {}",
            names()
        ),
    }
}

/// The cached catalog and when it was loaded.
type Slot = Arc<Mutex<Option<(Instant, Arc<Catalog>)>>>;

/// A [`Catalog`] that refreshes after `ttl`.
pub struct CatalogCache {
    ttl: Duration,
    orbit: Arc<OrbitClient>,
    workspace_id: Option<String>,
    inner: Slot,
}

impl CatalogCache {
    pub fn new(ttl: Duration, orbit: Arc<OrbitClient>, workspace_id: Option<String>) -> Self {
        Self {
            ttl,
            orbit,
            workspace_id,
            inner: Arc::new(Mutex::new(None)),
        }
    }

    /// The cached snapshot, loaded again when it is older than the TTL.
    ///
    /// The work runs in its own task, so a load always finishes and fills the
    /// cache, also when the caller stops waiting (autocomplete has a time
    /// limit). The lock makes parallel callers share one load.
    pub async fn get(&self) -> anyhow::Result<Arc<Catalog>> {
        let (ttl, orbit, workspace_id, inner) = (
            self.ttl,
            self.orbit.clone(),
            self.workspace_id.clone(),
            self.inner.clone(),
        );
        tokio::spawn(async move {
            let mut inner = inner.lock().await;
            if let Some((loaded_at, catalog)) = inner.as_ref()
                && loaded_at.elapsed() < ttl
            {
                return Ok(catalog.clone());
            }
            let catalog = Arc::new(Catalog::load(&orbit, workspace_id.as_deref()).await?);
            *inner = Some((Instant::now(), catalog.clone()));
            Ok(catalog)
        })
        .await
        .context("the catalog load stopped")?
    }

    /// Drop the snapshot so the next [`get`](Self::get) reads Orbit again.
    pub async fn invalidate(&self) {
        *self.inner.lock().await = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(id: &str, category: StatusCategory, position: i64) -> Status {
        Status {
            id: id.into(),
            project_id: "p".into(),
            name: id.into(),
            category,
            position,
        }
    }

    fn catalog(statuses: Vec<Status>) -> Catalog {
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
            statuses,
            members: Vec::new(),
            labels: Vec::new(),
        }
    }

    #[test]
    fn default_status_prefers_unstarted() {
        let c = catalog(vec![
            status("dup", StatusCategory::Duplicate, 0),
            status("doing", StatusCategory::Started, 1),
            status("todo", StatusCategory::Unstarted, 2),
            status("done", StatusCategory::Completed, 3),
        ]);
        assert_eq!(c.default_status("p").unwrap().id, "todo");
        assert_eq!(c.done_status("p").unwrap().id, "done");
    }

    #[test]
    fn default_status_skips_duplicate_without_unstarted() {
        let c = catalog(vec![
            status("dup", StatusCategory::Duplicate, 0),
            status("doing", StatusCategory::Started, 1),
        ]);
        assert_eq!(c.default_status("p").unwrap().id, "doing");
    }

    #[test]
    fn finds_projects_by_key_or_name() {
        let c = catalog(Vec::new());
        assert_eq!(c.find_project("prod").unwrap().id, "p");
        assert_eq!(c.find_project("product").unwrap().id, "p");
        assert_eq!(c.find_project("p").unwrap().id, "p");
        assert!(c.find_project("other").is_none());
    }

    #[test]
    fn builds_display_codes() {
        assert_eq!(
            display_code("PROD", "0199a3f2-6b1c-7d24-9e0a-3c5b7d9e4a5b"),
            "PROD-4A5B"
        );
    }

    #[test]
    fn picks_the_only_workspace_or_the_configured_one() {
        let ws = |id: &str| Workspace {
            id: id.into(),
            name: id.into(),
        };
        assert_eq!(pick_workspace(vec![ws("a")], None).unwrap().id, "a");
        assert!(pick_workspace(vec![ws("a"), ws("b")], None).is_err());
        assert_eq!(
            pick_workspace(vec![ws("a"), ws("b")], Some("b"))
                .unwrap()
                .id,
            "b"
        );
        assert!(pick_workspace(vec![ws("a")], Some("c")).is_err());
        assert!(pick_workspace(Vec::new(), None).is_err());
    }
}
