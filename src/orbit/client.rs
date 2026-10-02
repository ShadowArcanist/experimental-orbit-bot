//! HTTP client for the Orbit REST API (`orbit-api.md`).
//!
//! Orbit authenticates REST calls with a session cookie only. The client signs
//! in with the bot user's email and password, keeps the cookie in memory, and
//! signs in again once when Orbit answers `401`. Every request carries the
//! `Origin` header that Orbit requires on writes (§3.4).

use std::fmt;
use std::time::Duration;

use reqwest::header::{COOKIE, HeaderMap, ORIGIN, SET_COOKIE};
use reqwest::{Method, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::models::{
    Attachment, Label, Member, NewTask, Page, Project, Status, Task, TaskPatch, TaskQuery,
    Workspace,
};
use crate::config::OrbitConfig;

/// Session cookie names: production (`Secure`) and development.
const SESSION_COOKIES: [&str; 2] = ["__Host-orbit_session", "orbit_session_dev"];
/// Upper bound on pages read by `list_all`, so a bad cursor cannot loop forever.
const MAX_PAGES: usize = 50;

/// An error from talking to Orbit, with a message fit to show the user.
#[derive(Debug)]
pub enum OrbitError {
    /// Orbit answered with a problem+json error.
    Api {
        status: u16,
        code: String,
        detail: String,
        /// Set on `409 conflict`: the version Orbit has now.
        current_version: Option<u64>,
    },
    /// The request did not complete (network error or timeout).
    Unreachable(reqwest::Error),
    /// Orbit answered with a body the bot does not understand.
    InvalidResponse(String),
    /// Sign-in failed.
    Login(String),
}

impl OrbitError {
    /// The Orbit error code, when Orbit sent one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Api { code, .. } => Some(code),
            _ => None,
        }
    }

    fn conflict_version(&self) -> Option<u64> {
        match self {
            Self::Api {
                code,
                current_version,
                ..
            } if code == "conflict" => *current_version,
            _ => None,
        }
    }
}

impl fmt::Display for OrbitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api {
                status,
                code,
                detail,
                ..
            } => match code.as_str() {
                "origin_forbidden" => {
                    write!(
                        f,
                        "Orbit refused the request origin. ORBIT_URL must be exactly Orbit's public URL."
                    )
                }
                "https_required" => {
                    write!(
                        f,
                        "Orbit requires HTTPS. Use Orbit's public `https://` URL in ORBIT_URL."
                    )
                }
                "authentication_required" => write!(f, "Orbit did not accept the bot's session."),
                "task_resource_not_found" | "workspace_resource_not_found" => {
                    write!(
                        f,
                        "Orbit could not find that item. It may have been deleted."
                    )
                }
                "conflict" => write!(f, "The task changed in Orbit at the same time. Try again."),
                "rate_limit_exceeded" => write!(f, "Too many requests to Orbit. Wait one minute."),
                "validation_failed" => write!(f, "Orbit did not accept the value of `{detail}`."),
                _ => write!(f, "Orbit error {status} ({code}): {detail}"),
            },
            Self::Unreachable(error) if error.is_timeout() => {
                write!(f, "Orbit did not answer in time.")
            }
            Self::Unreachable(_) => {
                write!(f, "Cannot reach Orbit. Check ORBIT_URL and the network.")
            }
            Self::InvalidResponse(what) => write!(f, "Orbit sent an unexpected response ({what})."),
            Self::Login(why) => write!(f, "The bot cannot sign in to Orbit: {why}"),
        }
    }
}

impl std::error::Error for OrbitError {}

/// The problem+json error body (§5). Only the fields the bot reads.
#[derive(Deserialize)]
struct Problem {
    code: String,
    #[serde(default)]
    detail: String,
    conflict: Option<ConflictInfo>,
}

#[derive(Deserialize)]
struct ConflictInfo {
    current_version: Option<u64>,
}

/// A signed-in Orbit client. Cheap to share behind an `Arc`.
pub struct OrbitClient {
    http: reqwest::Client,
    base: String,
    email: String,
    password: String,
    /// The `name=value` session cookie, once signed in.
    session: Mutex<Option<String>>,
}

