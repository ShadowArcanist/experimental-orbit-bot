//! The task card with its images: reads the attachments from Orbit and
//! uploads the images with the message, because Discord cannot load files
//! from Orbit (it needs a session and may be on a private network).

use super::format::{self, CardExtras};
use crate::bot::Data;
use crate::catalog::Catalog;
use crate::orbit::models::{Attachment, Task, TaskQuery};
use crate::ui::{Reply, Upload};

/// Most images shown in the gallery (Discord allows 10).
const MAX_IMAGES: usize = 10;
/// Most bytes of images uploaded with one message (Discord allows 10 MiB).
const MAX_UPLOAD_BYTES: u64 = 8 * 1024 * 1024;

/// The task card, with the task's images in a gallery. Attachment problems
/// never fail the card; the images are then left out.
pub async fn task_reply(
    data: &Data,
    catalog: &Catalog,
    task: &Task,
    notice: Option<&str>,
) -> Reply {
    let ws = &catalog.workspace.id;
    let attachments = match data.orbit.attachments(ws, &task.id).await {
        Ok(attachments) => attachments,
        Err(e) => {
            tracing::warn!(task = %task.id, "cannot list attachments: {e}");
            Vec::new()
        }
    };

    let mut files = CardExtras {
        sub_issues: sub_issues(data, ws, task).await,
        ..CardExtras::default()
    };
    let mut uploads = Vec::new();
    let mut budget = MAX_UPLOAD_BYTES;
    for attachment in &attachments {
        let Some(extension) = image_extension(attachment) else {
            files.others.push(attachment.display_name.clone());
            continue;
        };
        if uploads.len() >= MAX_IMAGES || attachment.byte_size > budget {
            files.others.push(attachment.display_name.clone());
            continue;
        }
        match data.orbit.download(ws, &task.id, &attachment.id).await {
            Ok(bytes) if bytes.len() as u64 <= budget => {
                budget -= bytes.len() as u64;
                let filename = format!("image-{}.{extension}", uploads.len());
                files.images.push((
                    format!("attachment://{filename}"),
                    attachment.display_name.chars().take(1024).collect(),
                ));
                uploads.push(Upload {
                    filename,
                    media_type: attachment.media_type.clone(),
                    bytes,
                });
            }
            Ok(_) => files.others.push(attachment.display_name.clone()),
            Err(e) => {
                tracing::warn!(attachment = %attachment.id, "cannot download attachment: {e}");
                files.others.push(attachment.display_name.clone());
            }
        }
    }

    Reply {
        components: format::task_card(data, catalog, task, notice, &files),
        files: uploads,
    }
}

/// The file extension for image types Discord shows in a gallery.
fn image_extension(attachment: &Attachment) -> Option<&'static str> {
    match attachment.media_type.as_str() {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

/// The task's direct sub-issues, open and closed. Errors leave them out.
async fn sub_issues(data: &Data, ws: &str, task: &Task) -> Vec<Task> {
    if task.sub_issue_count == 0 {
        return Vec::new();
    }
    let mut query = TaskQuery::new(Vec::new(), true, 25);
    query.parent_task_id = Some(task.id.clone());
    match data.orbit.query_tasks(ws, &query).await {
        Ok(page) => page.items,
        Err(e) => {
            tracing::warn!(task = %task.id, "cannot list sub-issues: {e}");
            Vec::new()
        }
    }
}