impl OrbitClient {
    pub fn new(config: &OrbitConfig) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            http,
            base: config.url.clone(),
            email: config.email.clone(),
            password: config.password.clone(),
            session: Mutex::new(None),
        })
    }

    /// Orbit's public origin, used to build web links.
    pub fn base_url(&self) -> &str {
        &self.base
    }

    // ---- Typed endpoints -------------------------------------------------

    /// `GET /auth/me`: the bot user's display name.
    pub async fn me(&self) -> Result<String, OrbitError> {
        #[derive(Deserialize)]
        struct Me {
            display_name: String,
        }
        let me: Me = self.get("/api/v1/auth/me").await?;
        Ok(me.display_name)
    }

    pub async fn workspaces(&self) -> Result<Vec<Workspace>, OrbitError> {
        self.get("/api/v1/workspaces").await
    }

    pub async fn labels(&self, ws: &str) -> Result<Vec<Label>, OrbitError> {
        self.list_all(&format!("/api/v1/workspaces/{}/labels", seg(ws)))
            .await
    }

    pub async fn members(&self, ws: &str) -> Result<Vec<Member>, OrbitError> {
        self.list_all(&format!("/api/v1/workspaces/{}/members", seg(ws)))
            .await
    }

    pub async fn projects(&self, ws: &str) -> Result<Vec<Project>, OrbitError> {
        self.list_all(&format!("/api/v1/workspaces/{}/projects", seg(ws)))
            .await
    }

    pub async fn statuses(&self, ws: &str, project: &str) -> Result<Vec<Status>, OrbitError> {
        self.list_all(&format!(
            "/api/v1/workspaces/{}/projects/{}/statuses",
            seg(ws),
            seg(project)
        ))
        .await
    }

    pub async fn task(&self, ws: &str, id: &str) -> Result<Task, OrbitError> {
        self.get(&task_path(ws, id)).await
    }

    /// The first page of a task's own attachments (not comment attachments).
    pub async fn attachments(&self, ws: &str, task: &str) -> Result<Vec<Attachment>, OrbitError> {
        let page: Page<Attachment> = self
            .get(&format!("{}/attachments?limit=50", task_path(ws, task)))
            .await?;
        Ok(page.items)
    }

    /// The bytes of one attachment.
    pub async fn download(
        &self,
        ws: &str,
        task: &str,
        attachment: &str,
    ) -> Result<Vec<u8>, OrbitError> {
        let path = format!(
            "{}/attachments/{}/download",
            task_path(ws, task),
            seg(attachment)
        );
        let response = self.send(Method::GET, &path, None).await?;
        let bytes = response.bytes().await.map_err(OrbitError::Unreachable)?;
        Ok(bytes.to_vec())
    }

    /// One page of `POST /tasks/query`.
    pub async fn query_tasks(&self, ws: &str, query: &TaskQuery) -> Result<Page<Task>, OrbitError> {
        self.send_json(
            Method::POST,
            &format!("/api/v1/workspaces/{}/tasks/query", seg(ws)),
            Some(query),
        )
        .await
    }

    pub async fn create_task(&self, ws: &str, task: &NewTask) -> Result<Task, OrbitError> {
        self.send_json(
            Method::POST,
            &format!("/api/v1/workspaces/{}/tasks", seg(ws)),
            Some(task),
        )
        .await
    }

    /// `PATCH /tasks/{id}`. On a version conflict it retries once with the
    /// version Orbit reports, because the bot's changes do not depend on the
    /// rest of the task.
    pub async fn update_task(
        &self,
        ws: &str,
        id: &str,
        mut patch: TaskPatch,
    ) -> Result<Task, OrbitError> {
        match self
            .send_json(Method::PATCH, &task_path(ws, id), Some(&patch))
            .await
        {
            Err(error) if error.conflict_version().is_some() => {
                patch.expected_version = error.conflict_version().unwrap_or_default();
                self.send_json(Method::PATCH, &task_path(ws, id), Some(&patch))
                    .await
            }
            other => other,
        }
    }

    /// `DELETE /tasks/{id}` (soft delete, 30-day trash). Retries once on a
    /// version conflict.
    pub async fn delete_task(&self, ws: &str, id: &str, version: u64) -> Result<(), OrbitError> {
        let path = |v: u64| format!("{}?expected_version={v}", task_path(ws, id));
        match self.send(Method::DELETE, &path(version), None).await {
            Err(error) if error.conflict_version().is_some() => {
                let version = error.conflict_version().unwrap_or_default();
                self.send(Method::DELETE, &path(version), None)
                    .await
                    .map(drop)
            }
            other => other.map(drop),
        }
    }

    // ---- Transport -------------------------------------------------------

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, OrbitError> {
        self.send_json(Method::GET, path, None::<&()>).await
    }

    /// Read every page of a cursor-paginated list.
    async fn list_all<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>, OrbitError> {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let url = match &cursor {
                Some(c) => format!("{path}?limit=100&cursor={}", seg(c)),
                None => format!("{path}?limit=100"),
            };
            let page: Page<T> = self.get(&url).await?;
            items.extend(page.items);
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(items);
            }
        }
        Err(OrbitError::InvalidResponse(format!(
            "more than {MAX_PAGES} pages at {path}"
        )))
    }

    async fn send_json<T: DeserializeOwned, B: Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, OrbitError> {
        let body = body
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| OrbitError::InvalidResponse(format!("request body: {e}")))?;
        let response = self.send(method, path, body.as_ref()).await?;
        response
            .json()
            .await
            .map_err(|e| OrbitError::InvalidResponse(e.to_string()))
    }

    /// Send a request with the session cookie. On `401`, sign in again and
    /// retry once.
    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<Response, OrbitError> {
        let mut cookie = self.session(None).await?;
        let mut retried = false;
        loop {
            let mut request = self
                .http
                .request(method.clone(), format!("{}{path}", self.base))
                .header(ORIGIN, &self.base)
                .header(COOKIE, &cookie);
            if let Some(body) = body {
                request = request.json(body);
            }
            let response = request.send().await.map_err(OrbitError::Unreachable)?;
            if response.status() == StatusCode::UNAUTHORIZED && !retried {
                tracing::info!("Orbit session expired; signing in again");
                cookie = self.session(Some(&cookie)).await?;
                retried = true;
                continue;
            }
            return error_for_status(response).await;
        }
    }

    /// Return the current session cookie. Sign in when there is none, or when
    /// the current one equals `stale` (Orbit rejected it). The lock makes
    /// concurrent commands share one sign-in.
    async fn session(&self, stale: Option<&str>) -> Result<String, OrbitError> {
        let mut session = self.session.lock().await;
        if let Some(cookie) = session.as_deref()
            && Some(cookie) != stale
        {
            return Ok(cookie.to_owned());
        }
        let cookie = self.login().await?;
        *session = Some(cookie.clone());
        Ok(cookie)
    }

    async fn login(&self) -> Result<String, OrbitError> {
        let response = self
            .http
            .post(format!("{}/api/v1/auth/login", self.base))
            .header(ORIGIN, &self.base)
            .json(&serde_json::json!({ "email": self.email, "password": self.password }))
            .send()
            .await
            .map_err(OrbitError::Unreachable)?;
        let response = match error_for_status(response).await {
            Ok(response) => response,
            Err(OrbitError::Api { code, .. }) if code == "invalid_credentials" => {
                return Err(OrbitError::Login(
                    "wrong ORBIT_EMAIL or ORBIT_PASSWORD".into(),
                ));
            }
            Err(OrbitError::Api { code, .. }) if code == "authentication_throttled" => {
                return Err(OrbitError::Login(
                    "too many sign-in attempts; wait and try again".into(),
                ));
            }
            Err(error) => return Err(error),
        };
        let cookie = session_cookie(response.headers()).ok_or_else(|| {
            OrbitError::Login(
                "Orbit sent no session cookie (is ORBIT_URL the Orbit web URL?)".into(),
            )
        })?;
        tracing::info!("signed in to Orbit as {}", self.email);
        Ok(cookie)
    }
}

/// Turn a non-2xx response into an [`OrbitError`].
async fn error_for_status(response: Response) -> Result<Response, OrbitError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let bytes = response.bytes().await.unwrap_or_default();
    Err(match serde_json::from_slice::<Problem>(&bytes) {
        Ok(problem) => OrbitError::Api {
            status: status.as_u16(),
            code: problem.code,
            detail: problem.detail,
            current_version: problem.conflict.and_then(|c| c.current_version),
        },
        Err(_) => OrbitError::Api {
            status: status.as_u16(),
            code: "unknown".into(),
            detail: String::from_utf8_lossy(&bytes).chars().take(200).collect(),
            current_version: None,
        },
    })
}

/// Find the Orbit session cookie in `Set-Cookie` headers, as `name=value`.
fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            let pair = value.split(';').next()?.trim();
            let (name, cookie) = pair.split_once('=')?;
            (SESSION_COOKIES.contains(&name) && !cookie.is_empty()).then(|| pair.to_owned())
        })
}

fn task_path(ws: &str, id: &str) -> String {
    format!("/api/v1/workspaces/{}/tasks/{}", seg(ws), seg(id))
}

/// Percent-encode one URL path or query segment.
fn seg(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::http::{HeaderMap as AxumHeaders, StatusCode as AxumStatus};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use reqwest::header::HeaderValue;

    use super::*;

    #[test]
    fn finds_the_session_cookie() {
        let mut headers = HeaderMap::new();
        headers.append(SET_COOKIE, HeaderValue::from_static("other=1; Path=/"));
        headers.append(
            SET_COOKIE,
            HeaderValue::from_static("__Host-orbit_session=abc123; Path=/; Secure; HttpOnly"),
        );
        assert_eq!(
            session_cookie(&headers).as_deref(),
            Some("__Host-orbit_session=abc123")
        );

        let mut dev = HeaderMap::new();
        dev.append(
            SET_COOKIE,
            HeaderValue::from_static("orbit_session_dev=xyz; Path=/"),
        );
        assert_eq!(
            session_cookie(&dev).as_deref(),
            Some("orbit_session_dev=xyz")
        );

        let mut cleared = HeaderMap::new();
        cleared.append(
            SET_COOKIE,
            HeaderValue::from_static("__Host-orbit_session=; Max-Age=0"),
        );
        assert_eq!(session_cookie(&cleared), None);
    }

    #[test]
    fn encodes_path_segments() {
        assert_eq!(seg("abc-123"), "abc-123");
        assert_eq!(seg("a/b c"), "a%2Fb%20c");
    }

    /// A fake Orbit: login issues `session-N`; `/auth/me` accepts only the
    /// newest session and needs the right Origin.
    async fn fake_orbit() -> (String, Arc<AtomicUsize>) {
        let logins = Arc::new(AtomicUsize::new(0));
        let counter = logins.clone();
        let current = logins.clone();
        let app = Router::new()
            .route(
                "/api/v1/auth/login",
                post(move |headers: AxumHeaders| {
                    let counter = counter.clone();
                    async move {
                        assert!(headers.get("origin").is_some(), "login needs Origin");
                        let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                        (
                            [(
                                "set-cookie",
                                format!("orbit_session_dev=session-{n}; Path=/; HttpOnly"),
                            )],
                            "{}",
                        )
                    }
                }),
            )
            .route(
                "/api/v1/auth/me",
                get(move |headers: AxumHeaders| {
                    let current = current.clone();
                    async move {
                        let expected = format!(
                            "orbit_session_dev=session-{}",
                            current.load(Ordering::SeqCst)
                        );
                        let cookie = headers
                            .get("cookie")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("");
                        if cookie != expected {
                            return (
                                AxumStatus::UNAUTHORIZED,
                                r#"{"code":"authentication_required","detail":"x"}"#,
                            )
                                .into_response();
                        }
                        r#"{"id":"u","email":"e","display_name":"Orbit Bot"}"#.into_response()
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, logins)
    }

    fn client(url: &str) -> OrbitClient {
        OrbitClient::new(&OrbitConfig {
            url: url.to_owned(),
            email: "bot@example.com".into(),
            password: "secret".into(),
            workspace_id: None,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn signs_in_once_and_reuses_the_session() {
        let (url, logins) = fake_orbit().await;
        let orbit = client(&url);
        assert_eq!(orbit.me().await.unwrap(), "Orbit Bot");
        assert_eq!(orbit.me().await.unwrap(), "Orbit Bot");
        assert_eq!(logins.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn signs_in_again_when_the_session_expires() {
        let (url, logins) = fake_orbit().await;
        let orbit = client(&url);
        orbit.me().await.unwrap();
        // The server-side session is replaced; the client's cookie is now stale.
        logins.fetch_add(1, Ordering::SeqCst);
        assert_eq!(orbit.me().await.unwrap(), "Orbit Bot");
        assert_eq!(logins.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retries_a_conflicting_update_with_the_current_version() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
        let log = seen.clone();
        let app = Router::new()
            .route(
                "/api/v1/auth/login",
                post(|| async { ([("set-cookie", "orbit_session_dev=s; Path=/")], "{}") }),
            )
            .route(
                "/api/v1/workspaces/w/tasks/t",
                axum::routing::patch(move |axum::Json(body): axum::Json<serde_json::Value>| {
                    let log = log.clone();
                    async move {
                        let version = body["expected_version"].as_u64().unwrap();
                        log.lock().unwrap().push(version);
                        if version != 5 {
                            return (
                                AxumStatus::CONFLICT,
                                r#"{"code":"conflict","detail":"x","conflict":{"current_version":5}}"#,
                            )
                                .into_response();
                        }
                        axum::Json(serde_json::json!({
                            "id": "t", "workspace_id": "w", "project_id": "p", "status_id": "done",
                            "title": "T", "description": "", "source_url": null, "priority": "high",
                            "label_ids": [], "due_at": null, "version": 6,
                            "created_at": "2026-10-01T12:00:00.000Z", "auto_closed": []
                        }))
                        .into_response()
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let task = client(&url)
            .update_task(
                "w",
                "t",
                TaskPatch {
                    expected_version: 3,
                    status_id: Some("done".into()),
                },
            )
            .await
            .unwrap();
        assert_eq!(task.version, 6);
        assert_eq!(*seen.lock().unwrap(), vec![3, 5]);
    }
}
